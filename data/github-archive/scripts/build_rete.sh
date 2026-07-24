#!/usr/bin/env bash
# Emit RDF for the given day(s) (if missing) and build ONE .rete from them.
#
#   bash data/github-archive/scripts/build_rete.sh <out-name> <DAY> [DAY...]
#   bash data/github-archive/scripts/build_rete.sh gharchive-2025-07-22 2025-07-22
#
# Multi-day: all per-day .nt files (plus the ontology) are merged by rete build.
# For ~month scale prefer the external build: set MEMORY_BUDGET_MB (e.g. 16384)
# and the script switches to --memory-budget-mb (no pyramid in that path).
set -euo pipefail

OUT="$1"; shift
DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
REPO="$(cd "$DIR/../.." && pwd)"

INPUTS=(/work/data/github-archive/rdf/gharchive-ontology.nt)
for DAY in "$@"; do
  if [ "$(ls "$DIR/rdf/$DAY"/*.nt 2>/dev/null | wc -l)" -lt 6 ]; then
    echo "== emitting RDF for $DAY"
    MSYS_NO_PATHCONV=1 docker run --rm -v "$REPO:/w" -w //w -e DAY="$DAY" \
      python:3.12-slim bash -c \
      "pip -q install duckdb pyarrow 2>/dev/null && python data/github-archive/scripts/to_rdf.py"
  fi
  for f in "$DIR/rdf/$DAY"/*.nt; do
    INPUTS+=("/work/data/github-archive/rdf/$DAY/$(basename "$f")")
  done
done

echo "== building web/$OUT.rete from ${#INPUTS[@]} input(s)"
if [ -n "${MEMORY_BUDGET_MB:-}" ]; then
  bash "$REPO/skills/rete-from-graph/scripts/rete" build "${INPUTS[@]}" \
    -o "/work/web/$OUT.rete" \
    --memory-budget-mb "$MEMORY_BUDGET_MB" --tmp-dir /work/data/github-archive/spill \
    --card --title "GH Archive events" --license "Public GitHub timeline (gharchive.org; content © its authors)" \
    --source "https://www.gharchive.org/" \
    --description "Public GitHub event stream with RDF-star provenance: events as PROV activities, actors/repos/orgs as first-class nodes, volatile repo metadata (stars, forks) as time-annotated observations."
else
  bash "$REPO/skills/rete-from-graph/scripts/rete" build "${INPUTS[@]}" \
    -o "/work/web/$OUT.rete" \
    --pyramid-algo types --card \
    --title "GH Archive events" --license "Public GitHub timeline (gharchive.org; content © its authors)" \
    --source "https://www.gharchive.org/" \
    --description "Public GitHub event stream with RDF-star provenance: events as PROV activities, actors/repos/orgs as first-class nodes, volatile repo metadata (stars, forks) as time-annotated observations."
fi

bash "$REPO/skills/rete-from-graph/scripts/verify_rete.sh" "/work/web/$OUT.rete"
