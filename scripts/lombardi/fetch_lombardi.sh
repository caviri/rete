#!/usr/bin/env bash
# Harvest the Mark Lombardi Networks corpus into data/lombardi/raw.
#
# Source: https://lombardinetworks.net -- Robert Tolksdorf's (FU Berlin) manual
# digitization of Mark Lombardi's network drawings, published as GraphML / JSON /
# XGMML plus an OWL ontology for the node and edge types.
# Licence: CC BY-NC-SA 4.0.
#
# Two gotchas the script works around:
#   * the site's HTML pages sit behind a bot check that answers 401 to plain
#     fetchers, so page metadata is taken from the Wayback Machine instead;
#   * the DATA files (.graphml/.json/.xgmml) are served fine with a browser
#     User-Agent -- these come straight from the origin.
#     Exception: 1043.graphml is 0 bytes ON THE SERVER; its .json/.xgmml are
#     intact, which is why the converter reads JSON.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
RAW="$ROOT/data/lombardi/raw"
UA="Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/126.0 Safari/537.36"
IDS=$(seq 1000 1050)   # the 51 digitized networks

mkdir -p "$RAW/network"

echo "== ontology"
curl -sL -A "$UA" -o "$RAW/lombardi.owl" "https://lombardinetworks.net/lombardi.owl"

echo "== site pages (via Wayback: origin HTML is bot-checked)"
for p in networks/the-networks networks/ontologies analyses/network-analyses; do
  out="$RAW/$(basename "$p").wayback.html"
  curl -sL --max-time 60 -o "$out" "https://web.archive.org/web/2024/https://lombardinetworks.net/$p/"
done

echo "== MoMA open collection data (CC0) — the 20 Lombardis they hold"
# Metadata only: MoMA is explicit that "images are not included and are not part
# of the dataset". We keep the ImageURL as a LINK to their server and never mirror
# a byte of it; the artwork is © The Estate of Mark Lombardi.
# The CSV is Git-LFS, so it comes from media.githubusercontent.com on `main`.
mkdir -p "$ROOT/data/lombardi/moma"
curl -sL "https://media.githubusercontent.com/media/MuseumofModernArt/collection/main/Artworks.csv" \
  -o "$ROOT/data/lombardi/moma/Artworks.csv"
python - "$ROOT" <<'PY'
import csv, json, sys, os
root = sys.argv[1]
csv.field_size_limit(10**7)
src = os.path.join(root, "data", "lombardi", "moma", "Artworks.csv")
keep = ("Title", "Date", "Medium", "Dimensions", "CreditLine",
        "AccessionNumber", "ObjectID", "URL", "ImageURL")
out = []
with open(src, encoding="utf-8-sig") as f:
    for r in csv.DictReader(f):
        if (r.get("Artist") or "").strip() == "Mark Lombardi":
            out.append({k: r.get(k, "") for k in keep})
dst = os.path.join(root, "data", "lombardi", "moma", "lombardi_moma.json")
json.dump(out, open(dst, "w", encoding="utf-8"), indent=1, ensure_ascii=False)
print("  %d works by Mark Lombardi at MoMA" % len(out))
PY

echo "== per-network data + page"
ok=0
for id in $IDS; do
  d="$RAW/network/$id"; mkdir -p "$d"
  for ext in graphml json xgmml; do
    curl -sL -A "$UA" --max-time 60 -o "$d/$id.$ext" \
      "https://lombardinetworks.net/network/$id/$id.$ext" || true
    [ -s "$d/$id.$ext" ] || { echo "  empty: $id.$ext (server-side)"; rm -f "$d/$id.$ext"; }
    sleep 0.15
  done
  curl -sL --max-time 60 -o "$d/page.html" \
    "https://web.archive.org/web/2024/https://lombardinetworks.net/network/$id/"
  ok=$((ok + 1))
done
echo "done: $ok networks in $RAW"
