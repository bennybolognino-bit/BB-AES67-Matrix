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
  bytesSent: number;
  error: string;
  sdp: string;
}

interface Props {
  streams: Stream[];
  interfaceIp: string;
}

function RoutingEngine({ streams, interfaceIp }: Props) {
  const compatible = useMemo(
    () =>
      streams.filter((stream) =>
        ["L16", "L24"].includes(stream.codec.toUpperCase()),
      ),
    [streams],
  );

  const [sourceId, setSourceId] = useState("");
  const [name, setName] = useState("BB AES67 Output");
  const [address, setAddress] = useState("239.69.100.1");
  const [port, setPort] = useState(6000);
  const [gainDb, setGainDb] = useState(0);
  const [selectedChannels, setSelectedChannels] = useState<number[]>([]);
  const [routes, setRoutes] = useState<RouteStatus[]>([]);
  const [message, setMessage] = useState("Configura una nuova uscita AES67.");

  const source = compatible.find((stream) => stream.id === sourceId);

  useEffect(() => {
    if (!source && compatible[0]) selectSource(compatible[0].id);
  }, [compatible]);

  useEffect(() => {
    async function refresh() {
      setRoutes(await invoke<RouteStatus[]>("get_routes"));
    }

    refresh();
    const timer = window.setInterval(refresh, 1000);
    return () => window.clearInterval(timer);
  }, []);

  function selectSource(id: string) {
    const stream = compatible.find((item) => item.id === id);
    setSourceId(id);

    if (stream) {
      setName(`${stream.name} - BB Output`);
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

  async function start() {
    if (!source) return;

    try {
      await invoke("start_route", {
        name,
        sourceId: source.id,
        sourceAddress: source.address,
        sourcePort: source.port,
        sampleRate: source.sampleRate,
        codec: source.codec,
        sourceChannels: source.channels,
        selectedChannels,
        destinationAddress: address,
        destinationPort: port,
        interfaceIp,
        gainDb,
        mute: false,
      });

      setMessage(`Uscita AES67 avviata su ${address}:${port}`);
    } catch (error) {
      setMessage(`Errore routing: ${error}`);
    }
  }

  async function update(route: RouteStatus, gain: number, mute: boolean) {
    await invoke("update_route", {
      routeId: route.id,
      gainDb: gain,
      mute,
    });
  }

  async function stop(route: RouteStatus) {
    await invoke("stop_route", { routeId: route.id });
  }

  return (
    <section className="panel">
      <div className="panel-title">
        <div>
          <h2>Routing e ritrasmissione AES67</h2>
          <p>{message}</p>
        </div>
      </div>

      <div className="route-form">
        <label>
          <span>Sorgente</span>
          <select value={sourceId} onChange={(e) => selectSource(e.target.value)}>
            {compatible.map((stream) => (
              <option value={stream.id} key={stream.id}>{stream.name}</option>
            ))}
          </select>
        </label>

        <label>
          <span>Nome uscita</span>
          <input value={name} onChange={(e) => setName(e.target.value)} />
        </label>

        <label>
          <span>Multicast destinazione</span>
          <input value={address} onChange={(e) => setAddress(e.target.value)} />
        </label>

        <label>
          <span>Porta</span>
          <input
            type="number"
            value={port}
            onChange={(e) => setPort(Number(e.target.value))}
          />
        </label>

        <label>
          <span>Gain: {gainDb.toFixed(1)} dB</span>
          <input
            type="range"
            min="-60"
            max="12"
            step=".5"
            value={gainDb}
            onChange={(e) => setGainDb(Number(e.target.value))}
          />
        </label>
      </div>

      {source && (
        <div className="route-channels">
          {Array.from({ length: source.channels }, (_, channel) => (
            <label key={channel}>
              <input
                type="checkbox"
                checked={selectedChannels.includes(channel)}
                onChange={() => toggleChannel(channel)}
              />
              CH {channel + 1}
            </label>
          ))}
        </div>
      )}

      <div className="route-start">
        <button
          disabled={
            !source ||
            !interfaceIp ||
            selectedChannels.length === 0 ||
            !address ||
            !port
          }
          onClick={start}
        >
          Crea uscita AES67
        </button>
      </div>

      <div className="active-routes">
        {routes.map((route) => (
          <article key={route.id}>
            <div>
              <b>{route.name}</b>
              <small>{route.destination} · {route.channels} CH</small>
            </div>

            <span className={route.running ? "rtp-online" : "rtp-offline"}>
              {route.running ? "ON AIR" : "FERMA"}
            </span>

            <span>{route.packetsSent.toLocaleString()} pacchetti</span>
            <span>{(route.bytesSent / 1_000_000).toFixed(1)} MB</span>

            <label>
              <span>{route.gainDb.toFixed(1)} dB</span>
              <input
                type="range"
                min="-60"
                max="12"
                step=".5"
                defaultValue={route.gainDb}
                onChange={(event) =>
                  update(route, Number(event.target.value), route.mute)
                }
              />
            </label>

            <button
              className={route.mute ? "danger compact" : "secondary compact"}
              onClick={() => update(route, route.gainDb, !route.mute)}
            >
              {route.mute ? "MUTED" : "Mute"}
            </button>

            {route.running && (
              <button className="danger compact" onClick={() => stop(route)}>
                Stop
              </button>
            )}

            {route.error && <strong>{route.error}</strong>}
          </article>
        ))}
      </div>
    </section>
  );
}

export default RoutingEngine;