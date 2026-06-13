#!/usr/bin/env bash
# Fetch 25 vivid temporal+spatial themes from Wikidata (CC0) as N-Triples already
# in the atlas GeoSPARQL shape, one file per theme:
#   <item> a <http://ex/TYPE> ; rdfs:label ?label@en ;
#          <http://ex/year> ?yr (xsd:integer, negative = BCE) ;
#          geo:hasGeometry <item/geom> .  <item/geom> geo:asWKT "Point(lon lat)"^^geo:wktLiteral .
#
# Date property per theme: P585 (point in time, events) / P571 (inception, structures
# & polities) / P580 (start time, settlements). Class selector is P31/P279* except
# where the subclass tree is noisy (disasters, epidemics, treaties use plain P31, and
# world-heritage-sites are picked by heritage-designation P1435 wd:Q9259). Treaties
# carry no P625 of their own, so the coordinate comes via signing location P276 -> P625.
#
# Usage:  scripts/fetch_wikidata_themes.sh [out-dir]        (default: data/wikidata-themes)
#         LIMIT=10000 scripts/fetch_wikidata_themes.sh      (override per-theme cap)
# Then build each to .rete:
#         for f in data/wikidata-themes/*.nt; do rete build "$f" -o "${f%.nt}.rete"; done
set -e
EP="https://query.wikidata.org/sparql"
UA="rete-atlas/0.1 (https://github.com/caviri/rete; carlosvivarrios@gmail.com)"
OUT="${1:-data/wikidata-themes}"
LIM="${LIMIT:-6000}"
mkdir -p "$OUT"
PFX='PREFIX wd: <http://www.wikidata.org/entity/> PREFIX wdt: <http://www.wikidata.org/prop/direct/> PREFIX rdfs: <http://www.w3.org/2000/01/rdf-schema#> PREFIX geo: <http://www.opengis.net/ont/geosparql#>'
wd() { curl -sG "$EP" --data-urlencode "query=$1" -H "Accept: application/n-triples" -H "User-Agent: $UA"; }

# emit <key> <Type> <WHERE-body>   (body must bind ?x ?date ?coord ?label)
emit() {
  local key="$1" t="$2" body="$3"
  wd "$PFX
CONSTRUCT { ?x a <http://ex/$t> ; rdfs:label ?label ; <http://ex/year> ?yr ; geo:hasGeometry ?g . ?g geo:asWKT ?coord . }
WHERE { $body FILTER(LANG(?label)=\"en\") BIND(IRI(CONCAT(STR(?x),\"/geom\")) AS ?g) BIND(YEAR(?date) AS ?yr) } LIMIT $LIM" > "$OUT/$key.nt"
  printf '%-22s %7s triples\n' "$key" "$(wc -l < "$OUT/$key.nt")"
}

#     key                  Type               WHERE-body (selector ; date ; coord ; label)
emit military-operations   MilitaryOp         '?x wdt:P31/wdt:P279* wd:Q645883 ; wdt:P585 ?date ; wdt:P625 ?coord ; rdfs:label ?label .'
emit sieges                Siege              '?x wdt:P31/wdt:P279* wd:Q188055 ; wdt:P585 ?date ; wdt:P625 ?coord ; rdfs:label ?label .'
emit castles               Castle             '?x wdt:P31/wdt:P279* wd:Q23413 ; wdt:P571 ?date ; wdt:P625 ?coord ; rdfs:label ?label .'
emit fortifications        Fortification      '?x wdt:P31/wdt:P279* wd:Q1785071 ; wdt:P571 ?date ; wdt:P625 ?coord ; rdfs:label ?label .'
emit lighthouses           Lighthouse         '?x wdt:P31/wdt:P279* wd:Q39715 ; wdt:P571 ?date ; wdt:P625 ?coord ; rdfs:label ?label .'
emit cathedrals            Cathedral          '?x wdt:P31/wdt:P279* wd:Q2977 ; wdt:P571 ?date ; wdt:P625 ?coord ; rdfs:label ?label .'
emit monasteries           Monastery          '?x wdt:P31/wdt:P279* wd:Q44613 ; wdt:P571 ?date ; wdt:P625 ?coord ; rdfs:label ?label .'
emit abbeys                Abbey              '?x wdt:P31/wdt:P279* wd:Q160742 ; wdt:P571 ?date ; wdt:P625 ?coord ; rdfs:label ?label .'
emit palaces               Palace             '?x wdt:P31/wdt:P279* wd:Q16560 ; wdt:P571 ?date ; wdt:P625 ?coord ; rdfs:label ?label .'
emit forts                 Fort               '?x wdt:P31/wdt:P279* wd:Q57831 ; wdt:P571 ?date ; wdt:P625 ?coord ; rdfs:label ?label .'
emit universities          University         '?x wdt:P31/wdt:P279* wd:Q3918 ; wdt:P571 ?date ; wdt:P625 ?coord ; rdfs:label ?label .'
emit world-heritage-sites  WorldHeritageSite  '?x wdt:P1435 wd:Q9259 ; wdt:P571 ?date ; wdt:P625 ?coord ; rdfs:label ?label .'
emit earthquakes           Earthquake         '?x wdt:P31/wdt:P279* wd:Q7944 ; wdt:P585 ?date ; wdt:P625 ?coord ; rdfs:label ?label .'
emit disasters             Disaster           '?x wdt:P31 wd:Q3839081 ; wdt:P585 ?date ; wdt:P625 ?coord ; rdfs:label ?label .'
emit floods                Flood              '?x wdt:P31/wdt:P279* wd:Q8068 ; wdt:P585 ?date ; wdt:P625 ?coord ; rdfs:label ?label .'
emit meteorite-falls       MeteoriteFall      '?x wdt:P31/wdt:P279* wd:Q60186 ; wdt:P585 ?date ; wdt:P625 ?coord ; rdfs:label ?label .'
emit volcanic-eruptions    VolcanicEruption   '?x wdt:P31/wdt:P279* wd:Q7692360 ; wdt:P585 ?date ; wdt:P625 ?coord ; rdfs:label ?label .'
emit nuclear-explosions    NuclearExplosion   '?x wdt:P31/wdt:P279* wd:Q210112 ; wdt:P585 ?date ; wdt:P625 ?coord ; rdfs:label ?label .'
emit assassinations        Assassination      '?x wdt:P31/wdt:P279* wd:Q3882219 ; wdt:P585 ?date ; wdt:P625 ?coord ; rdfs:label ?label .'
emit treaties              Treaty             '?x wdt:P31 wd:Q131569 ; wdt:P585 ?date ; wdt:P276 ?loc . ?loc wdt:P625 ?coord . ?x rdfs:label ?label .'
emit epidemics             Epidemic           '?x wdt:P31/wdt:P279* wd:Q44512 ; wdt:P585 ?date ; wdt:P625 ?coord ; rdfs:label ?label .'
emit pyramids              Pyramid            '?x wdt:P31/wdt:P279* wd:Q12516 ; wdt:P571 ?date ; wdt:P625 ?coord ; rdfs:label ?label .'
emit polities              Polity             '?x wdt:P31/wdt:P279* wd:Q6256 ; wdt:P571 ?date ; wdt:P625 ?coord ; rdfs:label ?label .'
emit shipwrecks            Shipwreck          '?x wdt:P31/wdt:P279* wd:Q852190 ; wdt:P585 ?date ; wdt:P625 ?coord ; rdfs:label ?label .'
emit settlements           Settlement         '?x wdt:P31/wdt:P279* wd:Q486972 ; wdt:P580 ?date ; wdt:P625 ?coord ; rdfs:label ?label .'

echo "---"
echo "themes: $(ls "$OUT"/*.nt | wc -l) files, $(cat "$OUT"/*.nt | wc -l) total triples in $OUT"
