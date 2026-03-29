# CLAUDE.md — sumo-campaign-viewer

## Project Overview

Real-time SUIT campaign visualization and test tool. Connects to SOVD
servers to observe or drive multi-ECU firmware update campaigns.

### Two Modes

- **Observe**: Monitor campaigns driven by the onboard orchestrator
- **Drive**: Act as orchestrator for test bench / workshop use

### Key Views

- Campaign overview (ECU topology, manifest structure)
- Per-ECU state machine (real-time, color-coded)
- Timeline (parallel ECU operations)
- Manifest inspector (SUIT envelope details)
- DID dashboard (before/after comparison)

## Tech Stack

Tauri 2 (Rust backend + React/TypeScript frontend), same as SOVD Explorer.

### Rust Dependencies

- sovd-client — SOVD REST API
- sumo-codec — SUIT manifest parsing
- sumo-onboard — manifest validation
- sumo-sovd-orchestrator — drive mode campaign execution

## Build & Test

```bash
npm install
npm run tauri dev
```

## Related Projects

Same ecosystem as vm-mgr, SOVDd, sumo-rs, sumo-sovd, SOVD Explorer.
