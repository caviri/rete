#!/usr/bin/env bash
# Fetch the real-world knowledge graphs registered in the rete PLAYGROUND
# (web/playground-src/catalog.js). Each is a bounded, interesting subgraph built to
# a .rete: four are embedded in the page (linked-jazz, nomisma, mimotext, mmm),
# getty-ulan is large so it is served remote-lazy from the bucket.
#
# Usage:  scripts/fetch_playground_kgs.sh [linked-jazz|nomisma|mimotext|mmm|getty-ulan ...]
# Build (embed): rete build data/playground/<k>.{nt,ttl} -o web/<k>.rete  then build_playground.py
# Build (lazy) : rete build data/playground/getty-ulan.ttl -o data/playground/getty-ulan.rete
#                hf buckets cp ... hf://buckets/katospiegel/knowledge-graphs/playground/getty-ulan.rete
set -e
UA="rete-atlas/0.1 (https://github.com/caviri/rete; carlosvivarrios@gmail.com)"
OUT="data/playground"; mkdir -p "$OUT"
PY="${OHM_PYTHON:-python}"
SEL=("$@"); [ ${#SEL[@]} -eq 0 ] && SEL=(linked-jazz nomisma mimotext mmm getty-ulan)
want() { for s in "${SEL[@]}"; do [ "$s" = "$1" ] && return 0; done; return 1; }

if want linked-jazz; then
  echo "== linked-jazz (jazz musician social network, CC BY-SA) =="
  curl -sL "http://linkedjazz.org/api/people/all/nt"        > "$OUT/lj_people.nt"
  curl -sL "http://linkedjazz.org/api/relationships/all/nt" > "$OUT/lj_rel.nt"
  # The API's "nt" export breaks IRIs/literals containing ", Jr." across lines; keep
  # only well-formed N-Triples (object = IRI | blank | "literal"), dedupe.
  PYTHONIOENCODING=utf-8 "$PY" - "$OUT" <<'PY'
import re, sys
d=sys.argv[1]
pat=re.compile(r'^(<[^>]+>|_:\S+) <[^>]+> (<[^>]+>|_:\S+|"[^"]*"(@[\w-]+|\^\^<[^>]+>)?) \.$')
out=set()
for fn in (d+"/lj_people.nt", d+"/lj_rel.nt"):
    for raw in open(fn,encoding="utf-8"):
        t=raw.strip()
        if t and pat.match(t): out.add(t)
open(d+"/linked-jazz.nt","w",encoding="utf-8").write("\n".join(sorted(out))+"\n")
print("  linked-jazz:", len(out), "triples")
PY
fi

# --- SPARQL helper: GET a query file as the given Accept type ---
sparql() { # <endpoint> <accept> <out> <query-file>
  curl -s -L -G "$1" -H "Accept: $2" -H "User-Agent: $UA" --max-time 400 --data-urlencode "query@$4" -o "$3"
}

if want nomisma; then
  echo "== nomisma (coinage of Alexander the Great — PELLA, CC-BY) =="
  cat > "$OUT/nomisma.rq" <<'RQ'
PREFIX nmo: <http://nomisma.org/ontology#>
PREFIX nm: <http://nomisma.org/id/>
PREFIX void: <http://rdfs.org/ns/void#>
PREFIX skos: <http://www.w3.org/2004/02/skos/core#>
PREFIX rdfs: <http://www.w3.org/2000/01/rdf-schema#>
CONSTRUCT {
  ?t a nmo:TypeSeriesItem ; rdfs:label ?label ; nmo:hasMint ?mint ; nmo:hasAuthority ?auth ;
     nmo:hasMaterial ?mat ; nmo:hasDenomination ?den ; nmo:hasRegion ?reg ;
     nmo:hasStartDate ?sd ; nmo:hasEndDate ?ed .
  ?mint a nmo:Mint ; rdfs:label ?mintL . ?auth a nmo:Authority ; rdfs:label ?authL .
  ?mat a nmo:Material ; rdfs:label ?matL . ?den a nmo:Denomination ; rdfs:label ?denL .
  ?reg a nmo:Region ; rdfs:label ?regL .
} WHERE {
  ?t a nmo:TypeSeriesItem ; void:inDataset <http://numismatics.org/pella/> ; skos:prefLabel ?label . FILTER(LANG(?label)="en")
  OPTIONAL { ?t nmo:hasMint ?mint . FILTER(isIRI(?mint) && ?mint != nm:uncertain_value) OPTIONAL { ?mint skos:prefLabel ?mintL . FILTER(LANG(?mintL)="en") } }
  OPTIONAL { ?t nmo:hasAuthority ?auth . FILTER(isIRI(?auth) && ?auth != nm:uncertain_value) OPTIONAL { ?auth skos:prefLabel ?authL . FILTER(LANG(?authL)="en") } }
  OPTIONAL { ?t nmo:hasMaterial ?mat . FILTER(isIRI(?mat)) OPTIONAL { ?mat skos:prefLabel ?matL . FILTER(LANG(?matL)="en") } }
  OPTIONAL { ?t nmo:hasDenomination ?den . FILTER(isIRI(?den)) OPTIONAL { ?den skos:prefLabel ?denL . FILTER(LANG(?denL)="en") } }
  OPTIONAL { ?t nmo:hasRegion ?reg . FILTER(isIRI(?reg) && ?reg != nm:uncertain_value) OPTIONAL { ?reg skos:prefLabel ?regL . FILTER(LANG(?regL)="en") } }
  OPTIONAL { ?t nmo:hasStartDate ?sd } OPTIONAL { ?t nmo:hasEndDate ?ed }
}
RQ
  sparql "http://nomisma.org/query" "text/turtle" "$OUT/nomisma.ttl" "$OUT/nomisma.rq"
  echo "  nomisma: $(grep -c TypeSeriesItem "$OUT/nomisma.ttl") coin types"
fi

if want mimotext; then
  echo "== mimotext (French Enlightenment novels + stylometry, CC0) =="
  cat > "$OUT/mimotext.rq" <<'RQ'
PREFIX wdt: <http://data.mimotext.uni-trier.de/prop/direct/>
PREFIX p: <http://data.mimotext.uni-trier.de/prop/>
PREFIX ps: <http://data.mimotext.uni-trier.de/prop/statement/>
PREFIX pq: <http://data.mimotext.uni-trier.de/prop/qualifier/>
PREFIX rdfs:<http://www.w3.org/2000/01/rdf-schema#>
CONSTRUCT { ?s ?p ?o . ?a wdt:P49 ?b . ?st ps:P49 ?b ; pq:P52 ?dist . ?lbls rdfs:label ?lbl . } WHERE {
  { ?s ?p ?o . VALUES ?p { wdt:P2 wdt:P5 wdt:P7 wdt:P9 wdt:P10 wdt:P12 wdt:P32 wdt:P33 wdt:P36 wdt:P47 wdt:P57 wdt:P38 wdt:P39 wdt:P1 wdt:P50 wdt:P51 } }
  UNION { ?a p:P49 ?st . ?st ps:P49 ?b . OPTIONAL { ?st pq:P52 ?dist } }
  UNION { ?lbls rdfs:label ?lbl . FILTER(LANG(?lbl)="en") FILTER( STRSTARTS(STR(?lbls),"http://data.mimotext.uni-trier.de/entity/Q") || STRSTARTS(STR(?lbls),"http://data.mimotext.uni-trier.de/entity/P") ) } }
RQ
  sparql "https://query.mimotext.uni-trier.de/proxy/wdqs/bigdata/namespace/wdq/sparql" "text/turtle" "$OUT/mimotext.ttl" "$OUT/mimotext.rq"
  echo "  mimotext: $(wc -l < "$OUT/mimotext.ttl") lines"
fi

if want mmm; then
  echo "== mmm (medieval manuscript provenance, CC BY-NC) =="
  cat > "$OUT/mmm.rq" <<'RQ'
PREFIX crm: <http://erlangen-crm.org/current/>
PREFIX frbr: <http://erlangen-crm.org/efrbroo/>
PREFIX mmm: <http://ldf.fi/schema/mmm/>
PREFIX skos: <http://www.w3.org/2004/02/skos/core#>
PREFIX wgs: <http://www.w3.org/2003/01/geo/wgs84_pos#>
CONSTRUCT {
  ?ms a frbr:F4_Manifestation_Singleton ; skos:prefLabel ?msLabel ; mmm:produced_in ?place ; mmm:produced_when ?prodDate ;
      crm:P51_has_former_or_current_owner ?owner ; mmm:manuscript_author ?author ; mmm:manuscript_work ?work .
  ?place a crm:E53_Place ; skos:prefLabel ?placeLabel ; wgs:lat ?lat ; wgs:long ?long .
  ?owner a crm:E21_Person ; skos:prefLabel ?ownerLabel ; mmm:gender ?gender .
  ?author skos:prefLabel ?authorLabel . ?work skos:prefLabel ?workLabel .
} WHERE {
  VALUES ?place { <http://ldf.fi/mmm/place/tgn_7000457> <http://ldf.fi/mmm/place/tgn_7007867> <http://ldf.fi/mmm/place/tgn_7008929> <http://ldf.fi/mmm/place/tgn_7008323> }
  ?prod a crm:E12_Production ; crm:P108_has_produced ?ms ; crm:P7_took_place_at ?place . ?place skos:prefLabel ?placeLabel .
  OPTIONAL { ?place wgs:lat ?lat ; wgs:long ?long }
  OPTIONAL { ?prod crm:P4_has_time-span/skos:prefLabel ?prodDate }
  ?ms skos:prefLabel ?msLabel .
  OPTIONAL { ?ms crm:P51_has_former_or_current_owner ?owner . ?owner skos:prefLabel ?ownerLabel . OPTIONAL { ?owner mmm:gender ?gender } }
  OPTIONAL { ?ms mmm:manuscript_author ?author . OPTIONAL { ?author skos:prefLabel ?authorLabel } }
  OPTIONAL { ?ms mmm:manuscript_work ?work . OPTIONAL { ?work skos:prefLabel ?workLabel } }
}
RQ
  sparql "https://ldf.fi/mmm/sparql" "application/n-triples" "$OUT/mmm.nt" "$OUT/mmm.rq"
  echo "  mmm: $(wc -l < "$OUT/mmm.nt") triples"
fi

if want getty-ulan; then
  echo "== getty-ulan (artist teacher->pupil lineage, ODC-BY) — REMOTE-LAZY =="
  cat > "$OUT/ulan_rel.rq" <<'RQ'
PREFIX gvp: <http://vocab.getty.edu/ontology#>
CONSTRUCT { ?s gvp:teacherOf ?t . ?a gvp:influenced ?b . }
WHERE { { ?s gvp:ulan1101_teacher_of ?t } UNION { ?a gvp:ulan1107_influenced ?b } }
RQ
  cat > "$OUT/ulan_attr.rq" <<'RQ'
PREFIX gvp: <http://vocab.getty.edu/ontology#>
PREFIX skos: <http://www.w3.org/2004/02/skos/core#>
PREFIX foaf: <http://xmlns.com/foaf/0.1/>
PREFIX schema: <http://schema.org/>
PREFIX xl: <http://www.w3.org/2008/05/skos-xl#>
CONSTRUCT { ?p a foaf:Person ; skos:prefLabel ?plabel ; schema:description ?desc ; gvp:nationality ?natLabel ; gvp:birthYear ?birth ; gvp:deathYear ?death . }
WHERE {
  { ?p gvp:ulan1101_teacher_of [] } UNION { [] gvp:ulan1101_teacher_of ?p }
  UNION { ?p gvp:ulan1107_influenced [] } UNION { [] gvp:ulan1107_influenced ?p }
  ?p gvp:prefLabelGVP/xl:literalForm ?plabel .
  OPTIONAL { ?p foaf:focus ?ag .
    OPTIONAL { ?ag gvp:biographyPreferred ?bio . OPTIONAL { ?bio schema:description ?desc } OPTIONAL { ?bio gvp:estStart ?birth } OPTIONAL { ?bio gvp:estEnd ?death } }
    OPTIONAL { ?ag gvp:nationalityPreferred ?nat . ?nat xl:prefLabel/gvp:term ?natLabel . FILTER(LANG(?natLabel)="en") } }
}
RQ
  sparql "https://vocab.getty.edu/sparql" "text/turtle" "$OUT/ulan_rel.ttl"  "$OUT/ulan_rel.rq"
  sparql "https://vocab.getty.edu/sparql" "text/turtle" "$OUT/ulan_attr.ttl" "$OUT/ulan_attr.rq"
  cat "$OUT/ulan_rel.ttl" "$OUT/ulan_attr.ttl" > "$OUT/getty-ulan.ttl"
  echo "  getty-ulan: $(grep -c teacherOf "$OUT/getty-ulan.ttl") teacherOf edges (vocab.getty.edu is flaky; retry on 499)"
fi
echo "--- done ---"
