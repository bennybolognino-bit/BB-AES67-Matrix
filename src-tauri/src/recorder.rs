use hound::{WavSpec, WavWriter};
use serde::Serialize;
use socket2::{Domain, Protocol, Socket, Type};
use std::{
    fs::File,
    io::BufWriter,
    net::{Ipv4Addr, SocketAddrV4, UdpSocket},
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
    thread,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

#[derive(Clone, Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct RecordingStatus {
    running: bool,
    stream_id: String,
    file_path: String,
    frames_written: u64,
    bytes_written: u64,
    duration_seconds: f64,
    segment_index: u32,
    error: String,
}

#[derive(Clone, Default)]
pub struct RecorderState {
    worker: Arc<Mutex<Option<Arc<AtomicBool>>>>,
    status: Arc<Mutex<RecordingStatus>>,
}

fn unix_timestamp() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

fn sanitize_filename(value: &str) -> String {
    let result: String = value
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || character == '-' || character == '_' {
                character
            } else {
                '_'
            }
        })
        .collect();

    let result = result.trim_matches('_');

    if result.is_empty() {
        "AES67_recording".to_string()
    } else {
        result.to_string()
    }
}

fn segment_path(folder: &Path, base_name: &str, session: u64, segment: u32) -> PathBuf {
    folder.join(format!("{}_{}_part{:03}.wav", base_name, session, segment))
}

fn rtp_payload(packet: &[u8]) -> Option<&[u8]> {
    if packet.len() < 12 || packet[0] >> 6 != 2 {
        return None;
    }

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

    packet.get(offset..end)
}

fn create_socket(group: Ipv4Addr, port: u16, interface: Ipv4Addr) -> Result<UdpSocket, String> {
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
        .map_err(|error| format!("Impossibile ricevere {group} tramite {interface}: {error}"))?;

    socket
        .set_read_timeout(Some(Duration::from_secs(1)))
        .map_err(|error| error.to_string())?;

    Ok(socket)
}

fn create_writer(
    path: &Path,
    sample_rate: u32,
    bits: u16,
    channels: u16,
) -> Result<WavWriter<BufWriter<File>>, String> {
    let spec = WavSpec {
        channels,
        sample_rate,
        bits_per_sample: bits,
        sample_format: hound::SampleFormat::Int,
    };

    WavWriter::create(path, spec).map_err(|error| error.to_string())
}

fn write_pcm(
    writer: &mut WavWriter<BufWriter<File>>,
    payload: &[u8],
    codec: &str,
    source_channels: usize,
    selected_channels: &[usize],
) -> Result<u64, String> {
    let bytes_per_sample = match codec {
        "L16" => 2,
        "L24" => 3,
        _ => return Err("Codec WAV non supportato".into()),
    };

    let frame_size = bytes_per_sample * source_channels;

    if frame_size == 0 {
        return Ok(0);
    }

    let mut frames = 0u64;

    for frame in payload.chunks_exact(frame_size) {
        for channel in selected_channels {
            let offset = channel * bytes_per_sample;

            if codec == "L16" {
                let value = i16::from_be_bytes([frame[offset], frame[offset + 1]]);

                writer
                    .write_sample(value)
                    .map_err(|error| error.to_string())?;
            } else {
                let raw = ((frame[offset] as i32) << 16)
                    | ((frame[offset + 1] as i32) << 8)
                    | frame[offset + 2] as i32;

                let value = if raw & 0x80_0000 != 0 {
                    raw | !0xff_ffff
                } else {
                    raw
                };

                writer
                    .write_sample(value)
                    .map_err(|error| error.to_string())?;
            }
        }

        frames += 1;
    }

    Ok(frames)
}

#[tauri::command]
pub fn choose_recording_folder() -> Option<String> {
    rfd::FileDialog::new()
        .set_title("Cartella registrazioni AES67")
        .pick_folder()
        .map(|path| path.to_string_lossy().to_string())
}

#[allow(clippy::too_many_arguments)]
#[tauri::command]
pub fn start_recording(
    stream_id: String,
    address: String,
    port: u16,
    sample_rate: u32,
    codec: String,
    channels: u16,
    selected_channels: Vec<u16>,
    interface_ip: String,
    folder: String,
    base_name: String,
    split_minutes: u32,
    state: tauri::State<RecorderState>,
) -> Result<(), String> {
    let codec = codec.to_ascii_uppercase();

    if codec != "L16" && codec != "L24" {
        return Err("Registrazione disponibile soltanto per L16/L24".into());
    }

    if selected_channels.is_empty() {
        return Err("Seleziona almeno un canale".into());
    }

    if selected_channels.iter().any(|channel| *channel >= channels) {
        return Err("Selezione canali non valida".into());
    }

    let folder = PathBuf::from(folder);

    if !folder.is_dir() {
        return Err("Cartella di registrazione non valida".into());
    }

    let group: Ipv4Addr = address
        .parse()
        .map_err(|_| "Indirizzo multicast non valido".to_string())?;

    let interface: Ipv4Addr = interface_ip
        .parse()
        .map_err(|_| "Interfaccia non valida".to_string())?;

    if let Ok(mut worker) = state.worker.lock() {
        if let Some(previous) = worker.take() {
            previous.store(false, Ordering::SeqCst);
        }
    }

    thread::sleep(Duration::from_millis(300));

    let socket = create_socket(group, port, interface)?;
    let running = Arc::new(AtomicBool::new(true));

    *state
        .worker
        .lock()
        .map_err(|_| "Registratore non disponibile".to_string())? = Some(Arc::clone(&running));

    let status = Arc::clone(&state.status);
    let session = unix_timestamp();
    let base_name = sanitize_filename(&base_name);
    let selected: Vec<usize> = selected_channels
        .into_iter()
        .map(|value| value as usize)
        .collect();

    if let Ok(mut current) = status.lock() {
        *current = RecordingStatus {
            running: true,
            stream_id: stream_id.clone(),
            segment_index: 1,
            ..Default::default()
        };
    }

    thread::spawn(move || {
        let result = run_recorder(
            socket,
            running,
            Arc::clone(&status),
            folder,
            base_name,
            session,
            sample_rate,
            codec,
            channels as usize,
            selected,
            split_minutes,
        );

        if let Err(error) = result {
            if let Ok(mut current) = status.lock() {
                current.error = error;
            }
        }

        if let Ok(mut current) = status.lock() {
            current.running = false;
        }
    });

    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn run_recorder(
    socket: UdpSocket,
    running: Arc<AtomicBool>,
    status: Arc<Mutex<RecordingStatus>>,
    folder: PathBuf,
    base_name: String,
    session: u64,
    sample_rate: u32,
    codec: String,
    source_channels: usize,
    selected_channels: Vec<usize>,
    split_minutes: u32,
) -> Result<(), String> {
    let bits = if codec == "L16" { 16 } else { 24 };
    let output_channels = selected_channels.len() as u16;
    let bytes_per_sample = (bits / 8) as u64;
    let segment_limit = if split_minutes == 0 {
        0
    } else {
        sample_rate as u64 * split_minutes as u64 * 60
    };

    let mut segment = 1u32;
    let mut segment_frames = 0u64;
    let mut total_frames = 0u64;
    let mut packet = [0u8; 65_535];

    let mut path = segment_path(&folder, &base_name, session, segment);
    let mut writer = create_writer(&path, sample_rate, bits, output_channels)?;

    if let Ok(mut current) = status.lock() {
        current.file_path = path.to_string_lossy().to_string();
    }

    while running.load(Ordering::SeqCst) {
        match socket.recv_from(&mut packet) {
            Ok((size, _)) => {
                let Some(payload) = rtp_payload(&packet[..size]) else {
                    continue;
                };

                let frames = write_pcm(
                    &mut writer,
                    payload,
                    &codec,
                    source_channels,
                    &selected_channels,
                )?;

                segment_frames += frames;
                total_frames += frames;

                if let Ok(mut current) = status.lock() {
                    current.frames_written = total_frames;
                    current.bytes_written =
                        total_frames * output_channels as u64 * bytes_per_sample;
                    current.duration_seconds = total_frames as f64 / sample_rate as f64;
                }

                if segment_limit > 0 && segment_frames >= segment_limit {
                    writer.finalize().map_err(|error| error.to_string())?;

                    segment += 1;
                    segment_frames = 0;
                    path = segment_path(&folder, &base_name, session, segment);
                    writer = create_writer(&path, sample_rate, bits, output_channels)?;

                    if let Ok(mut current) = status.lock() {
                        current.segment_index = segment;
                        current.file_path = path.to_string_lossy().to_string();
                    }
                }
            }

            Err(error)
                if error.kind() == std::io::ErrorKind::TimedOut
                    || error.kind() == std::io::ErrorKind::WouldBlock => {}

            Err(error) => return Err(error.to_string()),
        }
    }

    writer.finalize().map_err(|error| error.to_string())?;
    Ok(())
}

#[tauri::command]
pub fn stop_recording(state: tauri::State<RecorderState>) -> Result<(), String> {
    if let Some(worker) = state
        .worker
        .lock()
        .map_err(|_| "Registratore non disponibile".to_string())?
        .take()
    {
        worker.store(false, Ordering::SeqCst);
    }

    Ok(())
}

#[tauri::command]
pub fn get_recording_status(state: tauri::State<RecorderState>) -> RecordingStatus {
    state.status.lock().map(|s| s.clone()).unwrap_or_default()
}
