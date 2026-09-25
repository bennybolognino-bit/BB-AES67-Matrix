import { useCallback, useEffect, useMemo, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import "./App.css";

interface Aes67Stream {
  id: string;
  name: string;
  address: string;
  port: number;
  codec: string;
  sampleRate: number;
  channels: number;
  source: string;
  lastSeen: number;
}

const destinations = [
  "Monitor Control Room",
  "Recorder A",
  "Program TX",
  "Studio Output",
];

const exampleSdp = `v=0
o=- 1 1 IN IP4 192.168.1.10
s=Example AES67 Stream
c=IN IP4 239.69.20.10/32
t=0 0
m=audio 5004 RTP/AVP 96
a=rtpmap:96 L24/48000/2
a=ptime:1`;

function App() {
  const [streams, setStreams] = useState<Aes67Stream[]>([]);
  const [routes, setRoutes] = useState<Record<string, boolean>>({});
  const [sdp, setSdp] = useState(exampleSdp);
  const [message, setMessage] = useState("Discovery SAP attiva");
  const [showImport, setShowImport] = useState(false);

  const refresh = useCallback(async () => {
    try {
      setStreams(await invoke<Aes67Stream[]>("get_streams"));
    } catch (error) {
      setMessage(String(error));
    }
  }, []);

  useEffect(() => {
    refresh();
    const timer = window.setInterval(refresh, 2000);
    return () => window.clearInterval(timer);
  }, [refresh]);

  useEffect(() => {
    const saved = localStorage.getItem("bb-aes67-routes");
    if (saved) setRoutes(JSON.parse(saved));
  }, []);

  const activeRoutes = useMemo(
    () => Object.values(routes).filter(Boolean).length,
    [routes],
  );

  function toggleRoute(streamId: string, destination: string) {
    const key = `${streamId}:${destination}`;
    const updated = { ...routes, [key]: !routes[key] };
    setRoutes(updated);
    localStorage.setItem("bb-aes67-routes", JSON.stringify(updated));
  }

  async function addDemo() {
    await invoke("add_demo_streams");
    await refresh();
    setMessage("Flussi dimostrativi caricati");
  }

  async function importSdp() {
    try {
      await invoke("import_sdp", { sdp });
      await refresh();
      setShowImport(false);
      setMessage("SDP importato correttamente");
    } catch (error) {
      setMessage(`Errore SDP: ${error}`);
    }
  }

  return (
    <main>
      <header className="topbar">
        <div>
          <p className="eyebrow">Broadcast IP Audio Control</p>
          <h1>BB AES67 Matrix</h1>
        </div>

        <div className="actions">
          <button className="secondary" onClick={addDemo}>
            Modalità demo
          </button>
          <button onClick={() => setShowImport(!showImport)}>
            Importa SDP
          </button>
        </div>
      </header>

      <section className="status-grid">
        <article>
          <span>Flussi rilevati</span>
          <strong>{streams.length}</strong>
        </article>
        <article>
          <span>Routing configurati</span>
          <strong>{activeRoutes}</strong>
        </article>
        <article>
          <span>Discovery</span>
          <strong className="online">SAP attivo</strong>
        </article>
        <article>
          <span>PTP</span>
          <strong className="warning">Non monitorato</strong>
        </article>
      </section>

      {showImport && (
        <section className="panel import-panel">
          <div className="panel-title">
            <div>
              <h2>Importazione SDP</h2>
              <p>Incolla la descrizione SDP annunciata dal trasmettitore.</p>
            </div>
            <button onClick={importSdp}>Aggiungi flusso</button>
          </div>
          <textarea value={sdp} onChange={(event) => setSdp(event.target.value)} />
        </section>
      )}

      <section className="panel">
        <div className="panel-title">
          <div>
            <h2>Flussi AES67</h2>
            <p>{message}</p>
          </div>
          <button className="secondary" onClick={refresh}>
            Aggiorna
          </button>
        </div>

        <div className="stream-table">
          <div className="table-row table-head">
            <span>Nome</span>
            <span>Multicast</span>
            <span>Formato</span>
            <span>Canali</span>
            <span>Origine</span>
          </div>

          {streams.length === 0 && (
            <div className="empty">
              Nessun flusso rilevato. Attiva la modalità demo oppure importa un SDP.
            </div>
          )}

          {streams.map((stream) => (
            <div className="table-row" key={stream.id}>
              <span>
                <i className="stream-dot" />
                <b>{stream.name}</b>
              </span>
              <span>{stream.address}:{stream.port}</span>
              <span>{stream.codec} · {stream.sampleRate / 1000} kHz</span>
              <span>{stream.channels}</span>
              <span>{stream.source}</span>
            </div>
          ))}
        </div>
      </section>

      <section className="panel">
        <div className="panel-title">
          <div>
            <h2>Matrice di routing</h2>
            <p>Seleziona gli incroci sorgente-destinazione.</p>
          </div>
        </div>

        <div className="matrix-wrap">
          <table className="matrix">
            <thead>
              <tr>
                <th>Sorgente</th>
                {destinations.map((destination) => (
                  <th key={destination}>{destination}</th>
                ))}
              </tr>
            </thead>
            <tbody>
              {streams.map((stream) => (
                <tr key={stream.id}>
                  <td>
                    <b>{stream.name}</b>
                    <small>{stream.channels} canali</small>
                  </td>
                  {destinations.map((destination) => {
                    const key = `${stream.id}:${destination}`;
                    return (
                      <td key={destination}>
                        <button
                          className={`crosspoint ${routes[key] ? "active" : ""}`}
                          onClick={() => toggleRoute(stream.id, destination)}
                          aria-label={`${stream.name} verso ${destination}`}
                        />
                      </td>
                    );
                  })}
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      </section>

      <footer>
        La matrice di questo MVP salva la configurazione ma non modifica ancora
        ricevitori Dante o NMOS.
      </footer>
    </main>
  );
}

export default App;
