#!/usr/bin/env bash
# Run a SPARQL query against a LOCAL .rete under a hard memory cap, to check whether
# it stays bounded. Completes = bounded within CAP; exit 137 = OOM-killed. Prints
# timing. Uses the repo's rete-dev Docker image (per repo convention).
#
# Usage: memcap_query.sh <file-relative-to-repo-root> "<sparql>" [cap]
#   bash skills/rete-local-query/scripts/memcap_query.sh web/mydata.rete \
#        "SELECT (COUNT(*) AS ?n) WHERE { ?s a <https://ex.org/C> }" 8g
# cap defaults to 8g. --memory-swap==--memory disables swap (a true RAM bound).
set -uo pipefail
export MSYS_NO_PATHCONV=1

FILE="${1:?usage: memcap_query.sh <file-relative-to-repo-root> \"<sparql>\" [cap]}"
QUERY="${2:?missing query}"
CAP="${3:-8g}"
IMAGE="${RETE_DEV_IMAGE:-rete-dev:latest}"

# repo root works from either skills/ mirror (skills/… or .claude/skills/…)
ROOT="$(git -C "$(dirname "$0")" rev-parse --show-toplevel 2>/dev/null || pwd)"

t=$(date +%s)
docker run --rm --memory="$CAP" --memory-swap="$CAP" \
  -v "$ROOT:/work" -w //work "$IMAGE" \
  //work/target/release/rete sparql "//work/$FILE" "$QUERY"
rc=$?
el=$(($(date +%s)-t))
if [ "$rc" -eq 137 ]; then
  echo ">> OOM-KILLED at cap=$CAP after ${el}s — query needs more than $CAP RAM"
else
  echo ">> exit=$rc, ${el}s — bounded within $CAP"
fi
exit "$rc"
