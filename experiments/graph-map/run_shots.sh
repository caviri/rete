#!/usr/bin/env bash
# Render screenshots of a viewer (inside the rete-dev container, which has node).
#   run_shots.sh <viewer.html> <out-prefix> <zooms>
# e.g. run_shots.sh viewer.html struct 0.5,3,6
set -e
cd /work/dev/playwright
npm i playwright@1.49.0 >/dev/null 2>&1
npx playwright install --with-deps chromium >/dev/null 2>&1 || npx playwright install chromium >/dev/null 2>&1 || true
node serve.mjs /work/experiments/graph-map 8090 & SRV=$!
sleep 1.5
node /work/experiments/graph-map/screenshot.mjs \
  "http://localhost:8090/$1" "/work/experiments/graph-map/out/$2" "$3"
kill $SRV 2>/dev/null || true
