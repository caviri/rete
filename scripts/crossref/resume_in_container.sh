#!/usr/bin/env bash
# Finish an external build that was killed after the dictionary merge, from its
# surviving spill dir — rebuilds only the missing permutations (SPO is reused),
# embeds the card JSON, writes the final .rete. ~19 min–1 h; NO 5 h re-chunk.
# Runs INSIDE a detached rete-dev container (survives host/turn kills).
# Mounts: /work = repo (D:), /spill = C:/rete-spill-crossref.
set -uo pipefail
SPILL_SUBDIR=${1:?spill subdir e.g. .rete-extbuild-109-0}
BUDGET_MB=${2:-12000}
LOG=/spill/resume.log
say(){ echo "$(date -Iseconds) $*" | tee -a "$LOG"; }

# Large remote-lazy dataset: stage on the spill drive, NOT web/ — it goes to R2
# (data.graphplaza.com, HTTP-range) and is registered remote, never embedded.
export RETE_RESUME_SPILL="/spill/$SPILL_SUBDIR"
export RETE_RESUME_OUT=/spill/crossref.rete
export RETE_RESUME_TERMS=599367204
export RETE_RESUME_QUADS=3777727303
export RETE_RESUME_CARD=/work/data/crossref/crossref-card.json
export RETE_RESUME_BUDGET_MB="$BUDGET_MB"

say "RESUME START spill=$RETE_RESUME_SPILL budget=${BUDGET_MB}MB"
cd /work
cargo test -p rete-core --release --lib extbuild::tests::resume_from_spill -- \
  --ignored --nocapture >>"$LOG" 2>&1
rc=$?
say "RESUME DONE rc=$rc out=$RETE_RESUME_OUT size=$(stat -c%s "$RETE_RESUME_OUT" 2>/dev/null)"
# propagate failure so `--restart on-failure` resumes (completed perms are kept)
exit "$rc"
