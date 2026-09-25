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

interface RtpStats {
  streamId: string;
  running: boolean;
  online: boolean;
  packets: number;
  estimatedLost: number;
  outOfOrder: number;
  duplicates: number;
  jitterMs: number;
  bitrateMbps: number;
  meterSupported: boolean;
  levelsDbfs: number[];
  silent: boolean;
  clipping: boolean;
}

interface Props {
  streams: Stream[];
  interfaceIp: string;
}

function Meter({ level, channel }: { level: number; channel: number }) {
  const width = Math.max(0, Math.min(100, ((level + 60) / 60) * 100));

  return (
    <div className="meter-line">
      <span>CH {channel + 1}</span>
      <div className="meter-track">
        <i
          className={level >= -0.5 ? "clip" : level >= -12 ? "hot" : ""}
          style={{ width: `${width}%` }}
        />
      </div>
      <b>{level.toFixed(1)}</b>
    </div>
  );
}

function RtpMonitor({ streams, interfaceIp }: Props) {
  const [stats, setStats] = useState<RtpStats[]>([]);
  const [message, setMessage] = useState("Avvia il monitor desiderato.");

  useEffect(() => {
    async function refresh() {
      setStats(await invoke<RtpStats[]>("get_rtp_stats"));
    }

    refresh();
    const timer = window.setInterval(refresh, 250);
    return () => window.clearInterval(timer);
  }, []);

  const byStream = useMemo(
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
        codec: stream.codec,
        channels: stream.channels,
        interfaceIp,
      });

      setMessage(`Monitor audio avviato: ${stream.name}`);
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
          <h2>Monitor RTP e livelli audio</h2>
          <p>{message}</p>
        </div>
      </div>

      <div className="monitor-cards">
        {streams.map((stream) => {
          const item = byStream.get(stream.id);
          const visibleLevels = item?.levelsDbfs.slice(0, 16) ?? [];

          return (
            <article className="monitor-card" key={stream.id}>
              <div className="monitor-heading">
                <div>
                  <b>{stream.name}</b>
                  <small>
                    {stream.address}:{stream.port} · {stream.codec} ·{" "}
                    {stream.channels} CH
                  </small>
                </div>

                <div className="monitor-actions">
                  <span
                    className={
                      item?.clipping
                        ? "audio-clip"
                        : item?.silent
                          ? "audio-silent"
                          : item?.online
                            ? "rtp-online"
                            : "rtp-offline"
                    }
                  >
                    {item?.clipping
                      ? "CLIP"
                      : item?.silent
                        ? "SILENZIO"
                        : item?.online
                          ? "ONLINE"
                          : item?.running
                            ? "ATTESA"
                            : "FERMO"}
                  </span>

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
                </div>
              </div>

              {item?.meterSupported && visibleLevels.length > 0 ? (
                <div className="meters">
                  {visibleLevels.map((level, index) => (
                    <Meter level={level} channel={index} key={index} />
                  ))}

                  {stream.channels > 16 && (
                    <small>+ {stream.channels - 16} canali non visualizzati</small>
                  )}
                </div>
              ) : (
                <div className="meter-unavailable">
                  {item?.running
                    ? "VU meter disponibile solamente per PCM L16/L24."
                    : "Avvia il monitor per visualizzare i livelli."}
                </div>
              )}

              <div className="monitor-stats">
                <span>Pacchetti <b>{item?.packets.toLocaleString() ?? "—"}</b></span>
                <span>Persi <b>{item?.estimatedLost.toLocaleString() ?? "—"}</b></span>
                <span>Fuori seq. <b>{item?.outOfOrder.toLocaleString() ?? "—"}</b></span>
                <span>Jitter <b>{item ? item.jitterMs.toFixed(3) : "—"} ms</b></span>
                <span>Bitrate <b>{item ? item.bitrateMbps.toFixed(2) : "—"} Mb/s</b></span>
              </div>
            </article>
          );
        })}
      </div>
    </section>
  );
}

export default RtpMonitor;