#!/usr/bin/env bash
# Self-contained variant of harvest_cycle.sh that runs ENTIRELY inside one
# python:3.12-slim container (no host docker client needed):
#
#   MSYS_NO_PATHCONV=1 docker run -d --name rumsey-cycle \
#     -v "D:/pro/rete:/w" -w //w python:3.12-slim \
#     bash data/davidrumsey-maps/scripts/cycle_incontainer.sh
#
# Repeats extract -> size0 sweep -> size2 sweep until the catalog consolidation
# marker (raw/items_index.tsv) exists, then does one final full cycle and
# exits. Progress: data/davidrumsey-maps/raw/cycle.log (and `docker logs`).
set -u
D=data/davidrumsey-maps
R=$D/raw
LOG=$R/cycle.log
n=0
while :; do
  n=$((n + 1))
  final=0
  [ -s "$R/items_index.tsv" ] && final=1
  python "$D/scripts/extract_metadata.py" >> "$LOG" 2>&1
  python "$D/scripts/fetch_tiles.py" "$R/assets/size0.tsv" "$R/images/size0" --workers 6 >> "$LOG" 2>&1
  python "$D/scripts/fetch_tiles.py" "$R/assets/size2.tsv" "$R/images/size2" --workers 6 >> "$LOG" 2>&1
  echo "[cycle $n] final=$final $(date -u +%H:%M:%S)" | tee -a "$LOG"
  if [ "$final" = 1 ]; then
    echo "[cycle] converged after final sweep" | tee -a "$LOG"
    break
  fi
  sleep 30
done
