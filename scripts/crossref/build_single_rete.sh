#!/usr/bin/env bash
# Crossref works+refs Parquet -> ONE crossref.rete (~4B triples: ~179.5M works +
# ~2B DOI-matched citation edges + authors/funders), via the memory-bounded
# external build. Modeled on scripts/orcid/build_single_rete.sh, but emit AND
# build run inside ONE DETACHED rete-dev container (`docker run -d`) — on this
# shared box, host background tasks get killed at turn boundaries, whereas a
# detached container (owned by the Docker daemon) survives. rete-dev ships
# python3+pip, so the emitter runs in-container too (parquet read via bind mount
# is fine: a full ref_doi scan is ~55 s).
#
# G: is a Google Drive virtual FS and CANNOT be docker-mounted — stage NT+spill
# on C: (needs ~620 GB free); output goes to the repo on D:.
#
# Usage:  bash scripts/crossref/build_single_rete.sh [SPILL_WIN] [BUDGET_MB]
#   SPILL_WIN (default C:/rete-spill-crossref)  BUDGET_MB (default 8000)
set -uo pipefail
cd "$(dirname "$0")/../.."
export MSYS_NO_PATHCONV=1

SPILL_WIN=${1:-C:/rete-spill-crossref}
BUDGET_MB=${2:-8000}

docker rm -f crossref-pipeline >/dev/null 2>&1 || true
# --restart on-failure: this box interrupts long containers every ~1.5-2h; the
# emit is resumable (skips completed shards) and the build restarts from the
# shards, so Docker auto-restarting on a non-zero exit grinds it to completion
# unattended. Exit 0 (success) does NOT restart.
docker run -d --name crossref-pipeline --oom-score-adj -500 \
  --restart on-failure:100 \
  -v "$PWD:/work" -v "$SPILL_WIN:/spill" -w /work \
  rete-dev:latest bash /work/scripts/crossref/build_in_container.sh "$BUDGET_MB"

echo "LAUNCHED crossref-pipeline"
echo "  progress: type \"$SPILL_WIN\\pipeline.log\"  (emit.log / build.log alongside)"
echo "  or:       docker logs -f crossref-pipeline"
