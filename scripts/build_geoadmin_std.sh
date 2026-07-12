#!/usr/bin/env bash
# Rebuild the geoadmin (world admin boundaries) .rete from data/geoadmin/geoadmin.nt.
# Standardises the custom vocab that lived under the fake TLD https://geoadmin.rete/:
#   prop/name   -> schema:name        prop/partOf -> dct:isPartOf
#   everything else (iso, adminLevel, population, geomFine, classes, node IRIs) is
#   de-branded from https://geoadmin.rete/ to https://w3id.org/rete/geoadmin/.
# The PMTiles basemap is a separate next-to file on R2 (not embedded), so untouched.
#
# Usage: bash scripts/build_geoadmin_std.sh [output.rete]
set -euo pipefail
cd "$(dirname "$0")/.."

OUT="${1:-data/geoadmin/geoadmin.rete}"
NT="data/geoadmin/geoadmin.nt"

echo "== [1/3] remap standards + de-brand in $NT =="
sed -i \
  -e 's|https://geoadmin\.rete/prop/name|http://schema.org/name|g' \
  -e 's|https://geoadmin\.rete/prop/partOf|http://purl.org/dc/terms/isPartOf|g' \
  -e 's|https://geoadmin\.rete/|https://w3id.org/rete/geoadmin/|g' \
  "$NT"
echo "   remaining geoadmin.rete refs: $(grep -c 'geoadmin\.rete' "$NT" || true)"

echo "== [2/3] rete build ($OUT) =="
MSYS_NO_PATHCONV=1 docker run --rm -v "$PWD:/work" -w /work rete-dev:latest \
  /work/target/release/rete build "/work/$NT" -o "/work/$OUT" \
  --pyramid-algo types --card \
  --title "geoBoundaries — world administrative boundaries (GeoSPARQL)" \
  --license "CC BY 4.0 (geoBoundaries)" \
  --source "https://www.geoboundaries.org/"

echo "== [3/3] verify =="
MSYS_NO_PATHCONV=1 docker run --rm -v "$PWD:/work" -w /work rete-dev:latest \
  /work/target/release/rete sparql "/work/$OUT" \
  "SELECT (COUNT(*) AS ?bad) WHERE { ?s ?p ?o FILTER(CONTAINS(STR(?p),'geoadmin.rete')) }" 2>/dev/null | grep -i bad
ls -la "$OUT"
