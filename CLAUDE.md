# CLAUDE.md — sumo-campaign-viewer

## Project Overview

Real-time SUIT campaign visualization and test tool for multi-ECU
firmware updates. Connects to SOVD servers to observe or drive
update campaigns with per-ECU state machine visualization.

### Architecture

Tauri 2 desktop app: Rust backend + React/TypeScript frontend.

```
src/                    # React frontend
  App.tsx               # Main UI: ECU cards, timeline, manifest inspector
  App.css               # Dark theme with phase-colored state indicators
  index.css             # CSS variables (colors, spacing)

src-tauri/              # Rust backend
  src/
    lib.rs              # Tauri commands: connect, parse_manifest, get_activation
    main.rs             # Entry point
```

### Two Modes

- **Observe**: Connect to SOVD gateway, poll component status, visualize
  what the onboard orchestrator is doing. Read-only.
- **Drive**: Embed sumo-sovd-orchestrator, parse campaign manifests, drive
  the full flash lifecycle via SOVD API. Interactive commit/rollback.

### Key Views

1. **ECU Cards** — per-ECU state machine (idle → session → security →
   upload → verify → flash → finalize → reset → trial → committed)
2. **Timeline** — horizontal view of all ECUs, shows parallel operations
3. **Manifest Inspector** — SUIT envelope details (sequences, parameters,
   text fields, encryption info)
4. **DID Dashboard** — UDS DIDs before/after update

### Dependencies

**Rust (src-tauri):**
- `sovd-client` — SOVD REST API (from SOVDd)
- `sumo-codec` — SUIT manifest CBOR parsing
- `sumo-onboard` — manifest validation, accessors
- `sumo-crypto` — crypto backend
- `sumo-sovd-orchestrator` — drive mode campaign execution

**TypeScript (src):**
- React 18 + Vite
- @tauri-apps/api for IPC

### Ports

- Dev server: `localhost:1421` (vite)
- Connects to SOVD gateway: `localhost:4000` (configurable)

## Build & Run

```bash
./run.sh                    # Install deps + launch Tauri dev
npm run tauri build         # Production build
```

## Workflow

- Plan mode for non-trivial changes
- Dark theme matches SOVD Explorer aesthetic
- State constants (not raw strings) for phase tracking
- Type-safe Tauri commands with serde serialization
