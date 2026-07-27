#!/usr/bin/env bash
# Full OpenCitations Meta -> ONE .rete, streamed (no huge intermediate .nt).
#   { ontology TBox ; parquet->NT (all fields, authors+editors) } | rete build -
# Memory-bounded external build (byte-identical to --no-pyramid). Resumable spill.
set -o pipefail
export MSYS_NO_PATHCONV=1

ROOT="$(cd "$(dirname "$0")/../../.." && pwd)"   # data/opencitations/scripts -> repo root
cd "$ROOT"
mkdir -p data/opencitations/_spill

BUDGET="${RETE_BUDGET_MB:-6000}"
echo "=== opencitations FULL build start (budget ${BUDGET} MiB) ==="

{ cat data/opencitations/opencitations-ontology.nt ;
  python data/opencitations/scripts/parquet_to_nt.py ; } | \
  docker run -i --rm -v "$ROOT:/work" -w /work rete-dev:latest \
    /work/target/release/rete build - --format nt --memory-budget-mb "$BUDGET" \
    -o /work/web/opencitations.rete --card \
    --title "OpenCitations Meta" \
    --license "CC0-1.0" \
    --source "https://opencitations.net/meta" \
    --description "OpenCitations Meta v13.1.0 (CC0) as one range-queryable .rete: 135.4M bibliographic resources with the OMID/DOI/PMID/OpenAlex/ISSN/ISBN crosswalk, titles, FaBiO types, dates, volume/issue/pages, venue (partOf), publisher, and the full authorship/editorship graph (reified oc:AgentRole with order; agents keyed by ORCID IRI where present). Subjects are doi.org IRIs where a DOI exists, so it joins the scholarly graph by IRI. Modelled on the OpenCitations Data Model via the rete oc: ontology."

rc=$?
echo "=== build exit code: $rc ==="
exit $rc
