#!/usr/bin/env bash
# Full DataCite (metadata + PID-links) -> ONE .rete, streamed (no huge .nt).
set -o pipefail
export MSYS_NO_PATHCONV=1
ROOT="$(cd "$(dirname "$0")/../../.." && pwd)"   # data/datacite/scripts -> repo root
cd "$ROOT"
BUDGET="${RETE_BUDGET_MB:-16000}"
# Spill to the container's native /tmp (overlay ext4, world-writable 1777, ~600GB
# free) — NOT the flaky Windows drvfs mount that lost a spill file on the first
# run. Output still goes to /work: a single big sequential write to D: is fine;
# it's the random spill I/O that drvfs mishandles.
echo "=== datacite FULL build start (budget ${BUDGET} MiB, spill on native /tmp) ==="

{ cat data/datacite/datacite-ontology.nt ;
  python data/datacite/scripts/parquet_to_nt.py ; } | \
  docker run -i --rm -v "$ROOT:/work" -w /work rete-dev:latest \
    /work/target/release/rete build - --format nt --memory-budget-mb "$BUDGET" \
    --tmp-dir /tmp \
    -o /work/web/datacite.rete --card \
    --title "DataCite" \
    --license "CC0-1.0" \
    --source "https://datacite.org" \
    --description "DataCite Public Data File (CC0) as one range-queryable .rete: ~233M DOI-identified research outputs (datasets, software, text, images, …) with creators (reified, ORCID-keyed), funders, subjects and metadata, PLUS the ~761M-edge PID Graph — the typed relation network (Cites, IsSupplementTo, IsVersionOf, IsPartOf, IsDerivedFrom, IsIdenticalTo, …) reified as provenance-bearing dcite:PidRelation. Resources are doi.org IRIs, agents orcid.org IRIs, so it joins the scholarly graph by IRI. Modelled via the rete dcite: ontology (schema.org/DCAT/FaBiO/CiTO/PAV/PROV/FRAPO)."

rc=$?
echo "=== build exit code: $rc ==="
exit $rc
