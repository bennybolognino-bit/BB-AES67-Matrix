import { useEffect, useMemo, useState } from "react";
import { invoke } from "@tauri-apps/api/core";

interface Stream {
  id: string;
  name: string;
  address: string;
  port: number;
  sampleRate: number;
  codec: string;
  channels: number;
}

interface AudioDevice {
  name: string;
}

interface AudioStatus {
  running: boolean;
  streamId: string;
  deviceName: string;
  bufferedFrames: number;
  underruns: number;
  error: string;
}

interface Props {
  streams: Stream[];
  interfaceIp: string;
}

function AudioMonitor({ streams, interfaceIp }: Props) {
  const compatible = useMemo(
    () =>
      streams.filter((stream) =>
        ["L16", "L24"].includes(stream.codec.toUpperCase()),
      ),
    [streams],
  );

  const [devices, setDevices] = useState<AudioDevice[]>([]);
  const [streamId, setStreamId] = useState("");
  const [deviceName, setDeviceName] = useState("");
  const [leftChannel, setLeftChannel] = useState(0);
  const [rightChannel, setRightChannel] = useState(1);
  const [status, setStatus] = useState<AudioStatus>({
    running: false,
    streamId: "",
    deviceName: "",
    bufferedFrames: 0,
    underruns: 0,
    error: "",
  });
  const [message, setMessage] = useState("Seleziona flusso e uscita audio.");

  useEffect(() => {
    invoke<AudioDevice[]>("list_audio_outputs")
      .then((items) => {
        setDevices(items);
        if (items[0]) setDeviceName(items[0].name);
      })
      .catch((error) => setMessage(String(error)));
  }, []);

  useEffect(() => {
    if (!streamId && compatible[0]) {
      setStreamId(compatible[0].id);
      setRightChannel(compatible[0].channels > 1 ? 1 : 0);
    }
  }, [compatible, streamId]);

  useEffect(() => {
    const timer = window.setInterval(async () => {
      setStatus(await invoke<AudioStatus>("get_audio_status"));
    }, 500);

    return () => window.clearInterval(timer);
  }, []);

  const selected = compatible.find((stream) => stream.id === streamId);

  function changeStream(id: string) {
    setStreamId(id);
    const stream = compatible.find((item) => item.id === id);
    setLeftChannel(0);
    setRightChannel(stream && stream.channels > 1 ? 1 : 0);
  }

  async function start() {
    if (!selected || !deviceName) return;

    try {
      await invoke("start_audio_monitor", {
        streamId: selected.id,
        address: selected.address,
        port: selected.port,
        sampleRate: selected.sampleRate,
        codec: selected.codec,
        channels: selected.channels,
        leftChannel,
        rightChannel,
        interfaceIp,
        deviceName,
      });

      setMessage(`Ascolto attivo: ${selected.name}`);
    } catch (error) {
      setMessage(`Errore ascolto: ${error}`);
    }
  }

  async function stop() {
    await invoke("stop_audio_monitor");
    setMessage("Ascolto arrestato");
  }

  return (
    <section className="panel">
      <div className="panel-title">
        <div>
          <h2>Ascolto locale AES67</h2>
          <p>{message}</p>
        </div>

        <span className={status.running ? "rtp-online" : "rtp-offline"}>
          {status.running ? "IN ASCOLTO" : "FERMO"}
        </span>
      </div>

      <div className="listen-controls">
        <label>
          <span>Flusso</span>
          <select value={streamId} onChange={(e) => changeStream(e.target.value)}>
            {compatible.map((stream) => (
              <option value={stream.id} key={stream.id}>
                {stream.name} — {stream.codec}/{stream.sampleRate}
              </option>
            ))}
          </select>
        </label>

        <label>
          <span>Uscita audio</span>
          <select
            value={deviceName}
            onChange={(e) => setDeviceName(e.target.value)}
          >
            {devices.map((device) => (
              <option value={device.name} key={device.name}>
                {device.name}
              </option>
            ))}
          </select>
        </label>

        <label>
          <span>Canale sinistro</span>
          <select
            value={leftChannel}
            onChange={(e) => setLeftChannel(Number(e.target.value))}
          >
            {Array.from({ length: selected?.channels ?? 0 }, (_, index) => (
              <option value={index} key={index}>CH {index + 1}</option>
            ))}
          </select>
        </label>

        <label>
          <span>Canale destro</span>
          <select
            value={rightChannel}
            onChange={(e) => setRightChannel(Number(e.target.value))}
          >
            {Array.from({ length: selected?.channels ?? 0 }, (_, index) => (
              <option value={index} key={index}>CH {index + 1}</option>
            ))}
          </select>
        </label>
      </div>

      <div className="listen-footer">
        <span>Buffer: {status.bufferedFrames} frame</span>
        <span>Underrun: {status.underruns}</span>
        {status.error && <strong>{status.error}</strong>}

        {status.running ? (
          <button className="danger" onClick={stop}>Ferma ascolto</button>
        ) : (
          <button
            disabled={!selected || !deviceName || !interfaceIp}
            onClick={start}
          >
            Avvia ascolto
          </button>
        )}
      </div>
    </section>
  );
}

export default AudioMonitor;