import { useState, useEffect, useRef } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import "./App.css";

// =============================================================================
// Types
// =============================================================================

interface EcuState {
  id: string;
  name: string;
  transferState?: string;
  activationState?: string;
  version?: string;
  previousVersion?: string;
  supportsRollback: boolean;
  progress?: number;
  error?: string;
  diagnostics: Record<string, unknown>;
}

interface StateChange {
  timestamp: string;
  ecu_id: string;
  field: string;
  value: string;
  prev_value: string | null;
}

// Backend types (snake_case)
interface BackendEcuStatus {
  id: string;
  name: string;
  transfer_state: string | null;
  activation_state: string | null;
  version: string | null;
  previous_version: string | null;
  supports_rollback: boolean;
  progress: number | null;
  error: string | null;
  diagnostics: Record<string, unknown>;
}

interface BackendCampaignStatus {
  ecus: BackendEcuStatus[];
  changes: StateChange[];
}

// =============================================================================
// State classification
// =============================================================================

type StateCategory = "idle" | "active" | "waiting" | "success" | "error";

function classifyState(state: string | undefined): StateCategory {
  if (!state) return "idle";
  const s = state.toLowerCase();
  if (["failed", "error", "aborted", "invalid"].includes(s)) return "error";
  if (["committed", "complete", "finished"].includes(s)) return "success";
  if (["activated", "awaiting_reboot", "awaitingreboot", "rolled_back", "rolledback"].includes(s))
    return "waiting";
  if (["queued", "pending", "preparing", "transferring", "running",
       "awaiting_activation", "awaitingactivation", "validated", "verified"].includes(s))
    return "active";
  return "active";
}

const CATEGORY_COLORS: Record<StateCategory, string> = {
  idle: "var(--text-secondary)",
  active: "var(--accent)",
  waiting: "var(--warning)",
  success: "var(--success)",
  error: "var(--error)",
};

function stateLabel(state: string): string {
  return state.replace(/_/g, " ").replace(/\b\w/g, (c) => c.toUpperCase());
}

function effectiveState(ecu: EcuState | undefined): string | undefined {
  if (!ecu) return undefined;
  if (ecu.transferState) {
    const cat = classifyState(ecu.transferState);
    if (cat === "active" || cat === "waiting" || cat === "error") return ecu.transferState;
  }
  return ecu.activationState ?? ecu.transferState;
}

function isUpdating(ecu: EcuState): boolean {
  const state = effectiveState(ecu);
  if (!state) return false;
  const cat = classifyState(state);
  return cat === "active" || cat === "waiting";
}

function mapBackendEcu(ecu: BackendEcuStatus): EcuState {
  return {
    id: ecu.id,
    name: ecu.name,
    transferState: ecu.transfer_state ?? undefined,
    activationState: ecu.activation_state ?? undefined,
    version: ecu.version ?? undefined,
    previousVersion: ecu.previous_version ?? undefined,
    supportsRollback: ecu.supports_rollback,
    progress: ecu.progress ?? undefined,
    error: ecu.error ?? undefined,
    diagnostics: ecu.diagnostics ?? {},
  };
}

// =============================================================================
// Diagnostic labels
// =============================================================================

const DIAG_LABELS: Record<string, string> = {
  active_bank: "Bank",
  committed: "Committed",
  boot_count: "Boot Count",
  min_security_ver: "Min SecVer",
  current_security_ver: "SecVer",
  guest_state: "Guest",
  heartbeat_seq: "HB Seq",
};

function formatDiagValue(key: string, value: unknown): string {
  if (key === "committed") return value ? "yes" : "no";
  if (value === null || value === undefined) return "-";
  return String(value);
}

// Heartbeat indicator — tracks previous hb_seq to detect frozen state
const lastHbSeq: Record<string, { seq: number; changed: number }> = {};

function HeartbeatIndicator({ ecu }: { ecu: EcuState }) {
  const guestState = ecu.diagnostics.guest_state as string | undefined;
  const hbSeq = ecu.diagnostics.heartbeat_seq as number | undefined;

  if (!guestState || guestState === "offline") {
    return (
      <div className="heartbeat-indicator offline" title="VM offline">
        <div className="hb-dot" />
        <span className="hb-label">Offline</span>
      </div>
    );
  }

  // Track heartbeat freshness
  const now = Date.now();
  const prev = lastHbSeq[ecu.id];
  if (hbSeq !== undefined) {
    if (!prev || prev.seq !== hbSeq) {
      lastHbSeq[ecu.id] = { seq: hbSeq, changed: now };
    }
  }
  const lastChange = lastHbSeq[ecu.id]?.changed ?? 0;
  const stale = now - lastChange > 5000;

  if (guestState === "running" && !stale) {
    return (
      <div className="heartbeat-indicator alive" title={`Running — HB #${hbSeq}`}>
        <div className="hb-dot pulse" />
        <span className="hb-label">Running</span>
      </div>
    );
  }

  if (stale && guestState === "running") {
    return (
      <div className="heartbeat-indicator frozen" title={`Heartbeat frozen at #${hbSeq}`}>
        <div className="hb-dot" />
        <span className="hb-label">Frozen</span>
      </div>
    );
  }

  // booting, degraded, shutting_down, etc.
  const label = guestState.replace(/_/g, " ").replace(/\b\w/g, (c) => c.toUpperCase());
  return (
    <div className="heartbeat-indicator transitioning" title={`${label} — HB #${hbSeq ?? "?"}`}>
      <div className="hb-dot" />
      <span className="hb-label">{label}</span>
    </div>
  );
}

// =============================================================================
// State Machine Steps
// =============================================================================

const TRANSFER_STEPS = ["Queued", "Preparing", "Transferring", "AwaitingActivation"];
const ACTIVATION_STEPS = ["AwaitingReboot", "Activated", "Committed"];

function normalizeState(state: string): string {
  return state.replace(/_/g, "").toLowerCase();
}

function stepIndex(steps: string[], current: string | undefined): number {
  if (!current) return -1;
  const norm = normalizeState(current);
  return steps.findIndex((s) => normalizeState(s) === norm);
}

type StepStatus = "done" | "current" | "future" | "idle";

function getStepStatus(idx: number, currentIdx: number, isActive: boolean): StepStatus {
  if (!isActive) return "idle";
  if (idx < currentIdx) return "done";
  if (idx === currentIdx) return "current";
  return "future";
}

function StateMachineStepper({ ecu }: { ecu: EcuState }) {
  const transferIdx = stepIndex(TRANSFER_STEPS, ecu.transferState);
  const activationIdx = stepIndex(ACTIVATION_STEPS, ecu.activationState);
  const hasError = ecu.error || classifyState(ecu.transferState) === "error" || classifyState(ecu.activationState) === "error";
  const isRolledBack = ecu.activationState && normalizeState(ecu.activationState) === "rolledback";

  // Transfer is "done" if we've moved into activation phase or completed transfer
  const transferDone = activationIdx >= 0 || (transferIdx >= 0 && classifyState(ecu.transferState) === "success");
  // Activation is active if we have an activation state
  const activationActive = activationIdx >= 0;

  return (
    <div className="state-machine">
      {/* Transfer phase */}
      <div className="sm-phase">
        <span className="sm-phase-label">Transfer</span>
        <div className="sm-steps">
          {TRANSFER_STEPS.map((step, idx) => {
            let status: StepStatus;
            if (hasError && idx === transferIdx) {
              status = "current"; // will get error styling via class
            } else if (transferDone) {
              status = "done";
            } else {
              status = getStepStatus(idx, transferIdx, transferIdx >= 0);
            }
            const isCurrent = idx === transferIdx && !transferDone;
            return (
              <div key={step} className={`sm-step ${status} ${hasError && isCurrent ? "error" : ""}`}>
                <div className="sm-dot" />
                {idx < TRANSFER_STEPS.length - 1 && <div className={`sm-line ${status === "done" || (idx < transferIdx) ? "done" : ""}`} />}
                <span className="sm-label">{stateLabel(step)}</span>
              </div>
            );
          })}
        </div>
        {ecu.progress !== undefined && ecu.progress < 100 && transferIdx >= 0 && (
          <div className="sm-progress">
            <div className="progress-track">
              <div className="progress-fill" style={{ width: `${ecu.progress}%` }} />
            </div>
            <span className="progress-pct">{ecu.progress.toFixed(0)}%</span>
          </div>
        )}
      </div>

      {/* Activation phase */}
      <div className="sm-phase">
        <span className="sm-phase-label">Activation</span>
        <div className="sm-steps">
          {ACTIVATION_STEPS.map((step, idx) => {
            let status: StepStatus;
            if (isRolledBack) {
              status = "idle";
            } else {
              status = getStepStatus(idx, activationIdx, activationActive);
            }
            return (
              <div key={step} className={`sm-step ${status}`}>
                <div className="sm-dot" />
                {idx < ACTIVATION_STEPS.length - 1 && <div className={`sm-line ${status === "done" || (idx < activationIdx && !isRolledBack) ? "done" : ""}`} />}
                <span className="sm-label">{stateLabel(step)}</span>
              </div>
            );
          })}
          {/* Rollback branch */}
          <div className={`sm-step sm-rollback ${isRolledBack ? "current" : "future"}`}>
            <div className="sm-dot" />
            <span className="sm-label">Rolled Back</span>
          </div>
        </div>
      </div>

      {/* Committed DID (ground truth) */}
      <div className="sm-committed">
        <span className="sm-committed-label">HW Committed</span>
        <span className={`sm-committed-value ${ecu.diagnostics.committed ? "yes" : "muted"}`}>
          {formatDiagValue("committed", ecu.diagnostics.committed)}
        </span>
      </div>

      {ecu.error && <div className="update-error">{ecu.error}</div>}
    </div>
  );
}

// =============================================================================
// ECU Row — split: left = ECU info, right = state machine
// =============================================================================

function EcuRow({ ecu }: { ecu: EcuState }) {
  const updating = isUpdating(ecu);

  return (
    <div className={`ecu-row ${updating ? "updating" : ""}`}>
      {/* Left half: ECU identity & static info */}
      <div className="ecu-info">
        <div className="ecu-identity">
          <span className="ecu-name">{ecu.name}</span>
          <span className="ecu-id">{ecu.id}</span>
        </div>
        <div className="ecu-fields">
          <div className="field">
            <span className="field-label">Rollback</span>
            <span className={`field-value ${ecu.supportsRollback ? "yes" : "muted"}`}>
              {ecu.supportsRollback ? "yes" : "-"}
            </span>
          </div>
          <div className="field">
            <span className="field-label">Boot Count</span>
            <span className="field-value">{formatDiagValue("boot_count", ecu.diagnostics.boot_count)}</span>
          </div>
          <div className="field">
            <span className="field-label">Bank</span>
            <span className="field-value">{formatDiagValue("active_bank", ecu.diagnostics.active_bank)}</span>
          </div>
          <div className="field">
            <span className="field-label">Version</span>
            <span className="field-value mono">{ecu.version ?? "-"}</span>
          </div>
          <div className="field">
            <span className="field-label">SecVer</span>
            <span className="field-value">{formatDiagValue("current_security_ver", ecu.diagnostics.current_security_ver)}</span>
          </div>
          <div className="field">
            <span className="field-label">Min SecVer</span>
            <span className="field-value">{formatDiagValue("min_security_ver", ecu.diagnostics.min_security_ver)}</span>
          </div>
          <HeartbeatIndicator ecu={ecu} />
        </div>
      </div>

      {/* Divider */}
      <div className="ecu-divider" />

      {/* Right half: state machine visualization */}
      <StateMachineStepper ecu={ecu} />
    </div>
  );
}

// =============================================================================
// Change Log — driven by backend state change events
// =============================================================================

function ChangeLog({ entries }: { entries: StateChange[] }) {
  const logEndRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    logEndRef.current?.scrollIntoView({ behavior: "smooth" });
  }, [entries.length]);

  if (entries.length === 0) {
    return <div className="placeholder">Waiting for state changes...</div>;
  }

  return (
    <div className="change-log">
      <div className="log-table">
        <div className="log-header-row">
          <span className="log-col-time">Time</span>
          <span className="log-col-ecu">ECU</span>
          <span className="log-col-field">Field</span>
          <span className="log-col-value">Value</span>
        </div>
        {entries.map((entry, idx) => {
          const isState = entry.field === "Transfer" || entry.field === "Activation";
          const category = isState ? classifyState(entry.value) : undefined;
          return (
            <div key={idx} className={`log-row ${entry.field === "Error" ? "log-error" : ""}`}>
              <span className="log-col-time">{entry.timestamp}</span>
              <span className="log-col-ecu">{entry.ecu_id}</span>
              <span className="log-col-field">{DIAG_LABELS[entry.field] ?? entry.field}</span>
              <span className={`log-col-value ${category ? `state-${category}` : ""}`}>
                {entry.prev_value && <span className="log-prev">{entry.prev_value} → </span>}
                {isState ? stateLabel(entry.value) : entry.value}
              </span>
            </div>
          );
        })}
        <div ref={logEndRef} />
      </div>
    </div>
  );
}

// =============================================================================
// App
// =============================================================================

function App() {
  const [serverUrl, setServerUrl] = useState("http://localhost:4000");
  const [mode, setMode] = useState<"observe" | "drive">("observe");
  const [connected, setConnected] = useState(false);
  const [connecting, setConnecting] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [ecus, setEcus] = useState<EcuState[]>([]);
  const [logEntries, setLogEntries] = useState<StateChange[]>([]);

  useEffect(() => {
    let unlisten: UnlistenFn | undefined;

    listen<BackendCampaignStatus>("campaign-state-update", (event) => {
      const newEcus = event.payload.ecus.map(mapBackendEcu);
      setEcus(newEcus);

      if (event.payload.changes.length > 0) {
        setLogEntries((prev) => [...prev, ...event.payload.changes]);
      }
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
      const rawEcus = await invoke<BackendEcuStatus[]>("connect", { url: serverUrl });
      setEcus(rawEcus.map(mapBackendEcu));
      setLogEntries([]);
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
    setEcus([]);
  }

  return (
    <div className="app">
      <header className="header">
        <h1>SUMO Campaign Viewer</h1>
        <div className="header-controls">
          <div className="mode-selector">
            <button className={`mode-btn ${mode === "observe" ? "active" : ""}`} onClick={() => setMode("observe")}>
              Observe
            </button>
            <button className={`mode-btn ${mode === "drive" ? "active" : ""}`} onClick={() => setMode("drive")}>
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
        <section className="panel">
          <h2>
            ECUs
            {connected && <span className="ecu-count">({ecus.length})</span>}
          </h2>
          {ecus.length === 0 ? (
            <div className="placeholder">
              {connected ? "No ECUs discovered" : "Connect to an SOVD server to discover ECUs"}
            </div>
          ) : (
            <div className="ecu-list">
              {ecus.map((ecu) => (
                <EcuRow key={ecu.id} ecu={ecu} />
              ))}
            </div>
          )}
        </section>

        <section className="panel log-panel">
          <h2>
            State Changes
            {logEntries.length > 0 && <span className="ecu-count">({logEntries.length})</span>}
          </h2>
          <ChangeLog entries={logEntries} />
        </section>
      </main>
    </div>
  );
}

export default App;
