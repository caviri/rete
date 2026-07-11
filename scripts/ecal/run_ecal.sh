#!/usr/bin/env bash
# Auto-restart driver for the ECAL harvest. The harness sometimes reaps detached
# background children; the harvest is fully checkpointed, so we just resume it
# (from state/ecal.json) whenever the python process exits, until it reports done.
cd "$(dirname "$0")" || exit 1
STATE="D:/pro/rete/data/ecal/state/ecal.json"
for attempt in $(seq 1 800); do
  python harvest_ecal.py --rate 0.83
  done=$(python -c "import json;print(json.load(open('$STATE'))['done'])" 2>/dev/null)
  if [ "$done" = "True" ]; then
    echo "ECAL HARVEST COMPLETE (attempt $attempt)"
    break
  fi
  nid=$(python -c "import json;print(json.load(open('$STATE'))['next_id'])" 2>/dev/null)
  echo "[attempt $attempt] harvest exited at id ${nid}; resuming in 12s..."
  sleep 12
done
