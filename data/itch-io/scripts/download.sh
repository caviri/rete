#!/usr/bin/env bash
# itch.io games acquisition from OPEN, robots-allowed sources:
#   1. sitemap    -> the complete game-URL index (~1.95M games)
#   2. browse feed -> per-game metadata cells (newest sort, resumable)
# (steamdb.info-style scraping avoided; /games is robots-allowed, /search is not)
set -o pipefail
export MSYS_NO_PATHCONV=1
ROOT="$(cd "$(dirname "$0")/../../.." && pwd)"
cd "$ROOT"
echo "=== 1/2 sitemap (complete game URL index) ==="
docker run --rm -v "$ROOT:/w" -w //w python:3.12-slim python data/itch-io/scripts/fetch_sitemap.py
echo "=== 2/2 browse feed (per-game metadata, newest) ==="
docker run --rm -v "$ROOT:/w" -w //w python:3.12-slim python data/itch-io/scripts/fetch_browse.py newest
echo "=== acquisition complete ==="
