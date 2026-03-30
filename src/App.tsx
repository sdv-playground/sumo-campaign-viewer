import { useState, useEffect } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import "./App.css";

// ECU state in the flash lifecycle
type EcuPhase =
  | "idle"
  | "session"
  | "security"
  | "uploading"
  | "verifying"
  | "flashing"
  | "finalizing"
  | "resetting"
  | "trial"
  | "committed"
  | "rolled_back"
  | "failed";

interface EcuState {
  id: string;
  name: string;
  phase: EcuPhase;
  version?: string;
  previousVersion?: string;
  supportsRollback: boolean;
  progress?: number;
  error?: string;
}

interface CampaignState {
  status: "idle" | "running" | "awaiting_commit" | "committed" | "rolled_back" | "failed";
  ecus: EcuState[];
}

// Backend types (snake_case from Rust)
interface BackendEcuStatus {
  id: string;
  name: string;
  phase: string;
  version: string | null;
  previous_version: string | null;
  supports_rollback: boolean;
  progress: number | null;
  error: string | null;
}

interface BackendCampaignStatus {
  ecus: BackendEcuStatus[];
}

// Phase display config
const PHASE_CONFIG: Record<EcuPhase, { label: string; color: string }> = {
  idle: { label: "Idle", color: "var(--pending)" },
  session: { label: "Session", color: "var(--accent)" },
  security: { label: "Security", color: "var(--accent)" },
  uploading: { label: "Upload", color: "var(--accent)" },
  verifying: { label: "Verify", color: "var(--accent)" },
  flashing: { label: "Flash", color: "var(--accent)" },
  finalizing: { label: "Finalize", color: "var(--accent)" },
  resetting: { label: "Reset", color: "var(--warning)" },
  trial: { label: "Trial", color: "var(--trial)" },
  committed: { label: "Committed", color: "var(--success)" },
  rolled_back: { label: "Rolled Back", color: "var(--warning)" },
  failed: { label: "Failed", color: "var(--error)" },
};

function mapBackendEcu(ecu: BackendEcuStatus): EcuState {
  return {
    id: ecu.id,
    name: ecu.name,
    phase: (ecu.phase as EcuPhase) || "idle",
    version: ecu.version ?? undefined,
    previousVersion: ecu.previous_version ?? undefined,
    supportsRollback: ecu.supports_rollback,
    progress: ecu.progress ?? undefined,
    error: ecu.error ?? undefined,
  };
}

function deriveStatus(ecus: EcuState[]): CampaignState["status"] {
  if (ecus.length === 0) return "idle";
  if (ecus.every((e) => e.phase === "idle")) return "idle";
  if (ecus.some((e) => e.phase === "failed")) return "failed";
  if (ecus.every((e) => e.phase === "committed")) return "committed";
  if (ecus.every((e) => e.phase === "rolled_back")) return "rolled_back";
  if (ecus.every((e) => e.phase === "trial" || e.phase === "committed"))
    return "awaiting_commit";
  return "running";
}

function EcuCard({ ecu }: { ecu: EcuState }) {
  const config = PHASE_CONFIG[ecu.phase];
  return (
    <div className="ecu-card">
      <div className="ecu-header">
        <span className="ecu-name">{ecu.name}</span>
        <span className="ecu-id">{ecu.id}</span>
      </div>
      <div className="ecu-phase" style={{ borderColor: config.color }}>
        <span className="phase-dot" style={{ background: config.color }} />
        <span className="phase-label">{config.label}</span>
      </div>
      <div className="ecu-details">
        {ecu.version && (
          <div className="ecu-detail-row">
            <span className="detail-label">Version</span>
            <span className="detail-value">{ecu.version}</span>
          </div>
        )}
        {ecu.previousVersion && (
          <div className="ecu-detail-row">
            <span className="detail-label">Previous</span>
            <span className="detail-value version-prev">{ecu.previousVersion}</span>
          </div>
        )}
        {ecu.version && (
          <div className="ecu-detail-row">
            <span className="detail-label">Rollback</span>
            <span className={`detail-value ${ecu.supportsRollback ? "rollback-yes" : "rollback-no"}`}>
              {ecu.supportsRollback ? "supported" : "n/a"}
            </span>
          </div>
        )}
      </div>
      {ecu.progress !== undefined && ecu.progress < 100 && (
        <div className="ecu-progress">
          <div className="progress-bar" style={{ width: `${ecu.progress}%` }} />
          <span className="progress-label">{ecu.progress.toFixed(0)}%</span>
        </div>
      )}
      {ecu.error && <div className="ecu-error">{ecu.error}</div>}
    </div>
  );
}

function CampaignTimeline({ ecus }: { ecus: EcuState[] }) {
  const phases: EcuPhase[] = [
    "session", "security", "uploading", "verifying",
    "flashing", "finalizing", "resetting", "trial",
  ];

  return (
    <div className="timeline">
      <div className="timeline-header">
        <div className="timeline-ecu-label" />
        {phases.map((p) => (
          <div key={p} className="timeline-phase-label">
            {PHASE_CONFIG[p].label}
          </div>
        ))}
      </div>
      {ecus.map((ecu) => {
        const currentIdx = phases.indexOf(ecu.phase as EcuPhase);
        return (
          <div key={ecu.id} className="timeline-row">
            <div className="timeline-ecu-label">{ecu.id}</div>
            {phases.map((p, i) => {
              const config = PHASE_CONFIG[p];
              const state =
                i < currentIdx ? "done" :
                i === currentIdx ? "active" : "pending";
              return (
                <div
                  key={p}
                  className={`timeline-cell ${state}`}
                  style={state === "active" ? { background: config.color } : {}}
                />
              );
            })}
          </div>
        );
      })}
    </div>
  );
}

function App() {
  const [serverUrl, setServerUrl] = useState("http://localhost:4000");
  const [mode, setMode] = useState<"observe" | "drive">("observe");
  const [connected, setConnected] = useState(false);
  const [connecting, setConnecting] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [campaign, setCampaign] = useState<CampaignState>({
    status: "idle",
    ecus: [],
  });

  // Listen for backend polling events
  useEffect(() => {
    let unlisten: UnlistenFn | undefined;

    listen<BackendCampaignStatus>("campaign-state-update", (event) => {
      const ecus = event.payload.ecus.map(mapBackendEcu);
      setCampaign({
        status: deriveStatus(ecus),
        ecus,
      });
    }).then((fn) => {
      unlisten = fn;
    });

    return () => {
      unlisten?.();
    };
  }, []);

  async function handleConnect() {
    setConnecting(true);
    setError(null);
    try {
      const ecus = await invoke<BackendEcuStatus[]>("connect", { url: serverUrl });
      setCampaign({
        status: "idle",
        ecus: ecus.map(mapBackendEcu),
      });
      setConnected(true);
    } catch (e) {
      setError(String(e));
    } finally {
      setConnecting(false);
    }
  }

  async function handleDisconnect() {
    try {
      await invoke("disconnect");
    } catch (_) {
      // ignore
    }
    setConnected(false);
    setCampaign({ status: "idle", ecus: [] });
  }

  return (
    <div className="app">
      <header className="header">
        <h1>SUMO Campaign Viewer</h1>
        <div className="header-controls">
          <div className="mode-selector">
            <button
              className={`mode-btn ${mode === "observe" ? "active" : ""}`}
              onClick={() => setMode("observe")}
            >
              Observe
            </button>
            <button
              className={`mode-btn ${mode === "drive" ? "active" : ""}`}
              onClick={() => setMode("drive")}
            >
              Drive
            </button>
          </div>
          <div className="connection-controls">
            <span className={`connection-dot ${connected ? "connected" : "disconnected"}`} />
            <input
              className="server-input"
              value={serverUrl}
              onChange={(e) => setServerUrl(e.target.value)}
              placeholder="SOVD Server URL"
              disabled={connected}
            />
            <button
              className={`connect-btn ${connected ? "connected" : ""}`}
              onClick={connected ? handleDisconnect : handleConnect}
              disabled={connecting}
            >
              {connecting ? "Connecting..." : connected ? "Disconnect" : "Connect"}
            </button>
          </div>
        </div>
      </header>

      {error && (
        <div className="error-banner">
          {error}
          <button className="error-dismiss" onClick={() => setError(null)}>×</button>
        </div>
      )}

      <main className="main">
        <section className="panel ecus-panel">
          <h2>
            ECUs
            {connected && <span className="ecu-count">({campaign.ecus.length})</span>}
          </h2>
          {campaign.ecus.length === 0 ? (
            <div className="placeholder">
              {connected ? "No ECUs discovered" : "Connect to an SOVD server to discover ECUs"}
            </div>
          ) : (
            <div className="ecu-grid">
              {campaign.ecus.map((ecu) => (
                <EcuCard key={ecu.id} ecu={ecu} />
              ))}
            </div>
          )}
        </section>

        <section className="panel timeline-panel">
          <h2>Timeline</h2>
          {campaign.ecus.length === 0 ? (
            <div className="placeholder">Waiting for ECU discovery</div>
          ) : (
            <CampaignTimeline ecus={campaign.ecus} />
          )}
        </section>

        <section className="panel manifest-panel">
          <h2>Manifest</h2>
          <div className="placeholder">Load a campaign manifest to inspect</div>
        </section>
      </main>
    </div>
  );
}

export default App;
