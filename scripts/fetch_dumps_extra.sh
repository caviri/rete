#!/usr/bin/env bash
# Atlas overlay providers served as static GitHub dumps (no flaky SPARQL):
#   theographic-bible — biblical events x places (CC BY-SA 4.0), INSTANT, ex:year
#   samian-ware       — Roman terra sigillata potters at production centres (DPPL), INTERVAL
# Outputs N-Triples in the atlas GeoSPARQL shape under data/atlas-extra/.
# Usage:  scripts/fetch_dumps_extra.sh [theographic|samian ...]   (default: both)
set -e
RAW_THEO="https://raw.githubusercontent.com/robertrouse/theographic-bible-metadata/master/CSV"
RAW_SAM="https://raw.githubusercontent.com/RGZM/samian-lod/main/data"
OUT="data/atlas-extra"
PY="${OHM_PYTHON:-python}"
SEL=("$@"); [ ${#SEL[@]} -eq 0 ] && SEL=(theographic samian)
want() { for s in "${SEL[@]}"; do [ "$s" = "$1" ] && return 0; done; return 1; }

if want theographic; then
  echo "== theographic-bible =="
  mkdir -p "$OUT/theographic"
  curl -sL "$RAW_THEO/Places.csv" -o "$OUT/theographic/Places.csv"
  curl -sL "$RAW_THEO/Events.csv" -o "$OUT/theographic/Events.csv"
  PYTHONIOENCODING=utf-8 "$PY" scripts/theographic_to_nt.py "$OUT/theographic/Places.csv" "$OUT/theographic/Events.csv" > "$OUT/theographic-bible.nt"
  echo "  -> $OUT/theographic-bible.nt ($(grep -c BibleEvent "$OUT/theographic-bible.nt") events)"
fi

if want samian; then
  echo "== samian-ware =="
  mkdir -p "$OUT/samian"
  for f in ae_independentpotter_1 ae_independentpotter_2 ae_chiefpotter_1 ae_dependentpotter_1 \
           ae_partnerpotter_1 ae_cooperationpotter_1 ae_cooperationandchiefpotter_1 \
           ct_ae_pc_1 loc_productioncentre_1; do
    curl -sL "$RAW_SAM/$f.ttl" -o "$OUT/samian/$f.ttl"
  done
  PYTHONIOENCODING=utf-8 "$PY" scripts/samian_to_nt.py "$OUT/samian" > "$OUT/samian-ware.nt"
  echo "  -> $OUT/samian-ware.nt ($(grep -c '/Potter' "$OUT/samian-ware.nt") potters)"
fi
echo "--- done ---"
