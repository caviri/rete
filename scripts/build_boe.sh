#!/usr/bin/env bash
# Rebuild the BOE (Legislación Consolidada) .rete from data/boe/boe.nt.
# The extension vocabulary moved from https://graphplaza.com/ns/boe# to the neutral
# w3id PURL https://w3id.org/rete/boe# (norm-type classes + BOE-specific properties
# that have no ELI equivalent). ELI stays the standard vocabulary for everything else.
#
# Usage: bash scripts/build_boe.sh [output.rete]
set -euo pipefail
cd "$(dirname "$0")/.."

OUT="${1:-data/boe/boe.rete}"
NT="data/boe/boe.nt"
ONT="data/boe/boe_ont.ttl"   # the enriched OWL 2 QL ontology (taxonomy + domain/range + inverses + disjointness)

echo "== [1/3] remap graphplaza -> w3id in $NT =="
# in-place, only the exact old namespace string
sed -i 's#https://graphplaza.com/ns/boe#https://w3id.org/rete/boe#g' "$NT"
echo "   remaining graphplaza refs: $(grep -c 'graphplaza.com/ns/boe' "$NT" || true)"

echo "== [2/3] rete build ($OUT) — data + ontology merged, coherence stamped =="
# Merge the TBox so 🧠 Reason has axioms to work with; --reason stamps the
# coherence verdict into the card (query-time reasoning stays lazy/opt-in).
MSYS_NO_PATHCONV=1 docker run --rm -v "$PWD:/work" -w /work rete-dev:latest \
  /work/target/release/rete build "/work/$NT" "/work/$ONT" -o "/work/$OUT" \
  --pyramid-algo types --card --reason \
  --title "BOE — Legislación Consolidada (ELI)" \
  --license "Ley 37/2007 + BOE standard reuse licence (Resolución 27-jun-2024)" \
  --source "Agencia Estatal Boletín Oficial del Estado — datos abiertos (API legislación consolidada)" \
  --description "Basado en datos de la Agencia Estatal Boletín Oficial del Estado. The entire Spanish consolidated-legislation corpus (12,330 in-force norms) as an ELI knowledge graph: each norm at its canonical ELI IRI with title, rango, dates, author and subjects, plus the norm-to-norm citation/derogation network (repeals/amends/corrects/cites/transposes...). SKOS vocabularies for rango, materia, ámbito and consolidation status; departamentos as organisations."

echo "== [3/3] verify =="
MSYS_NO_PATHCONV=1 docker run --rm -v "$PWD:/work" -w /work rete-dev:latest \
  /work/target/release/rete card "/work/$OUT" | sed -n '1,10p'
ls -la "$OUT"
