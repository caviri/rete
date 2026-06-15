#!/usr/bin/env bash
# Fetch the FULL OpenHistoricalMap daily planet snapshot and convert it to atlas
# N-Triples — the uncapped counterpart of scripts/fetch_ohm.sh (which pulls only
# a ~5,300-element Overpass slice). The Overpass API CANNOT bulk-export (caps +
# 180s/900s timeout); OHM's own docs say "use planet mirrors for full data".
#
# Source: https://planet.openhistoricalmap.org/  (Amazon S3, public, CC0 1.0)
#   daily current-state snapshot : planet/planet-YYMMDD_NNNN.osm.pbf   (~1.1 GB)
#   weekly full revision history : planet/full-history/...             (we do NOT
#                                  want this — it duplicates every edit version)
#
# Pipeline:  resolve latest daily .osm.pbf -> download (resumable) -> PyOsmium
#            stream + simplify -> data/ohm/ohm-full.nt   (then build to .rete).
#
# Requires: python3 with pyosmium>=4 and shapely  (pip install osmium shapely); curl.
#
# Usage:  scripts/fetch_ohm_planet.sh
# Env:    OHM_PBF=path          skip download, convert this local .osm.pbf
#         OHM_SIMPLIFY_TOL=deg  geometry simplify tolerance (default 0.0005; 0 off)
#         OHM_KEEP_PBF=0        delete the .pbf after converting (default: keep)
set -euo pipefail

HERE="$(cd "$(dirname "$0")" && pwd)"
OUTDIR="$(cd "$HERE/.." && pwd)/data/ohm"
mkdir -p "$OUTDIR"
S3="https://s3.amazonaws.com/planet.openhistoricalmap.org/"
UA="rete-atlas/0.1 (+https://github.com/caviri/rete; carlosvivarrios@gmail.com)"

# Pick a working python (Windows Store shim exits non-zero — probe with --version).
PY=""
for cand in "${OHM_PYTHON:-}" python3 python py; do
  [ -n "$cand" ] || continue
  if command -v "$cand" >/dev/null 2>&1 && "$cand" --version >/dev/null 2>&1; then PY="$cand"; break; fi
done
[ -n "$PY" ] || { echo "fetch_ohm_planet: no working python3 on PATH" >&2; exit 1; }

PBF="${OHM_PBF:-}"
if [ -z "$PBF" ]; then
  echo "fetch_ohm_planet: resolving latest daily planet file from S3 ..." >&2
  KEY="$("$PY" - "$S3" <<'PY'
import sys, re, urllib.request, datetime
s3 = sys.argv[1]
def latest_for(yy):
    url = f"{s3}?list-type=2&prefix=planet/planet-{yy}&max-keys=1000"
    req = urllib.request.Request(url, headers={"User-Agent": "Mozilla/5.0"})
    x = urllib.request.urlopen(req, timeout=60).read().decode("utf-8", "replace")
    keys = [k for k in re.findall(r"<Key>([^<]+)</Key>", x) if k.endswith(".osm.pbf")]
    return max(keys) if keys else None
yy = datetime.date.today().strftime("%y")
key = latest_for(yy) or latest_for(f"{int(yy)-1:02d}")
print(key or "", end="")
PY
)"
  [ -n "$KEY" ] || { echo "fetch_ohm_planet: could not resolve a planet key" >&2; exit 1; }
  PBF="$OUTDIR/$(basename "$KEY")"
  echo "fetch_ohm_planet: latest = $KEY" >&2
  echo "fetch_ohm_planet: downloading -> $PBF (resumable) ..." >&2
  curl -fL -A "$UA" -C - "${S3}${KEY}" -o "$PBF" \
    -w 'fetch_ohm_planet: HTTP %{http_code} | %{size_download} bytes | %{time_total}s\n' >&2
fi

OUT="$OUTDIR/ohm-full.nt"
echo "fetch_ohm_planet: converting $PBF -> $OUT ..." >&2
PYTHONIOENCODING=utf-8 "$PY" "$HERE/ohm_pbf_to_nt.py" "$PBF" > "$OUT"
echo "fetch_ohm_planet: wrote $OUT ($(grep -c OhmFeature "$OUT") features)" >&2

[ "${OHM_KEEP_PBF:-1}" = "1" ] || rm -f "$PBF"

echo "fetch_ohm_planet: next — build to .rete in Docker, then hf buckets cp (see data/README.md)" >&2
