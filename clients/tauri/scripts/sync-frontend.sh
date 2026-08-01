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

# Copy every module EXCEPT the wasm worker, rather than naming them one by one:
# a hardcoded list silently drops any module added later, and a missing import
# means app.js never evaluates — a blank window with the reason buried in a
# devtools console the user has no reason to open.
for f in "$SRC"/js/*.js; do
  case "$(basename "$f")" in
    fs-worker.js) continue ;;
  esac
  cp "$f" "$DIST/js/"
done

echo "sync-frontend: staged $(find "$DIST" -type f | wc -l) files into $DIST/"
echo "sync-frontend: js -> $(ls "$DIST/js" | tr '\n' ' ')"
