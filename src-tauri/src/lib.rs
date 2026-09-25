mod audio_monitor;
mod recorder;
mod rtp_monitor;

use if_addrs::get_if_addrs;
use serde::Serialize;
use socket2::{Domain, Protocol, Socket, Type};
use std::{
    collections::HashMap,
    hash::{Hash, Hasher},
    net::{IpAddr, Ipv4Addr, SocketAddrV4, UdpSocket},
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc, Mutex,
    },
    thread,
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tauri::Manager;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct Aes67Stream {
    id: String,
    name: String,
    address: String,
    port: u16,
    codec: String,
    sample_rate: u32,
    channels: u16,
    source: String,
    last_seen: u64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct NetworkInterface {
    name: String,
    ip: String,
}

#[derive(Debug, Clone, Serialize, Default)]
#[serde(rename_all = "camelCase")]
struct DiscoveryStatus {
    running: bool,
    interface_ip: String,
    message: String,
}

#[derive(Clone, Default)]
struct AppState {
    streams: Arc<Mutex<HashMap<String, Aes67Stream>>>,
    discovery: Arc<Mutex<DiscoveryStatus>>,
    generation: Arc<AtomicU64>,
}

fn timestamp() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

fn stream_id(name: &str, address: &str, port: u16) -> String {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    (name, address, port).hash(&mut hasher);
    format!("{:x}", hasher.finish())
}

fn parse_sdp(sdp: &str, source: &str) -> Result<Aes67Stream, String> {
    let mut name = "Flusso AES67".to_string();
    let mut address = String::new();
    let mut port = 0;
    let mut payload = String::new();
    let mut codec = "L24".to_string();
    let mut sample_rate = 48_000;
    let mut channels = 2;

    for line in sdp.lines().map(str::trim) {
        if let Some(value) = line.strip_prefix("s=") {
            if !value.is_empty() {
                name = value.to_string();
            }
        }

        if let Some(value) = line.strip_prefix("c=IN IP4 ") {
            address = value.split('/').next().unwrap_or_default().to_string();
        }

        if let Some(value) = line.strip_prefix("m=audio ") {
            let fields: Vec<&str> = value.split_whitespace().collect();
            port = fields.first().and_then(|v| v.parse().ok()).unwrap_or(0);
            payload = fields.get(2).unwrap_or(&"").to_string();
        }

        if let Some(value) = line.strip_prefix("a=rtpmap:") {
            let mut parts = value.split_whitespace();
            let rtp_payload = parts.next().unwrap_or_default();
            let format = parts.next().unwrap_or_default();

            if payload.is_empty() || payload == rtp_payload {
                let values: Vec<&str> = format.split('/').collect();
                codec = values.first().unwrap_or(&"L24").to_string();
                sample_rate = values.get(1).and_then(|v| v.parse().ok()).unwrap_or(48_000);
                channels = values.get(2).and_then(|v| v.parse().ok()).unwrap_or(2);
            }
        }
    }

    if address.is_empty() || port == 0 {
        return Err("SDP privo di multicast o porta audio validi".into());
    }

    Ok(Aes67Stream {
        id: stream_id(&name, &address, port),
        name,
        address,
        port,
        codec,
        sample_rate,
        channels,
        source: source.to_string(),
        last_seen: timestamp(),
    })
}

fn extract_sap_sdp(packet: &[u8]) -> Option<&str> {
    if packet.len() < 8 {
        return None;
    }

    let ipv6 = packet[0] & 0x10 != 0;
    let auth_words = packet[1] as usize;
    let source_size = if ipv6 { 16 } else { 4 };
    let offset = 4 + source_size + auth_words * 4;
    let payload = packet.get(offset..)?;

    let start = if payload.starts_with(b"v=0") {
        0
    } else {
        payload.iter().position(|byte| *byte == 0)? + 1
    };

    std::str::from_utf8(payload.get(start..)?).ok()
}

fn create_sap_socket(interface: Ipv4Addr) -> Result<UdpSocket, String> {
    let socket =
        Socket::new(Domain::IPV4, Type::DGRAM, Some(Protocol::UDP)).map_err(|e| e.to_string())?;

    socket.set_reuse_address(true).map_err(|e| e.to_string())?;

    socket
        .bind(&SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, 9875).into())
        .map_err(|e| format!("Impossibile aprire UDP/9875: {e}"))?;

    let socket: UdpSocket = socket.into();

    let mut joined = false;

    for group in [
        Ipv4Addr::new(239, 255, 255, 255),
        Ipv4Addr::new(224, 2, 127, 254),
    ] {
        if socket.join_multicast_v4(&group, &interface).is_ok() {
            joined = true;
        }
    }

    if !joined {
        return Err(format!(
            "Impossibile aderire ai gruppi SAP tramite {interface}"
        ));
    }

    socket
        .set_read_timeout(Some(Duration::from_secs(1)))
        .map_err(|e| e.to_string())?;

    Ok(socket)
}

#[tauri::command]
fn list_network_interfaces() -> Result<Vec<NetworkInterface>, String> {
    let mut result = Vec::new();

    for interface in get_if_addrs().map_err(|e| e.to_string())? {
        if let IpAddr::V4(ip) = interface.ip() {
            if !ip.is_loopback()
                && !result
                    .iter()
                    .any(|item: &NetworkInterface| item.ip == ip.to_string())
            {
                result.push(NetworkInterface {
                    name: interface.name,
                    ip: ip.to_string(),
                });
            }
        }
    }

    Ok(result)
}

#[tauri::command]
fn start_discovery(interface_ip: String, state: tauri::State<AppState>) -> Result<(), String> {
    let interface: Ipv4Addr = interface_ip
        .parse()
        .map_err(|_| "Indirizzo IPv4 non valido".to_string())?;

    let generation = state.generation.fetch_add(1, Ordering::SeqCst) + 1;

    thread::sleep(Duration::from_millis(1100));

    let socket = match create_sap_socket(interface) {
        Ok(socket) => socket,
        Err(error) => {
            if let Ok(mut status) = state.discovery.lock() {
                status.running = false;
                status.message = error.clone();
            }
            return Err(error);
        }
    };

    if let Ok(mut status) = state.discovery.lock() {
        status.running = true;
        status.interface_ip = interface_ip.clone();
        status.message = format!("SAP attivo su {interface_ip}");
    }

    let app_state = state.inner().clone();

    thread::spawn(move || {
        let mut buffer = [0u8; 65_535];

        while app_state.generation.load(Ordering::SeqCst) == generation {
            match socket.recv_from(&mut buffer) {
                Ok((size, sender)) => {
                    if let Some(sdp) = extract_sap_sdp(&buffer[..size]) {
                        if let Ok(stream) = parse_sdp(sdp, &format!("SAP · {}", sender.ip())) {
                            if let Ok(mut streams) = app_state.streams.lock() {
                                streams.insert(stream.id.clone(), stream);
                            }
                        }
                    }
                }
                Err(error)
                    if error.kind() == std::io::ErrorKind::TimedOut
                        || error.kind() == std::io::ErrorKind::WouldBlock => {}
                Err(error) => {
                    if let Ok(mut status) = app_state.discovery.lock() {
                        status.running = false;
                        status.message = error.to_string();
                    }
                    break;
                }
            }
        }
    });

    Ok(())
}

#[tauri::command]
fn get_discovery_status(state: tauri::State<AppState>) -> DiscoveryStatus {
    state
        .discovery
        .lock()
        .map(|s| s.clone())
        .unwrap_or_default()
}

#[tauri::command]
fn get_streams(state: tauri::State<AppState>) -> Vec<Aes67Stream> {
    let Ok(streams) = state.streams.lock() else {
        return Vec::new();
    };

    let mut result: Vec<_> = streams.values().cloned().collect();
    result.sort_by(|a, b| a.name.cmp(&b.name));
    result
}

#[tauri::command]
fn clear_streams(state: tauri::State<AppState>) -> Result<(), String> {
    state
        .streams
        .lock()
        .map_err(|_| "Archivio non disponibile".to_string())?
        .clear();

    Ok(())
}

#[tauri::command]
fn import_sdp(sdp: String, state: tauri::State<AppState>) -> Result<Aes67Stream, String> {
    let stream = parse_sdp(&sdp, "SDP manuale")?;

    state
        .streams
        .lock()
        .map_err(|_| "Archivio non disponibile".to_string())?
        .insert(stream.id.clone(), stream.clone());

    Ok(stream)
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(|app, _, _| {
            if let Some(window) = app.get_webview_window("main") {
                let _ = window.show();
                let _ = window.set_focus();
            }
        }))
        .manage(AppState::default())
        .manage(rtp_monitor::RtpMonitorState::default())
        .manage(audio_monitor::AudioMonitorState::default())
        .manage(recorder::RecorderState::default())
        .invoke_handler(tauri::generate_handler![
            list_network_interfaces,
            start_discovery,
            get_discovery_status,
            get_streams,
            clear_streams,
            import_sdp,
            rtp_monitor::start_rtp_monitor,
            rtp_monitor::stop_rtp_monitor,
            rtp_monitor::get_rtp_stats,
            audio_monitor::list_audio_outputs,
            audio_monitor::start_audio_monitor,
            audio_monitor::stop_audio_monitor,
            audio_monitor::get_audio_status,
            recorder::choose_recording_folder,
            recorder::start_recording,
            recorder::stop_recording,
            recorder::get_recording_status
        ])
        .run(tauri::generate_context!())
        .expect("Errore durante l'avvio di BB AES67 Matrix");
}
