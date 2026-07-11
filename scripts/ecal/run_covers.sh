#!/usr/bin/env bash
# Auto-restart driver for the ECAL full-res cover download. Resumable (skips files
# already on disk), so we just rerun until a pass completes cleanly (exit 0).
cd "$(dirname "$0")" || exit 1
for attempt in $(seq 1 800); do
  python download_covers.py --rate 0.83
  if [ $? -eq 0 ]; then
    echo "ECAL COVERS COMPLETE (attempt $attempt)"
    break
  fi
  echo "[attempt $attempt] cover pass exited; resuming in 12s..."
  sleep 12
done
