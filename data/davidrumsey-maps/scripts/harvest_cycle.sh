#!/usr/bin/env bash
# Trail the running catalog harvest with incremental extract+fetch cycles:
#   extract -> size0 sweep -> size2 sweep, repeat every ~30s.
# Exits when the catalog consolidation marker (raw/items_index.tsv, written by
# harvest_catalog.py at the very end) exists AND a full cycle downloads nothing
# new — i.e. images have converged on the complete catalog.
#
#   bash data/davidrumsey-maps/scripts/harvest_cycle.sh
#
# Safe to run alongside harvest_catalog.py: fetchers hit the static image
# hosts, the catalog lane holds the single /luna/servlet connection.
set -uo pipefail   # no -e: one transient docker/network hiccup must not kill the loop
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../../.." && pwd)"
DATA="data/davidrumsey-maps"
RAW="$DATA/raw"

PY() { MSYS_NO_PATHCONV=1 docker run --rm -v "$ROOT:/w" -w //w python:3.12-slim python "$@"; }

ok_of() { grep '^DONE' <<<"$1" | grep -oE 'ok=[0-9]+' | cut -d= -f2; }

cycle=0
while :; do
  cycle=$((cycle + 1))
  done_marker=0
  [ -s "$ROOT/$RAW/items_index.tsv" ] && done_marker=1
  PY "$DATA/scripts/extract_metadata.py" > /dev/null 2>&1
  out0="$(PY "$DATA/scripts/fetch_tiles.py" "$RAW/assets/size0.tsv" "$RAW/images/size0" --workers 6 2>&1)"
  out2="$(PY "$DATA/scripts/fetch_tiles.py" "$RAW/assets/size2.tsv" "$RAW/images/size2" --workers 6 2>&1)"
  ok0="$(ok_of "$out0")"; ok2="$(ok_of "$out2")"
  echo "[cycle $cycle] done_marker=$done_marker new: size0=${ok0:-?} size2=${ok2:-?}"
  if [ "$done_marker" = 1 ] && [ "${ok0:-1}" = 0 ] && [ "${ok2:-1}" = 0 ]; then
    echo "[cycle] converged: catalog complete, nothing new to fetch"
    break
  fi
  sleep 30
done

for t in size0 size2; do
  f="$ROOT/$RAW/images/$t/download_failures.txt"
  if [ -s "$f" ]; then
    echo "[cycle] WARNING: $(wc -l < "$f") persistent failures in $t (see $f)"
  fi
done
