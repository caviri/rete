#!/usr/bin/env bash
# Fetch vivid temporal+spatial themes from Wikidata (CC0) as N-Triples already in the
# atlas GeoSPARQL shape, one file per theme. Two temporal models:
#
#   emit  (INSTANT, point-in-time events)  ->  <http://ex/year> ?yr
#   emiti (INTERVAL, things with a lifespan) -> <http://ex/startYear> ?sy ; <http://ex/endYear> ?ey
#
# Events use P585 (point in time). Structures / institutions / polities use P571
# (inception) as the start and COALESCE(P576, P3999, P582, 2100) as the end:
#   P576 = dissolved/abolished/demolished (dominant end signal),
#   P3999 = date of official closure (institutions),
#   P582 = end time (thin fallback),
#   2100  = sentinel meaning "still present" (no recorded end).
# (Order matters: P576 > P3999 > P582 picks the most authoritative year on multi-valued
# subjects; all three are OPTIONAL so requiring them never drops a feature — the gate is
# the non-optional P571/P585 date, same as before. Verified live 2026-06-13.)
#
# Usage:  scripts/fetch_wikidata_themes.sh [out-dir]            (default: data/wikidata-themes)
#         THEMES="castles polities" scripts/fetch_wikidata_themes.sh   (only those keys)
#         LIMIT=10000 scripts/fetch_wikidata_themes.sh          (per-theme cap)
# Then build each to .rete:  for f in data/wikidata-themes/*.nt; do rete build "$f" -o "${f%.nt}.rete"; done
set -e
EP="https://query.wikidata.org/sparql"
UA="rete-atlas/0.1 (https://github.com/caviri/rete; carlosvivarrios@gmail.com)"
OUT="${1:-data/wikidata-themes}"
LIM="${LIMIT:-6000}"
mkdir -p "$OUT"
PFX='PREFIX wd: <http://www.wikidata.org/entity/> PREFIX wdt: <http://www.wikidata.org/prop/direct/> PREFIX rdfs: <http://www.w3.org/2000/01/rdf-schema#> PREFIX geo: <http://www.opengis.net/ont/geosparql#>'
wd() { curl -sG "$EP" --data-urlencode "query=$1" -H "Accept: application/n-triples" -H "User-Agent: $UA"; }
want() { [ -z "${THEMES:-}" ] && return 0; case " $THEMES " in *" $1 "*) return 0 ;; *) return 1 ;; esac; }

# emit <key> <Type> <WHERE-body>   INSTANT: body binds ?date ?coord ?label; emits ex:year.
emit() {
  local key="$1" t="$2" body="$3"; want "$key" || return 0
  wd "$PFX
CONSTRUCT { ?x a <http://ex/$t> ; rdfs:label ?label ; <http://ex/year> ?yr ; geo:hasGeometry ?g . ?g geo:asWKT ?coord . }
WHERE { $body FILTER(LANG(?label)=\"en\") BIND(IRI(CONCAT(STR(?x),\"/geom\")) AS ?g) BIND(YEAR(?date) AS ?yr) } LIMIT $LIM" > "$OUT/$key.nt"
  printf '%-22s %7s triples\n' "$key" "$(wc -l < "$OUT/$key.nt")"
}

# emiti <key> <Type> <class-selector>   INTERVAL: P571 start, COALESCE(P576,P3999,P582,2100) end.
emiti() {
  local key="$1" t="$2" sel="$3"; want "$key" || return 0
  wd "$PFX
CONSTRUCT { ?x a <http://ex/$t> ; rdfs:label ?label ; <http://ex/startYear> ?sy ; <http://ex/endYear> ?ey ; geo:hasGeometry ?g . ?g geo:asWKT ?coord . }
WHERE { ?x $sel ; wdt:P571 ?s ; wdt:P625 ?coord ; rdfs:label ?label . FILTER(LANG(?label)=\"en\")
  OPTIONAL { ?x wdt:P576 ?e1 } OPTIONAL { ?x wdt:P3999 ?e3 } OPTIONAL { ?x wdt:P582 ?e2 }
  BIND(YEAR(?s) AS ?sy) BIND(COALESCE(YEAR(?e1), YEAR(?e3), YEAR(?e2), 2100) AS ?ey)
  BIND(IRI(CONCAT(STR(?x),\"/geom\")) AS ?g) } LIMIT $LIM" > "$OUT/$key.nt"
  printf '%-22s %7s triples\n' "$key" "$(wc -l < "$OUT/$key.nt")"
}

# ---- INSTANT events (point in time, P585) ----
emit military-operations   MilitaryOp         '?x wdt:P31/wdt:P279* wd:Q645883 ; wdt:P585 ?date ; wdt:P625 ?coord ; rdfs:label ?label .'
emit sieges                Siege              '?x wdt:P31/wdt:P279* wd:Q188055 ; wdt:P585 ?date ; wdt:P625 ?coord ; rdfs:label ?label .'
emit earthquakes           Earthquake         '?x wdt:P31/wdt:P279* wd:Q7944 ; wdt:P585 ?date ; wdt:P625 ?coord ; rdfs:label ?label .'
emit disasters             Disaster           '?x wdt:P31 wd:Q3839081 ; wdt:P585 ?date ; wdt:P625 ?coord ; rdfs:label ?label .'
emit floods                Flood              '?x wdt:P31/wdt:P279* wd:Q8068 ; wdt:P585 ?date ; wdt:P625 ?coord ; rdfs:label ?label .'
emit meteorite-falls       MeteoriteFall      '?x wdt:P31/wdt:P279* wd:Q60186 ; wdt:P585 ?date ; wdt:P625 ?coord ; rdfs:label ?label .'
emit volcanic-eruptions    VolcanicEruption   '?x wdt:P31/wdt:P279* wd:Q7692360 ; wdt:P585 ?date ; wdt:P625 ?coord ; rdfs:label ?label .'
emit nuclear-explosions    NuclearExplosion   '?x wdt:P31/wdt:P279* wd:Q210112 ; wdt:P585 ?date ; wdt:P625 ?coord ; rdfs:label ?label .'
emit assassinations        Assassination      '?x wdt:P31/wdt:P279* wd:Q3882219 ; wdt:P585 ?date ; wdt:P625 ?coord ; rdfs:label ?label .'
emit treaties              Treaty             '?x wdt:P31 wd:Q131569 ; wdt:P585 ?date ; wdt:P276 ?loc . ?loc wdt:P625 ?coord . ?x rdfs:label ?label .'
emit epidemics             Epidemic           '?x wdt:P31/wdt:P279* wd:Q44512 ; wdt:P585 ?date ; wdt:P625 ?coord ; rdfs:label ?label .'
emit shipwrecks            Shipwreck          '?x wdt:P31/wdt:P279* wd:Q852190 ; wdt:P585 ?date ; wdt:P625 ?coord ; rdfs:label ?label .'
emit massacres             Massacre           '?x wdt:P31/wdt:P279* wd:Q3199915 ; wdt:P585 ?date ; wdt:P625 ?coord ; rdfs:label ?label .'
emit explosions            Explosion          '?x wdt:P31/wdt:P279* wd:Q179057 ; wdt:P585 ?date ; wdt:P625 ?coord ; rdfs:label ?label .'
emit aviation-accidents    AviationAccident   '?x wdt:P31/wdt:P279* wd:Q744913 ; wdt:P585 ?date ; wdt:P625 ?coord ; rdfs:label ?label .'
emit rail-accidents        RailAccident       '?x wdt:P31/wdt:P279* wd:Q1078765 ; wdt:P585 ?date ; wdt:P625 ?coord ; rdfs:label ?label .'
emit tsunamis              Tsunami            '?x wdt:P31/wdt:P279* wd:Q8070 ; wdt:P585 ?date ; wdt:P625 ?coord ; rdfs:label ?label .'
emit wildfires             Wildfire           '?x wdt:P31/wdt:P279* wd:Q169950 ; wdt:P585 ?date ; wdt:P625 ?coord ; rdfs:label ?label .'
emit coups                 Coup               '?x wdt:P31/wdt:P279* wd:Q45382 ; wdt:P585 ?date ; wdt:P625 ?coord ; rdfs:label ?label .'
emit revolutions           Revolution         '?x wdt:P31/wdt:P279* wd:Q10931 ; wdt:P585 ?date ; wdt:P625 ?coord ; rdfs:label ?label .'
emit terrorist-attacks     TerroristAttack    '?x wdt:P31/wdt:P279* wd:Q2223653 ; wdt:P585 ?date ; wdt:P625 ?coord ; rdfs:label ?label .'
emit expeditions           Expedition         '?x wdt:P31/wdt:P279* wd:Q2401485 ; wdt:P585 ?date ; wdt:P625 ?coord ; rdfs:label ?label .'
emit landslides            Landslide          '?x wdt:P31/wdt:P279* wd:Q167903 ; wdt:P585 ?date ; wdt:P625 ?coord ; rdfs:label ?label .'

# ---- INTERVAL structures / institutions / polities (P571 -> end, see emiti) ----
emiti castles              Castle             'wdt:P31/wdt:P279* wd:Q23413'
emiti fortifications       Fortification      'wdt:P31/wdt:P279* wd:Q1785071'
emiti lighthouses          Lighthouse         'wdt:P31/wdt:P279* wd:Q39715'
emiti cathedrals           Cathedral          'wdt:P31/wdt:P279* wd:Q2977'
emiti monasteries          Monastery          'wdt:P31/wdt:P279* wd:Q44613'
emiti abbeys               Abbey              'wdt:P31/wdt:P279* wd:Q160742'
emiti palaces              Palace             'wdt:P31/wdt:P279* wd:Q16560'
emiti forts                Fort               'wdt:P31/wdt:P279* wd:Q57831'
emiti universities         University         'wdt:P31/wdt:P279* wd:Q3918'
emiti world-heritage-sites WorldHeritageSite  'wdt:P1435 wd:Q9259'
emiti pyramids             Pyramid            'wdt:P31/wdt:P279* wd:Q12516'
emiti polities             Polity             'wdt:P31/wdt:P279* wd:Q6256'
emiti bridges              Bridge             'wdt:P31/wdt:P279* wd:Q12280'
emiti dams                 Dam                'wdt:P31/wdt:P279* wd:Q12323'
emiti museums              Museum             'wdt:P31/wdt:P279* wd:Q33506'
emiti mines                Mine               'wdt:P31/wdt:P279* wd:Q820477'
emiti stadiums             Stadium            'wdt:P31/wdt:P279* wd:Q483110'
emiti libraries            Library            'wdt:P31/wdt:P279* wd:Q7075'
emiti prisons              Prison             'wdt:P31/wdt:P279* wd:Q40357'
emiti mosques              Mosque             'wdt:P31/wdt:P279* wd:Q32815'
emiti synagogues           Synagogue          'wdt:P31/wdt:P279* wd:Q34627'
emiti temples              Temple             'wdt:P31/wdt:P279* wd:Q44539'
emiti towers               Tower              'wdt:P31/wdt:P279* wd:Q12518'
emiti theatres             Theatre            'wdt:P31/wdt:P279* wd:Q24354'
emiti observatories        Observatory        'wdt:P31/wdt:P279* wd:Q62832'
emiti railway-stations     RailwayStation     'wdt:P31/wdt:P279* wd:Q55488'
emiti amphitheatres        Amphitheatre       'wdt:P31/wdt:P279* wd:Q177380'
emiti aqueducts            Aqueduct           'wdt:P31/wdt:P279* wd:Q474'
emiti canals               Canal              'wdt:P31/wdt:P279* wd:Q12284'
emiti windmills            Windmill           'wdt:P31/wdt:P279* wd:Q38720'
emiti botanical-gardens    BotanicalGarden    'wdt:P31/wdt:P279* wd:Q167346'
emiti megaliths            Megalith           'wdt:P31/wdt:P279* wd:Q726870'
emiti churches             Church             'wdt:P31/wdt:P279* wd:Q16970'
emiti hospitals            Hospital           'wdt:P31/wdt:P279* wd:Q16917'
emiti cemeteries           Cemetery           'wdt:P31/wdt:P279* wd:Q39614'
emiti gardens              Garden             'wdt:P31/wdt:P279* wd:Q1107656'
emiti city-gates           CityGate           'wdt:P31/wdt:P279* wd:Q82117'
emiti power-stations       PowerStation       'wdt:P31/wdt:P279* wd:Q159719'
emiti factories            Factory            'wdt:P31/wdt:P279* wd:Q83405'
emiti breweries            Brewery            'wdt:P31/wdt:P279* wd:Q131734'
emiti shipyards            Shipyard           'wdt:P31/wdt:P279* wd:Q190928'
emiti airports             Airport            'wdt:P31/wdt:P279* wd:Q1248784'
emiti monuments            Monument           'wdt:P31/wdt:P279* wd:Q4989906'
emiti memorials            Memorial           'wdt:P31/wdt:P279* wd:Q5003624'
# (settlements Q486972, world-fairs Q56862, hillforts Q1130484, famines Q168247,
#  conferences Q2020153 dropped earlier: empty or too few features.)

echo "---"
echo "themes: $(ls "$OUT"/*.nt 2>/dev/null | wc -l) files in $OUT"
