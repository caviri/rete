#!/usr/bin/env bash
# Reproducible SteamDB-equivalent acquisition from open Steam APIs:
#   1. SteamSpy `all` pages  -> app index + core stats (owners/reviews/playtime/price/ccu)
#   2. Steam store appdetails -> rich per-game metadata
# Both resumable (skip existing). GetAppList/v2 is 404 in this environment, so
# SteamSpy is the app index.
set -o pipefail
export MSYS_NO_PATHCONV=1
ROOT="$(cd "$(dirname "$0")/../../.." && pwd)"
cd "$ROOT"
echo "=== 1/2 SteamSpy all pages (index + stats) ==="
docker run --rm -v "$ROOT:/w" -w //w python:3.12-slim python data/steamspy/scripts/fetch_steamspy.py
echo "=== 2/2 Steam appdetails (rich per-game metadata) ==="
docker run --rm -v "$ROOT:/w" -w //w python:3.12-slim python data/steamspy/scripts/fetch_appdetails.py
echo "=== acquisition complete ==="
