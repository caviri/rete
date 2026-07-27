#!/usr/bin/env bash
# Download one full UTC day of GH Archive hourly event files into raw/.
#
#   bash data/github-archive/scripts/download.sh [YYYY-MM-DD]
#
# Default day: 2026-07-22 (the snapshot documented in README.md).
# GH Archive publishes one gzipped newline-delimited-JSON file per UTC hour:
#   https://data.gharchive.org/YYYY-MM-DD-H.json.gz   (H = 0..23, no zero-pad)
# Each file is ~20 MB gz for 2026 traffic; a full day is ~500 MB.
set -euo pipefail

DAY="${1:-2026-07-22}"
DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
mkdir -p "$DIR/raw"

for H in $(seq 0 23); do
  F="$DAY-$H.json.gz"
  OUT="$DIR/raw/$F"
  if [ -s "$OUT" ]; then
    echo "skip  $F (already present)"
    continue
  fi
  echo "fetch $F"
  curl -sSL --fail --retry 3 --retry-delay 2 -o "$OUT.part" "https://data.gharchive.org/$F"
  mv "$OUT.part" "$OUT"
done

( cd "$DIR/raw" && sha256sum ./*.json.gz ) > "$DIR/SHA256SUMS.txt"
echo "done: $(ls "$DIR/raw/$DAY"-*.json.gz | wc -l) files, checksums in SHA256SUMS.txt"
