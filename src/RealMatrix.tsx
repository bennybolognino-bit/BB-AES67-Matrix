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

interface Destination {
  id: string;
  name: string;
  address: string;
  port: number;
}

interface RouteStatus {
  id: string;
  name: string;
  sourceId: string;
  destination: string;
  channels: number;
  gainDb: number;
  mute: boolean;
  running: boolean;
  packetsSent: number;
  error: string;
}

interface CrosspointConfig {
  selectedChannels: number[];
  gainDb: number;
  mute: boolean;
}

interface PresetConnection extends CrosspointConfig {
  sourceId: string;
  destinationId: string;
}

interface Preset {
  name: string;
  destinations: Destination[];
  connections: PresetConnection[];
}

interface Props {
  streams: Stream[];
  interfaceIp: string;
}

const defaultDestinations: Destination[] = [
  {
    id: "tx-1",
    name: "AES67 TX 1",
    address: "239.69.100.1",
    port: 6000,
  },
  {
    id: "tx-2",
    name: "AES67 TX 2",
    address: "239.69.100.2",
    port: 6002,
  },
];

const keyFor = (sourceId: string, destinationId: string) =>
  `${sourceId}:${destinationId}`;

const delay = (milliseconds: number) =>
  new Promise((resolve) => window.setTimeout(resolve, milliseconds));

function RealMatrix({ streams, interfaceIp }: Props) {
  const compatible = useMemo(
    () =>
      streams.filter((stream) =>
        ["L16", "L24"].includes(stream.codec.toUpperCase()),
      ),
    [streams],
  );

  const [destinations, setDestinations] = useState<Destination[]>(() => {
    const saved = localStorage.getItem("bb-real-destinations");
    return saved ? JSON.parse(saved) : defaultDestinations;
  });

  const [configs, setConfigs] = useState<Record<string, CrosspointConfig>>(
    () => {
      const saved = localStorage.getItem("bb-crosspoint-configs");
      return saved ? JSON.parse(saved) : {};
    },
  );

  const [presets, setPresets] = useState<Preset[]>(() => {
    const saved = localStorage.getItem("bb-routing-presets");
    return saved ? JSON.parse(saved) : [];
  });

  const [routes, setRoutes] = useState<RouteStatus[]>([]);
  const [editing, setEditing] = useState<{
    stream: Stream;
    destination: Destination;
  } | null>(null);

  const [editChannels, setEditChannels] = useState<number[]>([]);
  const [editGain, setEditGain] = useState(0);
  const [editMute, setEditMute] = useState(false);
  const [message, setMessage] = useState("Matrice collegata al routing reale.");

  async function refresh() {
    setRoutes(await invoke<RouteStatus[]>("get_routes"));
  }

  useEffect(() => {
    refresh();
    const timer = window.setInterval(refresh, 1000);
    return () => window.clearInterval(timer);
  }, []);

  useEffect(() => {
    localStorage.setItem(
      "bb-real-destinations",
      JSON.stringify(destinations),
    );
  }, [destinations]);

  useEffect(() => {
    localStorage.setItem(
      "bb-crosspoint-configs",
      JSON.stringify(configs),
    );
  }, [configs]);

  useEffect(() => {
    localStorage.setItem("bb-routing-presets", JSON.stringify(presets));
  }, [presets]);

  function activeRoute(stream: Stream, destination: Destination) {
    return routes.find(
      (route) =>
        route.running &&
        route.sourceId === stream.id &&
        route.destination === `${destination.address}:${destination.port}`,
    );
  }

  function configuration(stream: Stream, destination: Destination) {
    return (
      configs[keyFor(stream.id, destination.id)] ?? {
        selectedChannels: Array.from(
          { length: stream.channels },
          (_, index) => index,
        ),
        gainDb: 0,
        mute: false,
      }
    );
  }

  async function startRoute(
    stream: Stream,
    destination: Destination,
    config: CrosspointConfig,
  ) {
    await invoke("start_route", {
      name: `${stream.name} → ${destination.name}`,
      sourceId: stream.id,
      sourceAddress: stream.address,
      sourcePort: stream.port,
      sampleRate: stream.sampleRate,
      codec: stream.codec,
      sourceChannels: stream.channels,
      selectedChannels: config.selectedChannels,
      destinationAddress: destination.address,
      destinationPort: destination.port,
      interfaceIp,
      gainDb: config.gainDb,
      mute: config.mute,
    });
  }

  async function toggle(stream: Stream, destination: Destination) {
    const active = activeRoute(stream, destination);

    try {
      if (active) {
        await invoke("stop_route", { routeId: active.id });
        setMessage(`${stream.name} scollegato da ${destination.name}`);
      } else {
        await startRoute(
          stream,
          destination,
          configuration(stream, destination),
        );
        setMessage(`${stream.name} collegato a ${destination.name}`);
      }

      await refresh();
    } catch (error) {
      setMessage(`Errore matrice: ${error}`);
    }
  }

  function openConfiguration(
    stream: Stream,
    destination: Destination,
  ) {
    const config = configuration(stream, destination);

    setEditing({ stream, destination });
    setEditChannels(config.selectedChannels);
    setEditGain(config.gainDb);
    setEditMute(config.mute);
  }

  function toggleEditChannel(channel: number) {
    setEditChannels((current) =>
      current.includes(channel)
        ? current.filter((item) => item !== channel)
        : [...current, channel].sort((a, b) => a - b),
    );
  }

  async function applyConfiguration() {
    if (!editing || editChannels.length === 0) return;

    const key = keyFor(editing.stream.id, editing.destination.id);
    const config: CrosspointConfig = {
      selectedChannels: editChannels,
      gainDb: editGain,
      mute: editMute,
    };

    setConfigs((current) => ({ ...current, [key]: config }));

    const active = activeRoute(editing.stream, editing.destination);

    try {
      if (active) {
        await invoke("stop_route", { routeId: active.id });
        await delay(1100);
        await startRoute(editing.stream, editing.destination, config);
      }

      setMessage("Channel mapping aggiornato");
      setEditing(null);
      await refresh();
    } catch (error) {
      setMessage(`Errore configurazione: ${error}`);
    }
  }

  function addDestination() {
    const name = window.prompt("Nome destinazione:", "Nuova uscita AES67");
    if (!name) return;

    const address = window.prompt(
      "Indirizzo multicast:",
      `239.69.100.${destinations.length + 10}`,
    );
    if (!address) return;

    const portText = window.prompt(
      "Porta UDP:",
      String(6100 + destinations.length * 2),
    );
    if (!portText) return;

    setDestinations((current) => [
      ...current,
      {
        id: `destination-${Date.now()}`,
        name,
        address,
        port: Number(portText),
      },
    ]);
  }

  async function removeDestination(destination: Destination) {
    const related = routes.filter(
      (route) =>
        route.running &&
        route.destination ===
          `${destination.address}:${destination.port}`,
    );

    for (const route of related) {
      await invoke("stop_route", { routeId: route.id });
    }

    setDestinations((current) =>
      current.filter((item) => item.id !== destination.id),
    );
  }

  function savePreset() {
    const name = window.prompt("Nome preset:", `Preset ${presets.length + 1}`);
    if (!name) return;

    const connections: PresetConnection[] = [];

    for (const route of routes.filter((item) => item.running)) {
      const destination = destinations.find(
        (item) =>
          `${item.address}:${item.port}` === route.destination,
      );

      const stream = compatible.find(
        (item) => item.id === route.sourceId,
      );

      if (!destination || !stream) continue;

      const config = configuration(stream, destination);

      connections.push({
        sourceId: stream.id,
        destinationId: destination.id,
        selectedChannels: config.selectedChannels,
        gainDb: route.gainDb,
        mute: route.mute,
      });
    }

    setPresets((current) => [
      ...current.filter((preset) => preset.name !== name),
      {
        name,
        destinations,
        connections,
      },
    ]);

    setMessage(`Preset salvato: ${name}`);
  }

  async function recallPreset(preset: Preset) {
    setMessage(`Richiamo preset ${preset.name}`);

    for (const route of routes.filter((item) => item.running)) {
      await invoke("stop_route", { routeId: route.id });
    }

    await delay(1200);
    setDestinations(preset.destinations);

    const updatedConfigs = { ...configs };

    for (const connection of preset.connections) {
      const stream = compatible.find(
        (item) => item.id === connection.sourceId,
      );

      const destination = preset.destinations.find(
        (item) => item.id === connection.destinationId,
      );

      if (!stream || !destination) continue;

      const config: CrosspointConfig = {
        selectedChannels: connection.selectedChannels,
        gainDb: connection.gainDb,
        mute: connection.mute,
      };

      updatedConfigs[keyFor(stream.id, destination.id)] = config;
      await startRoute(stream, destination, config);
    }

    setConfigs(updatedConfigs);
    await refresh();
    setMessage(`Preset richiamato: ${preset.name}`);
  }

  function deletePreset(name: string) {
    setPresets((current) =>
      current.filter((preset) => preset.name !== name),
    );
  }

  return (
    <section className="panel real-matrix-panel">
      <div className="panel-title">
        <div>
          <h2>Matrice AES67 reale</h2>
          <p>{message}</p>
        </div>

        <div className="actions">
          <button className="secondary" onClick={addDestination}>
            + Destinazione
          </button>
          <button onClick={savePreset}>Salva preset</button>
        </div>
      </div>

      <div className="preset-bar">
        {presets.map((preset) => (
          <div key={preset.name}>
            <button
              className="secondary compact"
              onClick={() => recallPreset(preset)}
            >
              {preset.name}
            </button>
            <button
              className="preset-delete"
              onClick={() => deletePreset(preset.name)}
            >
              ×
            </button>
          </div>
        ))}
      </div>

      <div className="matrix-wrap">
        <table className="matrix real-matrix">
          <thead>
            <tr>
              <th>Sorgente AES67</th>
              {destinations.map((destination) => (
                <th key={destination.id}>
                  <b>{destination.name}</b>
                  <small>
                    {destination.address}:{destination.port}
                  </small>
                  <button
                    className="destination-remove"
                    onClick={() => removeDestination(destination)}
                  >
                    ×
                  </button>
                </th>
              ))}
            </tr>
          </thead>

          <tbody>
            {compatible.map((stream) => (
              <tr key={stream.id}>
                <td>
                  <b>{stream.name}</b>
                  <small>
                    {stream.codec}/{stream.sampleRate} · {stream.channels} CH
                  </small>
                </td>

                {destinations.map((destination) => {
                  const active = activeRoute(stream, destination);
                  const config = configuration(stream, destination);

                  return (
                    <td key={destination.id}>
                      <div className="matrix-cell">
                        <button
                          className={`crosspoint ${
                            active ? "active" : ""
                          }`}
                          onClick={() => toggle(stream, destination)}
                        />

                        <button
                          className="cell-settings"
                          onClick={() =>
                            openConfiguration(stream, destination)
                          }
                        >
                          ⚙
                        </button>

                        <small>
                          {config.selectedChannels.length} CH ·{" "}
                          {config.gainDb.toFixed(1)} dB
                        </small>
                      </div>
                    </td>
                  );
                })}
              </tr>
            ))}
          </tbody>
        </table>
      </div>

      {editing && (
        <div className="crosspoint-editor">
          <div className="panel-title">
            <div>
              <h2>
                {editing.stream.name} → {editing.destination.name}
              </h2>
              <p>Channel mapping, gain e mute.</p>
            </div>
            <button
              className="secondary"
              onClick={() => setEditing(null)}
            >
              Chiudi
            </button>
          </div>

          <div className="editor-channels">
            {Array.from(
              { length: editing.stream.channels },
              (_, channel) => (
                <label key={channel}>
                  <input
                    type="checkbox"
                    checked={editChannels.includes(channel)}
                    onChange={() => toggleEditChannel(channel)}
                  />
                  CH {channel + 1}
                </label>
              ),
            )}
          </div>

          <div className="editor-controls">
            <label>
              Gain: {editGain.toFixed(1)} dB
              <input
                type="range"
                min="-60"
                max="12"
                step=".5"
                value={editGain}
                onChange={(event) =>
                  setEditGain(Number(event.target.value))
                }
              />
            </label>

            <label>
              <input
                type="checkbox"
                checked={editMute}
                onChange={(event) => setEditMute(event.target.checked)}
              />
              Mute
            </label>

            <button
              disabled={editChannels.length === 0}
              onClick={applyConfiguration}
            >
              Applica
            </button>
          </div>
        </div>
      )}
    </section>
  );
}

export default RealMatrix;