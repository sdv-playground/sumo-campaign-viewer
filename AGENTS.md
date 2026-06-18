# SUMO Campaign Viewer Index

Tauri desktop app for observing or driving SUIT campaign updates across multiple ECUs over SOVD.

## Where to look

- `README.md` — modes, views, development commands, campaign flow.
- `CLAUDE.md` — architecture summary, ports, workflow conventions.
- `package.json` — npm scripts and frontend dependencies.
- `run.sh` — local dev launcher.
- `src/App.tsx` — React UI: ECU cards, timeline, manifest inspector, DID dashboard.
- `src-tauri/src/lib.rs` — Tauri commands and Rust backend orchestration.
- `src-tauri/Cargo.toml` — backend dependencies on SOVD/SUIT crates.

## Essential commands

No component-local `mise` file is present; use npm/Tauri scripts from this submodule root.

```bash
npm install
./run.sh
npm run dev
npm run build
npm run lint
npm run tauri -- build
```

Finding commands:

```bash
rg --files -g 'package.json' -g 'Cargo.toml' -g 'README*' -g 'CLAUDE.md'
rg -n "invoke\(|tauri::command|Observe|Drive|Campaign|manifest|activation|commit|rollback|DID" src src-tauri README.md CLAUDE.md
```

## Stack

- Tauri 2, Rust backend, React 18 + TypeScript + Vite frontend.
- Uses SOVD client and SUMO/SUIT crates for observe/drive flows.

## Guardrails

- Preserve Observe mode as read-only.
- Drive mode may flash/commit/rollback ECUs; keep user confirmation and state visibility clear.
- Use state constants rather than raw phase strings.
- Dark theme should stay aligned with the SOVD Explorer aesthetic.

## Gotchas

- Default SOVD gateway is `http://localhost:4000`.
- `npm run lint` requires ESLint config/deps to be present; verify before relying on it in CI.

## Missing docs/specs to watch

- No component-local `mise` or CI workflow is present.
- Tauri IPC command contract is documented by code/README, not a generated spec.
