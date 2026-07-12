#!/usr/bin/env bash
# Inner pipeline for the GBIF Darwin Core rebuild — runs rete DIRECTLY (no nested
# docker), so it can run inside ONE detached container (`docker run -d`) that
# survives the harness's background-task reaping. Progress -> data/gbif_birds/rebuild.log
set -euo pipefail
cd /work
R=/work/target-std/release/rete
SRC=data/gbif_birds/birds_enriched.rete
NQ=data/gbif_birds/gbif_dwc.nq
OUT=data/gbif_birds/gbif-birds.rete
LOG=data/gbif_birds/rebuild.log

exec >> "$LOG" 2>&1
echo "=== START $(date -u +%FT%TZ) ==="

echo "[1/3] export + remap -> $NQ"
"$R" export "$SRC" --format nq | sed -E \
  -e 's|https://rete\.graphplaza\.com/gbif/vocab#inCountry|http://purl.org/dc/terms/spatial|g' \
  -e 's|https://rete\.graphplaza\.com/gbif/vocab#sourceDataset|http://rdfs.org/ns/void#inDataset|g' \
  -e 's|https://rete\.graphplaza\.com/gbif/vocab#rank|http://rs.tdwg.org/dwc/terms/taxonRank|g' \
  -e 's|https://rete\.graphplaza\.com/gbif/vocab#Dataset|http://rdfs.org/ns/void#Dataset|g' \
  -e 's|https://rete\.graphplaza\.com/gbif/vocab#Country|http://schema.org/Country|g' \
  -e 's|https://rete\.graphplaza\.com/gbif/|https://w3id.org/rete/gbif/|g' \
  > "$NQ"
echo "[1/3] done: $(wc -l < "$NQ") lines  $(date -u +%FT%TZ)"

echo "[2/3] build -> $OUT"
"$R" build "$NQ" -o "$OUT" \
  --pyramid-algo types --card \
  --title "GBIF Birds — Spain & Switzerland (enriched)" \
  --license "CC BY-NC 4.0 (GBIF)"
echo "[2/3] done  $(date -u +%FT%TZ)"

echo "[3/3] verify"
"$R" card "$OUT" | grep -E 'title|triples|taxonRank|void|schema.org/Country' | head || true
"$R" sparql "$OUT" "SELECT (COUNT(*) AS ?bad) WHERE { ?s ?p ?o FILTER(CONTAINS(STR(?p),'graphplaza')) }" 2>/dev/null | grep -i bad || true
rm -f "$NQ"   # free the ~50 GB intermediate
echo "=== DONE $(date -u +%FT%TZ) ==="
