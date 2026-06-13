#!/usr/bin/env bash
# Fetch temporal+spatial themes from DBpedia (CC0/CC-BY-SA via Wikipedia) — a *second
# provider* alongside Wikidata — as N-Triples in the atlas GeoSPARQL shape. DBpedia stores
# coordinates as wgs84 geo:lat / geo:long; we BIND them into a WKT point.
#   conflicts : INSTANT  (dbo:MilitaryConflict, single dbo:date)            -> ex:year
#   power     : INTERVAL (dbo:PowerStation, opening->closing else present)  -> ex:startYear/ex:endYear
#
# Usage:  scripts/fetch_dbpedia_themes.sh [out-dir]   (default: data/dbpedia-themes)
#         THEMES="dbpedia-power" scripts/fetch_dbpedia_themes.sh
# Then:   for f in data/dbpedia-themes/*.nt; do rete build "$f" -o "${f%.nt}.rete"; done
# Note: dbpedia.org/sparql is flaky (intermittent 503 / "License has expired"); retry.
set -e
EP="https://dbpedia.org/sparql"
UA="rete-atlas/0.1 (https://github.com/caviri/rete; carlosvivarrios@gmail.com)"
OUT="${1:-data/dbpedia-themes}"
LIM="${LIMIT:-6000}"
mkdir -p "$OUT"
PFX='PREFIX dbo: <http://dbpedia.org/ontology/> PREFIX wgs: <http://www.w3.org/2003/01/geo/wgs84_pos#> PREFIX gs: <http://www.opengis.net/ont/geosparql#> PREFIX rdfs: <http://www.w3.org/2000/01/rdf-schema#>'
want() { [ -z "${THEMES:-}" ] && return 0; case " $THEMES " in *" $1 "*) return 0 ;; *) return 1 ;; esac; }
db() {  # retry the flaky endpoint a few times
  local q="$1" i out
  for i in 1 2 3 4 5; do
    out=$(curl -sG "$EP" --data-urlencode "query=$q" --data-urlencode "format=text/plain" -H "User-Agent: $UA")
    [ -n "$out" ] && ! grep -qi "License has expired\|Service Temporarily" <<<"$out" && { printf '%s' "$out"; return 0; }
    sleep 3
  done
  printf '%s' "$out"
}

# conflicts — INSTANT (point in time)
if want dbpedia-conflicts; then
  db "$PFX
CONSTRUCT { ?x a <http://ex/DbpConflict> ; rdfs:label ?l ; <http://ex/year> ?yr ; gs:hasGeometry ?g . ?g gs:asWKT ?wkt . }
WHERE { ?x a dbo:MilitaryConflict ; dbo:date ?date ; wgs:lat ?lat ; wgs:long ?long ; rdfs:label ?l .
  FILTER(lang(?l)=\"en\") BIND(year(?date) AS ?yr) FILTER(bound(?yr))
  BIND(IRI(CONCAT(STR(?x),\"/geom\")) AS ?g)
  BIND(STRDT(CONCAT(\"Point(\",STR(?long),\" \",STR(?lat),\")\"), gs:wktLiteral) AS ?wkt) } LIMIT $LIM" > "$OUT/dbpedia-conflicts.nt"
  printf '%-22s %7s triples\n' "dbpedia-conflicts" "$(wc -l < "$OUT/dbpedia-conflicts.nt")"
fi

# power — INTERVAL (opening -> closing, else 2100 sentinel "still operating")
if want dbpedia-power; then
  db "$PFX
CONSTRUCT { ?x a <http://ex/DbpPowerPlant> ; rdfs:label ?l ; <http://ex/startYear> ?sy ; <http://ex/endYear> ?ey ; gs:hasGeometry ?g . ?g gs:asWKT ?wkt . }
WHERE { ?x a dbo:PowerStation ; rdfs:label ?l ; dbo:openingDate ?od ; wgs:lat ?lat ; wgs:long ?long .
  FILTER(lang(?l)=\"en\") OPTIONAL { ?x dbo:closingDate ?cd }
  BIND(year(?od) AS ?sy) FILTER(bound(?sy)) BIND(IF(BOUND(?cd), year(?cd), 2100) AS ?ey)
  BIND(IRI(CONCAT(STR(?x),\"/geom\")) AS ?g)
  BIND(STRDT(CONCAT(\"Point(\",STR(?long),\" \",STR(?lat),\")\"), gs:wktLiteral) AS ?wkt) } LIMIT $LIM" > "$OUT/dbpedia-power.nt"
  printf '%-22s %7s triples\n' "dbpedia-power" "$(wc -l < "$OUT/dbpedia-power.nt")"
fi

echo "---"
echo "dbpedia themes: $(ls "$OUT"/*.nt 2>/dev/null | wc -l) files in $OUT"
