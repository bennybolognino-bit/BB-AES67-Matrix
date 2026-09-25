use serde::Serialize;
use socket2::{Domain, Protocol, Socket, Type};
use std::{
    collections::{hash_map::DefaultHasher, HashMap},
    hash::{Hash, Hasher},
    net::{Ipv4Addr, SocketAddrV4, UdpSocket},
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        Arc, Mutex,
    },
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

#[derive(Clone)]
struct RouteControls {
    gain_db: f64,
    mute: bool,
}

#[derive(Clone)]
struct RouteWorker {
    running: Arc<AtomicBool>,
    controls: Arc<Mutex<RouteControls>>,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RouteStatus {
    id: String,
    name: String,
    source_id: String,
    destination: String,
    channels: u16,
    gain_db: f64,
    mute: bool,
    running: bool,
    packets_sent: u64,
    bytes_sent: u64,
    error: String,
    sdp: String,
}

#[derive(Clone, Default)]
pub struct RoutingState {
    workers: Arc<Mutex<HashMap<String, RouteWorker>>>,
    statuses: Arc<Mutex<HashMap<String, RouteStatus>>>,
    counter: Arc<AtomicU64>,
}

fn timestamp_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

fn route_hash(value: &str) -> u64 {
    let mut hasher = DefaultHasher::new();
    value.hash(&mut hasher);
    hasher.finish()
}

fn rtp_payload(packet: &[u8]) -> Option<(u32, &[u8])> {
    if packet.len() < 12 || packet[0] >> 6 != 2 {
        return None;
    }

    let timestamp = u32::from_be_bytes([packet[4], packet[5], packet[6], packet[7]]);

    let csrc_count = (packet[0] & 0x0f) as usize;
    let mut offset = 12 + csrc_count * 4;

    if packet[0] & 0x10 != 0 {
        if offset + 4 > packet.len() {
            return None;
        }

        let words = u16::from_be_bytes([packet[offset + 2], packet[offset + 3]]) as usize;

        offset += 4 + words * 4;
    }

    let padding = if packet[0] & 0x20 != 0 {
        *packet.last()? as usize
    } else {
        0
    };

    let end = packet.len().checked_sub(padding)?;

    if offset > end {
        return None;
    }

    Some((timestamp, packet.get(offset..end)?))
}

fn create_receive_socket(
    group: Ipv4Addr,
    port: u16,
    interface: Ipv4Addr,
) -> Result<UdpSocket, String> {
    let socket = Socket::new(Domain::IPV4, Type::DGRAM, Some(Protocol::UDP))
        .map_err(|error| error.to_string())?;

    socket
        .set_reuse_address(true)
        .map_err(|error| error.to_string())?;

    socket
        .set_recv_buffer_size(8 * 1024 * 1024)
        .map_err(|error| error.to_string())?;

    socket
        .bind(&SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, port).into())
        .map_err(|error| format!("Impossibile aprire UDP/{port}: {error}"))?;

    let socket: UdpSocket = socket.into();

    socket
        .join_multicast_v4(&group, &interface)
        .map_err(|error| error.to_string())?;

    socket
        .set_read_timeout(Some(Duration::from_secs(1)))
        .map_err(|error| error.to_string())?;

    Ok(socket)
}

fn transform_pcm(
    payload: &[u8],
    codec: &str,
    source_channels: usize,
    selected_channels: &[usize],
    gain_db: f64,
    mute: bool,
) -> Vec<u8> {
    let bytes_per_sample = if codec == "L16" { 2 } else { 3 };
    let source_frame_size = bytes_per_sample * source_channels;

    if source_frame_size == 0 {
        return Vec::new();
    }

    let frames = payload.len() / source_frame_size;
    let mut output = Vec::with_capacity(frames * selected_channels.len() * bytes_per_sample);

    let gain = if mute {
        0.0
    } else {
        10.0_f64.powf(gain_db / 20.0)
    };

    for frame in payload.chunks_exact(source_frame_size) {
        for channel in selected_channels {
            let offset = channel * bytes_per_sample;

            if codec == "L16" {
                let sample = i16::from_be_bytes([frame[offset], frame[offset + 1]]) as f64;

                let value = (sample * gain)
                    .round()
                    .clamp(i16::MIN as f64, i16::MAX as f64) as i16;

                output.extend_from_slice(&value.to_be_bytes());
            } else {
                let raw = ((frame[offset] as i32) << 16)
                    | ((frame[offset + 1] as i32) << 8)
                    | frame[offset + 2] as i32;

                let sample = if raw & 0x80_0000 != 0 {
                    raw | !0xff_ffff
                } else {
                    raw
                };

                let value = (sample as f64 * gain)
                    .round()
                    .clamp(-8_388_608.0, 8_388_607.0) as i32;

                output.push(((value >> 16) & 0xff) as u8);
                output.push(((value >> 8) & 0xff) as u8);
                output.push((value & 0xff) as u8);
            }
        }
    }

    output
}

fn create_rtp_packet(payload: &[u8], sequence: u16, timestamp: u32, ssrc: u32) -> Vec<u8> {
    let mut packet = Vec::with_capacity(12 + payload.len());

    packet.push(0x80);
    packet.push(96);
    packet.extend_from_slice(&sequence.to_be_bytes());
    packet.extend_from_slice(&timestamp.to_be_bytes());
    packet.extend_from_slice(&ssrc.to_be_bytes());
    packet.extend_from_slice(payload);

    packet
}

fn build_sdp(
    name: &str,
    interface: Ipv4Addr,
    destination: Ipv4Addr,
    port: u16,
    codec: &str,
    sample_rate: u32,
    channels: u16,
    session: u64,
) -> String {
    format!(
        "v=0\r\n\
         o=- {session} {session} IN IP4 {interface}\r\n\
         s={name}\r\n\
         c=IN IP4 {destination}/32\r\n\
         t=0 0\r\n\
         m=audio {port} RTP/AVP 96\r\n\
         a=rtpmap:96 {codec}/{sample_rate}/{channels}\r\n\
         a=ptime:1\r\n"
    )
}

fn build_sap(sdp: &str, interface: Ipv4Addr, message_hash: u16, deletion: bool) -> Vec<u8> {
    let mut packet = Vec::new();
    packet.push(0x20 | if deletion { 0x04 } else { 0x00 });
    packet.push(0);
    packet.extend_from_slice(&message_hash.to_be_bytes());
    packet.extend_from_slice(&interface.octets());
    packet.extend_from_slice(b"application/sdp\0");
    packet.extend_from_slice(sdp.as_bytes());
    packet
}

fn send_sap(socket: &UdpSocket, packet: &[u8]) {
    let _ = socket.send_to(packet, ("239.255.255.255", 9875));
    let _ = socket.send_to(packet, ("224.2.127.254", 9875));
}

#[allow(clippy::too_many_arguments)]
#[tauri::command]
pub fn start_route(
    name: String,
    source_id: String,
    source_address: String,
    source_port: u16,
    sample_rate: u32,
    codec: String,
    source_channels: u16,
    selected_channels: Vec<u16>,
    destination_address: String,
    destination_port: u16,
    interface_ip: String,
    gain_db: f64,
    mute: bool,
    state: tauri::State<RoutingState>,
) -> Result<RouteStatus, String> {
    let codec = codec.to_ascii_uppercase();

    if codec != "L16" && codec != "L24" {
        return Err("Routing disponibile solamente per L16/L24".into());
    }

    if selected_channels.is_empty()
        || selected_channels
            .iter()
            .any(|channel| *channel >= source_channels)
    {
        return Err("Selezione canali non valida".into());
    }

    let source_group: Ipv4Addr = source_address
        .parse()
        .map_err(|_| "Multicast sorgente non valido".to_string())?;

    let destination: Ipv4Addr = destination_address
        .parse()
        .map_err(|_| "Multicast destinazione non valido".to_string())?;

    let interface: Ipv4Addr = interface_ip
        .parse()
        .map_err(|_| "Interfaccia non valida".to_string())?;

    if !destination.is_multicast() {
        return Err("La destinazione deve essere multicast".into());
    }

    let receive_socket = create_receive_socket(source_group, source_port, interface)?;

    let send_socket = Socket::new(Domain::IPV4, Type::DGRAM, Some(Protocol::UDP))
        .map_err(|error| error.to_string())?;

    send_socket
        .bind(&SocketAddrV4::new(interface, 0).into())
        .map_err(|error| error.to_string())?;

    send_socket
        .set_multicast_if_v4(&interface)
        .map_err(|error| error.to_string())?;

    send_socket
        .set_multicast_ttl_v4(32)
        .map_err(|error| error.to_string())?;

    let send_socket: UdpSocket = send_socket.into();

    let number = state.counter.fetch_add(1, Ordering::SeqCst) + 1;
    let id = format!("route-{}-{number}", timestamp_ms());
    let session = timestamp_ms();
    let output_channels = selected_channels.len() as u16;

    let sdp = build_sdp(
        &name,
        interface,
        destination,
        destination_port,
        &codec,
        sample_rate,
        output_channels,
        session,
    );

    let status = RouteStatus {
        id: id.clone(),
        name: name.clone(),
        source_id,
        destination: format!("{destination}:{destination_port}"),
        channels: output_channels,
        gain_db: gain_db.clamp(-60.0, 12.0),
        mute,
        running: true,
        packets_sent: 0,
        bytes_sent: 0,
        error: String::new(),
        sdp: sdp.clone(),
    };

    let controls = Arc::new(Mutex::new(RouteControls {
        gain_db: status.gain_db,
        mute,
    }));

    let running = Arc::new(AtomicBool::new(true));

    state
        .workers
        .lock()
        .map_err(|_| "Routing engine non disponibile".to_string())?
        .insert(
            id.clone(),
            RouteWorker {
                running: Arc::clone(&running),
                controls: Arc::clone(&controls),
            },
        );

    state
        .statuses
        .lock()
        .map_err(|_| "Stato routing non disponibile".to_string())?
        .insert(id.clone(), status.clone());

    let routing_state = state.inner().clone();
    let selected: Vec<usize> = selected_channels
        .into_iter()
        .map(|value| value as usize)
        .collect();

    thread::spawn(move || {
        let ssrc = route_hash(&id) as u32;
        let message_hash = (route_hash(&sdp) & 0xffff) as u16;
        let sap = build_sap(&sdp, interface, message_hash, false);
        let sap_delete = build_sap(&sdp, interface, message_hash, true);
        let mut last_sap = Instant::now() - Duration::from_secs(30);
        let mut sequence = 0u16;
        let mut packet = [0u8; 65_535];

        while running.load(Ordering::SeqCst) {
            if last_sap.elapsed() >= Duration::from_secs(5) {
                send_sap(&send_socket, &sap);
                last_sap = Instant::now();
            }

            match receive_socket.recv_from(&mut packet) {
                Ok((size, _)) => {
                    let Some((timestamp, payload)) = rtp_payload(&packet[..size]) else {
                        continue;
                    };

                    let current =
                        controls
                            .lock()
                            .map(|value| value.clone())
                            .unwrap_or(RouteControls {
                                gain_db: 0.0,
                                mute: false,
                            });

                    let transformed = transform_pcm(
                        payload,
                        &codec,
                        source_channels as usize,
                        &selected,
                        current.gain_db,
                        current.mute,
                    );

                    let output = create_rtp_packet(&transformed, sequence, timestamp, ssrc);

                    sequence = sequence.wrapping_add(1);

                    match send_socket
                        .send_to(&output, SocketAddrV4::new(destination, destination_port))
                    {
                        Ok(bytes) => {
                            if let Ok(mut statuses) = routing_state.statuses.lock() {
                                if let Some(status) = statuses.get_mut(&id) {
                                    status.packets_sent += 1;
                                    status.bytes_sent += bytes as u64;
                                    status.gain_db = current.gain_db;
                                    status.mute = current.mute;
                                }
                            }
                        }
                        Err(error) => {
                            if let Ok(mut statuses) = routing_state.statuses.lock() {
                                if let Some(status) = statuses.get_mut(&id) {
                                    status.error = error.to_string();
                                }
                            }
                        }
                    }
                }

                Err(error)
                    if error.kind() == std::io::ErrorKind::TimedOut
                        || error.kind() == std::io::ErrorKind::WouldBlock => {}

                Err(error) => {
                    if let Ok(mut statuses) = routing_state.statuses.lock() {
                        if let Some(status) = statuses.get_mut(&id) {
                            status.error = error.to_string();
                        }
                    }
                    break;
                }
            }
        }

        send_sap(&send_socket, &sap_delete);

        if let Ok(mut statuses) = routing_state.statuses.lock() {
            if let Some(status) = statuses.get_mut(&id) {
                status.running = false;
            }
        }
    });

    Ok(status)
}

#[tauri::command]
pub fn update_route(
    route_id: String,
    gain_db: f64,
    mute: bool,
    state: tauri::State<RoutingState>,
) -> Result<(), String> {
    let workers = state
        .workers
        .lock()
        .map_err(|_| "Routing engine non disponibile".to_string())?;

    let worker = workers
        .get(&route_id)
        .ok_or_else(|| "Route non trovata".to_string())?;

    let mut controls = worker
        .controls
        .lock()
        .map_err(|_| "Controlli route non disponibili".to_string())?;

    controls.gain_db = gain_db.clamp(-60.0, 12.0);
    controls.mute = mute;

    Ok(())
}

#[tauri::command]
pub fn stop_route(route_id: String, state: tauri::State<RoutingState>) -> Result<(), String> {
    if let Some(worker) = state
        .workers
        .lock()
        .map_err(|_| "Routing engine non disponibile".to_string())?
        .get(&route_id)
    {
        worker.running.store(false, Ordering::SeqCst);
    }

    Ok(())
}

#[tauri::command]
pub fn get_routes(state: tauri::State<RoutingState>) -> Vec<RouteStatus> {
    state
        .statuses
        .lock()
        .map(|statuses| statuses.values().cloned().collect())
        .unwrap_or_default()
}
