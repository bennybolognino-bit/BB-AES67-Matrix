use serde::Serialize;
use socket2::{Domain, Protocol, Socket, Type};
use std::{
    collections::HashMap,
    net::{Ipv4Addr, SocketAddrV4, UdpSocket},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RtpStats {
    stream_id: String,
    running: bool,
    online: bool,
    packets: u64,
    bytes: u64,
    estimated_lost: u64,
    out_of_order: u64,
    duplicates: u64,
    jitter_ms: f64,
    bitrate_mbps: f64,
    last_sequence: Option<u16>,
    last_packet_at: u64,
}

impl RtpStats {
    fn new(stream_id: String) -> Self {
        Self {
            stream_id,
            running: true,
            online: false,
            packets: 0,
            bytes: 0,
            estimated_lost: 0,
            out_of_order: 0,
            duplicates: 0,
            jitter_ms: 0.0,
            bitrate_mbps: 0.0,
            last_sequence: None,
            last_packet_at: 0,
        }
    }
}

#[derive(Clone, Default)]
pub struct RtpMonitorState {
    stats: Arc<Mutex<HashMap<String, RtpStats>>>,
    workers: Arc<Mutex<HashMap<String, Arc<AtomicBool>>>>,
}

fn timestamp_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

fn create_socket(group: Ipv4Addr, port: u16, interface: Ipv4Addr) -> Result<UdpSocket, String> {
    if !group.is_multicast() {
        return Err(format!("{group} non è un indirizzo multicast"));
    }

    let socket = Socket::new(Domain::IPV4, Type::DGRAM, Some(Protocol::UDP))
        .map_err(|error| error.to_string())?;

    socket
        .set_reuse_address(true)
        .map_err(|error| error.to_string())?;

    socket
        .set_recv_buffer_size(4 * 1024 * 1024)
        .map_err(|error| error.to_string())?;

    socket
        .bind(&SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, port).into())
        .map_err(|error| format!("Impossibile aprire {group}:{port}: {error}"))?;

    let socket: UdpSocket = socket.into();

    socket
        .join_multicast_v4(&group, &interface)
        .map_err(|error| format!("Impossibile ricevere {group} tramite {interface}: {error}"))?;

    socket
        .set_read_timeout(Some(Duration::from_secs(1)))
        .map_err(|error| error.to_string())?;

    Ok(socket)
}

#[tauri::command]
pub fn start_rtp_monitor(
    stream_id: String,
    address: String,
    port: u16,
    sample_rate: u32,
    interface_ip: String,
    state: tauri::State<RtpMonitorState>,
) -> Result<(), String> {
    let group: Ipv4Addr = address
        .parse()
        .map_err(|_| "Indirizzo multicast non valido".to_string())?;

    let interface: Ipv4Addr = interface_ip
        .parse()
        .map_err(|_| "Interfaccia IPv4 non valida".to_string())?;

    if let Ok(workers) = state.workers.lock() {
        if workers
            .get(&stream_id)
            .is_some_and(|worker| worker.load(Ordering::SeqCst))
        {
            return Ok(());
        }
    }

    let socket = create_socket(group, port, interface)?;
    let running = Arc::new(AtomicBool::new(true));

    state
        .workers
        .lock()
        .map_err(|_| "Monitor RTP non disponibile".to_string())?
        .insert(stream_id.clone(), Arc::clone(&running));

    state
        .stats
        .lock()
        .map_err(|_| "Statistiche RTP non disponibili".to_string())?
        .insert(stream_id.clone(), RtpStats::new(stream_id.clone()));

    let monitor_state = state.inner().clone();

    thread::spawn(move || {
        let mut buffer = [0u8; 65_535];
        let started = Instant::now();
        let mut previous_transit: Option<f64> = None;
        let mut jitter = 0.0;
        let mut window_started = Instant::now();
        let mut window_bytes = 0u64;

        while running.load(Ordering::SeqCst) {
            match socket.recv_from(&mut buffer) {
                Ok((size, _)) if size >= 12 => {
                    let sequence = u16::from_be_bytes([buffer[2], buffer[3]]);
                    let rtp_timestamp =
                        u32::from_be_bytes([buffer[4], buffer[5], buffer[6], buffer[7]]);

                    let arrival_units = started.elapsed().as_secs_f64() * sample_rate.max(1) as f64;
                    let transit = arrival_units - rtp_timestamp as f64;

                    if let Some(previous) = previous_transit {
                        let difference = (transit - previous).abs();
                        jitter += (difference - jitter) / 16.0;
                    }

                    previous_transit = Some(transit);
                    window_bytes += size as u64;

                    if let Ok(mut all_stats) = monitor_state.stats.lock() {
                        if let Some(stats) = all_stats.get_mut(&stream_id) {
                            if let Some(previous_sequence) = stats.last_sequence {
                                let distance = sequence.wrapping_sub(previous_sequence);

                                if distance == 0 {
                                    stats.duplicates += 1;
                                } else if distance < 32_768 {
                                    if distance > 1 {
                                        stats.estimated_lost += (distance - 1) as u64;
                                    }
                                    stats.last_sequence = Some(sequence);
                                } else {
                                    stats.out_of_order += 1;
                                }
                            } else {
                                stats.last_sequence = Some(sequence);
                            }

                            stats.running = true;
                            stats.online = true;
                            stats.packets += 1;
                            stats.bytes += size as u64;
                            stats.last_packet_at = timestamp_ms();
                            stats.jitter_ms = jitter * 1000.0 / sample_rate.max(1) as f64;

                            let window_seconds = window_started.elapsed().as_secs_f64();

                            if window_seconds >= 1.0 {
                                stats.bitrate_mbps =
                                    window_bytes as f64 * 8.0 / window_seconds / 1_000_000.0;

                                window_bytes = 0;
                                window_started = Instant::now();
                            }
                        }
                    }
                }

                Err(error)
                    if error.kind() == std::io::ErrorKind::TimedOut
                        || error.kind() == std::io::ErrorKind::WouldBlock =>
                {
                    if let Ok(mut all_stats) = monitor_state.stats.lock() {
                        if let Some(stats) = all_stats.get_mut(&stream_id) {
                            if timestamp_ms().saturating_sub(stats.last_packet_at) > 3_000 {
                                stats.online = false;
                                stats.bitrate_mbps = 0.0;
                            }
                        }
                    }
                }

                Err(_) => break,
                _ => {}
            }
        }

        if let Ok(mut all_stats) = monitor_state.stats.lock() {
            if let Some(stats) = all_stats.get_mut(&stream_id) {
                stats.running = false;
                stats.online = false;
                stats.bitrate_mbps = 0.0;
            }
        }
    });

    Ok(())
}

#[tauri::command]
pub fn stop_rtp_monitor(
    stream_id: String,
    state: tauri::State<RtpMonitorState>,
) -> Result<(), String> {
    if let Some(worker) = state
        .workers
        .lock()
        .map_err(|_| "Monitor RTP non disponibile".to_string())?
        .get(&stream_id)
    {
        worker.store(false, Ordering::SeqCst);
    }

    Ok(())
}

#[tauri::command]
pub fn get_rtp_stats(state: tauri::State<RtpMonitorState>) -> Vec<RtpStats> {
    state
        .stats
        .lock()
        .map(|stats| stats.values().cloned().collect())
        .unwrap_or_default()
}
