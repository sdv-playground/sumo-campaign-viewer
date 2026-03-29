#!/bin/bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
cd "$SCRIPT_DIR"

# Install npm deps if needed
if [ ! -d node_modules ]; then
    echo "[viewer] installing npm dependencies..."
    npm install
fi

echo "[viewer] starting SUMO Campaign Viewer..."
npm run tauri dev
