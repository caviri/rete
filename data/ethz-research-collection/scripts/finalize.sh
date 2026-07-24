#!/usr/bin/env bash
# Post-harvest finalize: profile + checksums. Run after the xoai harvest.
# Kept separate from download.sh so it is never edited mid-execution.
set -uo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")/../../.." || exit 1   # repo root
REL="data/ethz-research-collection"

echo "[finalize] profiling ($(date -u +%H:%M:%SZ)) ..."
MSYS_NO_PATHCONV=1 docker run --rm -v "$PWD:/w" -w //w python:3.12-slim \
  python "${REL}/scripts/inspect.py" > "${REL}/scripts/inspect.txt" 2>&1
echo "[finalize] wrote ${REL}/scripts/inspect.txt"

echo "[finalize] checksums ..."
( cd "${REL}/raw" && find . -name '*.xml.gz' | sort | xargs sha256sum > "../SHA256SUMS.txt" )
echo "[finalize] wrote ${REL}/SHA256SUMS.txt ($(wc -l < "${REL}/SHA256SUMS.txt") files)"

echo "FINALIZE COMPLETE"
