#!/usr/bin/env bash
# Stage the explorer page as the desktop app's frontend.
#
# There is one copy of the UI — experiments/rete-file-explorer — and the desktop
# build consumes it verbatim. Nothing is rewritten or forked here: app.js already
# picks its transport at runtime (Web Worker in a browser, Tauri commands in the
# app), so the same files serve both. The only thing dropped is fs-worker.js,
# which loads a wasm build the desktop app has no use for.
set -euo pipefail
cd "$(dirname "$0")/.."

SRC="../../experiments/rete-file-explorer"
DIST="dist"

[ -d "$SRC" ] || { echo "sync-frontend: missing $SRC" >&2; exit 1; }

rm -rf "$DIST"
mkdir -p "$DIST/js"
cp "$SRC/index.html" "$SRC/styles.css" "$DIST/"
cp "$SRC/js/rete-fs.js" "$SRC/js/app.js" "$SRC/js/tauri-bridge.js" "$DIST/js/"

echo "sync-frontend: staged $(find "$DIST" -type f | wc -l) files into $DIST/"
