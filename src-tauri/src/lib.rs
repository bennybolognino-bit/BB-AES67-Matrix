use serde::Serialize;
use std::{
    collections::HashMap,
    hash::{Hash, Hasher},
    net::{Ipv4Addr, UdpSocket},
    sync::{Arc, Mutex},
    thread,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

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

#[derive(Clone, Default)]
struct AppState {
    streams: Arc<Mutex<HashMap<String, Aes67Stream>>>,
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
    let mut port = 0u16;
    let mut payload = String::new();
    let mut codec = "L24".to_string();
    let mut sample_rate = 48_000u32;
    let mut channels = 2u16;

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
            port = fields
                .first()
                .and_then(|value| value.parse().ok())
                .unwrap_or(0);

            payload = fields.get(2).unwrap_or(&"").to_string();
        }

        if let Some(value) = line.strip_prefix("a=rtpmap:") {
            let mut parts = value.split_whitespace();
            let rtp_payload = parts.next().unwrap_or_default();
            let format = parts.next().unwrap_or_default();

            if payload.is_empty() || rtp_payload == payload {
                let format_parts: Vec<&str> = format.split('/').collect();

                codec = format_parts.first().unwrap_or(&"L24").to_string();
                sample_rate = format_parts
                    .get(1)
                    .and_then(|value| value.parse().ok())
                    .unwrap_or(48_000);
                channels = format_parts
                    .get(2)
                    .and_then(|value| value.parse().ok())
                    .unwrap_or(2);
            }
        }
    }

    if address.is_empty() {
        return Err("SDP privo dell'indirizzo multicast c=IN IP4".into());
    }

    if port == 0 {
        return Err("SDP privo di una porta audio valida".into());
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

    let flags = packet[0];
    let ipv6 = flags & 0x10 != 0;
    let auth_words = packet[1] as usize;
    let source_size = if ipv6 { 16 } else { 4 };
    let offset = 4 + source_size + auth_words * 4;

    if offset >= packet.len() {
        return None;
    }

    let payload = &packet[offset..];
    let content_start = payload
        .iter()
        .position(|byte| *byte == 0)
        .map(|position| position + 1)
        .unwrap_or(0);

    std::str::from_utf8(payload.get(content_start..)?).ok()
}

fn start_sap_listener(state: AppState) {
    thread::spawn(move || {
        let socket = match UdpSocket::bind(("0.0.0.0", 9875)) {
            Ok(socket) => socket,
            Err(error) => {
                eprintln!("Impossibile aprire SAP UDP/9875: {error}");
                return;
            }
        };

        if let Err(error) = socket.join_multicast_v4(
            &Ipv4Addr::new(239, 255, 255, 255),
            &Ipv4Addr::new(192, 168, 77, 85),
        ) {
            eprintln!("Impossibile entrare nel gruppo SAP: {error}");
        }

        let _ = socket.join_multicast_v4(
            &Ipv4Addr::new(224, 2, 127, 254),
            &Ipv4Addr::new(192, 168, 77, 85),
        );

        let _ = socket.set_read_timeout(Some(Duration::from_secs(1)));
        let mut buffer = [0u8; 65_535];

        loop {
            if let Ok((size, sender)) = socket.recv_from(&mut buffer) {
                if let Some(sdp) = extract_sap_sdp(&buffer[..size]) {
                    if let Ok(stream) = parse_sdp(sdp, &format!("SAP Ã‚Â· {}", sender.ip())) {
                        if let Ok(mut streams) = state.streams.lock() {
                            streams.insert(stream.id.clone(), stream);
                        }
                    }
                }
            }
        }
    });
}

#[tauri::command]
fn get_streams(state: tauri::State<AppState>) -> Vec<Aes67Stream> {
    let Ok(streams) = state.streams.lock() else {
        return Vec::new();
    };

    let mut result: Vec<Aes67Stream> = streams.values().cloned().collect();
    result.sort_by(|a, b| a.name.cmp(&b.name));
    result
}

#[tauri::command]
fn import_sdp(sdp: String, state: tauri::State<AppState>) -> Result<Aes67Stream, String> {
    let stream = parse_sdp(&sdp, "SDP manuale")?;

    state
        .streams
        .lock()
        .map_err(|_| "Archivio flussi non disponibile".to_string())?
        .insert(stream.id.clone(), stream.clone());

    Ok(stream)
}

#[tauri::command]
fn add_demo_streams(state: tauri::State<AppState>) -> Result<(), String> {
    let demo = [
        ("Dante Main L-R", "239.69.1.10", 5004, "L24", 48_000, 2),
        ("Studio Microphones", "239.69.1.11", 5006, "L24", 48_000, 8),
        ("RAVENNA Program", "239.69.1.12", 5008, "L16", 48_000, 2),
        ("Contribution 96K", "239.69.1.13", 5010, "L24", 96_000, 2),
    ];

    let mut streams = state
        .streams
        .lock()
        .map_err(|_| "Archivio flussi non disponibile".to_string())?;

    for (name, address, port, codec, sample_rate, channels) in demo {
        let stream = Aes67Stream {
            id: stream_id(name, address, port),
            name: name.into(),
            address: address.into(),
            port,
            codec: codec.into(),
            sample_rate,
            channels,
            source: "Demo".into(),
            last_seen: timestamp(),
        };

        streams.insert(stream.id.clone(), stream);
    }

    Ok(())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let state = AppState::default();
    start_sap_listener(state.clone());

    tauri::Builder::default()
        .manage(state)
        .invoke_handler(tauri::generate_handler![
            get_streams,
            import_sdp,
            add_demo_streams
        ])
        .run(tauri::generate_context!())
        .expect("Errore durante l'avvio di BB AES67 Matrix");
}
