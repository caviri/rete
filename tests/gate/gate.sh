#!/usr/bin/env bash
# The one-command regression gate. Run after EVERY playground/engine change:
#
#   bash tests/gate/gate.sh            # full: static + node harness + browser matrix (~4 min)
#   bash tests/gate/gate.sh fast       # static + node harness only (~15 s)
#   bash tests/gate/gate.sh --only=worldcup   # a single browser check
#   bash tests/gate/gate.sh --deployed # also probe the live GitHub Pages site (informational)
#
# Green gate = safe to commit. Red gate = fix before committing.
set -e
export MSYS_NO_PATHCONV=1 MSYS2_ARG_CONV_EXCL='*'
ROOT="$(git rev-parse --show-toplevel)"
mkdir -p "$ROOT/tests/gate/.cache"

# Fixture for the node async-wasm harness (cached; ~270 KB from R2).
FIX="$ROOT/tests/gate/.cache/worldcup2026.rete"
if [ ! -f "$FIX" ]; then
  echo "fetching gate fixture worldcup2026.rete…"
  curl -sSL "https://data.graphplaza.com/worldcup2026/worldcup2026.rete" -o "$FIX"
fi

# First run: install the playwright npm package next to the checks (the image
# ships the BROWSERS but not a global npm package; ESM import resolves from here).
if [ ! -d "$ROOT/tests/gate/node_modules/playwright" ]; then
  echo "installing gate deps (first run)…"
  docker run --rm --network host -v "$ROOT:/work" -w /work/tests/gate \
    mcr.microsoft.com/playwright:v1.49.0-jammy \
    bash -c 'PLAYWRIGHT_SKIP_BROWSER_DOWNLOAD=1 npm i --no-audit --no-fund --loglevel=error'
fi

docker run --rm --network host -v "$ROOT:/work" -w /work/tests/gate \
  mcr.microsoft.com/playwright:v1.49.0-jammy \
  node run.mjs "$@"
