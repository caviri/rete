#!/usr/bin/env bash
# Reproducible acquisition of the switzerland-fedlex dataset:
#   Layer A  metadata RDF KG (JOLux + ELI)  -> raw/quads/*.nq.gz   (SPARQL harvest)
#   Layer B  ontology (the TBox: JOLux/ELI/SKOS/event/PROV)        -> raw/ontology/
# Fedlex offers NO static RDF dump, so Layer A is harvested from the Virtuoso
# SPARQL endpoint (see scripts/fetch_sparql.py for the Virtuoso-specific strategy).
# Everything runs in Docker per repo convention; only plain curl runs on the host.
set -uo pipefail
export MSYS_NO_PATHCONV=1
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../../.." && pwd)"   # -> repo root
cd "$ROOT"
RAW="data/switzerland-fedlex/raw"

# --- Layer B: ontology bundle (JOLux + ELI + SKOS + event + PROV) --------------
ONT_ZIP="$RAW/ontology/jolux_ontology.zip"
if [ ! -s "$ONT_ZIP" ]; then
  echo "=== downloading ontology bundle ==="
  mkdir -p "$RAW/ontology"
  # The ontology zip is linked from the Fedlex open-data ontology page. If the URL
  # rotates, fetch the current 'JOLux ontology' link from the Fedlex opendata pages.
  curl -sSL --fail "https://fedlex.data.admin.ch/filestore/fedlex.data.admin.ch/ontology/jolux_ontology.zip" \
    -o "$ONT_ZIP" || echo "  (ontology auto-download failed; keep raw/ontology/jolux-ontology-owl/)"
fi
if [ -s "$ONT_ZIP" ] && [ ! -d "$RAW/ontology/jolux-ontology-owl" ]; then
  ( cd "$RAW/ontology" && unzip -o -q jolux_ontology.zip )
fi

# --- Layer A: metadata RDF KG via SPARQL harvest (Docker, stdlib-only python) --
echo "=== harvesting Fedlex RDF metadata KG (SPARQL -> N-Quads) ==="
docker run --rm -v "$ROOT:/work" -w //work python:3.12-slim \
  python data/switzerland-fedlex/scripts/fetch_sparql.py

echo "=== checksums ==="
( cd "$RAW" && find quads -name '*.nq.gz' -type f -exec sha256sum {} \; > ../SHA256SUMS.txt 2>/dev/null || true )
echo "DONE."
