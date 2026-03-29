# SUMO Campaign Viewer

Real-time visualization tool for [SUIT](https://datatracker.ietf.org/doc/draft-ietf-suit-manifest/) firmware update campaigns across multiple ECUs.

## Quick Start

```bash
# Prerequisites: Node.js, Rust, Tauri 2 system deps
./run.sh
```

Connect to your SOVD gateway (default `http://localhost:4000`).

## Modes

### Observe Mode
Connects to an SOVD gateway and monitors an ongoing campaign driven by the onboard orchestrator. Shows real-time progress without interfering. Use this in production to watch updates roll out.

### Drive Mode
Acts as the campaign orchestrator itself — for test bench and workshop use. Parses SUIT campaign manifests, drives per-ECU updates via SOVD, and provides interactive commit/rollback control.

## Views

### ECU State Machine
Each ECU shows its current phase in the flash lifecycle, color-coded:

```
[Idle] → [Session] → [Security] → [Upload] → [Verify]
   → [Flash] → [Finalize] → [Reset] → [Trial] → [Commit/Rollback]
```

| Color | Meaning |
|-------|---------|
| Gray | Idle / pending |
| Blue | Active operation |
| Amber | Awaiting action (reset, trial) |
| Green | Committed (permanent) |
| Red | Failed |

### Timeline
Horizontal view showing all ECUs in the campaign. Visualizes:
- Which ECU is doing what, when
- Parallel vs sequential operations
- Duration per phase
- The "install all → verify all → invoke all" staged pattern

### Manifest Inspector
Parses SUIT envelopes and displays:
- Command sequences (shared, install, validate, invoke) as visual flow
- Security version and sequence number
- Component identifiers and dependencies
- Text metadata (version, vendor, model)
- Encryption status (algorithm, recipients)

### DID Dashboard
Standard UDS DIDs (F187-F19E) before and after update:
- Version comparison (current vs target)
- Supplier and part information
- Security version floor tracking

## Architecture

```
┌─────────────────────────────────────┐
│     SUMO Campaign Viewer (Tauri)    │
│                                     │
│  React Frontend                     │
│  ├── ECU Cards (state machines)     │
│  ├── Timeline (parallel view)       │
│  ├── Manifest Inspector             │
│  └── DID Dashboard                  │
│                                     │
│  Rust Backend (Tauri Commands)      │
│  ├── connect() — discover ECUs      │
│  ├── parse_manifest() — inspect     │
│  ├── get_activation() — poll state  │
│  └── [Drive] orchestrator           │
└──────────────┬──────────────────────┘
               │ SOVD REST API
               ↓
┌──────────────────────────────────────┐
│         SOVD Gateway                 │
│  ├── ECU 1 (vm-mgr)                │
│  ├── ECU 2 (UDS via SOVDd)         │
│  └── ECU N                          │
└──────────────────────────────────────┘
```

## Development

```bash
./run.sh                    # Dev mode (hot reload)
npm run tauri build         # Production build
```

### Prerequisites

- Node.js 18+
- Rust toolchain
- Tauri 2 system dependencies ([install guide](https://v2.tauri.app/start/prerequisites/))

### Project Structure

```
src/                        # React frontend
  ├── App.tsx               # Main UI components
  ├── App.css               # Dark theme + state colors
  └── index.css             # CSS variables
src-tauri/                  # Rust backend
  ├── src/lib.rs            # Tauri commands
  ├── Cargo.toml            # Rust dependencies
  └── tauri.conf.json       # Window config
```

## Campaign Flow (SUIT Standard)

The viewer understands SUIT manifest command sequences:

| Manifest Type | install | validate | invoke | Viewer Shows |
|---|---|---|---|---|
| Firmware | directive-copy | condition-image-match | directive-invoke | Full flash lifecycle |
| CRL / Policy | — | — | — | "Policy applied" (no flash) |
| Multi-ECU | process-dependency ×N | per-ECU verify | per-ECU invoke | Staged: install all → verify all → invoke all |

## Related Projects

| Project | Description |
|---------|-------------|
| [sumo-rs](https://github.com/tr-sdv-sandbox/sumo-rs) | SUIT manifest library (Rust) |
| [sumo-sovd](https://github.com/sdv-playground/sumo-sovd) | Campaign orchestrator over SOVD |
| [vm-mgr](https://github.com/sdv-playground/vm-mgr) | VM lifecycle manager with SUIT |
| [SOVDd](https://github.com/sdv-playground/SOVDd) | SOVD diagnostic server |
| [SOVD Explorer](https://github.com/sdv-playground/SOVD-explorer) | General SOVD diagnostic GUI |
| [SUMO specs](https://github.com/tr-sdv-sandbox/sumo) | Specifications and feature mapping |
