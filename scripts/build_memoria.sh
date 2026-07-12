#!/usr/bin/env bash
# Rebuild the Memòria (Spanish Civil War) .rete from data/memoria/memoria.nt.
# Standardises the custom vocab that lived under the fake TLD https://memoria.rete/:
#   ns#deathDate  -> schema:deathDate     ns#sex -> schema:gender
#   ns#profession -> schema:jobTitle
#   everything else (domain-specific fields + node IRIs + classes) is de-branded
#   from https://memoria.rete/ to the neutral PURL https://w3id.org/rete/memoria/.
#
# Usage: bash scripts/build_memoria.sh [output.rete]
set -euo pipefail
cd "$(dirname "$0")/.."

OUT="${1:-data/memoria/memoria.rete}"
NT="data/memoria/memoria.nt"

echo "== [1/3] remap standards + de-brand in $NT =="
sed -i \
  -e 's|https://memoria\.rete/ns#deathDate|http://schema.org/deathDate|g' \
  -e 's|https://memoria\.rete/ns#sex|http://schema.org/gender|g' \
  -e 's|https://memoria\.rete/ns#profession|http://schema.org/jobTitle|g' \
  -e 's|https://memoria\.rete/|https://w3id.org/rete/memoria/|g' \
  "$NT"
echo "   remaining memoria.rete refs: $(grep -c 'memoria\.rete' "$NT" || true)"

echo "== [2/3] rete build ($OUT) =="
MSYS_NO_PATHCONV=1 docker run --rm -v "$PWD:/work" -w /work rete-dev:latest \
  /work/target/release/rete build "/work/$NT" -o "/work/$OUT" \
  --pyramid-algo types --card-file /work/data/memoria/card.json

echo "== [3/3] verify =="
MSYS_NO_PATHCONV=1 docker run --rm -v "$PWD:/work" -w /work rete-dev:latest \
  /work/target/release/rete card "/work/$OUT" | grep -E 'title|triples|schema.org/(deathDate|gender|jobTitle)|w3id' | head
ls -la "$OUT"
