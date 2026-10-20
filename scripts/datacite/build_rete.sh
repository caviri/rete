#!/usr/bin/env bash
# DataCite Parquet -> datacite.rete, the faithful build (run INSIDE rete-dev).
#
#   bash scripts/datacite/build_rete.sh ROOT OUT_DIR [BUDGET_MB] [WORKERS] [SPILL_DIR]
#   CARD_ONLY=1 bash scripts/datacite/build_rete.sh ROOT OUT_DIR - [WORKERS]
#
#   ROOT     the Parquet root: parquet-2023/ -2024/ -2025/ and parquet-links-2023/ -may2025/
#   OUT_DIR  receives plan.parquet(+.json, .links.parquet), datacite.tally.json,
#            guard.json, datacite.rete, build logs
#
# Needs python3 with pyarrow, duckdb and pyoxigraph, and a `rete` on PATH (or $RETE).
# Steps:
#  1. plan: R9/R10 record choice and R18 event choice (DuckDB). It REFUSES to run if
#     Python and DuckDB key a DOI differently, if two registered DOIs would mint one
#     IRI, or if a PID Links uuid is missing or repeated within a file.
#  2. emit: N-Triples, every IRI checked by oxiri, the rule tally, and every record IRI
#     written to OUT/guard/ so no subject is emitted as a record twice.
#  3. rete build --strict --memory-budget-mb BUDGET -> datacite.rete.part, carrying the
#     committed Dataset Card scripts/datacite/cards/datacite.card.json.
#  4. guard.py: no functional property stated twice, no record IRI twice, records and
#     events reconcile with the plan, and the embedded card is exactly the card this
#     tally produces. Only then is datacite.rete.part renamed to datacite.rete.
# CARD_ONLY=1 runs 1-2 without rete (output discarded), then writes the card from the
# tally (make_metadata.py --tally): do this once whenever the converter or its input
# changes, commit the card, then build. The converter is deterministic, so step 4
# proves the card describes the file.
set -euo pipefail
ROOT=${1:?parquet root}; OUT=${2:?output dir}; BUDGET=${3:-14000}; WORKERS=${4:-10}
SPILL=${5:-/tmp/datacite-spill}
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
RETE=${RETE:-rete}
CARD="$HERE/cards/datacite.card.json"
mkdir -p "$OUT" "$SPILL"

if [ ! -f "$OUT/plan.parquet" ]; then
  python3 "$HERE/parquet_to_nt.py" plan --root "$ROOT" --out "$OUT/plan.parquet" 2> "$OUT/plan.log"
fi

emit() {
  python3 "$HERE/parquet_to_nt.py" emit --root "$ROOT" --plan "$OUT/plan.parquet" \
    --report "$OUT/datacite.tally.json" --workers "$WORKERS" --guard-dir "$OUT/guard" 2> "$OUT/convert.log"
}

if [ "${CARD_ONLY:-0}" = 1 ]; then
  emit > /dev/null
  python3 "$HERE/guard.py" --plan "$OUT/plan.parquet" --tally "$OUT/datacite.tally.json" | tee "$OUT/guard.json"
  python3 "$HERE/make_metadata.py" --tally "$OUT/datacite.tally.json" --out "$OUT" --card "$CARD"
  exit 0
fi

rm -f "$OUT/datacite.rete.part"
{ cat "$HERE/datacite-ontology.nt"; emit; } \
  | "$RETE" build - --format nt --strict --memory-budget-mb "$BUDGET" --tmp-dir "$SPILL" \
      -o "$OUT/datacite.rete.part" --card-file "$CARD" --no-card-costs

if python3 "$HERE/guard.py" --plan "$OUT/plan.parquet" --tally "$OUT/datacite.tally.json" \
     --card "$CARD" > "$OUT/guard.json"; then
  mv -f "$OUT/datacite.rete.part" "$OUT/datacite.rete"
  echo "guard PASS: $OUT/datacite.rete" >&2
else
  cat "$OUT/guard.json" >&2
  echo "guard FAIL: the build is left as $OUT/datacite.rete.part and must not be published" >&2
  exit 1
fi
