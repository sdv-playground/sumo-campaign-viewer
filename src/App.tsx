import { useState } from "react";
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
  securityVersion?: number;
  progress?: number;
  error?: string;
}

interface CampaignState {
  id: string;
  status: "idle" | "running" | "awaiting_commit" | "committed" | "rolled_back" | "failed";
  ecus: EcuState[];
  manifestInfo?: ManifestInfo;
}

interface ManifestInfo {
  sequenceNumber: number;
  securityVersion?: number;
  dependencies: number;
  hasInstall: boolean;
  hasValidate: boolean;
  hasInvoke: boolean;
  textVersion?: string;
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
      {ecu.version && (
        <div className="ecu-version">
          {ecu.previousVersion && (
            <span className="version-prev">{ecu.previousVersion} → </span>
          )}
          <span className="version-current">{ecu.version}</span>
        </div>
      )}
      {ecu.progress !== undefined && ecu.progress < 100 && (
        <div className="ecu-progress">
          <div className="progress-bar" style={{ width: `${ecu.progress}%` }} />
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

  // Demo state for visualization
  const [campaign] = useState<CampaignState>({
    id: "demo",
    status: "idle",
    ecus: [
      { id: "os1", name: "OS1 VM", phase: "idle" },
      { id: "engine_ecu", name: "Engine ECU", phase: "idle" },
      { id: "body_ecu", name: "Body ECU", phase: "idle" },
    ],
  });

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
          <input
            className="server-input"
            value={serverUrl}
            onChange={(e) => setServerUrl(e.target.value)}
            placeholder="SOVD Server URL"
          />
        </div>
      </header>

      <main className="main">
        <section className="panel ecus-panel">
          <h2>ECUs</h2>
          <div className="ecu-grid">
            {campaign.ecus.map((ecu) => (
              <EcuCard key={ecu.id} ecu={ecu} />
            ))}
          </div>
        </section>

        <section className="panel timeline-panel">
          <h2>Timeline</h2>
          <CampaignTimeline ecus={campaign.ecus} />
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
