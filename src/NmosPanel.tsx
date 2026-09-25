import { useState } from "react";
import { invoke } from "@tauri-apps/api/core";

interface Registry {
  name: string;
  hostname: string;
  url: string;
  apiVersion: string;
  priority: string;
}

interface Sender {
  id: string;
  deviceId: string;
  flowId: string;
  label: string;
  description: string;
  transport: string;
  manifestHref: string;
}

interface Receiver {
  id: string;
  deviceId: string;
  label: string;
  description: string;
  transport: string;
  format: string;
  subscribedSenderId: string;
  connectionApi: string;
}

interface Resources {
  registryUrl: string;
  nodes: unknown[];
  devices: unknown[];
  senders: Sender[];
  receivers: Receiver[];
}

function NmosPanel() {
  const [registries, setRegistries] = useState<Registry[]>([]);
  const [url, setUrl] = useState(
    localStorage.getItem("bb-nmos-registry") ??
      "http://127.0.0.1:3211",
  );
  const [resources, setResources] = useState<Resources | null>(null);
  const [senderId, setSenderId] = useState("");
  const [receiverId, setReceiverId] = useState("");
  const [message, setMessage] = useState(
    "Ricerca registry NMOS non ancora avviata.",
  );
  const [busy, setBusy] = useState(false);

  const selectedReceiver = resources?.receivers.find(
    (receiver) => receiver.id === receiverId,
  );

  async function discover() {
    setBusy(true);
    setMessage("Ricerca NMOS tramite mDNS...");

    try {
      const result =
        await invoke<Registry[]>("discover_nmos_registries");

      setRegistries(result);

      if (result[0]) {
        setUrl(result[0].url);
        setMessage(`${result.length} registry NMOS rilevati`);
      } else {
        setMessage("Nessun registry NMOS rilevato");
      }
    } catch (error) {
      setMessage(`Errore discovery: ${error}`);
    } finally {
      setBusy(false);
    }
  }

  async function loadRegistry(target = url) {
    setBusy(true);
    setMessage("Caricamento risorse IS-04...");

    try {
      const result = await invoke<Resources>("query_nmos_registry", {
        url: target,
      });

      setResources(result);
      localStorage.setItem("bb-nmos-registry", target);

      if (result.senders[0]) setSenderId(result.senders[0].id);
      if (result.receivers[0]) setReceiverId(result.receivers[0].id);

      setMessage(
        `${result.senders.length} sender e ${result.receivers.length} receiver`,
      );
    } catch (error) {
      setMessage(`Errore IS-04: ${error}`);
    } finally {
      setBusy(false);
    }
  }

  async function connect() {
    if (!selectedReceiver || !senderId) return;

    try {
      const result = await invoke<string>("connect_nmos_receiver", {
        connectionApi: selectedReceiver.connectionApi,
        receiverId: selectedReceiver.id,
        senderId,
      });

      setMessage(result);
      await loadRegistry();
    } catch (error) {
      setMessage(`Errore IS-05: ${error}`);
    }
  }

  async function disconnect() {
    if (!selectedReceiver) return;

    try {
      const result = await invoke<string>("connect_nmos_receiver", {
        connectionApi: selectedReceiver.connectionApi,
        receiverId: selectedReceiver.id,
        senderId: null,
      });

      setMessage(result);
      await loadRegistry();
    } catch (error) {
      setMessage(`Errore IS-05: ${error}`);
    }
  }

  return (
    <section className="panel">
      <div className="panel-title">
        <div>
          <h2>NMOS IS-04 / IS-05</h2>
          <p>{message}</p>
        </div>

        <button disabled={busy} onClick={discover}>
          Cerca registry
        </button>
      </div>

      {registries.length > 0 && (
        <div className="nmos-registries">
          {registries.map((registry) => (
            <button
              className="secondary"
              key={registry.url}
              onClick={() => {
                setUrl(registry.url);
                loadRegistry(registry.url);
              }}
            >
              {registry.hostname} · {registry.apiVersion}
            </button>
          ))}
        </div>
      )}

      <div className="nmos-url">
        <input
          value={url}
          onChange={(event) => setUrl(event.target.value)}
          placeholder="http://registry:3211"
        />
        <button disabled={busy || !url} onClick={() => loadRegistry()}>
          Connetti registry
        </button>
      </div>

      {resources && (
        <>
          <div className="nmos-summary">
            <article>
              <span>Nodi</span>
              <b>{resources.nodes.length}</b>
            </article>
            <article>
              <span>Dispositivi</span>
              <b>{resources.devices.length}</b>
            </article>
            <article>
              <span>Sender</span>
              <b>{resources.senders.length}</b>
            </article>
            <article>
              <span>Receiver</span>
              <b>{resources.receivers.length}</b>
            </article>
          </div>

          <div className="nmos-routing">
            <label>
              <span>Sender</span>
              <select
                value={senderId}
                onChange={(event) => setSenderId(event.target.value)}
              >
                {resources.senders.map((sender) => (
                  <option value={sender.id} key={sender.id}>
                    {sender.label || sender.id} · {sender.transport}
                  </option>
                ))}
              </select>
            </label>

            <span className="nmos-arrow">→</span>

            <label>
              <span>Receiver</span>
              <select
                value={receiverId}
                onChange={(event) => setReceiverId(event.target.value)}
              >
                {resources.receivers.map((receiver) => (
                  <option value={receiver.id} key={receiver.id}>
                    {receiver.label || receiver.id}
                    {!receiver.connectionApi ? " · senza IS-05" : ""}
                  </option>
                ))}
              </select>
            </label>

            <button
              disabled={
                !senderId ||
                !selectedReceiver?.connectionApi
              }
              onClick={connect}
            >
              Collega
            </button>

            <button
              className="danger"
              disabled={!selectedReceiver?.connectionApi}
              onClick={disconnect}
            >
              Scollega
            </button>
          </div>

          <div className="nmos-resource-list">
            {resources.receivers.map((receiver) => (
              <div key={receiver.id}>
                <b>{receiver.label || receiver.id}</b>
                <span>{receiver.format}</span>
                <span>
                  {receiver.subscribedSenderId
                    ? `Sender: ${receiver.subscribedSenderId}`
                    : "Non collegato"}
                </span>
                <span>
                  {receiver.connectionApi ? "IS-05" : "Solo IS-04"}
                </span>
              </div>
            ))}
          </div>
        </>
      )}
    </section>
  );
}

export default NmosPanel;