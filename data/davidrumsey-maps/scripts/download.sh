#!/usr/bin/env bash
# Staged, reproducible harvest of the David Rumsey Historical Map Collection
# (LUNA, www.davidrumsey.com) — 150,017 items. Metadata via IIIF (robots-clean),
# images via the published Size0-4 JPEG tiers / JP2 masters.
#
#   bash data/davidrumsey-maps/scripts/download.sh [stage]
#
# Stages (default: all, in order; every stage is resume-safe — just re-run):
#   enum       ~15MB   enumerate all item ids via IIIF collection pagination
#   manifests  ~600MB  one gzipped IIIF manifest per item (the full metadata)
#   extract    ~150MB  flatten to rumsey_items.jsonl.gz + per-tier URL TSVs
#   thumbs     ~1.5GB  Size0 (~96px) for every map
#   size2      ~25GB   Size2 (~768px) for every map
#   size3      ~80GB   OPTIONAL, not run by default
#   masters    4-15TB  OPTIONAL, never run by default (JP2 full-res masters)
#
# DISK GUARD: stages refuse to start with < the space they need. Full-res
# masters for the whole collection are TB-scale — run `masters` only against a
# curated subset TSV (head -n N raw/assets/jp2_masters.tsv > subset.tsv).
set -euo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(cd "$HERE/../../.." && pwd)"       # repo root
DATA="data/davidrumsey-maps"
RAW="$DATA/raw"
STAGE="${1:-all}"

# Docker runner (repo convention: everything in Docker; MSYS_NO_PATHCONV guards
# Git-Bash path mangling on Windows). Override DOCKER_PY to use host python.
PY() {
  MSYS_NO_PATHCONV=1 docker run --rm -v "$ROOT:/w" -w //w python:3.12-slim python "$@"
}

need_gb() { # need_gb <GB> — abort if the volume holding raw/ has less free
  local need=$1 free
  free=$(df -BG --output=avail "$ROOT/$RAW" 2>/dev/null | tail -1 | tr -dc '0-9' || echo 0)
  if [ "${free:-0}" -lt "$need" ]; then
    echo "ABORT: need ~${need}GB free for this stage, have ${free}GB" >&2
    exit 3
  fi
}

run_enum()      { PY "$DATA/scripts/enumerate_iiif.py" --workers 4; }
run_manifests() { need_gb 2;  PY "$DATA/scripts/harvest_manifests.py" --workers 6; }
run_extract()   { need_gb 1;  PY "$DATA/scripts/extract_metadata.py"; }
run_thumbs()    { need_gb 3;  PY "$DATA/scripts/fetch_tiles.py" "$RAW/assets/size0.tsv" "$RAW/images/size0" --workers 8; }
run_size2()     { need_gb 30; PY "$DATA/scripts/fetch_tiles.py" "$RAW/assets/size2.tsv" "$RAW/images/size2" --workers 8; }
run_size3()     { need_gb 90; PY "$DATA/scripts/fetch_tiles.py" "$RAW/assets/size3.tsv" "$RAW/images/size3" --workers 6; }
run_masters()   { echo "run manually against a SUBSET of raw/assets/jp2_masters.tsv (full set is TB-scale)" >&2; exit 4; }

case "$STAGE" in
  enum)      run_enum ;;
  manifests) run_manifests ;;
  extract)   run_extract ;;
  thumbs)    run_thumbs ;;
  size2)     run_size2 ;;
  size3)     run_size3 ;;
  masters)   run_masters ;;
  all)       run_enum; run_manifests; run_extract; run_thumbs; run_size2 ;;
  *) echo "usage: download.sh [enum|manifests|extract|thumbs|size2|size3|masters|all]" >&2; exit 2 ;;
esac
