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

interface RecordingStatus {
  running: boolean;
  streamId: string;
  filePath: string;
  framesWritten: number;
  bytesWritten: number;
  durationSeconds: number;
  segmentIndex: number;
  error: string;
}

interface Props {
  streams: Stream[];
  interfaceIp: string;
}

function duration(value: number) {
  const seconds = Math.floor(value % 60).toString().padStart(2, "0");
  const minutes = Math.floor(value / 60).toString().padStart(2, "0");
  return `${minutes}:${seconds}`;
}

function Recorder({ streams, interfaceIp }: Props) {
  const compatible = useMemo(
    () =>
      streams.filter((stream) =>
        ["L16", "L24"].includes(stream.codec.toUpperCase()),
      ),
    [streams],
  );

  const [streamId, setStreamId] = useState("");
  const [folder, setFolder] = useState(
    localStorage.getItem("bb-aes67-recording-folder") ?? "",
  );
  const [baseName, setBaseName] = useState("AES67_recording");
  const [selectedChannels, setSelectedChannels] = useState<number[]>([]);
  const [splitMinutes, setSplitMinutes] = useState(60);
  const [status, setStatus] = useState<RecordingStatus>({
    running: false,
    streamId: "",
    filePath: "",
    framesWritten: 0,
    bytesWritten: 0,
    durationSeconds: 0,
    segmentIndex: 0,
    error: "",
  });

  const selected = compatible.find((stream) => stream.id === streamId);

  useEffect(() => {
    if (!selected && compatible[0]) {
      selectStream(compatible[0].id);
    }
  }, [compatible]);

  useEffect(() => {
    const timer = window.setInterval(async () => {
      setStatus(await invoke<RecordingStatus>("get_recording_status"));
    }, 500);

    return () => window.clearInterval(timer);
  }, []);

  function selectStream(id: string) {
    const stream = compatible.find((item) => item.id === id);
    setStreamId(id);

    if (stream) {
      setBaseName(stream.name);
      setSelectedChannels(
        Array.from({ length: stream.channels }, (_, index) => index),
      );
    }
  }

  function toggleChannel(channel: number) {
    setSelectedChannels((current) =>
      current.includes(channel)
        ? current.filter((item) => item !== channel)
        : [...current, channel].sort((a, b) => a - b),
    );
  }

  async function chooseFolder() {
    const result = await invoke<string | null>("choose_recording_folder");

    if (result) {
      setFolder(result);
      localStorage.setItem("bb-aes67-recording-folder", result);
    }
  }

  async function start() {
    if (!selected) return;

    await invoke("start_recording", {
      streamId: selected.id,
      address: selected.address,
      port: selected.port,
      sampleRate: selected.sampleRate,
      codec: selected.codec,
      channels: selected.channels,
      selectedChannels,
      interfaceIp,
      folder,
      baseName,
      splitMinutes,
    });
  }

  async function stop() {
    await invoke("stop_recording");
  }

  return (
    <section className="panel">
      <div className="panel-title">
        <div>
          <h2>Registrazione WAV</h2>
          <p>Registrazione PCM multicanale senza ricodifica.</p>
        </div>

        <span className={status.running ? "recording-active" : "rtp-offline"}>
          {status.running ? "● REC" : "FERMO"}
        </span>
      </div>

      <div className="recording-controls">
        <label>
          <span>Flusso</span>
          <select
            value={streamId}
            disabled={status.running}
            onChange={(event) => selectStream(event.target.value)}
          >
            {compatible.map((stream) => (
              <option value={stream.id} key={stream.id}>
                {stream.name}
              </option>
            ))}
          </select>
        </label>

        <label>
          <span>Nome file</span>
          <input
            value={baseName}
            disabled={status.running}
            onChange={(event) => setBaseName(event.target.value)}
          />
        </label>

        <label>
          <span>Suddivisione</span>
          <select
            value={splitMinutes}
            disabled={status.running}
            onChange={(event) => setSplitMinutes(Number(event.target.value))}
          >
            <option value={0}>File unico</option>
            <option value={5}>5 minuti</option>
            <option value={15}>15 minuti</option>
            <option value={30}>30 minuti</option>
            <option value={60}>60 minuti</option>
          </select>
        </label>
      </div>

      <div className="folder-row">
        <button
          className="secondary"
          disabled={status.running}
          onClick={chooseFolder}
        >
          Scegli cartella
        </button>
        <span>{folder || "Nessuna cartella selezionata"}</span>
      </div>

      {selected && (
        <div className="channel-selection">
          <div>
            <b>Canali da registrare</b>
            <button
              className="text-button"
              onClick={() =>
                setSelectedChannels(
                  Array.from(
                    { length: selected.channels },
                    (_, index) => index,
                  ),
                )
              }
            >
              Tutti
            </button>
            <button
              className="text-button"
              onClick={() => setSelectedChannels([])}
            >
              Nessuno
            </button>
          </div>

          <div className="channel-grid">
            {Array.from(
              { length: selected.channels },
              (_, channel) => (
                <label key={channel}>
                  <input
                    type="checkbox"
                    disabled={status.running}
                    checked={selectedChannels.includes(channel)}
                    onChange={() => toggleChannel(channel)}
                  />
                  CH {channel + 1}
                </label>
              ),
            )}
          </div>
        </div>
      )}

      <div className="recording-status">
        <span>Durata <b>{duration(status.durationSeconds)}</b></span>
        <span>
          Dimensione <b>{(status.bytesWritten / 1_000_000).toFixed(1)} MB</b>
        </span>
        <span>Segmento <b>{status.segmentIndex || "—"}</b></span>
        <span className="recording-file">{status.filePath}</span>

        {status.error && <strong>{status.error}</strong>}

        {status.running ? (
          <button className="danger" onClick={stop}>
            Ferma registrazione
          </button>
        ) : (
          <button
            disabled={
              !selected ||
              !folder ||
              !interfaceIp ||
              selectedChannels.length === 0
            }
            onClick={start}
          >
            Avvia registrazione
          </button>
        )}
      </div>
    </section>
  );
}

export default Recorder;