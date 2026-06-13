#!/usr/bin/env bash
# Fetch OpenHistoricalMap (OHM) named, dated features from the OHM Overpass API
# and convert them to N-Triples in the atlas GeoSPARQL shape:
#   <x> a <http://ex/OhmFeature> ; rdfs:label ?name@en ;
#       ex:startYear ?sy^^xsd:integer ; ex:endYear ?ey^^xsd:integer ;
#       geo:hasGeometry <x/geom> .  <x/geom> geo:asWKT "WKT"^^geo:wktLiteral .
#   x = https://www.openhistoricalmap.org/<node|way|relation>/<id>
#
# OHM data is CC0 1.0 (public domain) — NOT ODbL. Recommended (not required)
# credit: "Data: OpenHistoricalMap contributors (CC0)". Per-element license=*/
# attribution=* tags (third-party imports) are NOT carried through here; this
# query does not select them and OHM's bulk is overwhelmingly CC0.
#
# We issue THREE separate `out geom N` statements (node / way / relation) so the
# result is a true type MIX: a single bare `out geom` exhausts its cap on nodes
# (Overpass emits node->way->relation) and returns no way/relation geometry.
# `out` statements CANNOT be nested in a (...) union group (HTTP 400), hence the
# three statements. Boundary relations are capped low (each ~13.5 KB of polygon
# geometry); raising the relation cap blows up payload size fast.
#
# Usage:  scripts/fetch_ohm.sh [out-nt]        (default: data/ohm/ohm.nt)
# Then build to .rete:  rete build data/ohm/ohm.nt -o data/ohm/ohm.rete
set -euo pipefail

EP="https://overpass-api.openhistoricalmap.org/api/interpreter"
UA="rete-atlas/0.1 (https://github.com/caviri/rete; carlosvivarrios@gmail.com)"
OUT="${1:-data/ohm/ohm.nt}"
RAW="${OHM_RAW:-${OUT%.nt}_union.json}"

# Per-type caps (override via env). ~5300 elements / ~16 MB at the defaults.
NODE_CAP="${NODE_CAP:-2500}"
WAY_CAP="${WAY_CAP:-2500}"
REL_CAP="${REL_CAP:-300}"

# Find a python that actually runs: prefer python3, fall back to python. On
# Windows, %LOCALAPPDATA%\Microsoft\WindowsApps\python3.exe is a Store shim that
# exits non-zero, so probe each candidate with `--version` and keep the first
# that works.
PY=""
for cand in "${OHM_PYTHON:-}" python3 python py; do
  [ -n "$cand" ] || continue
  if command -v "$cand" >/dev/null 2>&1 && "$cand" --version >/dev/null 2>&1; then
    PY="$cand"; break
  fi
done
[ -n "$PY" ] || { echo "fetch_ohm: no working python3/python on PATH" >&2; exit 1; }
HERE="$(cd "$(dirname "$0")" && pwd)"

mkdir -p "$(dirname "$OUT")"

QL="[out:json][timeout:120];
node[\"start_date\"][\"name\"];
out geom ${NODE_CAP};
way[\"start_date\"][\"name\"];
out geom ${WAY_CAP};
relation[\"start_date\"][\"name\"][\"boundary\"=\"administrative\"];
out geom ${REL_CAP};"

echo "fetch_ohm: querying OHM Overpass (node ${NODE_CAP} / way ${WAY_CAP} / relation ${REL_CAP}) ..." >&2
curl -s -m 130 -G "$EP" \
  -H "User-Agent: $UA" \
  --data-urlencode "data=${QL}" \
  -o "$RAW" -w 'fetch_ohm: HTTP %{http_code} | %{size_download} bytes | %{time_total}s\n' >&2

# Overpass returns UTF-8; force UTF-8 stdin/stdout (Windows default cp1252 fails
# on byte 0x8d). Converter writes a "kept N/total" summary to stderr.
PYTHONIOENCODING=utf-8 "$PY" "$HERE/ohm_overpass_to_nt.py" < "$RAW" > "$OUT"

echo "fetch_ohm: wrote $OUT ($(grep -c OhmFeature "$OUT") features)" >&2
echo "fetch_ohm: License CC0 1.0 — credit: Data: OpenHistoricalMap contributors (CC0)" >&2
