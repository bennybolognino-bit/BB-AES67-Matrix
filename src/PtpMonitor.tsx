import { useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";

interface PtpStatus {
  running: boolean;
  healthy: boolean;
  interfaceIp: string;
  domain: number;
  grandmasterIdentity: string;
  priority1: number;
  priority2: number;
  clockClass: number;
  clockAccuracy: number;
  clockVariance: number;
  stepsRemoved: number;
  timeSource: number;
  utcOffset: number | null;
  syncRate: number;
  softwareOffsetUs: number | null;
  lastSyncMs: number;
  lastAnnounceMs: number;
  syncPackets: number;
  announcePackets: number;
  message: string;
}

interface Props {
  interfaceIp: string;
}

function PtpMonitor({ interfaceIp }: Props) {
  const startedInterface = useRef("");
  const [status, setStatus] = useState<PtpStatus | null>(null);
  const [error, setError] = useState("");

  async function start() {
    if (!interfaceIp) return;

    try {
      await invoke("start_ptp_monitor", { interfaceIp });
      startedInterface.current = interfaceIp;
      setError("");
    } catch (reason) {
      setError(String(reason));
    }
  }

  useEffect(() => {
    if (
      interfaceIp &&
      startedInterface.current !== interfaceIp
    ) {
      start();
    }
  }, [interfaceIp]);

  useEffect(() => {
    const timer = window.setInterval(async () => {
      setStatus(await invoke<PtpStatus>("get_ptp_status"));
    }, 1000);

    return () => window.clearInterval(timer);
  }, []);

  const syncAge = status?.lastSyncMs
    ? (Date.now() - status.lastSyncMs) / 1000
    : null;

  return (
    <section className="panel">
      <div className="panel-title">
        <div>
          <h2>Monitor PTPv2</h2>
          <p>{error || status?.message || "Inizializzazione PTP"}</p>
        </div>

        <div className="ptp-actions">
          <span
            className={
              status?.healthy
                ? "ptp-locked"
                : status?.running
                  ? "ptp-waiting"
                  : "rtp-offline"
            }
          >
            {status?.healthy
              ? "LOCKED"
              : status?.running
                ? "IN ATTESA"
                : "FERMO"}
          </span>

          <button className="secondary compact" onClick={start}>
            Riavvia
          </button>
        </div>
      </div>

      <div className="ptp-grid">
        <article>
          <span>Grandmaster</span>
          <b>{status?.grandmasterIdentity || "Non rilevato"}</b>
        </article>

        <article>
          <span>Dominio</span>
          <b>{status?.domain ?? "—"}</b>
        </article>

        <article>
          <span>Clock class</span>
          <b>{status?.clockClass || "—"}</b>
        </article>

        <article>
          <span>Priority 1 / 2</span>
          <b>
            {status
              ? `${status.priority1} / ${status.priority2}`
              : "—"}
          </b>
        </article>

        <article>
          <span>Steps removed</span>
          <b>{status?.stepsRemoved ?? "—"}</b>
        </article>

        <article>
          <span>Sync rate</span>
          <b>{status ? `${status.syncRate.toFixed(1)} pkt/s` : "—"}</b>
        </article>

        <article>
          <span>Offset software</span>
          <b>
            {status?.softwareOffsetUs != null
              ? `${status.softwareOffsetUs.toFixed(1)} µs`
              : "Non disponibile"}
          </b>
        </article>

        <article>
          <span>Ultimo Sync</span>
          <b>{syncAge != null ? `${syncAge.toFixed(1)} s fa` : "—"}</b>
        </article>
      </div>

      <div className="ptp-note">
        Offset calcolato con timestamp software Windows: utile per diagnosi,
        non per certificazione PTP.
      </div>
    </section>
  );
}

export default PtpMonitor;