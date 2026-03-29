# SUMO Campaign Viewer

Real-time visualization tool for SUIT firmware update campaigns across multiple ECUs.

## Modes

### Observe Mode
Connects to an SOVD gateway and monitors an ongoing campaign driven by the onboard orchestrator. Shows real-time progress without interfering.

### Drive Mode
Acts as the campaign orchestrator itself — for test bench and workshop use. Parses SUIT campaign manifests, drives per-ECU updates via SOVD, and provides interactive commit/rollback control.

## Features

### Campaign View
- Parse and visualize L1 campaign manifests (ECU targets, dependencies, command sequences)
- Show install → validate → invoke sequence flow
- Display security_version policy and anti-rollback state
- Content-addressable firmware details (digest, size, encryption)

### Per-ECU State Machine
Real-time visualization of each ECU through the SOVD flash lifecycle:

```
[Default] → [Programming] → [Security Unlock] → [Upload] → [Verify]
    → [Flash] → [Finalize] → [Reset] → [Activated/Trial] → [Commit/Rollback]
```

Color-coded states: pending (gray), active (blue), success (green), failed (red), trial (amber)

### Timeline
- Horizontal timeline showing all ECUs
- Which ECU is doing what, when
- Parallel vs sequential operations visible
- Duration tracking per phase

### Manifest Inspector
- SUIT envelope structure (authentication, manifest, integrated payloads)
- Command sequences (shared, install, validate, invoke) as a visual flow
- Parameter details per component (vendor_id, class_id, digest, URI, security_version)
- Text metadata (version, vendor name, model name)
- Encryption info (algorithm, recipients, key IDs)

### DID Dashboard
- Standard UDS DIDs (F187-F19E) before/after update
- Version comparison (current vs target)
- Security version floor tracking

## Architecture

```
┌─────────────────────────────────────┐
│       Campaign Viewer (Tauri)       │
│  ┌──────────┐  ┌─────────────────┐  │
│  │ Manifest  │  │ Campaign State  │  │
│  │ Parser    │  │ Tracker         │  │
│  │ (WASM)    │  │ (polls SOVD)   │  │
│  └──────────┘  └─────────────────┘  │
│  ┌──────────────────────────────┐   │
│  │ Visualization Components     │   │
│  │ - ECU state machines         │   │
│  │ - Timeline                   │   │
│  │ - Manifest inspector         │   │
│  │ - DID dashboard              │   │
│  └──────────────────────────────┘   │
└──────────────┬──────────────────────┘
               │ SOVD REST API
               ↓
┌──────────────────────────────┐
│   SOVD Gateway               │
│   ├── ECU 1 (vm-mgr)        │
│   ├── ECU 2 (UDS)           │
│   └── ECU 3 (UDS)           │
└──────────────────────────────┘
```

### Drive Mode Additional Components

```
┌─────────────────────────────────────┐
│       Campaign Viewer (Drive)       │
│  ┌──────────────────────────────┐   │
│  │ sumo-sovd-orchestrator       │   │
│  │ (embedded, drives campaign)  │   │
│  └──────────────────────────────┘   │
│  ┌──────────────────────────────┐   │
│  │ Security Helper Client       │   │
│  └──────────────────────────────┘   │
└─────────────────────────────────────┘
```

## Tech Stack

- **Frontend**: React + TypeScript (Tauri)
- **Backend**: Rust (Tauri commands)
  - sovd-client for SOVD API
  - sumo-codec for manifest parsing
  - sumo-sovd-orchestrator for drive mode
- **Visualization**: React components with state machine diagrams

## Related Projects

- [sumo-rs](https://github.com/tr-sdv-sandbox/sumo-rs) — SUIT manifest library
- [sumo-sovd](https://github.com/sdv-playground/sumo-sovd) — Campaign orchestrator
- [SOVDd](https://github.com/sdv-playground/SOVDd) — SOVD diagnostic server
- [vm-mgr](https://github.com/sdv-playground/vm-mgr) — VM lifecycle manager
- [SOVD Explorer](https://github.com/sdv-playground/SOVD-explorer) — General SOVD diagnostic GUI
