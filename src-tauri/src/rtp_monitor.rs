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
    meter_supported: bool,
    levels_dbfs: Vec<f64>,
    silent: bool,
    clipping: bool,
}

impl RtpStats {
    fn new(stream_id: String, meter_supported: bool, channels: usize) -> Self {
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
            meter_supported,
            levels_dbfs: if meter_supported {
                vec![-120.0; channels]
            } else {
                Vec::new()
            },
            silent: false,
            clipping: false,
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

fn rtp_payload(packet: &[u8]) -> Option<&[u8]> {
    if packet.len() < 12 || packet[0] >> 6 != 2 {
        return None;
    }

    let csrc_count = (packet[0] & 0x0f) as usize;
    let mut offset = 12 + csrc_count * 4;

    if offset > packet.len() {
        return None;
    }

    if packet[0] & 0x10 != 0 {
        if offset + 4 > packet.len() {
            return None;
        }

        let extension_words = u16::from_be_bytes([packet[offset + 2], packet[offset + 3]]) as usize;

        offset += 4 + extension_words * 4;
    }

    if offset > packet.len() {
        return None;
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

    packet.get(offset..end)
}

fn pcm_levels(payload: &[u8], codec: &str, channels: usize) -> Option<Vec<f64>> {
    let bytes_per_sample = match codec.to_ascii_uppercase().as_str() {
        "L16" => 2,
        "L24" => 3,
        _ => return None,
    };

    let channels = channels.max(1);
    let frame_size = bytes_per_sample * channels;

    if payload.len() < frame_size {
        return Some(vec![-120.0; channels]);
    }

    let mut peaks = vec![0.0_f64; channels];

    for frame in payload.chunks_exact(frame_size) {
        for (channel, peak) in peaks.iter_mut().enumerate() {
            let offset = channel * bytes_per_sample;

            let normalized = if bytes_per_sample == 2 {
                let value = i16::from_be_bytes([frame[offset], frame[offset + 1]]) as i32;

                value.unsigned_abs() as f64 / 32_768.0
            } else {
                let raw = ((frame[offset] as i32) << 16)
                    | ((frame[offset + 1] as i32) << 8)
                    | frame[offset + 2] as i32;

                let signed = if raw & 0x80_0000 != 0 {
                    raw | !0xff_ffff
                } else {
                    raw
                };

                signed.unsigned_abs() as f64 / 8_388_608.0
            };

            *peak = peak.max(normalized);
        }
    }

    Some(
        peaks
            .into_iter()
            .map(|peak| {
                if peak <= 0.000_001 {
                    -120.0
                } else {
                    (20.0 * peak.log10()).clamp(-120.0, 0.0)
                }
            })
            .collect(),
    )
}

fn create_socket(group: Ipv4Addr, port: u16, interface: Ipv4Addr) -> Result<UdpSocket, String> {
    if !group.is_multicast() {
        return Err(format!("{group} non è multicast"));
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
        .map_err(|error| format!("Impossibile aprire UDP/{port}: {error}"))?;

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
    codec: String,
    channels: u16,
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
    let meter_supported = matches!(codec.to_ascii_uppercase().as_str(), "L16" | "L24");

    state
        .workers
        .lock()
        .map_err(|_| "Monitor RTP non disponibile".to_string())?
        .insert(stream_id.clone(), Arc::clone(&running));

    state
        .stats
        .lock()
        .map_err(|_| "Statistiche RTP non disponibili".to_string())?
        .insert(
            stream_id.clone(),
            RtpStats::new(stream_id.clone(), meter_supported, channels as usize),
        );

    let monitor_state = state.inner().clone();

    thread::spawn(move || {
        let mut buffer = [0u8; 65_535];
        let started = Instant::now();
        let mut previous_transit: Option<f64> = None;
        let mut jitter = 0.0;
        let mut window_started = Instant::now();
        let mut window_bytes = 0u64;
        let mut silence_started: Option<Instant> = None;

        while running.load(Ordering::SeqCst) {
            match socket.recv_from(&mut buffer) {
                Ok((size, _)) if size >= 12 => {
                    let sequence = u16::from_be_bytes([buffer[2], buffer[3]]);
                    let rtp_timestamp =
                        u32::from_be_bytes([buffer[4], buffer[5], buffer[6], buffer[7]]);

                    let arrival = started.elapsed().as_secs_f64() * sample_rate.max(1) as f64;
                    let transit = arrival - rtp_timestamp as f64;

                    if let Some(previous) = previous_transit {
                        let difference = (transit - previous).abs();
                        jitter += (difference - jitter) / 16.0;
                    }

                    previous_transit = Some(transit);
                    window_bytes += size as u64;

                    let levels = rtp_payload(&buffer[..size])
                        .and_then(|payload| pcm_levels(payload, &codec, channels as usize));

                    let quiet = levels
                        .as_ref()
                        .is_some_and(|values| values.iter().all(|level| *level < -60.0));

                    if quiet {
                        silence_started.get_or_insert_with(Instant::now);
                    } else {
                        silence_started = None;
                    }

                    let silent = silence_started
                        .is_some_and(|instant| instant.elapsed() >= Duration::from_secs(3));

                    let clipping = levels
                        .as_ref()
                        .is_some_and(|values| values.iter().any(|level| *level >= -0.5));

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
                            stats.silent = silent;
                            stats.clipping = clipping;

                            if let Some(levels) = levels {
                                stats.levels_dbfs = levels;
                            }

                            let seconds = window_started.elapsed().as_secs_f64();

                            if seconds >= 1.0 {
                                stats.bitrate_mbps =
                                    window_bytes as f64 * 8.0 / seconds / 1_000_000.0;

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
                                stats.silent = false;
                                stats.clipping = false;
                                stats.bitrate_mbps = 0.0;

                                for level in &mut stats.levels_dbfs {
                                    *level = -120.0;
                                }
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
