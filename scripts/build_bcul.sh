#!/usr/bin/env bash
# Build the BCU Lausanne "digital twin" .rete from the harvested + normalized data.
#
#   1. merge per-source JSONL      -> data/bcul/normalized/bcul.jsonl
#   2. convert to N-Triples (graph)-> data/bcul/bcul.nt
#   3. rete build (typed pyramid + card) -> data/bcul/bcul.rete
#   4. verify
#
# Usage: bash scripts/build_bcul.sh [output.rete]
set -euo pipefail
cd "$(dirname "$0")/.."

OUT="${1:-data/bcul/bcul.rete}"
NT="data/bcul/bcul.nt"

echo "== [1/4] merge per-source JSONL -> bcul.jsonl =="
python scripts/bcul/normalize.py

echo "== [2/4] JSONL -> N-Triples =="
python scripts/bcul/jsonl_to_nt.py --out "$NT"

echo "== [3/4] rete build ($OUT) =="
MSYS_NO_PATHCONV=1 docker run --rm -v "$PWD:/work" -w /work rete-dev:latest \
  /work/target/release/rete build "/work/$NT" -o "/work/$OUT" \
  --pyramid-algo types --card \
  --title "Bibliothèque cantonale et universitaire de Lausanne — Digital Twin" \
  --source "https://www.bcu-lausanne.ch/" \
  --description "A range-queryable graph twin of BCU Lausanne: the Renouvaud catalogue (Alma SRU), the Patrinum digital-heritage repository (OAI-PMH), medieval manuscripts from e-codices (IIIF), and the Scriptorium digitized press. Metadata aggregated from the library's public APIs; rights vary per source (see dcterms:rights)." \
  --created "2026-07-07"

echo "== [4/4] verify =="
MSYS_NO_PATHCONV=1 docker run --rm -v "$PWD:/work" -w /work rete-dev:latest \
  /work/target/release/rete card "/work/$OUT" | head -40

echo "done -> $OUT"
ls -la "$OUT"
