import { useEffect, useRef, useState } from "react";
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
  estimatedLost: number;
  jitterMs: number;
  silent: boolean;
  clipping: boolean;
}

interface AlarmEvent {
  id: string;
  timestamp: number;
  severity: "info" | "warning" | "critical";
  key: string;
  message: string;
}

interface Props {
  streams: Stream[];
  interfaceIp: string;
}

function AlarmCenter({ streams, interfaceIp }: Props) {
  const streamsRef = useRef(streams);
  const activeRef = useRef(new Set<string>());
  const previousLossRef = useRef(new Map<string, number>());

  const [events, setEvents] = useState<AlarmEvent[]>(() => {
    const saved = localStorage.getItem("bb-aes67-events");
    return saved ? JSON.parse(saved) : [];
  });

  const [activeCount, setActiveCount] = useState(0);
  const [jitterThreshold, setJitterThreshold] = useState(() =>
    Number(localStorage.getItem("bb-jitter-threshold") ?? 2),
  );
  const [message, setMessage] = useState(
    "Gli allarmi richiedono il monitor RTP attivo.",
  );

  useEffect(() => {
    streamsRef.current = streams;
  }, [streams]);

  function emit(
    key: string,
    severity: AlarmEvent["severity"],
    message: string,
  ) {
    const event: AlarmEvent = {
      id: `${Date.now()}-${Math.random()}`,
      timestamp: Date.now(),
      severity,
      key,
      message,
    };

    setEvents((current) => {
      const updated = [event, ...current].slice(0, 500);
      localStorage.setItem("bb-aes67-events", JSON.stringify(updated));
      return updated;
    });
  }

  function synchronizeAlarm(
    key: string,
    condition: boolean,
    severity: AlarmEvent["severity"],
    message: string,
  ) {
    const active = activeRef.current;

    if (condition && !active.has(key)) {
      active.add(key);
      emit(key, severity, message);
    } else if (!condition && active.has(key)) {
      active.delete(key);
      emit(`${key}-resolved`, "info", `Risolto: ${message}`);
    }
  }

  useEffect(() => {
    const timer = window.setInterval(async () => {
      const stats = await invoke<RtpStats[]>("get_rtp_stats");
      const names = new Map(
        streamsRef.current.map((stream) => [stream.id, stream.name]),
      );

      for (const item of stats) {
        if (!item.running) continue;

        const name = names.get(item.streamId) ?? item.streamId;

        synchronizeAlarm(
          `${item.streamId}:offline`,
          !item.online,
          "critical",
          `${name}: flusso RTP assente`,
        );

        synchronizeAlarm(
          `${item.streamId}:silence`,
          item.online && item.silent,
          "warning",
          `${name}: silenzio audio oltre 3 secondi`,
        );

        synchronizeAlarm(
          `${item.streamId}:clip`,
          item.online && item.clipping,
          "critical",
          `${name}: clipping audio`,
        );

        synchronizeAlarm(
          `${item.streamId}:jitter`,
          item.online && item.jitterMs > jitterThreshold,
          "warning",
          `${name}: jitter superiore a ${jitterThreshold.toFixed(1)} ms`,
        );

        const previousLoss =
          previousLossRef.current.get(item.streamId) ?? item.estimatedLost;

        if (item.estimatedLost > previousLoss) {
          emit(
            `${item.streamId}:loss`,
            "critical",
            `${name}: ${item.estimatedLost - previousLoss} nuovi pacchetti RTP persi`,
          );
        }

        previousLossRef.current.set(
          item.streamId,
          item.estimatedLost,
        );
      }

      setActiveCount(activeRef.current.size);
    }, 1000);

    return () => window.clearInterval(timer);
  }, [jitterThreshold]);

  async function monitorAll() {
    const compatible = streams.filter((stream) =>
      ["L16", "L24"].includes(stream.codec.toUpperCase()),
    );

    let started = 0;

    for (const stream of compatible) {
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
        started += 1;
      } catch {
        // Gli errori dei singoli flussi non fermano gli altri monitor.
      }
    }

    setMessage(`Sorveglianza attiva su ${started} flussi`);
  }

  function changeThreshold(value: number) {
    setJitterThreshold(value);
    localStorage.setItem("bb-jitter-threshold", String(value));
  }

  function clearHistory() {
    setEvents([]);
    localStorage.removeItem("bb-aes67-events");
  }

  return (
    <section className="panel">
      <div className="panel-title">
        <div>
          <h2>Allarmi e storico eventi</h2>
          <p>{message}</p>
        </div>

        <div className="alarm-summary">
          <span className={activeCount ? "alarm-active" : "rtp-online"}>
            {activeCount} allarmi attivi
          </span>
          <button onClick={monitorAll}>Sorveglia tutti</button>
          <button className="secondary" onClick={clearHistory}>
            Cancella storico
          </button>
        </div>
      </div>

      <div className="alarm-settings">
        <label>
          Soglia jitter
          <input
            type="number"
            min=".1"
            max="100"
            step=".1"
            value={jitterThreshold}
            onChange={(event) =>
              changeThreshold(Number(event.target.value))
            }
          />
          ms
        </label>
      </div>

      <div className="event-list">
        {events.length === 0 && (
          <div className="empty">Nessun evento registrato.</div>
        )}

        {events.slice(0, 100).map((event) => (
          <div className="event-row" key={event.id}>
            <span className={`event-severity ${event.severity}`}>
              {event.severity}
            </span>
            <time>
              {new Date(event.timestamp).toLocaleString()}
            </time>
            <span>{event.message}</span>
          </div>
        ))}
      </div>
    </section>
  );
}

export default AlarmCenter;