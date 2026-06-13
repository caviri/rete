#!/usr/bin/env bash
# Fetch temporal+spatial themes from DBpedia (CC-BY-SA) — a *second provider*
# alongside Wikidata — as N-Triples in the atlas GeoSPARQL shape:
#   <x> a <http://ex/TYPE> ; rdfs:label ?label@en ; <http://ex/year> ?yr ;
#       geo:hasGeometry <x/geom> .  <x/geom> geo:asWKT "Point(lon lat)"^^wktLiteral .
# DBpedia stores coordinates as wgs84 geo:lat / geo:long; we BIND them into a WKT point
# and pull a year out of dbo:date (events) or dbo:foundingDate / dbo:openingDate.
#
# Usage:  scripts/fetch_dbpedia_themes.sh [out-dir]        (default: data/dbpedia-themes)
# Then build each to .rete:
#         for f in data/dbpedia-themes/*.nt; do rete build "$f" -o "${f%.nt}.rete"; done
set -e
EP="https://dbpedia.org/sparql"
UA="rete-atlas/0.1 (https://github.com/caviri/rete; carlosvivarrios@gmail.com)"
OUT="${1:-data/dbpedia-themes}"
LIM="${LIMIT:-6000}"
mkdir -p "$OUT"
PFX='PREFIX dbo: <http://dbpedia.org/ontology/> PREFIX geo: <http://www.w3.org/2003/01/geo/wgs84_pos#> PREFIX gs: <http://www.opengis.net/ont/geosparql#> PREFIX rdfs: <http://www.w3.org/2000/01/rdf-schema#>'
db() { curl -sG "$EP" --data-urlencode "query=$1" --data-urlencode "format=text/plain" -H "User-Agent: $UA"; }

# emit <key> <Type> <class> <date-pred>
emit() {
  local key="$1" t="$2" cls="$3" dp="$4"
  if [ -n "${THEMES:-}" ]; then case " $THEMES " in *" $key "*) ;; *) return 0 ;; esac; fi
  db "$PFX
CONSTRUCT { ?x a <http://ex/$t> ; rdfs:label ?label ; <http://ex/year> ?yr ; gs:hasGeometry ?g . ?g gs:asWKT ?wkt . }
WHERE { ?x a $cls ; $dp ?date ; geo:lat ?lat ; geo:long ?long ; rdfs:label ?label .
  FILTER(lang(?label)=\"en\") BIND(year(?date) AS ?yr) FILTER(bound(?yr))
  BIND(IRI(CONCAT(STR(?x),\"/geom\")) AS ?g)
  BIND(STRDT(CONCAT(\"Point(\",STR(?long),\" \",STR(?lat),\")\"), gs:wktLiteral) AS ?wkt) } LIMIT $LIM" > "$OUT/$key.nt"
  printf '%-22s %7s triples\n' "$key" "$(wc -l < "$OUT/$key.nt")"
}

emit dbpedia-conflicts  DbpConflict   dbo:MilitaryConflict  dbo:date
emit dbpedia-power      DbpPowerPlant dbo:PowerStation      dbo:openingDate
# (dbo:Volcano + dbo:eruptionDate dropped: eruption dates are almost never populated.)

echo "---"
echo "dbpedia themes: $(ls "$OUT"/*.nt 2>/dev/null | wc -l) files in $OUT"
