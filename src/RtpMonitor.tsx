import { useEffect, useMemo, useState } from "react";
import { invoke } from "@tauri-apps/api/core";

interface Stream {
  id: string;
  name: string;
  address: string;
  port: number;
  sampleRate: number;
}

interface RtpStats {
  streamId: string;
  running: boolean;
  online: boolean;
  packets: number;
  bytes: number;
  estimatedLost: number;
  outOfOrder: number;
  duplicates: number;
  jitterMs: number;
  bitrateMbps: number;
  lastSequence: number | null;
  lastPacketAt: number;
}

interface Props {
  streams: Stream[];
  interfaceIp: string;
}

function RtpMonitor({ streams, interfaceIp }: Props) {
  const [stats, setStats] = useState<RtpStats[]>([]);
  const [message, setMessage] = useState(
    "Avvia il monitor sul flusso desiderato.",
  );

  useEffect(() => {
    async function refresh() {
      setStats(await invoke<RtpStats[]>("get_rtp_stats"));
    }

    refresh();
    const timer = window.setInterval(refresh, 1000);
    return () => window.clearInterval(timer);
  }, []);

  const statsByStream = useMemo(
    () => new Map(stats.map((item) => [item.streamId, item])),
    [stats],
  );

  async function start(stream: Stream) {
    try {
      await invoke("start_rtp_monitor", {
        streamId: stream.id,
        address: stream.address,
        port: stream.port,
        sampleRate: stream.sampleRate,
        interfaceIp,
      });

      setMessage(`Monitor RTP avviato: ${stream.name}`);
    } catch (error) {
      setMessage(`Errore monitor: ${error}`);
    }
  }

  async function stop(stream: Stream) {
    await invoke("stop_rtp_monitor", { streamId: stream.id });
    setMessage(`Monitor arrestato: ${stream.name}`);
  }

  return (
    <section className="panel">
      <div className="panel-title">
        <div>
          <h2>Monitor RTP</h2>
          <p>{message}</p>
        </div>
      </div>

      <div className="rtp-table">
        <div className="rtp-row rtp-head">
          <span>Flusso</span>
          <span>Stato</span>
          <span>Pacchetti</span>
          <span>Perduti</span>
          <span>Fuori seq.</span>
          <span>Jitter</span>
          <span>Bitrate</span>
          <span>Controllo</span>
        </div>

        {streams.map((stream) => {
          const item = statsByStream.get(stream.id);

          return (
            <div className="rtp-row" key={stream.id}>
              <span>
                <b>{stream.name}</b>
                <small>
                  {stream.address}:{stream.port}
                </small>
              </span>

              <span className={item?.online ? "rtp-online" : "rtp-offline"}>
                {item?.online
                  ? "ONLINE"
                  : item?.running
                    ? "ATTESA"
                    : "FERMO"}
              </span>

              <span>{item?.packets.toLocaleString() ?? "—"}</span>
              <span>{item?.estimatedLost.toLocaleString() ?? "—"}</span>
              <span>{item?.outOfOrder.toLocaleString() ?? "—"}</span>
              <span>
                {item ? `${item.jitterMs.toFixed(3)} ms` : "—"}
              </span>
              <span>
                {item ? `${item.bitrateMbps.toFixed(2)} Mb/s` : "—"}
              </span>

              <span>
                {item?.running ? (
                  <button className="danger compact" onClick={() => stop(stream)}>
                    Stop
                  </button>
                ) : (
                  <button
                    className="compact"
                    disabled={!interfaceIp}
                    onClick={() => start(stream)}
                  >
                    Monitora
                  </button>
                )}
              </span>
            </div>
          );
        })}
      </div>
    </section>
  );
}

export default RtpMonitor;
