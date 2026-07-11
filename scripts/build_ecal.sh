#!/usr/bin/env bash
# Build the ECAL Library "digital twin" .rete from the harvested + normalized data.
#
#   1. retarget covers -> R2 WebP     -> data/ecal/normalized/ecal.r2.jsonl
#   2. convert to N-Triples (graph)   -> data/ecal/ecal.nt
#   3. rete build (typed pyramid + card) -> data/ecal/ecal.rete
#   4. verify
#
# Covers themselves: scripts/ecal/covers_to_webp.py (jpg->webp) then
# scripts/ecal/upload_covers_r2.py (-> R2 rete/ecal/covers/<id>.webp,
# served at https://data.graphplaza.com/ecal/covers/<id>.webp).
#
# Usage: bash scripts/build_ecal.sh [output.rete]
set -euo pipefail
cd "$(dirname "$0")/.."

OUT="${1:-data/ecal/ecal.rete}"
NT="data/ecal/ecal.nt"
JSONL="data/ecal/normalized/ecal.r2.jsonl"

echo "== [1/4] retarget covers -> R2 WebP =="
python scripts/ecal/retarget_covers_r2.py

echo "== [2/4] JSONL -> N-Triples =="
python scripts/bcul/jsonl_to_nt.py --in "$JSONL" --out "$NT" --base "https://data.ecal.ch/" --mint

echo "== [3/4] rete build ($OUT) =="
MSYS_NO_PATHCONV=1 docker run --rm -v "$PWD:/work" -w /work rete-dev:latest \
  /work/target/release/rete build "/work/$NT" -o "/work/$OUT" \
  --pyramid-algo types --card \
  --title "ECAL — École cantonale d'art de Lausanne, Bibliothèque (Digital Twin)" \
  --source "https://library.ecal.ch/" \
  --description "A range-queryable graph twin of the ECAL Library — the art & design library of the École cantonale d'art de Lausanne: 28,741 records across Art visuel, Photographie, Design (graphique & industriel), Cinéma and Théorie, with book covers, call-number sections and an author/subject network. Harvested politely from the BiblioMaker OPAC. Same schema.org + Dublin Core ontology as the BCU twin. Metadata only; rights per source." \
  --created "2026-07-11"

echo "== [4/4] verify =="
MSYS_NO_PATHCONV=1 docker run --rm -v "$PWD:/work" -w /work rete-dev:latest \
  /work/target/release/rete card "/work/$OUT" | head -20

echo "done -> $OUT"
ls -la "$OUT"
