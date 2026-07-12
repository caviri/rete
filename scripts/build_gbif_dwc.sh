#!/usr/bin/env bash
# Rebuild the GBIF-birds .rete with STANDARD vocabulary, no re-download.
# Transforms the local birds_enriched.rete in place: export -> remap -> build.
#
#   graphplaza custom vocab            -> standard term
#   vocab#rank                         -> dwc:taxonRank        (Darwin Core, literal)
#   vocab#inCountry                    -> dct:spatial          (Dublin Core, resource)
#   vocab#sourceDataset                -> void:inDataset       (VoID, resource)
#   vocab#Dataset (class)              -> void:Dataset
#   vocab#Country (class)              -> schema:Country
#   https://rete.graphplaza.com/gbif/  -> https://w3id.org/rete/gbif/   (taxon + node IRIs de-branded)
#
# Usage: bash scripts/build_gbif_dwc.sh
set -euo pipefail
cd "$(dirname "$0")/.."

SRC="data/gbif_birds/birds_enriched.rete"
NQ="data/gbif_birds/gbif_dwc.nq"
OUT="data/gbif_birds/gbif-birds.rete"
D="rete-dev:latest"

echo "[1/3] export + remap -> $NQ  ($(date -u +%H:%M:%SZ))"
MSYS_NO_PATHCONV=1 docker run --rm -v "$PWD:/work" -w /work "$D" \
  /work/target-std/release/rete export "/work/$SRC" --format nq 2>/dev/null | \
  sed -E \
    -e 's|https://rete\.graphplaza\.com/gbif/vocab#inCountry|http://purl.org/dc/terms/spatial|g' \
    -e 's|https://rete\.graphplaza\.com/gbif/vocab#sourceDataset|http://rdfs.org/ns/void#inDataset|g' \
    -e 's|https://rete\.graphplaza\.com/gbif/vocab#rank|http://rs.tdwg.org/dwc/terms/taxonRank|g' \
    -e 's|https://rete\.graphplaza\.com/gbif/vocab#Dataset|http://rdfs.org/ns/void#Dataset|g' \
    -e 's|https://rete\.graphplaza\.com/gbif/vocab#Country|http://schema.org/Country|g' \
    -e 's|https://rete\.graphplaza\.com/gbif/|https://w3id.org/rete/gbif/|g' \
  > "$NQ"
echo "[1/3] wrote $(wc -l < "$NQ") lines  ($(date -u +%H:%M:%SZ))"

echo "[2/3] build -> $OUT  ($(date -u +%H:%M:%SZ))"
MSYS_NO_PATHCONV=1 docker run --rm -v "$PWD:/work" -w /work "$D" \
  /work/target-std/release/rete build "/work/$NQ" -o "/work/$OUT" \
  --pyramid-algo types --card \
  --title "GBIF Birds — Spain & Switzerland (enriched)" \
  --license "CC BY-NC 4.0 (GBIF)"

echo "[3/3] verify  ($(date -u +%H:%M:%SZ))"
MSYS_NO_PATHCONV=1 docker run --rm -v "$PWD:/work" -w /work "$D" \
  /work/target-std/release/rete card "/work/$OUT" | grep -E 'title|triples|taxonRank|dwc|void|schema.org/Country' | head
echo "-- any graphplaza predicate left? (expect 0) --"
MSYS_NO_PATHCONV=1 docker run --rm -v "$PWD:/work" -w /work "$D" \
  /work/target-std/release/rete sparql "/work/$OUT" \
  "SELECT (COUNT(*) AS ?n) WHERE { ?s ?p ?o FILTER(CONTAINS(STR(?p),'graphplaza')) }" 2>/dev/null | grep -i '?n'
ls -la "$OUT"
echo "DONE gbif dwc rebuild  ($(date -u +%H:%M:%SZ))"
