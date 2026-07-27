#!/usr/bin/env bash
# Download the hub-stats Parquet backbone (models/datasets/spaces/papers/posts).
#
# Source: https://huggingface.co/datasets/cfahlgren1/hub-stats  (Apache-2.0, updated daily)
# Pinned to the 2026-07-23 snapshot for reproducibility — bump REV to take a newer one.
set -euo pipefail

REV="4c7906281206eb8c8445711afba1c9f53f54e599"   # 2026-07-23T13:41:40Z
BASE="https://huggingface.co/datasets/cfahlgren1/hub-stats/resolve/${REV}"
DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
OUT="${DIR}/raw/hub-stats"
mkdir -p "$OUT"

FILES=(models.parquet datasets.parquet spaces.parquet arxiv_papers.parquet daily_papers.parquet posts.parquet)

for f in "${FILES[@]}"; do
  if [ -s "${OUT}/${f}" ]; then echo "skip ${f} (exists)"; continue; fi
  echo "fetching ${f} ..."
  curl -sSL --fail --retry 5 --retry-delay 5 -C - -o "${OUT}/${f}.part" "${BASE}/${f}"
  mv "${OUT}/${f}.part" "${OUT}/${f}"
done

echo "$REV" > "${OUT}/REVISION.txt"

( cd "$OUT" && sha256sum "${FILES[@]}" ) > "${DIR}/SHA256SUMS.txt"
echo "done:"; ls -la "$OUT"
