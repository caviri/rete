#!/usr/bin/env bash
# Tests for site/query-like-a-child, in Docker.
#   bash tests/query-like-a-child/run.sh           unit tests, then the browser check
#   bash tests/query-like-a-child/run.sh unit      the sketch → SPARQL model only
# The browser check needs the network: it queries Linked Jazz over range requests.
# It uses playwright-core from tests/gate/node_modules (npm ci there first) with
# the Chromium of the mcr.microsoft.com/playwright:v1.49.0-jammy image.
set -euo pipefail
cd "$(dirname "$0")/../.."
export MSYS_NO_PATHCONV=1
here=$(pwd -W 2>/dev/null || pwd)

docker run --rm --memory 1g -v "$here:/work:ro" -w /work node:22-slim \
  node --test tests/query-like-a-child/model.test.mjs tests/query-like-a-child/parse.test.mjs

[ "${1:-}" = unit ] && exit 0

shots=${SHOTS:-"$here/tests/query-like-a-child/shots"}
mkdir -p "$shots"
docker run --rm --memory 4g -v "$here:/work:ro" -v "$shots:/shots" \
  -e NODE_PATH=/work/tests/gate/node_modules -e SHOTS=/shots \
  -e CHROMIUM=/ms-playwright/chromium-1148/chrome-linux/chrome -w /work \
  mcr.microsoft.com/playwright:v1.49.0-jammy node tests/query-like-a-child/e2e.cjs
