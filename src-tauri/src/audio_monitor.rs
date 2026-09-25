use cpal::{
    traits::{DeviceTrait, HostTrait, StreamTrait},
    Device, FromSample, SampleFormat, SizedSample, Stream, StreamConfig,
};
use serde::Serialize;
use socket2::{Domain, Protocol, Socket, Type};
use std::{
    collections::VecDeque,
    net::{Ipv4Addr, SocketAddrV4, UdpSocket},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
    thread,
    time::Duration,
};

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AudioDevice {
    name: String,
}

#[derive(Clone, Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct AudioStatus {
    running: bool,
    stream_id: String,
    device_name: String,
    buffered_frames: usize,
    underruns: u64,
    error: String,
}

#[derive(Clone, Default)]
pub struct AudioMonitorState {
    worker: Arc<Mutex<Option<Arc<AtomicBool>>>>,
    status: Arc<Mutex<AudioStatus>>,
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

fn decode_sample(bytes: &[u8], codec: &str) -> Option<f32> {
    match codec {
        "L16" => {
            let value = i16::from_be_bytes([*bytes.first()?, *bytes.get(1)?]);
            Some(value as f32 / 32_768.0)
        }
        "L24" => {
            let raw = ((*bytes.first()? as i32) << 16)
                | ((*bytes.get(1)? as i32) << 8)
                | *bytes.get(2)? as i32;

            let signed = if raw & 0x80_0000 != 0 {
                raw | !0xff_ffff
            } else {
                raw
            };

            Some(signed as f32 / 8_388_608.0)
        }
        _ => None,
    }
}

fn decode_audio(
    payload: &[u8],
    codec: &str,
    channels: usize,
    left_channel: usize,
    right_channel: usize,
    queue: &Arc<Mutex<VecDeque<f32>>>,
    maximum_samples: usize,
) {
    let bytes_per_sample = match codec {
        "L16" => 2,
        "L24" => 3,
        _ => return,
    };

    let frame_size = bytes_per_sample * channels;

    if frame_size == 0 {
        return;
    }

    let Ok(mut queue) = queue.lock() else {
        return;
    };

    for frame in payload.chunks_exact(frame_size) {
        let left_offset = left_channel * bytes_per_sample;
        let right_offset = right_channel * bytes_per_sample;

        let left = decode_sample(&frame[left_offset..left_offset + bytes_per_sample], codec)
            .unwrap_or(0.0);

        let right = decode_sample(&frame[right_offset..right_offset + bytes_per_sample], codec)
            .unwrap_or(left);

        queue.push_back(left);
        queue.push_back(right);
    }

    while queue.len() > maximum_samples {
        queue.pop_front();
        queue.pop_front();
    }
}

fn build_output_stream<T>(
    device: &Device,
    config: &StreamConfig,
    queue: Arc<Mutex<VecDeque<f32>>>,
    status: Arc<Mutex<AudioStatus>>,
) -> Result<Stream, String>
where
    T: SizedSample + FromSample<f32>,
{
    let output_channels = config.channels as usize;
    let error_status = Arc::clone(&status);

    device
        .build_output_stream(
            config,
            move |output: &mut [T], _| {
                let mut missing = 0u64;

                if let Ok(mut queue) = queue.lock() {
                    for frame in output.chunks_mut(output_channels) {
                        let left = queue.pop_front();
                        let right = queue.pop_front();

                        let (left, right) = match (left, right) {
                            (Some(left), Some(right)) => (left, right),
                            _ => {
                                missing += 1;
                                (0.0, 0.0)
                            }
                        };

                        if output_channels == 1 {
                            frame[0] = T::from_sample((left + right) * 0.5);
                        } else {
                            frame[0] = T::from_sample(left);
                            frame[1] = T::from_sample(right);

                            for sample in frame.iter_mut().skip(2) {
                                *sample = T::from_sample(0.0);
                            }
                        }
                    }

                    if let Ok(mut current) = status.lock() {
                        current.buffered_frames = queue.len() / 2;
                        current.underruns += missing;
                    }
                }
            },
            move |error| {
                if let Ok(mut current) = error_status.lock() {
                    current.running = false;
                    current.error = error.to_string();
                }
            },
            None,
        )
        .map_err(|error| error.to_string())
}

fn create_socket(group: Ipv4Addr, port: u16, interface: Ipv4Addr) -> Result<UdpSocket, String> {
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
        .map_err(|error| error.to_string())?;

    socket
        .set_read_timeout(Some(Duration::from_secs(1)))
        .map_err(|error| error.to_string())?;

    Ok(socket)
}

#[tauri::command]
pub fn list_audio_outputs() -> Result<Vec<AudioDevice>, String> {
    let host = cpal::default_host();
    let devices = host.output_devices().map_err(|error| error.to_string())?;

    Ok(devices
        .filter_map(|device| device.name().ok())
        .map(|name| AudioDevice { name })
        .collect())
}

#[allow(clippy::too_many_arguments)]
#[tauri::command]
pub fn start_audio_monitor(
    stream_id: String,
    address: String,
    port: u16,
    sample_rate: u32,
    codec: String,
    channels: u16,
    left_channel: u16,
    right_channel: u16,
    interface_ip: String,
    device_name: String,
    state: tauri::State<AudioMonitorState>,
) -> Result<(), String> {
    let codec = codec.to_ascii_uppercase();

    if codec != "L16" && codec != "L24" {
        return Err("Ascolto disponibile soltanto per PCM L16/L24".into());
    }

    if channels == 0 || left_channel >= channels || right_channel >= channels {
        return Err("Selezione canali non valida".into());
    }

    if let Ok(mut worker) = state.worker.lock() {
        if let Some(previous) = worker.take() {
            previous.store(false, Ordering::SeqCst);
        }
    }

    thread::sleep(Duration::from_millis(300));

    let group: Ipv4Addr = address
        .parse()
        .map_err(|_| "Multicast non valido".to_string())?;

    let interface: Ipv4Addr = interface_ip
        .parse()
        .map_err(|_| "Interfaccia non valida".to_string())?;

    let socket = create_socket(group, port, interface)?;
    let running = Arc::new(AtomicBool::new(true));

    *state
        .worker
        .lock()
        .map_err(|_| "Monitor audio non disponibile".to_string())? = Some(Arc::clone(&running));

    if let Ok(mut status) = state.status.lock() {
        *status = AudioStatus {
            running: true,
            stream_id: stream_id.clone(),
            device_name: device_name.clone(),
            buffered_frames: 0,
            underruns: 0,
            error: String::new(),
        };
    }

    let status = Arc::clone(&state.status);

    thread::spawn(move || {
        let result = run_audio(
            socket,
            running,
            status.clone(),
            stream_id,
            device_name,
            sample_rate,
            codec,
            channels as usize,
            left_channel as usize,
            right_channel as usize,
        );

        if let Err(error) = result {
            if let Ok(mut current) = status.lock() {
                current.running = false;
                current.error = error;
            }
        }
    });

    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn run_audio(
    socket: UdpSocket,
    running: Arc<AtomicBool>,
    status: Arc<Mutex<AudioStatus>>,
    _stream_id: String,
    device_name: String,
    sample_rate: u32,
    codec: String,
    channels: usize,
    left_channel: usize,
    right_channel: usize,
) -> Result<(), String> {
    let host = cpal::default_host();

    let device = host
        .output_devices()
        .map_err(|error| error.to_string())?
        .find(|device| device.name().ok().as_deref() == Some(&device_name))
        .ok_or_else(|| "Dispositivo audio non trovato".to_string())?;

    let supported = device
        .supported_output_configs()
        .map_err(|error| error.to_string())?
        .find(|config| {
            config.min_sample_rate().0 <= sample_rate && config.max_sample_rate().0 >= sample_rate
        })
        .ok_or_else(|| format!("Il dispositivo selezionato non supporta {} Hz", sample_rate))?;

    let sample_format = supported.sample_format();
    let config = supported
        .with_sample_rate(cpal::SampleRate(sample_rate))
        .config();

    let queue = Arc::new(Mutex::new(VecDeque::<f32>::new()));

    let stream = match sample_format {
        SampleFormat::I8 => {
            build_output_stream::<i8>(&device, &config, queue.clone(), status.clone())?
        }
        SampleFormat::I16 => {
            build_output_stream::<i16>(&device, &config, queue.clone(), status.clone())?
        }
        SampleFormat::I32 => {
            build_output_stream::<i32>(&device, &config, queue.clone(), status.clone())?
        }
        SampleFormat::I64 => {
            build_output_stream::<i64>(&device, &config, queue.clone(), status.clone())?
        }
        SampleFormat::U8 => {
            build_output_stream::<u8>(&device, &config, queue.clone(), status.clone())?
        }
        SampleFormat::U16 => {
            build_output_stream::<u16>(&device, &config, queue.clone(), status.clone())?
        }
        SampleFormat::U32 => {
            build_output_stream::<u32>(&device, &config, queue.clone(), status.clone())?
        }
        SampleFormat::U64 => {
            build_output_stream::<u64>(&device, &config, queue.clone(), status.clone())?
        }
        SampleFormat::F32 => {
            build_output_stream::<f32>(&device, &config, queue.clone(), status.clone())?
        }
        SampleFormat::F64 => {
            build_output_stream::<f64>(&device, &config, queue.clone(), status.clone())?
        }
        _ => return Err(format!("Formato audio non gestito: {sample_format:?}")),
    };

    stream.play().map_err(|error| error.to_string())?;

    let mut packet = [0u8; 65_535];
    let maximum_samples = sample_rate as usize * 4;

    while running.load(Ordering::SeqCst) {
        match socket.recv_from(&mut packet) {
            Ok((size, _)) => {
                if let Some(payload) = rtp_payload(&packet[..size]) {
                    decode_audio(
                        payload,
                        &codec,
                        channels,
                        left_channel,
                        right_channel,
                        &queue,
                        maximum_samples,
                    );
                }
            }
            Err(error)
                if error.kind() == std::io::ErrorKind::TimedOut
                    || error.kind() == std::io::ErrorKind::WouldBlock => {}
            Err(error) => return Err(error.to_string()),
        }
    }

    if let Ok(mut current) = status.lock() {
        current.running = false;
        current.buffered_frames = 0;
    }

    drop(stream);
    Ok(())
}

#[tauri::command]
pub fn stop_audio_monitor(state: tauri::State<AudioMonitorState>) -> Result<(), String> {
    if let Some(worker) = state
        .worker
        .lock()
        .map_err(|_| "Monitor audio non disponibile".to_string())?
        .take()
    {
        worker.store(false, Ordering::SeqCst);
    }

    Ok(())
}

#[tauri::command]
pub fn get_audio_status(state: tauri::State<AudioMonitorState>) -> AudioStatus {
    state.status.lock().map(|s| s.clone()).unwrap_or_default()
}
