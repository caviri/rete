#!/usr/bin/env bash
# Build Wikidata-XXL as N independent --no-pyramid shards — one per ~100M-triple
# Parquet partition of the piebro/wikidata-extraction truthy dump. For each shard:
# stream the partition → N-Triples (DuckDB httpfs, no full download) → rete build
# --no-pyramid → DELETE the NT. So peak disk ≈ one shard's NT (~10 GB) + the
# accumulated .rete shards. RESUMABLE: a shard whose .rete already exists is skipped.
#
# The shards federate (UNION + cross-source join) into one logical dataset via a
# manifest (see scripts/wikidata_xxl_manifest.py). 1 shard ≈ 100M triples ≈ ~1 GB .rete.
#
# Usage:  bash scripts/build_wikidata_xxl.sh <num-shards> [start-index]
#   bash scripts/build_wikidata_xxl.sh 10        # shards 0..9  (~1B triples, ~10 GB)
#   bash scripts/build_wikidata_xxl.sh 20 10     # shards 10..19 (resume/extend)
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"
N="${1:?usage: build_wikidata_xxl.sh <num-shards> [start-index]}"
START="${2:-0}"
SHARDS=data/wikidata-xxl
NT=data/wikidata-xxl/nt
RB=./target/release/rete
mkdir -p "$SHARDS" "$NT"

# Datatype recovery: `heuristic` (offline — typed dateTime + WKT, numbers stay plain)
# avoids WDQS, which rate-limits the property-map lookup hard (429) and stalls a long
# multi-shard build. If data/wd_property_types.csv exists, `auto` would use it for free;
# default to heuristic for robustness. Override with DTYPES=auto.
DTYPES="${DTYPES:-heuristic}"

for i in $(seq "$START" $((START + N - 1))); do
  shard=$(printf "%s/shard_%04d.rete" "$SHARDS" "$i")
  if [ -f "$shard" ]; then echo "shard $i exists ($(ls -lh "$shard" | awk '{print $5}')), skip"; continue; fi
  nt=$(printf "%s/shard_%04d.nt" "$NT" "$i")
  t0=$(date +%s)
  echo "=== shard $i: stream partition $i → NT ==="
  python scripts/wikidata_parquet_to_nt.py --part-index "$i" --datatypes "$DTYPES" -o "$nt"
  echo "=== shard $i: rete build --no-pyramid ==="
  MSYS_NO_PATHCONV=1 docker run --rm -v "$PWD:/work" -w /work rete-dev:latest \
    "$RB" build "/work/$nt" -o "/work/$shard" --no-pyramid --card \
    --title "Wikidata-XXL shard $i" --license "CC0-1.0" --source "https://www.wikidata.org"
  rm -f "$nt"
  echo "=== shard $i DONE: $(ls -lh "$shard" | awk '{print $5}') in $(( $(date +%s) - t0 ))s ==="
done
echo "ALL REQUESTED SHARDS DONE. Manifest: python scripts/wikidata_xxl_manifest.py"
