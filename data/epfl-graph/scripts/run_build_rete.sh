#!/usr/bin/env bash
# EPFL GraphOntology -> ONE .rete, streamed (nodes + edges + RDF-star scores).
# Small budget so it coexists with a heavier build running in the same Docker VM.
set -o pipefail
export MSYS_NO_PATHCONV=1
ROOT="$(cd "$(dirname "$0")/../../.." && pwd)"   # data/epfl-graph/scripts -> repo root
cd "$ROOT"
mkdir -p data/epfl-graph/_spill
BUDGET="${RETE_BUDGET_MB:-4000}"
echo "=== epfl-graph build start (budget ${BUDGET} MiB) ==="

{ cat data/epfl-graph/epfl-graph-ontology.nt ;
  python data/epfl-graph/scripts/parquet_to_nt.py ; } | \
  docker run -i --rm -v "$ROOT:/work" -w /work rete-dev:latest \
    /work/target/release/rete build - --format nt --memory-budget-mb "$BUDGET" \
    -o /work/web/epfl-graph.rete --card \
    --title "EPFL GraphOntology" \
    --license "Apache-2.0" \
    --source "https://www.epfl.ch/about/data/epfl-graph/" \
    --description "The EPFL GraphOntology (Apache-2.0) as one range-queryable .rete: a Wikipedia-derived concept graph modelled on SKOS. 6.2M concept nodes (epfl:Concept) + Wikipedia categories, linked by the concept-concept relation network — epfl:related / epfl:relatedDirected, the 192.6M-edge embedding-similarity graph (epfl:similarTo with the similarity score attached via RDF-star as epfl:score), category hierarchy (epfl:broader), anchor pages and OpenAlex-topic alignment. Vocabulary epfl: = w3id.org/rete/epflgraph#, aligned to SKOS/Dublin Core/schema.org. Built by data/epfl-graph/scripts/parquet_to_nt.py -> streamed rete build. (Node full-text and raw embedding vectors are separate optional layers, not in this build.)"

rc=$?
echo "=== build exit code: $rc ==="
exit $rc
