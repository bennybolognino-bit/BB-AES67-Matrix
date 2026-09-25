import {
  useCallback,
  useEffect,
  useMemo,
  useRef,
  useState,
} from "react";
import { invoke } from "@tauri-apps/api/core";
import "./App.css";
import RtpMonitor from "./RtpMonitor";
import AudioMonitor from "./AudioMonitor";
import Recorder from "./Recorder";

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

interface NetworkInterface {
  name: string;
  ip: string;
}

interface DiscoveryStatus {
  running: boolean;
  interfaceIp: string;
  message: string;
}

const destinations = [
  "Monitor Control Room",
  "Recorder A",
  "Program TX",
  "Studio Output",
];

const exampleSdp = `v=0
o=- 1 1 IN IP4 192.168.77.10
s=Example AES67 Stream
c=IN IP4 239.69.20.10/32
t=0 0
m=audio 5004 RTP/AVP 96
a=rtpmap:96 L24/48000/2
a=ptime:1`;

function App() {
  const initialized = useRef(false);

  const [streams, setStreams] = useState<Aes67Stream[]>([]);
  const [interfaces, setInterfaces] = useState<NetworkInterface[]>([]);
  const [selectedInterface, setSelectedInterface] = useState("");
  const [discovery, setDiscovery] = useState<DiscoveryStatus>({
    running: false,
    interfaceIp: "",
    message: "Discovery non avviata",
  });

  const [routes, setRoutes] = useState<Record<string, boolean>>({});
  const [sdp, setSdp] = useState(exampleSdp);
  const [message, setMessage] = useState("Selezione interfaccia AES67");
  const [showImport, setShowImport] = useState(false);
  const [switching, setSwitching] = useState(false);

  const refresh = useCallback(async () => {
    try {
      const [streamList, status] = await Promise.all([
        invoke<Aes67Stream[]>("get_streams"),
        invoke<DiscoveryStatus>("get_discovery_status"),
      ]);

      setStreams(streamList);
      setDiscovery(status);
    } catch (error) {
      setMessage(String(error));
    }
  }, []);

  const startDiscovery = useCallback(async (ip: string) => {
    if (!ip) return;

    setSwitching(true);
    setMessage(`Avvio discovery su ${ip}`);

    try {
      await invoke("start_discovery", { interfaceIp: ip });
      localStorage.setItem("bb-aes67-interface", ip);
      setMessage(`Discovery SAP avviata su ${ip}`);
    } catch (error) {
      setMessage(`Errore discovery: ${error}`);
    } finally {
      setSwitching(false);
    }
  }, []);

  useEffect(() => {
    if (initialized.current) return;
    initialized.current = true;

    async function initialize() {
      try {
        const available =
          await invoke<NetworkInterface[]>("list_network_interfaces");

        setInterfaces(available);

        const saved = localStorage.getItem("bb-aes67-interface");

        const preferred =
          available.find((item) => item.ip === saved) ??
          available.find((item) =>
            item.name.toUpperCase().includes("DANTE"),
          ) ??
          available[0];

        if (preferred) {
          setSelectedInterface(preferred.ip);
          await startDiscovery(preferred.ip);
        } else {
          setMessage("Nessuna interfaccia IPv4 disponibile");
        }

        await refresh();
      } catch (error) {
        setMessage(String(error));
      }
    }

    initialize();
  }, [refresh, startDiscovery]);

  useEffect(() => {
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

  async function changeInterface(ip: string) {
    setSelectedInterface(ip);
    await startDiscovery(ip);
    await refresh();
  }

  function toggleRoute(streamId: string, destination: string) {
    const key = `${streamId}:${destination}`;
    const updated = { ...routes, [key]: !routes[key] };

    setRoutes(updated);
    localStorage.setItem("bb-aes67-routes", JSON.stringify(updated));
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

  async function clearStreams() {
    await invoke("clear_streams");
    await refresh();
    setMessage("Elenco flussi azzerato");
  }

  return (
    <main>
      <header className="topbar">
        <div>
          <p className="eyebrow">Broadcast IP Audio Control</p>
          <h1>BB AES67 Matrix</h1>
        </div>

        <div className="actions">
          <label className="network-selector">
            <span>Interfaccia AES67</span>
            <select
              value={selectedInterface}
              disabled={switching}
              onChange={(event) => changeInterface(event.target.value)}
            >
              {interfaces.map((item) => (
                <option value={item.ip} key={`${item.name}-${item.ip}`}>
                  {item.name} — {item.ip}
                </option>
              ))}
            </select>
          </label>

          <button
            className="secondary"
            onClick={() => setShowImport(!showImport)}
          >
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
          <span>Discovery SAP</span>
          <strong className={discovery.running ? "online" : "offline"}>
            {discovery.running ? "Attivo" : "Fermo"}
          </strong>
        </article>

        <article>
          <span>Interfaccia</span>
          <strong className="interface-address">
            {discovery.interfaceIp || "Non selezionata"}
          </strong>
        </article>
      </section>

      <div className={`discovery-message ${discovery.running ? "ok" : "error"}`}>
        <span>{discovery.message || message}</span>
        <span>{message}</span>
      </div>

      {showImport && (
        <section className="panel import-panel">
          <div className="panel-title">
            <div>
              <h2>Importazione SDP</h2>
              <p>Incolla la descrizione SDP del trasmettitore.</p>
            </div>
            <button onClick={importSdp}>Aggiungi flusso</button>
          </div>

          <textarea
            value={sdp}
            onChange={(event) => setSdp(event.target.value)}
          />
        </section>
      )}

      <section className="panel">
        <div className="panel-title">
          <div>
            <h2>Flussi AES67</h2>
            <p>Flussi annunciati tramite SAP sulla rete selezionata.</p>
          </div>

          <div className="actions">
            <button className="secondary" onClick={refresh}>
              Aggiorna
            </button>
            <button className="danger" onClick={clearStreams}>
              Pulisci elenco
            </button>
          </div>
        </div>

        <div className="stream-table">
          <div className="table-row table-head">
            <span>Nome</span>
            <span>Multicast</span>
            <span>Formato</span>
            <span>Canali</span>
            <span>Ultimo annuncio</span>
          </div>

          {streams.length === 0 && (
            <div className="empty">
              Nessun flusso rilevato sulla scheda selezionata.
            </div>
          )}

          {streams.map((stream) => (
            <div className="table-row" key={stream.id}>
              <span>
                <i className="stream-dot" />
                <b>{stream.name}</b>
              </span>

              <span>
                {stream.address}:{stream.port}
              </span>

              <span>
                {stream.codec} · {stream.sampleRate / 1000} kHz
              </span>

              <span>{stream.channels}</span>

              <span>
                {new Date(stream.lastSeen * 1000).toLocaleTimeString()}
              </span>
            </div>
          ))}
        </div>
      </section>
      <RtpMonitor streams={streams} interfaceIp={selectedInterface} />

      <AudioMonitor streams={streams} interfaceIp={selectedInterface} />

      <Recorder streams={streams} interfaceIp={selectedInterface} />


      <section className="panel">
        <div className="panel-title">
          <div>
            <h2>Matrice di routing</h2>
            <p>Configurazione locale sorgenti × destinazioni.</p>
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
                    <small>
                      {stream.address}:{stream.port}
                    </small>
                  </td>

                  {destinations.map((destination) => {
                    const key = `${stream.id}:${destination}`;

                    return (
                      <td key={destination}>
                        <button
                          className={`crosspoint ${
                            routes[key] ? "active" : ""
                          }`}
                          onClick={() =>
                            toggleRoute(stream.id, destination)
                          }
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
        Prossima fase: monitor RTP, perdita pacchetti, sequence error e jitter.
      </footer>
    </main>
  );
}

export default App;
