#!/usr/bin/env bash
# Bring the downloadable bioexplora Sketchfab models (CC BY, account laboratorinatura
# = Museu de Ciencies Naturals de Barcelona, the "Atles osteologic" skull/bone scans)
# to the bucket so they stream inline: download each .glb via the Sketchfab Data API,
# Draco+webp compress it (~40x), upload to the bucket, and write a uid->mesh-url TSV.
# Needs: SKETCHFAB_TOKEN (in .env), docker (rete-gltf image), the hf CLI.
#   sh scripts/bioexplora_sketchfab.sh [LIMIT]
set -uo pipefail
cd "$(dirname "$0")/.."
set -a; . ./.env 2>/dev/null; set +a
: "${SKETCHFAB_TOKEN:?need SKETCHFAB_TOKEN in .env}"
RAW=data/bioexplora/sk_raw; GLB=data/bioexplora/sk_glb
mkdir -p "$RAW" "$GLB"
HF="/c/Users/Kato/AppData/Local/Programs/Python/Python312/Scripts/hf"
DSTDIR="hf://buckets/katospiegel/knowledge-graphs/playground/bioexplora-3d"
BASE="https://katospiegel-rete.hf.space/data/playground/bioexplora-3d"
TOK="token=sfdbgf1094by21hd128ru39802"
LIMIT="${1:-0}"

python - <<'PY' > /tmp/uids.txt
import json
for x in json.load(open("data/bioexplora/models3d.json", encoding="utf-8")):
    if x.get("isDownloadable"):
        print(x["uid"])
PY
echo "downloadable models: $(wc -l < /tmp/uids.txt)"

# Ask the Data API for a model's signed .glb URL. The /download endpoint is rate
# limited, so retry with exponential backoff when it returns nothing (throttled).
glb_url() {
  local uid="$1" tries=0 wait=2 url=""
  while [ "$tries" -lt 5 ]; do
    url=$(curl -s -H "Authorization: Token $SKETCHFAB_TOKEN" \
          "https://api.sketchfab.com/v3/models/$uid/download" </dev/null \
          | python -c "import sys,json;print(json.load(sys.stdin).get('glb',{}).get('url',''))" 2>/dev/null | tr -d '\r')
    [ -n "$url" ] && { printf '%s' "$url"; return 0; }
    tries=$((tries + 1)); sleep "$wait"; wait=$((wait * 2))   # 2,4,8,16 s backoff
  done
  return 1
}

# --- 1. download each .glb (signed URL from the Data API, expires in 300 s) -------
i=0
while read -r uid; do
  uid=${uid%$'\r'}                      # Windows python writes the uid list as CRLF
  i=$((i + 1)); [ "$LIMIT" -gt 0 ] && [ "$i" -gt "$LIMIT" ] && break
  [ -s "$GLB/$uid.glb" ] && continue
  [ -s "$RAW/$uid.glb" ] && continue
  url=$(glb_url "$uid") || { echo "no-url $uid"; continue; }
  # download to .part then rename, so an interrupted curl never leaves a
  # truncated .glb that the [ -s ] guard would wrongly treat as complete
  if curl -sf -o "$RAW/$uid.glb.part" "$url" </dev/null; then
    mv -f "$RAW/$uid.glb.part" "$RAW/$uid.glb"
  else
    rm -f "$RAW/$uid.glb.part"; echo "dl-fail $uid"
  fi
  sleep 1                               # stay under the /download rate limit
  [ $((i % 25)) -eq 0 ] && echo "downloaded $i ($(ls "$RAW"/*.glb 2>/dev/null | wc -l) ok)"
done < /tmp/uids.txt
echo "raw glbs: $(ls "$RAW"/*.glb 2>/dev/null | wc -l)"

# --- 2. Draco + webp compress (one container, loop) ------------------------------
MSYS_NO_PATHCONV=1 docker run --rm -v "$PWD":/work -w /work rete-gltf sh -c '
for f in data/bioexplora/sk_raw/*.glb; do
  u=$(basename "$f" .glb)
  [ -s "data/bioexplora/sk_glb/$u.glb" ] && continue
  if gltf-transform optimize "$f" "data/bioexplora/sk_glb/$u.glb.part" --compress draco --texture-compress webp >/dev/null 2>&1; then
    mv -f "data/bioexplora/sk_glb/$u.glb.part" "data/bioexplora/sk_glb/$u.glb"
  else
    rm -f "data/bioexplora/sk_glb/$u.glb.part"; echo "compress-fail $u"
  fi
done
echo "compressed: $(ls data/bioexplora/sk_glb/*.glb 2>/dev/null | wc -l)"'

# --- 3. upload to the bucket + 4. write the uid -> mesh-url map -------------------
: > data/bioexplora/meshes.tsv
for f in "$GLB"/*.glb; do
  [ -s "$f" ] || continue
  u=$(basename "$f" .glb)
  if "$HF" buckets cp "$f" "$DSTDIR/$u.glb" >/dev/null 2>&1; then
    printf '%s\t%s/%s.glb?%s\n' "$u" "$BASE" "$u" "$TOK" >> data/bioexplora/meshes.tsv
  fi
done
echo "uploaded + mapped: $(wc -l < data/bioexplora/meshes.tsv) meshes"
