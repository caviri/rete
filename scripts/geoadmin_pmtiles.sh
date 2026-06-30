#!/usr/bin/env bash
# Build geoadmin.pmtiles — a PMTiles vector basemap PAIRED with the geoadmin .rete
# (geo-LOD "option B"). tippecanoe does the real per-zoom LOD/tiling that GeoSPARQL
# can't; the playground's "Tiles" output renders it with protomaps-leaflet (Canvas on
# Leaflet, no WebGL) and highlights the SPARQL result features on top. The tiles draw
# the geometry; rete answers the query next to them.
#
# Inputs (full-detail GeoJSON; tippecanoe simplifies per zoom, so feed it raw):
#   data/geoadmin/gb_adm0.geojson  (ADM0 countries)   -> layer "countries"
#   data/geoadmin/gb_adm1.geojson  (ADM1 regions)     -> layer "regions"
#   data/geoadmin/gb_adm2.geojson  (ADM2 districts)   -> layer "districts"
#   data/geoadmin/places50m.geojson (Natural Earth)   -> layer "places"
# Output: web/geoadmin.pmtiles (~113 MB, z0-9, 54k features) — bucket-only (gitignored).
# Then: hf buckets cp web/geoadmin.pmtiles hf://buckets/katospiegel/knowledge-graphs/playground/geoadmin.pmtiles
#
# Run in the rete-tippecanoe image (tippecanoe v2.80, emits PMTiles v3 directly):
#   MSYS_NO_PATHCONV=1 docker run --rm -v "$PWD:/work" -w /work rete-tippecanoe:latest bash scripts/geoadmin_pmtiles.sh
set -euo pipefail
D=/work/data/geoadmin
tippecanoe -o /work/web/geoadmin.pmtiles -f \
  -Z0 -z9 \
  -y shapeName -y shapeGroup -y shapeID -y NAME -y name -y POP_MAX \
  --drop-densest-as-needed --extend-zooms-if-still-dropping --simplification=4 --no-tile-size-limit \
  -L countries:"$D/gb_adm0.geojson" \
  -L regions:"$D/gb_adm1.geojson" \
  -L districts:"$D/gb_adm2.geojson" \
  -L places:"$D/places50m.geojson"
ls -lh /work/web/geoadmin.pmtiles
