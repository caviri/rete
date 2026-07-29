#!/usr/bin/env bash
# Launch a harvest stage detached in Docker, authenticated via the host `hf` login.
# (The token goes into the container environment only — never onto disk.)
#
#   bash data/hugging-face/scripts/run_harvest.sh all        # profiles → members → followers → following
#   bash data/hugging-face/scripts/run_harvest.sh profiles|members|followers|following
#
# Watch:  docker logs -f hf-<stage>     Stop:  docker stop hf-<stage>
# Re-running after a stop just continues (the _done.txt files carry the state).
set -euo pipefail

STAGE="${1:?usage: run_harvest.sh all|profiles|members|followers|following}"
TOKEN="$(hf auth token)"
[ -n "$TOKEN" ] || { echo "no hf token — run 'hf auth login' first" >&2; exit 1; }

S="data/hugging-face/scripts"
case "$STAGE" in
  all)       CHAIN="python $S/harvest_spaces_links.py \
    && python $S/harvest_profiles.py --workers 12 \
    && python $S/harvest_edges.py --kind members --workers 12 \
    && python $S/harvest_edges.py --kind followers --workers 12 \
    && python $S/harvest_edges.py --kind following --workers 12" ;;
  spacelinks) CHAIN="python $S/harvest_spaces_links.py" ;;
  profiles)  CHAIN="python $S/harvest_profiles.py --workers 12" ;;
  *)         CHAIN="python $S/harvest_edges.py --kind $STAGE --workers 12" ;;
esac

docker rm -f "hf-$STAGE" 2>/dev/null || true
MSYS_NO_PATHCONV=1 docker run -d --name "hf-$STAGE" \
  -e HF_TOKEN="$TOKEN" \
  -v "D:/pro/rete:/w" -w //w python:3.12-slim bash -c "$CHAIN"
echo "started hf-$STAGE — follow with: docker logs -f hf-$STAGE"
