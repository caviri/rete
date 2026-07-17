#!/usr/bin/env bash
# ORCID summaries -> ONE orcid.rete (1.30B triples, 397M terms) via the
# memory-bounded external build (`rete build --memory-budget-mb`).
#
# Two ROBUST phases instead of one host pipeline — a host-side kill once took
# an attached build container down mid-permutation, so:
#   1. emitter -> raw N-Triples file on the spill drive (cheap to restart)
#   2. DETACHED docker build (docker run -d) reading that file
#
# Usage:  bash scripts/orcid/build_single_rete.sh [SPILL_DIR]
#   SPILL_DIR (default C:/Users/Kato/rete-spill) needs ~350 GB free and must be
#   on a Docker-shared drive. Build takes ~5h, peak RSS stays inside the budget.
set -uo pipefail
cd "$(dirname "$0")/../.."
export MSYS_NO_PATHCONV=1 PYTHONIOENCODING=utf-8

SPILL_WIN=${1:-C:/Users/Kato/rete-spill}
SPILL_MSYS=/$(echo "$SPILL_WIN" | sed 's/^\([A-Za-z]\):/\L\1/')
NT="$SPILL_MSYS/orcid.nt"

echo "EMIT START $(date -Iseconds)"
# Windows-style script path: MSYS_NO_PATHCONV=1 (needed for docker -v) also
# stops msys from rewriting /d/... paths for the Windows python.exe
python "$(cygpath -m "$PWD")/scripts/orcid/orcid_to_nt.py" > "$NT" 2>emit-orcid.log
rc=$?
echo "EMIT EXIT rc=$rc $(date -Iseconds) size=$(stat -c%s "$NT" 2>/dev/null)"
if [ "$rc" -ne 0 ]; then
  echo "EMIT FAILED — not launching the build"
  exit 1
fi

docker run -d --name orcid-build \
  -v "$PWD:/work" -v "$SPILL_WIN:/spill" -w /work \
  rete-dev:latest /work/target/release/rete build /spill/orcid.nt \
    --memory-budget-mb 16384 --tmp-dir /spill \
    -o /work/web/orcid.rete \
    --title "ORCID Public Data File 2025 — researchers, works, affiliations" \
    --license "CC0-1.0" \
    --source "https://orcid.org" \
    --description "The ORCID 2025 summaries as one graph: 25M researchers, 149.8M works (78% with DOIs), 25M affiliations (ROR-linked where disambiguated), 1.8M fundings. Researcher IRIs are canonical https://orcid.org/... URLs; affiliations point at ROR organization IRIs, joining ror.rete directly."
echo "BUILD CONTAINER LAUNCHED (docker logs -f orcid-build) $(date -Iseconds)"
