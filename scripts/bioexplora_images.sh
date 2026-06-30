#!/usr/bin/env bash
# Mirror bioexplora specimen photos to the bucket as WebP so they load fast and
# reliably (the coeli iiif endpoint 503s; the app portraitMedia URL 303-redirects
# to the CORS-open S3 original). For each N-id in data/bioexplora/img_nids.txt:
# download the portraitMedia, resize + WebP via ImageMagick (~10x smaller), upload,
# and write a uid -> preview-url TSV that bioexplora_to_nt.py emits as prop:preview.
# Download+convert run in the rete-dev container (curl + ImageMagick); upload uses
# the host hf CLI. Resumable (skips an existing .webp).
#   sh scripts/bioexplora_images.sh
set -uo pipefail
cd "$(dirname "$0")/.."
NIDS=data/bioexplora/img_nids.txt
WEBP=data/bioexplora/img_webp
HF="/c/Users/Kato/AppData/Local/Programs/Python/Python312/Scripts/hf"
DSTDIR="hf://buckets/katospiegel/knowledge-graphs/playground/bioexplora-img"
BASE="https://katospiegel-rete.hf.space/data/playground/bioexplora-img"
TOK="token=sfdbgf1094by21hd128ru39802"
mkdir -p "$WEBP"
echo "N-ids to mirror: $(wc -l < "$NIDS")"

# 1. download + resize-to-WebP (rete-dev has curl + ImageMagick `convert`).
MSYS_NO_PATHCONV=1 docker run --rm --user root -v "$PWD":/work -w /work rete-dev:latest bash -c '
i=0
while read -r nid; do
  [ -z "$nid" ] && continue
  out="data/bioexplora/img_webp/$nid.webp"
  [ -s "$out" ] && continue
  i=$((i + 1))
  url="https://app.coeli.cat/coeli/ICUB-NAT/HeritageObject/$nid/portraitMedia"
  if curl -sL --max-time 60 -o /tmp/x.jpg "$url" </dev/null && [ -s /tmp/x.jpg ]; then
    if convert /tmp/x.jpg -resize "800x800>" -quality 80 "$out.part" >/dev/null 2>&1; then
      mv -f "$out.part" "$out"
    else rm -f "$out.part"; echo "conv-fail $nid"; fi
  else echo "dl-fail $nid"; fi
  [ $((i % 100)) -eq 0 ] && echo "processed $i ($(ls data/bioexplora/img_webp/*.webp 2>/dev/null | wc -l) webp ok)"
done < data/bioexplora/img_nids.txt
echo "webp total: $(ls data/bioexplora/img_webp/*.webp 2>/dev/null | wc -l)"
'

# 2. upload the WebP dir to the bucket (host hf CLI).
MSYS_NO_PATHCONV=1 "$HF" buckets sync "$WEBP" "$DSTDIR" --format quiet 2>&1 | tail -2

# 3. write the uid -> preview-url TSV (prop:preview).
: > data/bioexplora/images.tsv
for f in "$WEBP"/*.webp; do
  [ -s "$f" ] || continue
  u=$(basename "$f" .webp)
  printf '%s\t%s/%s.webp?%s\n' "$u" "$BASE" "$u" "$TOK" >> data/bioexplora/images.tsv
done
echo "mapped: $(wc -l < data/bioexplora/images.tsv) previews"
