#!/usr/bin/env bash
# Runs INSIDE a detached rete-dev container (see build_single_rete.sh):
#   1. emit the Crossref works+refs Parquet -> one crossref.nt (ontology embedded)
#   2. external-build it -> web/crossref.rete (memory-bounded, spills to disk)
# Both phases inside ONE `docker run -d` so the whole thing survives host/turn
# kills. Mounts: /work = repo (D:/pro/rete), /spill = C:/rete-spill-crossref.
#
# Args: [BUDGET_MB]  (external-build RAM budget, default 8000)
set -uo pipefail
BUDGET_MB=${1:-8000}
LOG=/spill/pipeline.log
say(){ echo "$(date -Iseconds) $*" | tee -a "$LOG"; }

say "PIPELINE START budget=${BUDGET_MB}MB"
pip3 install -q --break-system-packages duckdb pyarrow rdflib 2>>"$LOG" || { say "pip FAILED"; exit 1; }

say "EMIT START -> /spill/nt (resumable shards)"
python3 /work/scripts/crossref/crossref_to_nt.py \
  --base /work/data/crossref/parquet-2026 \
  --ontology /work/data/crossref/crossref.ttl \
  --shard-dir /spill/nt --threads 8 \
  --memory-limit 20GB --temp-dir /spill/ddb-tmp 2>>/spill/emit.log
rc=$?
say "EMIT DONE rc=$rc shards=$(ls /spill/nt/*.nt 2>/dev/null | wc -l) bytes=$(du -sb /spill/nt 2>/dev/null | cut -f1)"
[ "$rc" -ne 0 ] && { say "EMIT FAILED — see /spill/emit.log (rerun resumes from shards)"; exit 1; }

say "BUILD START (memory-budget ${BUDGET_MB}MB, multi-shard input)"
/work/target/release/rete build /spill/nt/*.nt \
  --format nt --memory-budget-mb "$BUDGET_MB" --tmp-dir /spill \
  -o /work/web/crossref.rete \
  --title "Crossref March 2026 — scholarly works, metadata & the citation graph" \
  --license "CC-BY-4.0" \
  --source "https://www.crossref.org/learning/public-data-file/" \
  --description "The Crossref March 2026 Public Data File as one graph: ~179.5M DOI-registered works (journal articles, book chapters, proceedings, datasets, preprints, …) with title, container/venue, ISSN, publication year, publisher and authors (names + ORCID links), funders (Crossref Funder IDs), and ~2B DOI-matched citation edges (cx:cites). Work IRIs are the canonical https://doi.org/<doi>; authors point at https://orcid.org/<id> and funders at https://doi.org/10.13039/<id>, so it joins the ORCID, DataCite, OpenAIRE, DBLP and OpenCitations datasets on shared PIDs via the rete scholar hub. Aligned to FaBiO/CiTO/PRISM/FRAPO (ontology https://w3id.org/rete/crossref#). CC-BY 4.0." \
  >>/spill/build.log 2>&1
brc=$?
say "BUILD DONE rc=$brc out=/work/web/crossref.rete size=$(stat -c%s /work/web/crossref.rete 2>/dev/null)"
say "PIPELINE END rc=$brc"
