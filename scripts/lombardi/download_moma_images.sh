#!/usr/bin/env bash
# Download every MoMA-held Lombardi drawing at the highest signed resolution
# (2000px) into data/lombardi/raw/moma-images/ — a LOCAL reference cache for the
# tracing work. The images are (c) The Estate of Mark Lombardi, hosted by MoMA;
# this is a private working copy, not for redistribution. data/ is gitignored.
#
# Source URLs come from data/lombardi/moma/lombardi_moma.json (FullImageURL),
# produced by moma_hires.py.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
SRC="$ROOT/data/lombardi/moma/lombardi_moma.json"
OUT="$ROOT/data/lombardi/raw/moma-images"
UA="Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/126.0 Safari/537.36"

mkdir -p "$OUT"

# emit "<filename>\t<url>\t<px>" lines: accession + title slug, hi-res URL, longest edge
python - "$SRC" > "$OUT/.manifest.tsv" <<'PY'
import json, re, sys
for w in json.load(open(sys.argv[1], encoding="utf-8")):
    url = w.get("FullImageURL") or w.get("ImageURL")
    if not url:
        continue
    acc = re.sub(r"[^0-9A-Za-z.-]", "", w["AccessionNumber"])
    slug = re.sub(r"[^a-z0-9]+", "-", w["Title"].lower()).strip("-")[:48]
    px = w.get("FullImageSize", 1024)
    print("%s_%s.jpg\t%s\t%s" % (acc, slug, url, px))
PY

ok=0; total=0
while IFS=$'\t' read -r name url px; do
  total=$((total + 1))
  dest="$OUT/$name"
  code=$(curl -sL -A "$UA" --max-time 90 -o "$dest" -w "%{http_code}" "$url")
  sz=$(wc -c < "$dest" 2>/dev/null || echo 0)
  if [ "$code" = "200" ] && [ "$sz" -gt 5000 ]; then
    ok=$((ok + 1)); printf "  %-52s %4spx  %6sKB\n" "$name" "$px" "$((sz / 1024))"
  else
    echo "  ! FAILED $name ($code, ${sz}B)"; rm -f "$dest"
  fi
  sleep 0.3
done < "$OUT/.manifest.tsv"

echo "downloaded $ok of $total images -> $OUT"
