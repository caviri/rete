#!/usr/bin/env bash
# Fetch the FULL Mapping Manuscript Migrations (MMM) knowledge graph and convert
# it into a `.rete` graph + lossless per-class entity tables (Parquet/DuckDB).
#
# MMM unified three manuscript-provenance databases — the Schoenberg Database of
# Manuscripts (SDBM, U. Penn), Bibale (IRHT-CNRS), and Medieval Manuscripts in
# Oxford Libraries (Bodleian) — into one CIDOC-CRM / FRBRoo graph of ~23.4M
# triples. Source: Zenodo DOI 10.5281/zenodo.4019643 (v2.1.0), CC BY-NC 4.0.
# This is the big sibling of the tiny `mmm` playground sample built by
# scripts/fetch_playground_kgs.sh (which CONSTRUCTs a 4-place slice).
#
# Pipeline:  Zenodo .zip  ──▶  4 source .ttl + schema  ──▶  mmm-full.rete
#                                                       └─▶  mmm-full.nt (export)
#                                                            └─▶  tables/*.parquet + mmm-tables.duckdb
#
# Usage:  scripts/fetch_mmm_full.sh [all|fetch|build|export|tables]   (default: all)
#
# The build/export run the Linux `rete` ELF in Docker (matching data/README.md);
# override $RETE to use a native binary, e.g.  RETE="rete" scripts/fetch_mmm_full.sh
set -euo pipefail
UA="rete-atlas/0.1 (https://github.com/caviri; carlosvivarrios@gmail.com)"
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
OUT="$ROOT/data/mmm"
ZIP="$OUT/mmm_data_v2.1.0.zip"
ZURL="https://zenodo.org/records/4019643/files/mmm_data_v2.1.0.zip?download=1"
MD5="2f97635fcee7561fd0cfdad60b810e85"
# Data graphs to ingest (the RDF/XML ontologies cidoc-crm.rdf / frbroo.rdf are
# TBox-only and not parsed by `rete build`; mmm-schema.ttl adds MMM term labels).
DATA=(mmm_sdbm.ttl mmm_bibale.ttl mmm_bodley.ttl mmm_places.ttl mmm-schema.ttl)
# Default `rete`: the release ELF in the project's Docker image (see README).
RETE="${RETE:-MSYS_NO_PATHCONV=1 docker run --rm -v $ROOT:/work -w /work rust:1.92-bookworm ./target/release/rete}"
PY="${PY:-uv run --no-project --with duckdb python}"
STEP="${1:-all}"
mkdir -p "$OUT"

do_fetch() {
  echo "== fetch MMM v2.1.0 (66.5 MB, CC BY-NC) =="
  [ -f "$ZIP" ] || curl -fSL -A "$UA" -o "$ZIP" "$ZURL"
  echo "$MD5  $ZIP" | md5sum -c - || { echo "MD5 mismatch — delete $ZIP and retry"; exit 1; }
  ( cd "$OUT" && unzip -o "$ZIP" >/dev/null )
  echo "  extracted: $(ls "$OUT"/*.ttl | wc -l) Turtle files"
}

do_build() {
  echo "== build mmm-full.rete (~23.4M triples; needs ~10 GB RAM) =="
  local inputs=(); for f in "${DATA[@]}"; do inputs+=("data/mmm/$f"); done
  $RETE build "${inputs[@]}" --no-pyramid \
    --card --title "Mapping Manuscript Migrations" --license "CC-BY-NC-4.0" \
    --source "https://doi.org/10.5281/zenodo.4019643" \
    -o data/mmm/mmm-full.rete
}

do_export() {
  echo "== export the graph to lossless N-Triples =="
  # Rust's line-buffered stdout flushes ~23.3M times; over a Docker-on-Windows
  # bind mount that is ~30× slower than writing to container-local tmpfs and
  # copying out once. Use that path for the Docker default; a native $RETE just
  # redirects.
  if [[ "$RETE" == *"docker run"* ]]; then
    MSYS_NO_PATHCONV=1 docker run --rm -v "$ROOT":/work -w /work rust:1.92-bookworm \
      bash -c './target/release/rete export data/mmm/mmm-full.rete > /tmp/mmm.nt && cp /tmp/mmm.nt /work/data/mmm/mmm-full.nt'
  else
    $RETE export data/mmm/mmm-full.rete > "$OUT/mmm-full.nt"
  fi
  echo "  $(wc -l < "$OUT/mmm-full.nt") triples -> mmm-full.nt"
}

do_tables() {
  echo "== build lossless per-class entity tables (Parquet + DuckDB) =="
  $PY "$ROOT/scripts/mmm_to_tables.py" --nt "$OUT/mmm-full.nt" \
    -o "$OUT/tables" --duckdb "$OUT/mmm-tables.duckdb" --verify
}

case "$STEP" in
  fetch)  do_fetch ;;
  build)  do_build ;;
  export) do_export ;;
  tables) do_tables ;;
  all)    do_fetch; do_build; do_export; do_tables ;;
  *) echo "unknown step: $STEP (use all|fetch|build|export|tables)"; exit 2 ;;
esac
echo "--- done ($STEP) ---"
