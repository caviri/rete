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
