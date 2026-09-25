use serde::Serialize;
use socket2::{Domain, Protocol, Socket, Type};
use std::{
    collections::HashMap,
    net::{Ipv4Addr, SocketAddrV4, UdpSocket},
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc, Mutex,
    },
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

#[derive(Clone)]
struct PendingSync {
    arrival_ns: i128,
    correction_ns: f64,
}

#[derive(Clone, Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct PtpStatus {
    running: bool,
    healthy: bool,
    interface_ip: String,
    domain: u8,
    grandmaster_identity: String,
    priority1: u8,
    priority2: u8,
    clock_class: u8,
    clock_accuracy: u8,
    clock_variance: u16,
    steps_removed: u16,
    time_source: u8,
    utc_offset: Option<i16>,
    sync_rate: f64,
    software_offset_us: Option<f64>,
    last_sync_ms: u64,
    last_announce_ms: u64,
    sync_packets: u64,
    announce_packets: u64,
    message: String,
}

#[derive(Clone, Default)]
pub struct PtpMonitorState {
    status: Arc<Mutex<PtpStatus>>,
    generation: Arc<AtomicU64>,
    pending: Arc<Mutex<HashMap<u16, PendingSync>>>,
}

fn timestamp_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

fn timestamp_ns() -> i128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos() as i128
}

fn create_ptp_socket(port: u16, interface: Ipv4Addr) -> Result<UdpSocket, String> {
    let socket = Socket::new(Domain::IPV4, Type::DGRAM, Some(Protocol::UDP))
        .map_err(|error| error.to_string())?;

    socket
        .set_reuse_address(true)
        .map_err(|error| error.to_string())?;

    socket
        .set_recv_buffer_size(2 * 1024 * 1024)
        .map_err(|error| error.to_string())?;

    socket
        .bind(&SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, port).into())
        .map_err(|error| format!("Impossibile aprire PTP UDP/{port}: {error}"))?;

    let socket: UdpSocket = socket.into();

    socket
        .join_multicast_v4(&Ipv4Addr::new(224, 0, 1, 129), &interface)
        .map_err(|error| format!("Impossibile aderire al multicast PTP: {error}"))?;

    socket
        .set_read_timeout(Some(Duration::from_secs(1)))
        .map_err(|error| error.to_string())?;

    Ok(socket)
}

fn valid_ptpv2(packet: &[u8]) -> bool {
    packet.len() >= 34 && packet[1] & 0x0f == 2
}

fn sequence_id(packet: &[u8]) -> u16 {
    u16::from_be_bytes([packet[30], packet[31]])
}

fn correction_ns(packet: &[u8]) -> f64 {
    let raw = i64::from_be_bytes([
        packet[8], packet[9], packet[10], packet[11], packet[12], packet[13], packet[14],
        packet[15],
    ]);

    raw as f64 / 65_536.0
}

fn ptp_timestamp_ns(packet: &[u8], offset: usize) -> Option<i128> {
    if packet.len() < offset + 10 {
        return None;
    }

    let mut seconds = 0u64;

    for byte in &packet[offset..offset + 6] {
        seconds = (seconds << 8) | *byte as u64;
    }

    let nanoseconds = u32::from_be_bytes([
        packet[offset + 6],
        packet[offset + 7],
        packet[offset + 8],
        packet[offset + 9],
    ]);

    Some(seconds as i128 * 1_000_000_000 + nanoseconds as i128)
}

fn clock_identity(bytes: &[u8]) -> String {
    bytes
        .iter()
        .map(|byte| format!("{byte:02X}"))
        .collect::<Vec<_>>()
        .join(":")
}

fn calculate_offset_us(
    arrival_ns: i128,
    origin_tai_ns: i128,
    utc_offset: i16,
    correction: f64,
) -> Option<f64> {
    let origin_utc_ns = origin_tai_ns - utc_offset as i128 * 1_000_000_000;

    let offset = (arrival_ns as f64 - origin_utc_ns as f64 - correction) / 1_000.0;

    if offset.is_finite() && offset.abs() < 10_000_000.0 {
        Some(offset)
    } else {
        None
    }
}

#[tauri::command]
pub fn start_ptp_monitor(
    interface_ip: String,
    state: tauri::State<PtpMonitorState>,
) -> Result<(), String> {
    if let Ok(status) = state.status.lock() {
        if status.running && status.interface_ip == interface_ip {
            return Ok(());
        }
    }

    let interface: Ipv4Addr = interface_ip
        .parse()
        .map_err(|_| "Interfaccia PTP non valida".to_string())?;

    let generation = state.generation.fetch_add(1, Ordering::SeqCst) + 1;

    thread::sleep(Duration::from_millis(1100));

    let event_socket = create_ptp_socket(319, interface)?;
    let general_socket = create_ptp_socket(320, interface)?;

    if let Ok(mut status) = state.status.lock() {
        *status = PtpStatus {
            running: true,
            interface_ip,
            message: "In attesa di messaggi PTPv2".into(),
            ..Default::default()
        };
    }

    let event_state = state.inner().clone();
    let general_state = state.inner().clone();

    thread::spawn(move || {
        monitor_event_messages(event_socket, event_state, generation);
    });

    thread::spawn(move || {
        monitor_general_messages(general_socket, general_state, generation);
    });

    Ok(())
}

fn monitor_event_messages(socket: UdpSocket, state: PtpMonitorState, generation: u64) {
    let mut packet = [0u8; 2048];
    let mut rate_started = Instant::now();
    let mut rate_packets = 0u64;

    while state.generation.load(Ordering::SeqCst) == generation {
        match socket.recv_from(&mut packet) {
            Ok((size, _)) if valid_ptpv2(&packet[..size]) => {
                let data = &packet[..size];
                let message_type = data[0] & 0x0f;

                if message_type != 0 {
                    continue;
                }

                let arrival = timestamp_ns();
                let sequence = sequence_id(data);
                let two_step = u16::from_be_bytes([data[6], data[7]]) & 0x0200 != 0;

                rate_packets += 1;

                if two_step {
                    if let Ok(mut pending) = state.pending.lock() {
                        pending.insert(
                            sequence,
                            PendingSync {
                                arrival_ns: arrival,
                                correction_ns: correction_ns(data),
                            },
                        );
                    }
                } else if let Some(origin) = ptp_timestamp_ns(data, 34) {
                    let utc_offset = state
                        .status
                        .lock()
                        .ok()
                        .and_then(|status| status.utc_offset);

                    if let Some(utc_offset) = utc_offset {
                        let offset =
                            calculate_offset_us(arrival, origin, utc_offset, correction_ns(data));

                        if let Ok(mut status) = state.status.lock() {
                            status.software_offset_us = offset;
                        }
                    }
                }

                if let Ok(mut status) = state.status.lock() {
                    status.domain = data[4];
                    status.last_sync_ms = timestamp_ms();
                    status.sync_packets += 1;
                    status.message = "PTPv2 Sync ricevuto".into();

                    let elapsed = rate_started.elapsed().as_secs_f64();

                    if elapsed >= 1.0 {
                        status.sync_rate = rate_packets as f64 / elapsed;
                        rate_packets = 0;
                        rate_started = Instant::now();
                    }
                }
            }

            Err(error)
                if error.kind() == std::io::ErrorKind::TimedOut
                    || error.kind() == std::io::ErrorKind::WouldBlock => {}

            Err(error) => {
                if let Ok(mut status) = state.status.lock() {
                    status.message = error.to_string();
                }
                break;
            }

            _ => {}
        }
    }
}

fn monitor_general_messages(socket: UdpSocket, state: PtpMonitorState, generation: u64) {
    let mut packet = [0u8; 2048];

    while state.generation.load(Ordering::SeqCst) == generation {
        match socket.recv_from(&mut packet) {
            Ok((size, _)) if valid_ptpv2(&packet[..size]) => {
                let data = &packet[..size];
                let message_type = data[0] & 0x0f;

                if message_type == 11 && data.len() >= 64 {
                    let utc_offset = i16::from_be_bytes([data[44], data[45]]);

                    if let Ok(mut status) = state.status.lock() {
                        status.domain = data[4];
                        status.utc_offset = Some(utc_offset);
                        status.priority1 = data[47];
                        status.clock_class = data[48];
                        status.clock_accuracy = data[49];
                        status.clock_variance = u16::from_be_bytes([data[50], data[51]]);
                        status.priority2 = data[52];
                        status.grandmaster_identity = clock_identity(&data[53..61]);
                        status.steps_removed = u16::from_be_bytes([data[61], data[62]]);
                        status.time_source = data[63];
                        status.last_announce_ms = timestamp_ms();
                        status.announce_packets += 1;
                        status.message = "PTPv2 Grandmaster rilevato".into();
                    }
                }

                if message_type == 8 {
                    let sequence = sequence_id(data);
                    let pending = state
                        .pending
                        .lock()
                        .ok()
                        .and_then(|mut values| values.remove(&sequence));

                    let utc_offset = state
                        .status
                        .lock()
                        .ok()
                        .and_then(|status| status.utc_offset);

                    if let (Some(pending), Some(utc_offset), Some(origin)) =
                        (pending, utc_offset, ptp_timestamp_ns(data, 34))
                    {
                        let offset = calculate_offset_us(
                            pending.arrival_ns,
                            origin,
                            utc_offset,
                            pending.correction_ns + correction_ns(data),
                        );

                        if let Ok(mut status) = state.status.lock() {
                            status.software_offset_us = offset;
                        }
                    }
                }
            }

            Err(error)
                if error.kind() == std::io::ErrorKind::TimedOut
                    || error.kind() == std::io::ErrorKind::WouldBlock => {}

            Err(error) => {
                if let Ok(mut status) = state.status.lock() {
                    status.message = error.to_string();
                }
                break;
            }

            _ => {}
        }
    }
}

#[tauri::command]
pub fn get_ptp_status(state: tauri::State<PtpMonitorState>) -> PtpStatus {
    let mut result = state
        .status
        .lock()
        .map(|status| status.clone())
        .unwrap_or_default();

    let now = timestamp_ms();

    result.healthy = result.running
        && now.saturating_sub(result.last_sync_ms) < 3_000
        && now.saturating_sub(result.last_announce_ms) < 10_000;

    result
}

#[tauri::command]
pub fn stop_ptp_monitor(state: tauri::State<PtpMonitorState>) -> Result<(), String> {
    state.generation.fetch_add(1, Ordering::SeqCst);

    if let Ok(mut status) = state.status.lock() {
        status.running = false;
        status.healthy = false;
        status.message = "Monitor PTP arrestato".into();
    }

    Ok(())
}
