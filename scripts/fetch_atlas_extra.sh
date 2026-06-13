#!/usr/bin/env bash
# Extra atlas overlay providers beyond Wikidata/DBpedia/OHM — each a live SPARQL
# endpoint, fetched (paginated) as Turtle already in the atlas GeoSPARQL shape:
#   INTERVAL: <x> a ex:TYPE ; rdfs:label ?l ; ex:startYear ?sy ; ex:endYear ?ey ;
#             geo:hasGeometry <x/geom> . <x/geom> geo:asWKT "Point(lon lat)"^^geo:wktLiteral .
#
#   nomisma   — ancient coin TYPES at their mints (CC-BY 3.0), deep BCE.   http://nomisma.org/query
#   factgrid  — geolocated begin/end-dated places & events (CC0), an       https://database.factgrid.de/sparql
#               INDEPENDENT Wikibase with its own P-numbers (P48 coord, P49 begin, P50 end).
#   getty-tgn — TGN historical places with name-usage spans (ODC-By 1.0).  https://vocab.getty.edu/sparql
#
# Usage:  scripts/fetch_atlas_extra.sh [nomisma|factgrid|getty-tgn ...]   (default: all)
#         PAGES=4 PAGE=5000 scripts/fetch_atlas_extra.sh nomisma
set -e
UA="rete-atlas/0.1 (https://github.com/caviri/rete; carlosvivarrios@gmail.com)"
PAGE="${PAGE:-5000}"
OUTDIR="data/atlas-extra"
mkdir -p "$OUTDIR"
SEL=("$@"); [ ${#SEL[@]} -eq 0 ] && SEL=(nomisma factgrid getty-tgn)
want() { for s in "${SEL[@]}"; do [ "$s" = "$1" ] && return 0; done; return 1; }

# paginate <endpoint> <out.ttl> <pages> <query-with-LIMIT/OFFSET-placeholders __LIM__ __OFF__>
paginate() {
  local ep="$1" out="$2" pages="$3" q="$4" i off body n
  : > "$out"
  for ((i=0; i<pages; i++)); do
    off=$((i*PAGE))
    body=$(curl -s --max-time 600 -G "$ep" --data-urlencode "query=${q//__OFF__/$off}" -H "Accept: text/turtle" -H "User-Agent: $UA")
    n=$(printf '%s' "$body" | grep -c "asWKT" || true)
    printf '%s\n' "$body" >> "$out"
    printf '  page %d (offset %d): %s features\n' "$i" "$off" "$n"
    [ "$n" -lt "$PAGE" ] && break   # last page
  done
  printf '%-12s %s asWKT triples -> %s\n' "$(basename "${out%.ttl}")" "$(grep -c asWKT "$out" || true)" "$out"
}

if want nomisma; then
  echo "== nomisma (coin types at mints) =="
  paginate "http://nomisma.org/query" "$OUTDIR/nomisma.ttl" "${PAGES:-2}" \
'PREFIX nmo: <http://nomisma.org/ontology#>
PREFIX geo: <http://www.w3.org/2003/01/geo/wgs84_pos#>
PREFIX gs: <http://www.opengis.net/ont/geosparql#>
PREFIX skos: <http://www.w3.org/2004/02/skos/core#>
PREFIX rdfs: <http://www.w3.org/2000/01/rdf-schema#>
PREFIX ex: <http://ex/>
PREFIX xsd: <http://www.w3.org/2001/XMLSchema#>
CONSTRUCT { ?t a ex:CoinType ; rdfs:label ?label ; ex:startYear ?sy ; ex:endYear ?ey ; gs:hasGeometry ?geom . ?geom gs:asWKT ?wkt . }
WHERE { ?t a nmo:TypeSeriesItem ; skos:prefLabel ?label ; nmo:hasStartDate ?start ; nmo:hasEndDate ?end ; nmo:hasMint ?mint .
  ?mint geo:location ?loc . ?loc geo:lat ?lat ; geo:long ?long . FILTER(LANG(?label)="en")
  BIND(xsd:integer(xsd:string(?start)) + 0 AS ?sy) BIND(xsd:integer(xsd:string(?end)) + 0 AS ?ey)
  BIND(IRI(CONCAT(STR(?t),"/geom")) AS ?geom)
  BIND(STRDT(CONCAT("Point(", STR(?long), " ", STR(?lat), ")"), gs:wktLiteral) AS ?wkt) }
ORDER BY ?t LIMIT '"$PAGE"' OFFSET __OFF__'
fi

if want factgrid; then
  echo "== factgrid (places & events) =="
  paginate "https://database.factgrid.de/sparql" "$OUTDIR/factgrid.ttl" "${PAGES:-3}" \
'PREFIX fpt: <https://database.factgrid.de/prop/direct/>
PREFIX rdfs: <http://www.w3.org/2000/01/rdf-schema#>
PREFIX gs: <http://www.opengis.net/ont/geosparql#>
PREFIX ex: <http://ex/>
CONSTRUCT { ?s a ex:Place ; rdfs:label ?label ; ex:startYear ?sy ; ex:endYear ?ey ; gs:hasGeometry ?geom . ?geom gs:asWKT ?coord . }
WHERE { ?s fpt:P48 ?coord ; fpt:P49 ?begin ; rdfs:label ?label . FILTER(LANG(?label)="en")
  OPTIONAL { ?s fpt:P50 ?end . }
  BIND(YEAR(?begin) AS ?sy) BIND(IF(BOUND(?end), YEAR(?end), 2100) AS ?ey)
  BIND(IRI(CONCAT(STR(?s),"/geom")) AS ?geom) }
ORDER BY ?s LIMIT '"$PAGE"' OFFSET __OFF__'
fi

if want getty-tgn; then
  echo "== getty-tgn (historical places) =="
  # vocab.getty.edu MUST be POSTed from a file: a GET --data-urlencode mangles the
  # FILTER(LANG=\"en\") backslashes and silently returns 0 rows. It is also flaky
  # (HTTP 499 "Service temporarily degraded") — retry a few times.
  cat > "$OUTDIR/getty-tgn.rq" <<'RQ'
PREFIX gvp: <http://vocab.getty.edu/ontology#>
PREFIX xl: <http://www.w3.org/2008/05/skos-xl#>
PREFIX foaf: <http://xmlns.com/foaf/0.1/>
PREFIX wgs: <http://www.w3.org/2003/01/geo/wgs84_pos#>
PREFIX gs: <http://www.opengis.net/ont/geosparql#>
PREFIX ex: <http://ex/>
PREFIX rdfs: <http://www.w3.org/2000/01/rdf-schema#>
PREFIX xsd: <http://www.w3.org/2001/XMLSchema#>
CONSTRUCT { ?subj a ex:HistoricPlace ; rdfs:label ?label ; ex:startYear ?syi ; ex:endYear ?eyi ; gs:hasGeometry ?geom . ?geom gs:asWKT ?wkt . }
WHERE { ?subj xl:prefLabel|xl:altLabel ?term . ?term gvp:estStart ?sy . ?term gvp:estEnd ?ey .
  ?subj foaf:focus ?pl . ?pl wgs:lat ?lat ; wgs:long ?lon .
  ?subj gvp:prefLabelGVP/gvp:term ?label . FILTER(LANG(?label)="en")
  BIND(xsd:integer(STR(?sy)) AS ?syi) BIND(xsd:integer(STR(?ey)) AS ?eyi)
  BIND(IRI(CONCAT(STR(?subj),"/geom")) AS ?geom)
  BIND(STRDT(CONCAT("Point(", STR(?lon), " ", STR(?lat), ")"), gs:wktLiteral) AS ?wkt) }
ORDER BY ?subj LIMIT 6000
RQ
  for i in 1 2 3 4 5; do
    curl -sL https://vocab.getty.edu/sparql --data-urlencode "query@$OUTDIR/getty-tgn.rq" -H "Accept: text/turtle" -H "User-Agent: $UA" -o "$OUTDIR/getty-tgn.ttl"
    [ "$(grep -c asWKT "$OUTDIR/getty-tgn.ttl" || true)" -gt 0 ] && break
    sleep 5
  done
  printf '%-12s %s asWKT triples -> %s\n' "getty-tgn" "$(grep -c asWKT "$OUTDIR/getty-tgn.ttl" || true)" "$OUTDIR/getty-tgn.ttl"
fi
echo "--- done ---"
