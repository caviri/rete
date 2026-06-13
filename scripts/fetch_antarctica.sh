#!/usr/bin/env bash
# Antarctic atlas overlays. Five layers in the atlas GeoSPARQL shape under data/antarctica/:
#   claims   — 7 territorial-claim sectors + Peter I Island + Marie Byrd gap (synthetic, INTERVAL polygons)
#   stations — Wikidata research stations, dated founded->present (CC0, INTERVAL)
#   deaths   — Wikidata people who died in Antarctica, at the death place (CC0, INSTANT)
#   sites    — Wikidata Heroic-Age huts/refuges/depots + memorials/crosses/tombs (CC0, INTERVAL)
#   places   — SCAR Composite Gazetteer of Antarctica, ~19k named features (CC-BY, INTERVAL basemap)
# Usage:  scripts/fetch_antarctica.sh [claims|stations|deaths|sites|places ...]
set -e
UA="rete-atlas/0.1 (https://github.com/caviri/rete; carlosvivarrios@gmail.com)"
WQS="https://query.wikidata.org/sparql"
OUT="data/antarctica"; mkdir -p "$OUT"
PY="${OHM_PYTHON:-python}"
SEL=("$@"); [ ${#SEL[@]} -eq 0 ] && SEL=(claims stations deaths sites places)
want() { for s in "${SEL[@]}"; do [ "$s" = "$1" ] && return 0; done; return 1; }
wq() { curl -s -G "$WQS" --data-urlencode "query@$1" -H "Accept: application/n-triples" -H "User-Agent: $UA" --max-time 200; }
PFX='PREFIX wd: <http://www.wikidata.org/entity/> PREFIX wdt: <http://www.wikidata.org/prop/direct/> PREFIX p: <http://www.wikidata.org/prop/> PREFIX psv: <http://www.wikidata.org/prop/statement/value/> PREFIX wikibase: <http://wikiba.se/ontology#> PREFIX geo: <http://www.opengis.net/ont/geosparql#> PREFIX geof: <http://www.opengis.net/def/function/geosparql/> PREFIX ex: <http://ex/> PREFIX rdfs: <http://www.w3.org/2000/01/rdf-schema#>'

if want claims; then
  echo "== claims (synthetic sector polygons) =="
  PYTHONIOENCODING=utf-8 "$PY" scripts/antarctic_claims.py > "$OUT/claims.nt"
  echo "  -> $(grep -c 'ex/Claim>' "$OUT/claims.nt") claims"
fi

if want stations; then
  echo "== stations (Wikidata, dated) =="
  printf '%s\nCONSTRUCT { ?item a ex:Station ; rdfs:label ?label ; ex:startYear ?sy ; ex:endYear ?ey ; geo:hasGeometry ?g . ?g geo:asWKT ?wkt . }\nWHERE { VALUES ?class { wd:Q195339 wd:Q749622 wd:Q59217270 wd:Q29826390 } ?item wdt:P31 ?class . ?item p:P625/psv:P625 ?cv . ?cv wikibase:geoLatitude ?lat . FILTER(?lat < -60) ?item wdt:P625 ?wkt . ?item wdt:P571 ?inc . BIND(YEAR(?inc) AS ?sy) OPTIONAL { ?item wdt:P576 ?dis } BIND(IF(BOUND(?dis), YEAR(?dis), 2100) AS ?ey) ?item rdfs:label ?label . FILTER(LANG(?label)="en") BIND(IRI(CONCAT(STR(?item),"/geom")) AS ?g) }' "$PFX" > "$OUT/stations.rq"
  wq "$OUT/stations.rq" > "$OUT/stations.nt"
  echo "  -> $(grep -c 'ex/Station>' "$OUT/stations.nt") stations"
fi

if want deaths; then
  echo "== deaths (Wikidata, died in Antarctica) =="
  # Pivot on death-place continent = Antarctica (Q51); emit the place coordinate directly (no geof scan).
  printf '%s\nCONSTRUCT { ?p a ex:Death ; rdfs:label ?name ; ex:year ?year ; geo:hasGeometry ?g . ?g geo:asWKT ?wkt . }\nWHERE { ?place wdt:P30 wd:Q51 ; wdt:P625 ?wkt . ?p wdt:P20 ?place ; wdt:P570 ?d ; rdfs:label ?name . FILTER(LANG(?name)="en") BIND(YEAR(?d) AS ?year) BIND(IRI(CONCAT(STR(?p),"/geom")) AS ?g) }' "$PFX" > "$OUT/deaths.rq"
  wq "$OUT/deaths.rq" > "$OUT/deaths.nt"
  echo "  -> $(grep -c 'ex/Death>' "$OUT/deaths.nt") deaths"
fi

if want sites; then
  echo "== sites (Wikidata Historic Sites & Monuments of Antarctica) =="
  # P1435 = wd:Q21013851 "Historic Site or Monument (Antarctica)" — the official HSM list:
  # Heroic-Age huts, depots, crosses, graves, memorials, monuments. Antarctic by definition (no geo filter).
  printf '%s\nCONSTRUCT { ?h a ex:Site ; rdfs:label ?lbl ; ex:startYear ?sy ; ex:endYear ?ey ; geo:hasGeometry ?g . ?g geo:asWKT ?wkt . }\nWHERE { ?h wdt:P1435 wd:Q21013851 ; wdt:P625 ?wkt ; rdfs:label ?lbl . FILTER(LANG(?lbl)="en") OPTIONAL { ?h wdt:P571 ?inc } BIND(IF(BOUND(?inc), YEAR(?inc), 1820) AS ?sy) OPTIONAL { ?h wdt:P582 ?en } BIND(IF(BOUND(?en), YEAR(?en), 2100) AS ?ey) BIND(IRI(CONCAT(STR(?h),"/geom")) AS ?g) }' "$PFX" > "$OUT/sites.rq"
  wq "$OUT/sites.rq" > "$OUT/sites.nt"
  echo "  -> $(grep -c 'ex/Site>' "$OUT/sites.nt") sites"
fi

if want places; then
  echo "== places (SCAR Composite Gazetteer, ~39k records, CSV) =="
  curl -s --max-time 420 -A "$UA" 'https://data.aad.gov.au/geoserver/ows?service=WFS&version=2.0.0&request=GetFeature&typeNames=aadc:SCAR_CGA_PLACE_NAMES&outputFormat=csv&propertyName=place_name_gazetteer,latitude,longitude,feature_type_name,scar_common_id' -o "$OUT/scar_cga.csv"
  PYTHONIOENCODING=utf-8 "$PY" scripts/scar_to_nt.py "$OUT/scar_cga.csv" > "$OUT/places.nt"
  echo "  -> $(grep -c 'ex/AntarcticPlace>' "$OUT/places.nt") places"
fi
echo "--- done ---"
