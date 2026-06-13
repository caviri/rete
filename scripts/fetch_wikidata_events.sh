#!/usr/bin/env bash
# Fetch vivid temporal+spatial subsets from Wikidata (CC0) as N-Triples already in
# the atlas GeoSPARQL shape: ex:year (xsd:integer, negative = BCE), rdfs:label@en,
# geo:hasGeometry/geo:asWKT (wktLiteral "Point(lon lat)"), and an ex:<type> rdf:type.
# Concatenates battles + historical states + archaeological sites into one .nt.
#
# Usage: scripts/fetch_wikidata_events.sh [out-dir]   (default: data/wd-events)
# Then:  rete build <out>/history-events.nt -o <out>/history-events.rete
set -e
EP="https://query.wikidata.org/sparql"
UA="rete-atlas/0.1 (https://github.com/caviri/rete; carlosvivarrios@gmail.com)"
OUT="${1:-data/wd-events}"
mkdir -p "$OUT"
PFX='PREFIX wd: <http://www.wikidata.org/entity/> PREFIX wdt: <http://www.wikidata.org/prop/direct/> PREFIX rdfs: <http://www.w3.org/2000/01/rdf-schema#> PREFIX geo: <http://www.opengis.net/ont/geosparql#>'
wd() { curl -sG "$EP" --data-urlencode "query=$1" -H "Accept: application/n-triples" -H "User-Agent: $UA"; }

# Battles (Q178561): point in time P585, coordinates P625.
wd "$PFX
CONSTRUCT { ?b a <http://ex/Battle> ; rdfs:label ?label ; <http://ex/year> ?yr ; geo:hasGeometry ?g . ?g geo:asWKT ?coord . }
WHERE { ?b wdt:P31 wd:Q178561 ; wdt:P585 ?date ; wdt:P625 ?coord ; rdfs:label ?label .
  FILTER(LANG(?label)=\"en\") BIND(IRI(CONCAT(STR(?b),\"/geom\")) AS ?g) BIND(YEAR(?date) AS ?yr) } LIMIT 5000" > "$OUT/battles.nt"
echo "battles: $(wc -l < "$OUT/battles.nt") triples"

# Historical/former states (Q3024240): inception P571, dissolved P576, capital P36 → its coords P625.
wd "$PFX
CONSTRUCT { ?s a <http://ex/State> ; rdfs:label ?label ; <http://ex/year> ?yr ; <http://ex/startYear> ?yr ; <http://ex/endYear> ?ey ; geo:hasGeometry ?g . ?g geo:asWKT ?coord . }
WHERE { ?s wdt:P31 wd:Q3024240 ; wdt:P571 ?inc ; wdt:P36 ?cap ; rdfs:label ?label . ?cap wdt:P625 ?coord .
  OPTIONAL { ?s wdt:P576 ?dis BIND(YEAR(?dis) AS ?ey) }
  FILTER(LANG(?label)=\"en\") BIND(IRI(CONCAT(STR(?s),\"/geom\")) AS ?g) BIND(YEAR(?inc) AS ?yr) } LIMIT 5000" > "$OUT/states.nt"
echo "states: $(wc -l < "$OUT/states.nt") triples"

# Archaeological sites (Q839954): inception P571, coordinates P625.
wd "$PFX
CONSTRUCT { ?a a <http://ex/Site> ; rdfs:label ?label ; <http://ex/year> ?yr ; geo:hasGeometry ?g . ?g geo:asWKT ?coord . }
WHERE { ?a wdt:P31 wd:Q839954 ; wdt:P571 ?inc ; wdt:P625 ?coord ; rdfs:label ?label .
  FILTER(LANG(?label)=\"en\") BIND(IRI(CONCAT(STR(?a),\"/geom\")) AS ?g) BIND(YEAR(?inc) AS ?yr) } LIMIT 5000" > "$OUT/sites.nt"
echo "sites: $(wc -l < "$OUT/sites.nt") triples"

cat "$OUT/battles.nt" "$OUT/states.nt" "$OUT/sites.nt" > "$OUT/history-events.nt"
echo "combined: $(wc -l < "$OUT/history-events.nt") triples -> $OUT/history-events.nt"
