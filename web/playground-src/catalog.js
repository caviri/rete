window.RETE_PLAYGROUND_CATALOG = {
  defaultDataset: "scholar",
  families: ["Summary", "Select", "Path", "Aggregate", "Geo", "Construct"],
  datasets: [
    {
      key: "scholar",
      label: "scholar.rete - synthetic scholarly world",
      description: "250 papers, 137 authors, 36 venues from scripts/synth_graph.py (seed 42): power-law citations, field communities, Zipfian venues, and typed literals."
    },
    {
      key: "wikidata",
      kind: "remote-lazy",
      url: "https://katospiegel-rete.hf.space/data/wikidata-1GB/wikidata.rete?token=sfdbgf1094by21hd128ru39802",
      // Wikidata types entities with "instance of" (wdt:P31), NOT rdf:type — tell
      // the Explore tab so it groups entities into tables by their real classes.
      typePredicate: "<http://www.wikidata.org/prop/direct/P31>",
      label: "wikidata-1GB.rete - a real 1 GB Wikidata graph (remote, lazy)",
      description: "A real ~1.04 GB slice of Wikidata, queried entirely in the browser over HTTP range requests - the point is that a GIGABYTE-scale knowledge graph stays interactive because a selective SPARQL query only faults in the bytes it needs (a typical query reads ~40 MB of the 1.04 GB, never the whole file). Holds both PLACES (e.g. wd:Q100001 Bemelen, with multilingual schema:description) and PEOPLE (wd:Q5 humans with wdt:P106 occupation, wdt:P569 birth date, wdt:P737 influenced-by). Entity/property IRIs stay as wikidata.org/{entity,prop/direct}/* so nodes round-trip to live Wikidata. Use bound subjects/objects and occupation intersections for snappy reads; avoid full scans. CC0 (Wikidata). See also the dedicated 100 MB / 1 GB explorer."
    },
    {
      key: "scholar-noisy",
      label: "scholar-noisy.rete - same world, 25% noise",
      description: "The same generator at --noise 0.25: rewired citations (incl. temporal violations), missing ORCIDs and ISSNs, and whitespace-mangled titles - for SHACL and data-quality demos."
    },
    {
      key: "citations",
      label: "citations - OpenCitations sample",
      description: "Real citation edges around the AlphaFold paper, enriched with labelled synthetic metadata for richer SPARQL examples."
    },
    {
      key: "typed",
      label: "typed.rete - people and orgs",
      description: "Small typed graph for fast schema, SHACL, and social-query smoke tests."
    },
    {
      key: "deps",
      label: "deps.rete - dependency graph",
      description: "Package dependency graph for impact analysis, transitive reachability, and CVE-style examples."
    },
    {
      key: "history",
      label: "history.rete - historical world borders (GeoSPARQL)",
      description: "World territorial borders at 7 snapshots from 323 BCE to 1994 CE (aourednik/historical-basemaps, GPL-3.0), each polygon stored as a geo:wktLiteral with an integer year. Query it with GeoSPARQL: point-in-polygon containment, bbox intersection, and distance — combined with temporal filters. Coordinates are CRS84 lon/lat, simplified to ~1 km."
    },
    {"key": "linked-jazz", "label": "linked-jazz.rete - jazz musician social network", "description": "Linked Jazz - a social network of jazz musicians reconstructed from oral-history transcripts. 54 interviewed musicians (the ego hubs) connect outward to ~1,940 people they mention, with who-knows-whom ties typed by the REL vocab (knowsOf, friendOf, hasMet, influencedBy, mentorOf), Music Ontology (collaborated_with) and the project's own ontology (playedTogether, inBandTogether, touredWith, bandLeaderOf). 40 of the 54 hubs also appear as objects, so genuine multi-hop paths exist (transitive knowsOf+ / mentorOf+ chains). Each person carries a foaf:name and usually a dbo:thumbnail. ~9,470 triples: 3,649 knowsOf, 2,009 names, 1,555 thumbnails, plus ~1,800 typed ties. Person IRIs are mostly dbpedia.org/resource, so nodes link out to DBpedia. CC BY-SA 3.0 (Pratt Institute; person data from DBpedia)."},
    {"key": "getty-ulan", "kind": "remote-lazy", "url": "https://katospiegel-rete.hf.space/data/playground/getty-ulan.rete?token=sfdbgf1094by21hd128ru39802", "label": "getty-ulan.rete - artist mentorship lineage (remote, lazy)", "description": "Getty ULAN \"who-taught-whom\" lineage: a directed social graph of ~28,300 artists/agents from the Union List of Artist Names. Each person carries a preferred name (skos:prefLabel), an English one-line biography (schema:description, e.g. \"Dutch painter, printmaker, 1606-1669\"), nationality, and birth/death years (xsd:gYear). Persons are connected by gvp:teacherOf (34,561 master->pupil edges) and gvp:influenced (534 edges). Densely connected and deeply transitive - Rembrandt taught ~38 pupils and has ~369 artistic descendants via teacherOf+. IRIs stay as vocab.getty.edu/ulan/NNN so nodes round-trip to live Getty LOD. ~205k triples after de-dup. ODC-BY 1.0 (attribute The J. Paul Getty Trust)."},
    {"key": "nomisma", "label": "nomisma.rete - coinage of Alexander the Great (PELLA)", "description": "PELLA - Coinage of Alexander the Great and the Macedonian kingdom, from Nomisma.org. 7,228 ancient Greek coin TYPES struck under Philip II, Alexander III, Philip III Arrhidaeus and the Diadochi (Cassander, Lysimachus, Ptolemy I), spanning 359-65 BC. Each type links by real IRIs to its mint (~150 cities from Pella and Amphipolis to Babylon, Sardis, Sidon and Susa), issuing authority, material (silver/gold/bronze), denomination (tetradrachm, drachm, stater...), region, and start/end dates as xsd:gYear. Every mint/authority/material/denomination/region carries an English rdfs:label, so the graph is fully self-describing. ~53,535 triples, embeds to ~150 KB. ODbL 1.0 (PELLA coin data) + CC-BY 3.0 (Nomisma vocabulary); attribute the American Numismatic Society / Nomisma.org."},
    {"key": "mimotext", "label": "mimotext.rete - French Enlightenment novels + stylometry", "description": "MiMoTextBase, a Wikibase of French Enlightenment novels (c. 1751-1800) from the Trier Center for Digital Humanities. A self-contained literary graph: 1,774 works linked to authors (956 people), publication dates and places, genres, languages, narrative form and location, and 375 thematic concepts (4,096 work->theme edges). Its distinctive layer is computational: 520 work-to-work STYLOMETRIC SIMILARITY edges, each carrying a Burrows-Delta-style distance value as a qualifier, plus 191 scholarship-mention edges. English labels for every entity. ~25,155 triples, ~828 KB Turtle -> embeds. A browsable network of who wrote what, which novels are thematically and stylistically close, and which scholarship discusses them together. CC0 (public domain)."},
    {"key": "mmm", "label": "mmm.rete - medieval manuscript provenance (GeoSPARQL)", "description": "Mapping Manuscript Migrations (MMM) - a CIDOC-CRM/FRBRoo graph (24M triples live) tracing the provenance of medieval and Renaissance manuscripts, unifying the Schoenberg Database, Bibale (IRHT) and Medieval Manuscripts in Oxford. This bounded subset is a self-contained provenance graph: 3,155 manuscripts produced in four book-production cities (Florence, Bruges, Rouen, Tours), each linked to its production city (with WGS84 coordinates) and date-range, its former/current owners (named historical persons with gender), the texts it carries and their authors. ~48,191 triples, ~6.5 MB N-Triples -> small .rete. Good for Path (ownership chains), Aggregate (books per city, most-traded manuscripts), Construct (provenance ego-networks), and Geo (the production cities carry coordinates). CC BY-NC 4.0 - non-commercial, attribute MMM."},
    {"key": "openalex-astrocytes", "label": "openalex-astrocytes.rete - astrocyte research graph (OpenAlex)", "description": "The 500 most-cited works on astrocytes (the star-shaped glial cells of the brain) from OpenAlex (CC0), as a connected citation core: 4,113 cito:cites edges linking 500 papers to 2,074 authors, 537 institutions and 875 sub-topics. Explore the most-cited papers, the most prolific labs, and which fields astrocyte research bridges (reactive astrocytes, blood-brain barrier, neuroinflammation, stem cells)."},
    {"key": "antarctic-expeditions", "label": "antarctic-expeditions.rete - Heroic-Age expeditions, crews & ships", "description": "Heroic-Age Antarctic exploration as an explorable social graph: 6 landmark expeditions (Discovery, Nimrod, Terra Nova, Endurance, Australasian, Belgian, 1897-1917) linked by ex:participant to ~76 crew, by ex:vessel to their 5 ships, and by ex:leader to their commanders (Scott, Shackleton, Mawson, de Gerlache). Each expedition carries ex:startYear/ex:endYear. Because expeditions share personnel and ships, genuine multi-hop paths exist (shared-crew bridges, leaders who served on earlier voyages). IRIs stay as wikidata.org/entity so nodes round-trip to live Wikidata. CC0. Pairs with the atlas 'Heroic-Age Sites' overlay: the huts and deaths on the map are where these crews lived and died."},
    {"key": "factgrid-illuminati", "label": "factgrid-illuminati.rete - Order of the Illuminati prosopography", "description": "The 18th-century secret society as a prosopographical graph from FactGrid (CC0), an independent historical Wikibase: ~1,300 members of the Order of the Illuminati (Q10677) with their FactGrid properties and English labels. Property and object labels are resolved so the opaque P-numbers read in plain language."},
    {"key": "theographic-graph", "label": "theographic-graph.rete - biblical narrative graph", "description": "Theographic Bible (CC BY-SA) as a narrative/genealogy graph (distinct from the atlas geo-events layer): ~3,000 people linked by tg:father/mother/child/sibling/partner, born/died places, group memberships, and events with participants. Walk genealogies and event chains."},
    {"key": "monarch", "label": "monarch.rete - disease/gene/phenotype graph", "description": "A bounded slice of the Monarch Initiative biomedical KG (CC-BY): a disease neighbourhood linking genes (biolink:Gene), phenotypes (biolink:has_phenotype), gene-gene interactions (biolink:interacts_with) and taxa, with rdfs:labels and skos:exactMatch cross-references."},
    {"key": "opencitations", "label": "opencitations.rete - a citation neighborhood", "description": "A citation neighbourhood from OpenCitations (CC0) around a seed paper: cito:cites edges plus dct:title / dct:date / dct:creator (foaf:name) / dct:publisher bibliographic metadata. Distinct from the small AlphaFold sample already in the catalog."},
    {"key": "orkg", "label": "orkg.rete - research contributions", "description": "A slice of the Open Research Knowledge Graph (CC-BY): papers (orkg:Paper) and their structured contributions (orkg:Contribution), research problems, methods and results, with rdfs:labels - scholarly knowledge as data, not prose."},
    {"key": "ohm-full", "kind": "remote-lazy", "url": "https://katospiegel-rete.hf.space/data/playground/ohm-full.rete?token=sfdbgf1094by21hd128ru39802", "label": "ohm-full.rete - all of OpenHistoricalMap (remote, lazy)", "description": "The ENTIRE current OpenHistoricalMap planet (daily snapshot, 2026-06-14) as one range-queried .rete: 1,021,295 named + dated + geolocated historical features (~6.1M triples, 98 MB), queried lazily over HTTP - only the dictionary chunks and index tiles each query touches are fetched, never the whole file. Each feature is openhistoricalmap.org/{node,way,relation}/<id> with rdfs:label, ex:startYear/ex:endYear (signed integers, -10000..2100; 2100 = still present) and GeoSPARQL geometry (176k points, 690k lines, 155k polygons; admin boundaries assembled from multipolygon/boundary relations, simplified to ~50 m). Built from planet.openhistoricalmap.org with PyOsmium (scripts/fetch_ohm_planet.sh). CC0 1.0 - credit OpenHistoricalMap contributors. Pick selective shapes (a bound subject, a name, a point-in-polygon) for snappy lazy reads."},
    {"key": "wikidata-100mb", "kind": "remote-lazy", "url": "https://katospiegel-rete.hf.space/data/wikidata-100MB/wikidata.rete?token=sfdbgf1094by21hd128ru39802", "typePredicate": "<http://www.wikidata.org/prop/direct/P31>", "label": "wikidata-100MB.rete - a real 100 MB Wikidata slice (remote, lazy)", "description": "A real ~104 MB slice of Wikidata, queried lazily over HTTP range requests straight from a Hugging Face bucket - the browser fetches only the dictionary chunks and index tiles each query touches (a typical selective query reads ~10 MB of the 104 MB), never the whole file. People (wd:Q5 humans) carry rdfs:label (multilingual), occupation (wdt:P106 -> e.g. physicist Q169470, philosopher Q4964182, writer Q36180, politician Q82955), date of birth (wdt:P569), place of birth (wdt:P19), citizenship (wdt:P27) and 'influenced by' (wdt:P737). Entity/property IRIs stay as wikidata.org/{entity,prop/direct}/* so nodes round-trip to live Wikidata. Pick SELECTIVE shapes (a bound subject/object, an occupation intersection) for snappy reads; aggregates over a whole predicate scan more. CC0 (Wikidata). The 1 GB version (key: wikidata) is the same idea at 10x the data."}
  ],
  examples: {
    "wikidata-100mb": [
      {"family": "Select", "label": "Physicists who are also philosophers", "view": "graph", "tip": "An occupation intersection (wdt:P106 twice). Selective - the lazy reader faults in ~10 MB of the 104 MB file and returns scientist-philosophers like Ilya Prigogine and Marin Mersenne.", "q": "PREFIX wdt: <http://www.wikidata.org/prop/direct/>\nPREFIX wd: <http://www.wikidata.org/entity/>\nPREFIX rdfs: <http://www.w3.org/2000/01/rdf-schema#>\nSELECT ?who WHERE {\n  ?p wdt:P106 wd:Q169470 ;   # physicist\n     wdt:P106 wd:Q4964182 ;  # philosopher\n     rdfs:label ?who .\n  FILTER(LANG(?who) = \"en\")\n} LIMIT 50"},
      {"family": "Select", "label": "People influenced by Plato", "view": "graph", "tip": "Bound-object star: everyone whose 'influenced by' (wdt:P737) points at Plato (wd:Q859). A bound term = a selective lazy read.", "q": "PREFIX wdt: <http://www.wikidata.org/prop/direct/>\nPREFIX wd: <http://www.wikidata.org/entity/>\nPREFIX rdfs: <http://www.w3.org/2000/01/rdf-schema#>\nSELECT ?who WHERE {\n  ?p wdt:P737 wd:Q859 ;   # influenced by Plato\n     rdfs:label ?who .\n  FILTER(LANG(?who) = \"en\")\n} LIMIT 100"},
      {"family": "Path", "label": "Lines of influence from Plato (transitive)", "view": "graph", "tip": "wdt:P737+ walks the 'influenced by' chain transitively from Plato (wd:Q859) - intellectual lineage across generations. LIMIT keeps the lazy walk bounded.", "q": "PREFIX wdt: <http://www.wikidata.org/prop/direct/>\nPREFIX wd: <http://www.wikidata.org/entity/>\nPREFIX rdfs: <http://www.w3.org/2000/01/rdf-schema#>\nSELECT DISTINCT ?who WHERE {\n  ?p wdt:P737+ wd:Q859 .\n  ?p rdfs:label ?who . FILTER(LANG(?who) = \"en\")\n} LIMIT 150"},
      {"family": "Select", "label": "Writers who are also politicians", "view": "table", "tip": "Another occupation intersection (wdt:P106 writer + politician) - novelists and poets who also held office.", "q": "PREFIX wdt: <http://www.wikidata.org/prop/direct/>\nPREFIX wd: <http://www.wikidata.org/entity/>\nPREFIX rdfs: <http://www.w3.org/2000/01/rdf-schema#>\nSELECT ?who WHERE {\n  ?p wdt:P106 wd:Q36180 ;   # writer\n     wdt:P106 wd:Q82955 ;   # politician\n     rdfs:label ?who .\n  FILTER(LANG(?who) = \"en\")\n} LIMIT 50"},
      {"family": "Aggregate", "label": "Most common occupations", "view": "table", "tip": "Counts people per occupation (wdt:P106). This aggregates over a WHOLE predicate, so it faults in more tiles than a selective query - heavier, but still streamed lazily, not a full download.", "q": "PREFIX wdt: <http://www.wikidata.org/prop/direct/>\nSELECT ?occupation (COUNT(?p) AS ?people) WHERE {\n  ?p wdt:P106 ?occupation\n}\nGROUP BY ?occupation\nORDER BY DESC(?people)\nLIMIT 20"},
      {"family": "Construct", "label": "People with occupation + birth date", "view": "graph", "tip": "CONSTRUCT a small subgraph of (person)->occupation and (person)->birth-date edges; touches only those two predicates' tiles.", "q": "PREFIX wdt: <http://www.wikidata.org/prop/direct/>\nCONSTRUCT { ?p wdt:P106 ?occ ; wdt:P569 ?dob }\nWHERE {\n  ?p wdt:P106 ?occ .\n  OPTIONAL { ?p wdt:P569 ?dob }\n} LIMIT 200"}
    ],
    "ohm-full": [
      {"family": "Geo", "label": "Who ruled here? (point-in-polygon + time)", "view": "table", "tip": "A point (Berlin, 13.405 52.52) tested against every feature's polygon and filtered to the year 1900 returns the nested jurisdictions of that place and time: the city, the German Empire (Deutsches Reich) and the Kingdom of Prussia. This is the query the whole dataset exists for.", "q": "PREFIX ex: <http://ex/>\nPREFIX rdfs: <http://www.w3.org/2000/01/rdf-schema#>\nPREFIX geo: <http://www.opengis.net/ont/geosparql#>\nPREFIX geof: <http://www.opengis.net/def/function/geosparql/>\nSELECT ?label ?s ?e WHERE {\n  ?x rdfs:label ?label ; ex:startYear ?s ; ex:endYear ?e ;\n     geo:hasGeometry/geo:asWKT ?w .\n  FILTER(?s <= 1900 && ?e >= 1900)\n  FILTER(geof:sfContains(?w, \"POINT(13.405 52.52)\"^^geo:wktLiteral))\n}"},
      {"family": "Select", "label": "Find a place by name", "view": "table", "tip": "A bound object on rdfs:label routes to just the tiles holding that name - a few range reads of a 98 MB file. Abdera is an ancient Greek colony on the Thracian coast.", "q": "PREFIX ex: <http://ex/>\nPREFIX rdfs: <http://www.w3.org/2000/01/rdf-schema#>\nSELECT ?x ?s ?e WHERE {\n  ?x rdfs:label \"Abdera\"@en ; ex:startYear ?s ; ex:endYear ?e\n}"},
      {"family": "Select", "label": "All facts about one feature", "view": "table", "tip": "A bound subject is the most selective shape - minimal bytes fetched. This is the OHM node for Abdera; the IRI round-trips to openhistoricalmap.org.", "q": "SELECT ?p ?o WHERE { <https://www.openhistoricalmap.org/node/2095928201> ?p ?o }"},
      {"family": "Aggregate", "label": "How many features predate the Common Era?", "view": "table", "tip": "Counts features whose start year is negative (BCE) - 2,057. An aggregate scans the whole ex:startYear predicate, so it fetches more tiles than the selective examples above.", "q": "PREFIX ex: <http://ex/>\nSELECT (COUNT(*) AS ?n) WHERE { ?x ex:startYear ?s . FILTER(?s < 0) }"},
      {"family": "Select", "label": "The oldest things on the map", "view": "table", "tip": "Order every feature by its start year, oldest first - deep-BCE sites (down to the -10000 clamp): ancient settlements, megaliths and prehistoric landmarks.", "q": "PREFIX ex: <http://ex/>\nPREFIX rdfs: <http://www.w3.org/2000/01/rdf-schema#>\nSELECT ?label ?s ?e WHERE {\n  ?x rdfs:label ?label ; ex:startYear ?s ; ex:endYear ?e\n} ORDER BY ?s LIMIT 25"}
    ],
    wikidata: [
      {
        family: "Select",
        label: "All facts about an entity",
        view: "table",
        tip: "A bound subject (Bemelen, Q100001) routes to just the tiles holding it - a few range reads of the whole file. The coordinate comes back as a geo:wktLiteral (recovered datatype).",
        q: `SELECT ?p ?o WHERE { <http://www.wikidata.org/entity/Q100001> ?p ?o }`
      },
      {
        family: "Select",
        label: "English labels of an entity",
        view: "table",
        tip: "Bound subject + bound predicate: the most selective shape - minimal bytes fetched.",
        q: `PREFIX rdfs: <http://www.w3.org/2000/01/rdf-schema#>
SELECT ?label WHERE {
  <http://www.wikidata.org/entity/Q100001> rdfs:label ?label
} LIMIT 50`
      },
      {
        family: "Select",
        label: "Coordinates of a place",
        view: "table",
        tip: "Returns a geo:wktLiteral - the datatype recovered during the parquet->rete conversion.",
        q: `PREFIX wdt: <http://www.wikidata.org/prop/direct/>
SELECT ?coord WHERE {
  <http://www.wikidata.org/entity/Q100001> wdt:P625 ?coord
}`
      },
      {
        family: "Path",
        label: "Subclasses of a class",
        view: "table",
        tip: "Reverse bound-object lookup (P279 = subclass of): who declares this as their superclass.",
        q: `PREFIX wdt: <http://www.wikidata.org/prop/direct/>
SELECT ?sub WHERE {
  ?sub wdt:P279 <http://www.wikidata.org/entity/Q515>
} LIMIT 50`
      },
      {
        family: "Select",
        label: "Physicists who are also philosophers (in 1 GB)",
        view: "graph",
        tip: "A whole gigabyte graph, queried in the browser: this occupation intersection (wdt:P106 twice) faults in only ~40 MB of the 1.04 GB file - the rest never leaves the server. Returns scientist-philosophers like Ilya Prigogine.",
        q: `PREFIX wdt: <http://www.wikidata.org/prop/direct/>
PREFIX wd: <http://www.wikidata.org/entity/>
PREFIX rdfs: <http://www.w3.org/2000/01/rdf-schema#>
SELECT ?who WHERE {
  ?p wdt:P106 wd:Q169470 ;   # physicist
     wdt:P106 wd:Q4964182 ;  # philosopher
     rdfs:label ?who .
  FILTER(LANG(?who) = "en")
} LIMIT 50`
      }
    ],
    scholar: [
      {
        family: "Summary",
        label: "Predicate totals",
        strategy: "progressive",
        view: "table",
        tip: "Exact predicate counts from the pyramid summary; the triple index is skipped.",
        q: `SELECT ?p (COUNT(*) AS ?triples) WHERE { ?s ?p ?o } GROUP BY ?p`
      },
      {
        family: "Select",
        label: "Author profiles",
        view: "table",
        tip: "Names, integer-typed h-index values, and affiliations, highest h-index first.",
        q: `PREFIX ex: <http://ex/>
PREFIX foaf: <http://xmlns.com/foaf/0.1/>
SELECT ?author ?name ?h ?institution WHERE {
  ?author a ex:Person ;
    foaf:name ?name ;
    ex:hIndex ?h ;
    ex:affiliation ?institution
} ORDER BY DESC(?h) LIMIT 50`
      },
      {
        family: "Select",
        label: "High-novelty papers",
        view: "table",
        tip: "FILTER over an xsd:double literal (noveltyScore is log-normal, so the tail is short).",
        q: `PREFIX ex: <http://ex/>
PREFIX dct: <http://purl.org/dc/terms/>
SELECT ?title ?score WHERE {
  ?paper ex:noveltyScore ?score ;
    dct:title ?title .
  FILTER(?score > 2.0)
} ORDER BY DESC(?score)`
      },
      {
        family: "Select",
        label: "High-novelty, split by community",
        strategy: "community",
        view: "table",
        tip: "The split strategy: subject stars evaluate per pyramid community, joins recombine the partials globally, FILTER/ORDER BY semantics intact — identical rows to the whole-index run.",
        q: `PREFIX ex: <http://ex/>
PREFIX dct: <http://purl.org/dc/terms/>
SELECT ?title ?score WHERE {
  ?paper ex:noveltyScore ?score ;
    dct:title ?title .
  FILTER(?score > 2.0)
} ORDER BY DESC(?score)`
      },
      {
        family: "Path",
        label: "Citation closure",
        view: "table",
        tip: "Transitive cito:cites+ from one recent paper reaches 73 papers (citations only point backwards in time).",
        q: `PREFIX cito: <http://purl.org/spar/cito/>
SELECT DISTINCT ?reached WHERE { <http://ex/paper/245> cito:cites+ ?reached }`
      },
      {
        family: "Aggregate",
        label: "Most-cited papers",
        view: "table",
        tip: "The preferential-attachment power law: a few papers soak up most citations.",
        q: `PREFIX cito: <http://purl.org/spar/cito/>
PREFIX dct: <http://purl.org/dc/terms/>
SELECT ?title (COUNT(?citing) AS ?citations) WHERE {
  ?citing cito:cites ?paper .
  ?paper dct:title ?title
} GROUP BY ?title ORDER BY DESC(?citations) LIMIT 10`
      },
      {
        family: "Aggregate",
        label: "Papers per field",
        view: "table",
        tip: "Zipfian field sizes: genomics dominates, the tail is thin.",
        q: `PREFIX ex: <http://ex/>
SELECT ?field (COUNT(?paper) AS ?papers) WHERE {
  ?paper a ex:Paper ;
    ex:hasField ?field
} GROUP BY ?field ORDER BY DESC(?papers)`
      },
      {
        family: "Aggregate",
        label: "Authors above the mean h-index",
        view: "table",
        tip: "A subquery computes the average h-index in its own scope; the outer query keeps only authors beating it. Nested SELECT support makes this one query.",
        q: `PREFIX ex: <http://ex/>
PREFIX foaf: <http://xmlns.com/foaf/0.1/>
SELECT ?name ?h WHERE {
  ?a foaf:name ?name ; ex:hIndex ?h .
  { SELECT (AVG(?x) AS ?avg) WHERE { ?p ex:hIndex ?x } }
  FILTER(?h > ?avg)
} ORDER BY DESC(?h) LIMIT 20`
      },
      {
        family: "Select",
        label: "Novelty tiers (IF)",
        view: "table",
        tip: "Nested IF() buckets each paper's xsd:double novelty score into high / medium / low — a BIND the rest of the query can see.",
        q: `PREFIX ex: <http://ex/>
PREFIX dct: <http://purl.org/dc/terms/>
SELECT ?title ?tier WHERE {
  ?paper dct:title ?title ; ex:noveltyScore ?score .
  BIND(IF(?score > 2.0, "high", IF(?score > 1.0, "medium", "low")) AS ?tier)
} LIMIT 50`
      },
      {
        family: "Select",
        label: "Title fingerprints (SHA-256)",
        view: "table",
        tip: "SHA256 (and MD5 / SHA1 / SHA384 / SHA512) plus STRLEN are part of the SPARQL 1.1 function library — a content hash of each title and its length.",
        q: `PREFIX dct: <http://purl.org/dc/terms/>
SELECT ?title (SHA256(STR(?title)) AS ?fingerprint) (STRLEN(?title) AS ?len) WHERE {
  ?paper dct:title ?title
} LIMIT 20`
      },
      {
        family: "Path",
        label: "Everything but citations",
        view: "table",
        tip: "A negated property set !(cito:cites) walks one step over every predicate except citation edges — all the non-citation facts of one paper.",
        q: `PREFIX cito: <http://purl.org/spar/cito/>
SELECT ?o WHERE { <http://ex/paper/15> !(cito:cites) ?o }`
      },
      {
        family: "Construct",
        label: "Coauthor ego network",
        view: "graph",
        tip: "Two hops of coauthorship around the busiest hub author.",
        q: `PREFIX ex: <http://ex/>
CONSTRUCT { ?a ex:coauthor ?b } WHERE {
  { <http://ex/author/105> ex:coauthor ?b BIND(<http://ex/author/105> AS ?a) }
  UNION
  { <http://ex/author/105> ex:coauthor ?a . ?a ex:coauthor ?b }
}`
      }
    ],
    "scholar-noisy": [
      {
        family: "Summary",
        label: "Predicate totals",
        strategy: "progressive",
        view: "table",
        tip: "Exact predicate counts from the pyramid summary; the triple index is skipped.",
        q: `SELECT ?p (COUNT(*) AS ?triples) WHERE { ?s ?p ?o } GROUP BY ?p`
      },
      {
        family: "Select",
        label: "Mangled titles",
        view: "table",
        tip: "REGEX catches the whitespace mess the noise knob injected into 20 titles.",
        q: `PREFIX dct: <http://purl.org/dc/terms/>
SELECT ?paper ?title WHERE {
  ?paper dct:title ?title .
  FILTER(REGEX(?title, "^  "))
}`
      },
      {
        family: "Select",
        label: "Authors missing ORCID",
        view: "table",
        tip: "NOT EXISTS finds the 16 author records the noise stripped an ORCID from.",
        q: `PREFIX ex: <http://ex/>
PREFIX foaf: <http://xmlns.com/foaf/0.1/>
SELECT ?author ?name WHERE {
  ?author a ex:Person ;
    foaf:name ?name .
  FILTER NOT EXISTS { ?author ex:orcid ?orcid }
}`
      },
      {
        family: "Path",
        label: "Noise-inflated closure",
        view: "table",
        tip: "The same seed paper reaches 16 papers in the clean graph - rewired citations inflate it to 228 here.",
        q: `PREFIX cito: <http://purl.org/spar/cito/>
SELECT DISTINCT ?reached WHERE { <http://ex/paper/249> cito:cites+ ?reached }`
      },
      {
        family: "Aggregate",
        label: "Temporal violations",
        view: "table",
        tip: "Papers citing later-dated papers - impossible without noise; 298 of them here.",
        q: `PREFIX cito: <http://purl.org/spar/cito/>
PREFIX dct: <http://purl.org/dc/terms/>
SELECT (COUNT(*) AS ?violations) WHERE {
  ?citing cito:cites ?cited .
  ?citing dct:date ?d1 .
  ?cited dct:date ?d2 .
  FILTER(STR(?d2) > STR(?d1))
}`
      },
      {
        family: "Aggregate",
        label: "Cross-field citation pairs",
        view: "table",
        tip: "Citations that jump fields are mostly noise rewires; the clean graph keeps them rare.",
        q: `PREFIX ex: <http://ex/>
PREFIX cito: <http://purl.org/spar/cito/>
SELECT ?from ?to (COUNT(*) AS ?n) WHERE {
  ?a cito:cites ?b .
  ?a ex:hasField ?from .
  ?b ex:hasField ?to .
  FILTER(?from != ?to)
} GROUP BY ?from ?to ORDER BY DESC(?n) LIMIT 15`
      },
      {
        family: "Construct",
        label: "Cross-field cites from genomics",
        view: "graph",
        tip: "Draws the noise: genomics papers citing into other fields.",
        q: `PREFIX ex: <http://ex/>
PREFIX cito: <http://purl.org/spar/cito/>
CONSTRUCT { ?a ex:crossFieldCite ?b } WHERE {
  ?a cito:cites ?b .
  ?a ex:hasField <http://ex/field/genomics> .
  ?b ex:hasField ?other .
  FILTER(?other != <http://ex/field/genomics>)
}`
      }
    ],
    citations: [
      {
        family: "Summary",
        label: "Count citation edges",
        strategy: "progressive",
        view: "table",
        tip: "Exact cito:cites count from summary metadata only.",
        q: `PREFIX cito: <http://purl.org/spar/cito/>
SELECT (COUNT(*) AS ?citationEdges) WHERE { ?s cito:cites ?o }`
      },
      {
        family: "Select",
        label: "Paper titles",
        view: "table",
        tip: "Single-pattern title scan over enriched citation metadata.",
        q: `PREFIX dct: <http://purl.org/dc/terms/>
SELECT ?paper ?title WHERE { ?paper dct:title ?title } LIMIT 50`
      },
      {
        family: "Aggregate",
        label: "Citations per year",
        view: "table",
        tip: "Join real citation edges with year metadata and group by date.",
        q: `PREFIX cito: <http://purl.org/spar/cito/>
PREFIX dct: <http://purl.org/dc/terms/>
SELECT ?year (COUNT(?paper) AS ?n) WHERE {
  ?paper cito:cites <https://doi.org/10.1038/s41586-021-03819-2> .
  ?paper dct:date ?year
} GROUP BY ?year ORDER BY ?year`
      },
      {
        family: "Path",
        label: "Collaborator closure",
        view: "table",
        tip: "A large coauthor transitive path from a high-degree author.",
        q: `PREFIX ex: <http://ex/>
SELECT DISTINCT ?collaborator WHERE {
  <http://ex/author/1235> ex:coauthor+ ?collaborator
} LIMIT 100`
      },
      {
        family: "Aggregate",
        label: "Papers above average citations",
        view: "table",
        tip: "A subquery computes the mean citation count; the outer query keeps the papers beating it — the new nested-SELECT support makes this a single query.",
        q: `PREFIX ex: <http://ex/>
PREFIX dct: <http://purl.org/dc/terms/>
SELECT ?title ?c WHERE {
  ?p ex:citationCount ?c ; dct:title ?title .
  { SELECT (AVG(?x) AS ?avg) WHERE { ?q ex:citationCount ?x } }
  FILTER(?c > ?avg)
} ORDER BY DESC(?c) LIMIT 20`
      },
      {
        family: "Construct",
        label: "Hub ego network",
        view: "graph",
        tip: "Constructs one author's direct coauthor network for the graph renderer.",
        q: `PREFIX ex: <http://ex/>
CONSTRUCT { <http://ex/author/1235> ex:coauthor ?b } WHERE {
  <http://ex/author/1235> ex:coauthor ?b
}`
      }
    ],
    typed: [
      {
        family: "Summary",
        label: "Count knows edges",
        strategy: "progressive",
        view: "table",
        tip: "Exact social-edge count from summary predicate totals.",
        q: `PREFIX ex: <http://ex/>
SELECT (COUNT(*) AS ?knowsEdges) WHERE { ?s ex:knows ?o }`
      },
      {
        family: "Select",
        label: "Who works where",
        view: "table",
        tip: "Lists employment edges between people and organizations.",
        q: `PREFIX ex: <http://ex/>
SELECT ?person ?org WHERE { ?person ex:worksAt ?org }`
      },
      {
        family: "Select",
        label: "Who knows whom",
        view: "table",
        tip: "Lists direct social links.",
        q: `PREFIX ex: <http://ex/>
SELECT ?a ?b WHERE { ?a ex:knows ?b }`
      },
      {
        family: "Construct",
        label: "Social graph",
        view: "graph",
        tip: "Draws the knows and worksAt graph.",
        q: `PREFIX ex: <http://ex/>
CONSTRUCT { ?a ?p ?b } WHERE {
  { ?a ex:knows ?b BIND(ex:knows AS ?p) }
  UNION
  { ?a ex:worksAt ?b BIND(ex:worksAt AS ?p) }
}`
      }
    ],
    deps: [
      {
        family: "Summary",
        label: "Count dependency edges",
        strategy: "progressive",
        view: "table",
        tip: "Exact dependsOn count from the summary.",
        q: `PREFIX ex: <http://ex/>
SELECT (COUNT(*) AS ?dependencyEdges) WHERE { ?s ex:dependsOn ?o }`
      },
      {
        family: "Select",
        label: "Direct dependencies",
        view: "table",
        tip: "Lists direct package dependency edges.",
        q: `PREFIX ex: <http://ex/>
SELECT ?package ?dependency WHERE { ?package ex:dependsOn ?dependency } LIMIT 100`
      },
      {
        family: "Path",
        label: "Blast radius of log4x",
        view: "table",
        tip: "Packages transitively depending on the vulnerable component.",
        q: `PREFIX ex: <http://ex/>
SELECT DISTINCT ?dependent WHERE { ?dependent ex:dependsOn+ ex:log4x }`
      },
      {
        family: "Aggregate",
        label: "Dependencies per package",
        view: "table",
        tip: "Direct dependency fan-out by package.",
        q: `PREFIX ex: <http://ex/>
SELECT ?package (COUNT(?dependency) AS ?deps) WHERE {
  ?package ex:dependsOn ?dependency
} GROUP BY ?package ORDER BY DESC(?deps)`
      },
      {
        family: "Construct",
        label: "Dependency graph",
        view: "graph",
        tip: "Draws the dependency graph.",
        q: `PREFIX ex: <http://ex/>
CONSTRUCT { ?a ex:dependsOn ?b } WHERE { ?a ex:dependsOn ?b }`
      }
    ],
    history: [
      {
        family: "Geo",
        label: "Who ruled Paris in 1914?",
        view: "table",
        tip: "GeoSPARQL point-in-polygon: geof:sfContains tests the (2.35, 48.85) point against every 1914 border polygon — a temporal filter (year = 1914) and a spatial predicate composed in one FILTER.",
        q: `PREFIX geo: <http://www.opengis.net/ont/geosparql#>
PREFIX geof: <http://www.opengis.net/def/function/geosparql/>
PREFIX ex: <http://ex/>
PREFIX rdfs: <http://www.w3.org/2000/01/rdf-schema#>
SELECT ?territory WHERE {
  ?t ex:year 1914 ; rdfs:label ?territory ;
     geo:hasGeometry/geo:asWKT ?w .
  FILTER(geof:sfContains(?w, "POINT(2.35 48.85)"^^geo:wktLiteral))
}`
      },
      {
        family: "Geo",
        label: "Empires over Beijing through time",
        view: "table",
        tip: "The same point (116.4, 39.9) against every era's borders — watch the polity change across snapshots (Liao, Jin, Yuan, Ming, Qing, …).",
        q: `PREFIX geo: <http://www.opengis.net/ont/geosparql#>
PREFIX geof: <http://www.opengis.net/def/function/geosparql/>
PREFIX ex: <http://ex/>
PREFIX rdfs: <http://www.w3.org/2000/01/rdf-schema#>
SELECT ?year ?territory WHERE {
  ?t ex:year ?year ; rdfs:label ?territory ;
     geo:hasGeometry/geo:asWKT ?w .
  FILTER(geof:sfContains(?w, "POINT(116.4 39.9)"^^geo:wktLiteral))
} ORDER BY ?year`
      },
      {
        family: "Geo",
        label: "Territories around the British Isles (1815)",
        view: "table",
        tip: "geof:sfIntersects against a bounding-box polygon — every 1815 territory whose borders overlap a box drawn around the British Isles.",
        q: `PREFIX geo: <http://www.opengis.net/ont/geosparql#>
PREFIX geof: <http://www.opengis.net/def/function/geosparql/>
PREFIX ex: <http://ex/>
PREFIX rdfs: <http://www.w3.org/2000/01/rdf-schema#>
SELECT ?territory WHERE {
  ?t ex:year 1815 ; rdfs:label ?territory ;
     geo:hasGeometry/geo:asWKT ?w .
  FILTER(geof:sfIntersects(?w,
    "POLYGON((-11 49, 2 49, 2 61, -11 61, -11 49))"^^geo:wktLiteral))
}`
      },
      {
        family: "Geo",
        label: "Nearest neighbours of London, 1914",
        view: "table",
        tip: "geof:distance returns metres (haversine on the closest point of each border); divide by 1000 for km and sort — the territories nearest a point over London in 1914.",
        q: `PREFIX geo: <http://www.opengis.net/ont/geosparql#>
PREFIX geof: <http://www.opengis.net/def/function/geosparql/>
PREFIX uom: <http://www.opengis.net/def/uom/OGC/1.0/>
PREFIX ex: <http://ex/>
PREFIX rdfs: <http://www.w3.org/2000/01/rdf-schema#>
SELECT ?territory ?km WHERE {
  ?t ex:year 1914 ; rdfs:label ?territory ;
     geo:hasGeometry/geo:asWKT ?w .
  BIND(geof:distance(?w, "POINT(0 51)"^^geo:wktLiteral, uom:metre) / 1000 AS ?km)
} ORDER BY ?km LIMIT 8`
      },
      {
        family: "Geo",
        label: "Bounding box of each 1492 territory",
        view: "table",
        tip: "geof:envelope returns each polygon's axis-aligned bounding box as a new geo:wktLiteral — a cheap spatial summary.",
        q: `PREFIX geo: <http://www.opengis.net/ont/geosparql#>
PREFIX geof: <http://www.opengis.net/def/function/geosparql/>
PREFIX ex: <http://ex/>
PREFIX rdfs: <http://www.w3.org/2000/01/rdf-schema#>
SELECT ?territory ?bbox WHERE {
  ?t ex:year 1492 ; rdfs:label ?territory ;
     geo:hasGeometry/geo:asWKT ?w .
  BIND(geof:envelope(?w) AS ?bbox)
} LIMIT 12`
      },
      {
        family: "Aggregate",
        label: "Territories per era",
        view: "table",
        tip: "The temporal axis: how many mapped territories each snapshot carries (capped at the 90 largest per era in this demo build).",
        q: `PREFIX ex: <http://ex/>
SELECT ?year (COUNT(*) AS ?territories) WHERE {
  ?t ex:year ?year
} GROUP BY ?year ORDER BY ?year`
      }
    ],
    "linked-jazz": [{"family": "Summary","label": "Relationship-type totals","view": "table","tip": "How the jazz community is wired: knowsOf dominates (3,649 edges), then the typed ties influencedBy / collaborated_with / mentorOf. foaf:name and dbo:thumbnail are the per-person metadata.","q": "SELECT ?p (COUNT(*) AS ?triples) WHERE { ?s ?p ?o } GROUP BY ?p ORDER BY DESC(?triples)"},{"family": "Select","label": "Everything Mary Lou Williams said about people","view": "table","tip": "A bound subject (one of the 54 interviewed musicians) returns her whole ego: 216 facts, including 151 knowsOf links plus mentorOf, friendOf and playedTogether ties pulled from her oral-history transcript.","q": "PREFIX foaf: <http://xmlns.com/foaf/0.1/>\nSELECT ?rel ?name WHERE {\n  <http://dbpedia.org/resource/Mary_Lou_Williams> ?rel ?other .\n  ?other foaf:name ?name .\n} ORDER BY ?rel LIMIT 100"},{"family": "Aggregate","label": "Most talked-about musicians","view": "table","tip": "In-degree across every social predicate, joined to names. Count Basie tops the network at 165 mentions, then Louis Armstrong (117) and Duke Ellington (72) - the gravitational centres of jazz memory.","q": "PREFIX foaf: <http://xmlns.com/foaf/0.1/>\nSELECT ?name (COUNT(?s) AS ?mentions) WHERE {\n  ?s ?rel ?person .\n  FILTER(?rel != foaf:name && ?rel != <http://dbpedia.org/ontology/thumbnail>)\n  ?person foaf:name ?name .\n} GROUP BY ?name ORDER BY DESC(?mentions) LIMIT 15"},{"family": "Path","label": "Who Count Basie reaches by word of mouth","view": "graph","tip": "A transitive rel:knowsOf+ closure from a hub that is both a major subject and the #1 object. Because 40 of the 54 ego-musicians cross-reference each other, the chain hops Basie -> his circle -> their circles across the network.","q": "PREFIX rel: <http://purl.org/vocab/relationship/>\nPREFIX foaf: <http://xmlns.com/foaf/0.1/>\nSELECT DISTINCT ?name WHERE {\n  <http://dbpedia.org/resource/Count_Basie> rel:knowsOf+ ?reached .\n  ?reached foaf:name ?name .\n} ORDER BY ?name LIMIT 100"},{"family": "Aggregate","label": "Most-cited influences","view": "table","tip": "Restricting to rel:influencedBy reveals the acknowledged masters: Louis Armstrong (21), Count Basie (17) and Duke Ellington (11) are named most often as the people who shaped others.","q": "PREFIX foaf: <http://xmlns.com/foaf/0.1/>\nPREFIX rel: <http://purl.org/vocab/relationship/>\nSELECT ?name (COUNT(?s) AS ?cited) WHERE {\n  ?s rel:influencedBy ?infl .\n  ?infl foaf:name ?name .\n} GROUP BY ?name ORDER BY DESC(?cited) LIMIT 12"},{"family": "Construct","label": "Mary Lou Williams collaboration ego-network","view": "graph","tip": "Builds a drawable subgraph of who she actually made music with - mo:collaborated_with + lj:playedTogether edges (14 of them) - then re-labels each node so the renderer shows real names instead of IRIs.","q": "PREFIX foaf: <http://xmlns.com/foaf/0.1/>\nPREFIX mo: <http://purl.org/ontology/mo/>\nPREFIX lj: <http://linkedjazz.org/ontology/>\nCONSTRUCT {\n  <http://dbpedia.org/resource/Mary_Lou_Williams> ?p ?other .\n  ?other foaf:name ?name .\n} WHERE {\n  <http://dbpedia.org/resource/Mary_Lou_Williams> ?p ?other .\n  FILTER(?p = mo:collaborated_with || ?p = lj:playedTogether)\n  ?other foaf:name ?name .\n}"}],
    "getty-ulan": [{"family": "Select","label": "The pupils of Rembrandt","view": "graph","tip": "Bound-subject star: everyone Rembrandt (ulan:500011051) is recorded as teacher of, with their bios. The selective shape lazy access wants.","q": "PREFIX gvp: <http://vocab.getty.edu/ontology#>\nPREFIX skos: <http://www.w3.org/2004/02/skos/core#>\nPREFIX schema: <http://schema.org/>\nPREFIX ulan: <http://vocab.getty.edu/ulan/>\nSELECT ?pupil ?name ?bio WHERE {\n  ulan:500011051 gvp:teacherOf ?pupil .\n  OPTIONAL { ?pupil skos:prefLabel ?name }\n  OPTIONAL { ?pupil schema:description ?bio }\n}"},{"family": "Path","label": "Artistic descendants of Rembrandt (transitive)","view": "graph","tip": "Follows gvp:teacherOf+ down the master->pupil lineage from one bound seed - returns ~369 descendants.","q": "PREFIX gvp: <http://vocab.getty.edu/ontology#>\nPREFIX skos: <http://www.w3.org/2004/02/skos/core#>\nPREFIX ulan: <http://vocab.getty.edu/ulan/>\nSELECT DISTINCT ?descendant ?name WHERE {\n  ulan:500011051 gvp:teacherOf+ ?descendant .\n  OPTIONAL { ?descendant skos:prefLabel ?name }\n} LIMIT 500"},{"family": "Path","label": "Two-generation teaching chains","view": "graph","tip": "master -> pupil -> grand-pupil triples: how technique passed down two academic generations.","q": "PREFIX gvp: <http://vocab.getty.edu/ontology#>\nPREFIX skos: <http://www.w3.org/2004/02/skos/core#>\nSELECT ?masterName ?pupilName ?grandPupilName WHERE {\n  ?master gvp:teacherOf ?pupil . ?pupil gvp:teacherOf ?grandPupil .\n  ?master skos:prefLabel ?masterName . ?pupil skos:prefLabel ?pupilName . ?grandPupil skos:prefLabel ?grandPupilName .\n} LIMIT 100"},{"family": "Aggregate","label": "Most prolific teachers","view": "table","tip": "Ranks masters by number of recorded pupils - surfaces the great academic studios (top result taught 163 students).","q": "PREFIX gvp: <http://vocab.getty.edu/ontology#>\nPREFIX skos: <http://www.w3.org/2004/02/skos/core#>\nPREFIX schema: <http://schema.org/>\nSELECT ?teacher ?name ?bio (COUNT(?pupil) AS ?pupils) WHERE {\n  ?teacher gvp:teacherOf ?pupil .\n  OPTIONAL { ?teacher skos:prefLabel ?name } OPTIONAL { ?teacher schema:description ?bio }\n} GROUP BY ?teacher ?name ?bio ORDER BY DESC(?pupils) LIMIT 25"},{"family": "Aggregate","label": "Teaching ties by nationality","view": "table","tip": "Counts master->pupil edges grouped by the teacher's nationality - which national schools dominate the lineage.","q": "PREFIX gvp: <http://vocab.getty.edu/ontology#>\nSELECT ?nationality (COUNT(*) AS ?teachingLinks) WHERE {\n  ?teacher gvp:teacherOf ?pupil ; gvp:nationality ?nationality .\n} GROUP BY ?nationality ORDER BY DESC(?teachingLinks) LIMIT 25"}],
    "nomisma": [{"family": "Summary","label": "Shape of the corpus","view": "table","tip": "Predicate totals over the whole graph: ~7.2k coin types each carrying a label, dates, material, denomination, and (mostly) a mint and authority. 9 predicates.","q": "SELECT ?p (COUNT(*) AS ?triples) WHERE { ?s ?p ?o } GROUP BY ?p ORDER BY DESC(?triples)"},{"family": "Select","label": "Silver tetradrachms of Alexander the Great","view": "table","tip": "Bound-authority + bound-material + bound-denomination star, joined to mint labels: the famous AR tetradrachms of Alexander III, by mint (Abydus, Aegae, Amphipolis...).","q": "PREFIX nmo: <http://nomisma.org/ontology#>\nPREFIX nm: <http://nomisma.org/id/>\nPREFIX rdfs: <http://www.w3.org/2000/01/rdf-schema#>\nSELECT ?type ?label ?mintName WHERE {\n  ?type a nmo:TypeSeriesItem ; rdfs:label ?label ;\n        nmo:hasAuthority nm:alexander_iii ; nmo:hasMaterial nm:ar ; nmo:hasDenomination nm:tetradrachm ; nmo:hasMint ?mint .\n  ?mint rdfs:label ?mintName .\n} ORDER BY ?mintName LIMIT 50"},{"family": "Aggregate","label": "Most prolific mints","view": "table","tip": "GROUP BY mint: the Macedonian capital Pella (1501 types) and Amphipolis (1266) dominate, then the eastern conquests Babylon and Sardis.","q": "PREFIX nmo: <http://nomisma.org/ontology#>\nPREFIX rdfs: <http://www.w3.org/2000/01/rdf-schema#>\nSELECT ?mintName (COUNT(?type) AS ?types) WHERE {\n  ?type a nmo:TypeSeriesItem ; nmo:hasMint ?mint .\n  ?mint rdfs:label ?mintName .\n} GROUP BY ?mintName ORDER BY DESC(?types) LIMIT 15"},{"family": "Aggregate","label": "Coin types per issuing authority","view": "table","tip": "The cast of the Macedonian story by output: Alexander III (1972), Philip III Arrhidaeus (1025), Philip II (480), then the Diadochi Cassander, Lysimachus and Ptolemy I.","q": "PREFIX nmo: <http://nomisma.org/ontology#>\nPREFIX rdfs: <http://www.w3.org/2000/01/rdf-schema#>\nSELECT ?authName (COUNT(?type) AS ?types) WHERE {\n  ?type a nmo:TypeSeriesItem ; nmo:hasAuthority ?auth .\n  ?auth rdfs:label ?authName .\n} GROUP BY ?authName ORDER BY DESC(?types)"},{"family": "Path","label": "Mints used by 3+ successive rulers","view": "table","tip": "Two-hop join through a shared mint reveals political continuity: Amphipolis was struck by FOUR rulers (Philip II -> Alexander III -> Philip III -> Cassander). HAVING + GROUP_CONCAT name them.","q": "PREFIX nmo: <http://nomisma.org/ontology#>\nPREFIX rdfs: <http://www.w3.org/2000/01/rdf-schema#>\nSELECT ?mintName (COUNT(DISTINCT ?auth) AS ?rulers) (GROUP_CONCAT(DISTINCT ?authName; SEPARATOR=\", \") AS ?who) WHERE {\n  ?type a nmo:TypeSeriesItem ; nmo:hasMint ?mint ; nmo:hasAuthority ?auth .\n  ?mint rdfs:label ?mintName . ?auth rdfs:label ?authName .\n} GROUP BY ?mintName HAVING(COUNT(DISTINCT ?auth) >= 3) ORDER BY DESC(?rulers)"},{"family": "Construct","label": "Who else struck at Cassander's mints","view": "graph","tip": "Builds a compact succession star: every authority that minted at a city Cassander also used. Centres on Amphipolis, linking Philip II, Alexander III, Philip III and Cassander - the whole Macedonian dynastic line in one mint.","q": "PREFIX nmo: <http://nomisma.org/ontology#>\nPREFIX nm: <http://nomisma.org/id/>\nCONSTRUCT { ?auth nmo:hasMint ?mint . } WHERE {\n  ?tc a nmo:TypeSeriesItem ; nmo:hasAuthority nm:cassander ; nmo:hasMint ?mint .\n  ?t a nmo:TypeSeriesItem ; nmo:hasMint ?mint ; nmo:hasAuthority ?auth .\n}"}],
    "mimotext": [{"family": "Summary","label": "What is in this graph?","view": "table","tip": "Counts the main entity kinds (literary works, people, themes, spatial concepts) so you see the shape of the literary network at a glance.","q": "PREFIX wdt: <http://data.mimotext.uni-trier.de/prop/direct/>\nPREFIX wd:  <http://data.mimotext.uni-trier.de/entity/>\nSELECT ?kind (COUNT(DISTINCT ?x) AS ?n) WHERE {\n  VALUES (?class ?kind) { (wd:Q2 \"literary work\") (wd:Q10 \"person\") (wd:Q20 \"thematic concept\") (wd:Q26 \"spatial concept\") }\n  ?x wdt:P2 ?class .\n} GROUP BY ?kind ORDER BY DESC(?n)"},{"family": "Aggregate","label": "Most common themes across the novels","view": "table","tip": "Ranks thematic concepts (P36 'about') by how many distinct novels treat them - the dominant motifs of the Enlightenment novel (sentiment, sentimentalism, travel, love).","q": "PREFIX wdt: <http://data.mimotext.uni-trier.de/prop/direct/>\nPREFIX rdfs:<http://www.w3.org/2000/01/rdf-schema#>\nSELECT ?theme (COUNT(DISTINCT ?work) AS ?novels) WHERE {\n  ?work wdt:P36 ?t . ?t rdfs:label ?theme . FILTER(LANG(?theme)=\"en\")\n} GROUP BY ?theme ORDER BY DESC(?novels) LIMIT 15"},{"family": "Select","label": "Voltaire's novels, by year, with genre","view": "table","tip": "Lists one author's works with publication year (from xsd:dateTime P9) and genre. Names are stored 'SURNAME, Given', so the match uses CONTAINS.","q": "PREFIX wdt: <http://data.mimotext.uni-trier.de/prop/direct/>\nPREFIX rdfs:<http://www.w3.org/2000/01/rdf-schema#>\nSELECT ?work ?year ?genre WHERE {\n  ?author rdfs:label ?aName . FILTER(LANG(?aName)=\"en\" && CONTAINS(?aName,\"VOLTAIRE\"))\n  ?w wdt:P5 ?author ; rdfs:label ?work . FILTER(LANG(?work)=\"en\")\n  OPTIONAL { ?w wdt:P9 ?d . BIND(YEAR(?d) AS ?year) }\n  OPTIONAL { ?w wdt:P12 ?g . ?g rdfs:label ?genre FILTER(LANG(?genre)=\"en\") }\n} ORDER BY ?year"},{"family": "Aggregate","label": "Novels that share the most themes","view": "table","tip": "Finds the pair of novels with the largest overlap of shared thematic concepts - a thematic 'bridge' between two books.","q": "PREFIX wdt: <http://data.mimotext.uni-trier.de/prop/direct/>\nPREFIX rdfs:<http://www.w3.org/2000/01/rdf-schema#>\nSELECT ?novelA ?novelB (COUNT(?t) AS ?sharedThemes) WHERE {\n  ?a wdt:P36 ?t . ?b wdt:P36 ?t . FILTER(STR(?a) < STR(?b))\n  ?a rdfs:label ?novelA FILTER(LANG(?novelA)=\"en\")\n  ?b rdfs:label ?novelB FILTER(LANG(?novelB)=\"en\")\n} GROUP BY ?novelA ?novelB ORDER BY DESC(?sharedThemes) LIMIT 10"},{"family": "Select","label": "Stylometric nearest neighbours of Candide","view": "table","tip": "Reads the computed P49 stylometric-similarity edges and their P52 distance values to rank the novels closest in writing style to Candide (they turn out to be other Voltaire works: L'Ingenu 0.745, Histoire de Jenni 0.743).","q": "PREFIX p:   <http://data.mimotext.uni-trier.de/prop/>\nPREFIX ps:  <http://data.mimotext.uni-trier.de/prop/statement/>\nPREFIX pq:  <http://data.mimotext.uni-trier.de/prop/qualifier/>\nPREFIX rdfs:<http://www.w3.org/2000/01/rdf-schema#>\nSELECT ?other ?distance WHERE {\n  ?a rdfs:label ?aL . FILTER(LANG(?aL)=\"en\" && CONTAINS(?aL,\"Candide\"))\n  ?a p:P49 ?st . ?st ps:P49 ?b ; pq:P52 ?distance .\n  ?b rdfs:label ?other FILTER(LANG(?other)=\"en\")\n} ORDER BY DESC(?distance) LIMIT 10"},{"family": "Construct","label": "Author ego-network: Voltaire -> works -> themes","view": "graph","tip": "Builds a small subgraph of one author, their novels, and the themes those novels treat - rendered as a node-link diagram.","q": "PREFIX wdt: <http://data.mimotext.uni-trier.de/prop/direct/>\nPREFIX rdfs:<http://www.w3.org/2000/01/rdf-schema#>\nCONSTRUCT {\n  ?author wdt:P7 ?work . ?work wdt:P36 ?theme .\n  ?author rdfs:label ?aL . ?work rdfs:label ?wL . ?theme rdfs:label ?tL .\n} WHERE {\n  ?author rdfs:label ?aL . FILTER(LANG(?aL)=\"en\" && CONTAINS(?aL,\"VOLTAIRE\"))\n  ?work wdt:P5 ?author ; rdfs:label ?wL . FILTER(LANG(?wL)=\"en\")\n  ?work wdt:P36 ?theme . ?theme rdfs:label ?tL FILTER(LANG(?tL)=\"en\")\n}"}],
    "mmm": [{"family": "Summary","label": "Predicate totals","view": "table","tip": "Shape of the provenance graph in one shot: ownership links, works, production places and dates. 10 distinct predicates over 48,191 triples.","q": "SELECT ?p (COUNT(*) AS ?triples) WHERE { ?s ?p ?o } GROUP BY ?p ORDER BY DESC(?triples)"},{"family": "Select","label": "Manuscripts made in Florence, with date and text","view": "table","tip": "Bound-object slice on mmm:produced_in (Florence = TGN 7000457): each manuscript's label, production date-range and the work it carries. 1,484 manuscripts match.","q": "PREFIX mmm: <http://ldf.fi/schema/mmm/>\nPREFIX skos: <http://www.w3.org/2004/02/skos/core#>\nSELECT ?ms ?label ?date ?workTitle WHERE {\n  ?ms mmm:produced_in <http://ldf.fi/mmm/place/tgn_7000457> ; skos:prefLabel ?label .\n  OPTIONAL { ?ms mmm:produced_when ?date }\n  OPTIONAL { ?ms mmm:manuscript_work/skos:prefLabel ?workTitle }\n} LIMIT 100"},{"family": "Aggregate","label": "Most-traded manuscripts (owner counts)","view": "table","tip": "Counts crm:P51 former/current owners per manuscript - the ones that changed hands most. Top results have 20 recorded owners.","q": "PREFIX crm: <http://erlangen-crm.org/current/>\nPREFIX skos: <http://www.w3.org/2004/02/skos/core#>\nSELECT ?ms ?label (COUNT(?owner) AS ?owners) WHERE {\n  ?ms crm:P51_has_former_or_current_owner ?owner ; skos:prefLabel ?label .\n} GROUP BY ?ms ?label ORDER BY DESC(?owners) LIMIT 15"},{"family": "Aggregate","label": "Books per production city + coordinates","view": "table","tip": "Groups manuscripts by production city and pulls the WGS84 point - Florence 1484, Bruges 693, Rouen 517, Tours 469. The coords let a map/graph view place each scriptorium.","q": "PREFIX mmm: <http://ldf.fi/schema/mmm/>\nPREFIX skos: <http://www.w3.org/2004/02/skos/core#>\nPREFIX wgs: <http://www.w3.org/2003/01/geo/wgs84_pos#>\nSELECT ?city ?lat ?long (COUNT(?ms) AS ?manuscripts) WHERE {\n  ?ms mmm:produced_in ?place . ?place skos:prefLabel ?city .\n  OPTIONAL { ?place wgs:lat ?lat ; wgs:long ?long }\n} GROUP BY ?city ?lat ?long ORDER BY DESC(?manuscripts)"},{"family": "Path","label": "Owners of Florentine manuscripts (named persons)","view": "table","tip": "A two-step join: manuscripts produced in Florence, their owners, and only owners typed as E21_Person (named historical people, with gender). Surfaces real provenance names like Philippe de Crevecoeur.","q": "PREFIX crm: <http://erlangen-crm.org/current/>\nPREFIX mmm: <http://ldf.fi/schema/mmm/>\nPREFIX skos: <http://www.w3.org/2004/02/skos/core#>\nSELECT DISTINCT ?ownerName ?gender ?ms WHERE {\n  ?ms mmm:produced_in <http://ldf.fi/mmm/place/tgn_7000457> ; crm:P51_has_former_or_current_owner ?owner .\n  ?owner a crm:E21_Person ; skos:prefLabel ?ownerName .\n  OPTIONAL { ?owner mmm:gender ?gender }\n} LIMIT 100"},{"family": "Construct","label": "Provenance ego-network of one manuscript","view": "graph","tip": "Builds a small star graph around one manuscript (bibale_12101, 8 owners, made in Florence): its production city, its owners, and the work it carries.","q": "PREFIX crm: <http://erlangen-crm.org/current/>\nPREFIX mmm: <http://ldf.fi/schema/mmm/>\nPREFIX skos: <http://www.w3.org/2004/02/skos/core#>\nCONSTRUCT {\n  ?ms mmm:produced_in ?cityName ; crm:P51_has_former_or_current_owner ?ownerName ; mmm:manuscript_work ?workTitle .\n} WHERE {\n  BIND(<http://ldf.fi/mmm/manifestation_singleton/bibale_12101> AS ?ms)\n  OPTIONAL { ?ms mmm:produced_in/skos:prefLabel ?cityName }\n  OPTIONAL { ?ms crm:P51_has_former_or_current_owner/skos:prefLabel ?ownerName }\n  OPTIONAL { ?ms mmm:manuscript_work/skos:prefLabel ?workTitle }\n}"}],
    "openalex-astrocytes": [{"family": "Summary", "label": "What's in the graph", "view": "table", "tip": "Predicate totals: ex:author and ex:topic edges dominate, with 4,113 cito:cites citation links and the dct:title / ex:citationCount paper metadata.", "q": "SELECT ?p (COUNT(*) AS ?n) WHERE { ?s ?p ?o } GROUP BY ?p ORDER BY DESC(?n)"}, {"family": "Aggregate", "label": "Most-cited astrocyte papers", "view": "table", "tip": "ex:citationCount is the global OpenAlex citation count; Liddelow 2017 'Neurotoxic reactive astrocytes' tops the field.", "q": "PREFIX dct: <http://purl.org/dc/terms/>\nPREFIX ex: <http://ex/>\nSELECT ?title ?c WHERE { ?w a ex:Work ; dct:title ?title ; ex:citationCount ?c } ORDER BY DESC(?c) LIMIT 15"}, {"family": "Aggregate", "label": "Most prolific astrocyte authors", "view": "table", "tip": "Count papers per author across the citation core - the researchers who define the field.", "q": "PREFIX ex: <http://ex/>\nPREFIX foaf: <http://xmlns.com/foaf/0.1/>\nSELECT ?name (COUNT(?w) AS ?papers) WHERE { ?w ex:author ?a . ?a foaf:name ?name } GROUP BY ?name ORDER BY DESC(?papers) LIMIT 15"}, {"family": "Aggregate", "label": "Leading institutions", "view": "table", "tip": "Join author -> ex:affiliation -> institution label; which labs/universities produce the most astrocyte research.", "q": "PREFIX ex: <http://ex/>\nPREFIX rdfs: <http://www.w3.org/2000/01/rdf-schema#>\nSELECT ?inst (COUNT(DISTINCT ?w) AS ?papers) WHERE { ?w ex:author ?a . ?a ex:affiliation ?i . ?i rdfs:label ?inst } GROUP BY ?inst ORDER BY DESC(?papers) LIMIT 15"}, {"family": "Aggregate", "label": "Adjacent sub-topics", "view": "table", "tip": "The OpenAlex concepts co-tagged with these papers - the neighbouring fields astrocyte research bridges into.", "q": "PREFIX ex: <http://ex/>\nPREFIX rdfs: <http://www.w3.org/2000/01/rdf-schema#>\nSELECT ?topic (COUNT(?w) AS ?papers) WHERE { ?w ex:topic ?t . ?t rdfs:label ?topic } GROUP BY ?topic ORDER BY DESC(?papers) LIMIT 20"}, {"family": "Path", "label": "Who cites the field's landmark paper", "view": "table", "tip": "Reverse cito:cites against Liddelow 2017 (W2572710398): every paper in the core that cites the most-cited astrocyte study, newest first.", "q": "PREFIX cito: <http://purl.org/spar/cito/>\nPREFIX dct: <http://purl.org/dc/terms/>\nPREFIX ex: <http://ex/>\nSELECT ?title ?year WHERE { ?citing cito:cites <https://openalex.org/W2572710398> ; dct:title ?title ; ex:year ?year } ORDER BY DESC(?year) LIMIT 50"}, {"family": "Construct", "label": "Citation network of the top papers", "view": "graph", "tip": "Draws cito:cites among the most-cited works (citationCount > 1500) - the backbone of the astrocyte literature.", "q": "PREFIX cito: <http://purl.org/spar/cito/>\nPREFIX ex: <http://ex/>\nCONSTRUCT { ?a cito:cites ?b } WHERE { ?a cito:cites ?b . ?a ex:citationCount ?ca . FILTER(?ca > 1500) . ?b ex:citationCount ?cb . FILTER(?cb > 1500) }"}],
    "antarctic-expeditions": [{"family": "Summary", "label": "Shape of the expedition graph", "view": "table", "tip": "Predicate totals: ex:participant dominates (~76 crew edges), then ex:vessel / ex:leader, plus rdfs:label and ex:startYear/ex:endYear. Shows the 6-expedition / 5-ship / ~76-person skeleton at a glance.", "q": "SELECT ?p (COUNT(*) AS ?triples) WHERE { ?s ?p ?o } GROUP BY ?p ORDER BY DESC(?triples)"}, {"family": "Select", "label": "The crew of the Endurance", "view": "graph", "tip": "Bound-subject star on Shackleton's Endurance (Q1162294): every ex:participant with their name. Includes Mrs. Chippy, the ship's cat, a genuine P710 participant.", "q": "PREFIX ex: <http://ex/>\nPREFIX wd: <http://www.wikidata.org/entity/>\nPREFIX rdfs: <http://www.w3.org/2000/01/rdf-schema#>\nSELECT ?person ?name WHERE {\n  wd:Q1162294 ex:participant ?person .\n  ?person rdfs:label ?name .\n} ORDER BY ?name"}, {"family": "Path", "label": "Crew who served on more than one expedition", "view": "table", "tip": "Self-join through a shared ex:participant: people linked to two different expeditions are the network bridges (men who sailed with both Scott and Shackleton). STR(?e1)<STR(?e2) dedups the pair.", "q": "PREFIX ex: <http://ex/>\nPREFIX rdfs: <http://www.w3.org/2000/01/rdf-schema#>\nSELECT ?name ?exp1 ?exp2 WHERE {\n  ?e1 ex:participant ?p ; rdfs:label ?exp1 .\n  ?e2 ex:participant ?p ; rdfs:label ?exp2 .\n  FILTER(STR(?e1) < STR(?e2))\n  ?p rdfs:label ?name .\n} ORDER BY ?name"}, {"family": "Aggregate", "label": "Largest crews", "view": "table", "tip": "GROUP BY expedition counting ex:participant edges, joined to label and years — ranks the voyages by recorded crew size.", "q": "PREFIX ex: <http://ex/>\nPREFIX rdfs: <http://www.w3.org/2000/01/rdf-schema#>\nSELECT ?expedition ?startYear (COUNT(?p) AS ?crew) WHERE {\n  ?e ex:participant ?p ; rdfs:label ?expedition ; ex:startYear ?startYear .\n} GROUP BY ?expedition ?startYear ORDER BY DESC(?crew)"}, {"family": "Construct", "label": "Expedition -> leader + ship + crew ego-network", "view": "graph", "tip": "Builds one drawable star around an expedition (Terra Nova, Q973919): its leader, its vessel, its crew, each re-labelled so the renderer shows real names instead of QIDs.", "q": "PREFIX ex: <http://ex/>\nPREFIX wd: <http://www.wikidata.org/entity/>\nPREFIX rdfs: <http://www.w3.org/2000/01/rdf-schema#>\nCONSTRUCT {\n  wd:Q973919 ?p ?o . ?o rdfs:label ?name .\n} WHERE {\n  wd:Q973919 ?p ?o .\n  FILTER(?p = ex:leader || ?p = ex:vessel || ?p = ex:participant)\n  ?o rdfs:label ?name .\n}"}],
    "factgrid-illuminati": [{"family": "Summary", "label": "What's in the graph", "view": "table", "tip": "Predicate totals: P91 (member of), P2 (instance of) and the other FactGrid properties on each member.", "q": "SELECT ?p (COUNT(*) AS ?n) WHERE { ?s ?p ?o } GROUP BY ?p ORDER BY DESC(?n) LIMIT 20"}, {"family": "Select", "label": "Members of the Illuminati", "view": "table", "tip": "Everyone linked by P91 (member of) to the Order of the Illuminati (Q10677).", "q": "PREFIX wdt: <https://database.factgrid.de/prop/direct/>\nPREFIX rdfs: <http://www.w3.org/2000/01/rdf-schema#>\nSELECT ?name WHERE { ?m wdt:P91 <https://database.factgrid.de/entity/Q10677> ; rdfs:label ?name } ORDER BY ?name LIMIT 200"}, {"family": "Aggregate", "label": "Which properties describe members", "view": "table", "tip": "Group every fact about the members by predicate and show the FactGrid property label - the schema in plain language.", "q": "PREFIX wdt: <https://database.factgrid.de/prop/direct/>\nPREFIX rdfs: <http://www.w3.org/2000/01/rdf-schema#>\nSELECT ?plabel (COUNT(*) AS ?n) WHERE { ?m wdt:P91 <https://database.factgrid.de/entity/Q10677> . ?m ?p ?o . OPTIONAL { ?p rdfs:label ?plabel } } GROUP BY ?plabel ORDER BY DESC(?n)"}],
    "theographic-graph": [{"family": "Summary", "label": "What's in the graph", "view": "table", "tip": "Predicate totals: tg:sibling/child/father kinship, foaf:gender, places with wgs84 coordinates, events.", "q": "SELECT ?p (COUNT(*) AS ?n) WHERE { ?s ?p ?o } GROUP BY ?p ORDER BY DESC(?n) LIMIT 20"}, {"family": "Path", "label": "Descendants of Abraham", "view": "table", "tip": "Transitive tg:child+ from Abraham - the genealogical tree the text traces from the patriarch.", "q": "PREFIX tg: <http://theographic/ontology#>\nPREFIX rdfs: <http://www.w3.org/2000/01/rdf-schema#>\nSELECT DISTINCT ?name WHERE { <http://ex/person/abraham_58> tg:child+ ?d . ?d rdfs:label ?name } LIMIT 200"}, {"family": "Aggregate", "label": "Who had the most children", "view": "table", "tip": "Count tg:child edges per person - the prolific patriarchs and kings.", "q": "PREFIX tg: <http://theographic/ontology#>\nPREFIX rdfs: <http://www.w3.org/2000/01/rdf-schema#>\nSELECT ?name (COUNT(?c) AS ?children) WHERE { ?p tg:child ?c ; rdfs:label ?name } GROUP BY ?name ORDER BY DESC(?children) LIMIT 15"}, {"family": "Construct", "label": "Abraham's children", "view": "graph", "tip": "Child edges one hop from Abraham, relabelled to names for the graph view.", "q": "PREFIX tg: <http://theographic/ontology#>\nPREFIX rdfs: <http://www.w3.org/2000/01/rdf-schema#>\nCONSTRUCT { <http://ex/person/abraham_58> tg:child ?c . ?c rdfs:label ?n } WHERE { <http://ex/person/abraham_58> tg:child ?c . ?c rdfs:label ?n }"}],
    "monarch": [{"family": "Summary", "label": "What's in the graph", "view": "table", "tip": "Predicate totals across the biolink associations: has_phenotype, interacts_with, in_taxon, plus labels.", "q": "SELECT ?p (COUNT(*) AS ?n) WHERE { ?s ?p ?o } GROUP BY ?p ORDER BY DESC(?n) LIMIT 20"}, {"family": "Select", "label": "Phenotypes in the graph", "view": "table", "tip": "Everything typed as a biolink PhenotypicFeature, with its label.", "q": "PREFIX bl: <https://w3id.org/biolink/vocab/>\nPREFIX rdfs: <http://www.w3.org/2000/01/rdf-schema#>\nSELECT ?name WHERE { ?ph a bl:PhenotypicFeature ; rdfs:label ?name } ORDER BY ?name LIMIT 100"}, {"family": "Aggregate", "label": "Most-connected genes", "view": "table", "tip": "Rank genes by their biolink:interacts_with degree - the hubs of the interaction network.", "q": "PREFIX bl: <https://w3id.org/biolink/vocab/>\nPREFIX rdfs: <http://www.w3.org/2000/01/rdf-schema#>\nSELECT ?name (COUNT(?o) AS ?deg) WHERE { ?g bl:interacts_with ?o . ?g rdfs:label ?name } GROUP BY ?name ORDER BY DESC(?deg) LIMIT 15"}],
    "opencitations": [{"family": "Summary", "label": "What's in the graph", "view": "table", "tip": "Predicate totals: cito:cites citation edges and the Dublin Core / FOAF bibliographic metadata.", "q": "SELECT ?p (COUNT(*) AS ?n) WHERE { ?s ?p ?o } GROUP BY ?p ORDER BY DESC(?n) LIMIT 20"}, {"family": "Aggregate", "label": "Most-cited works", "view": "table", "tip": "In-degree over cito:cites joined to dct:title - the references everything points back to.", "q": "PREFIX cito: <http://purl.org/spar/cito/>\nPREFIX dct: <http://purl.org/dc/terms/>\nSELECT ?title (COUNT(?citing) AS ?cites) WHERE { ?citing cito:cites ?w . ?w dct:title ?title } GROUP BY ?title ORDER BY DESC(?cites) LIMIT 15"}, {"family": "Aggregate", "label": "Publications per year", "view": "table", "tip": "Group dct:date (xsd:gYear) to see the time profile of the neighbourhood.", "q": "PREFIX dct: <http://purl.org/dc/terms/>\nSELECT ?year (COUNT(?w) AS ?n) WHERE { ?w dct:date ?year } GROUP BY ?year ORDER BY ?year"}, {"family": "Path", "label": "Citation closure of a seed paper", "view": "table", "tip": "Transitive cito:cites+ from one JAMA article - everything it reaches by following references.", "q": "PREFIX cito: <http://purl.org/spar/cito/>\nSELECT DISTINCT ?w WHERE { <https://doi.org/10.1001/jama.2014.16543> cito:cites+ ?w } LIMIT 100"}],
    "orkg": [{"family": "Summary", "label": "What's in the graph", "view": "table", "tip": "Predicate totals across the ORKG model: papers, contributions, hasAuthors and the Pxx contribution properties.", "q": "SELECT ?p (COUNT(*) AS ?n) WHERE { ?s ?p ?o } GROUP BY ?p ORDER BY DESC(?n) LIMIT 20"}, {"family": "Select", "label": "Papers", "view": "table", "tip": "Everything typed as an ORKG Paper, with its title.", "q": "PREFIX rdfs: <http://www.w3.org/2000/01/rdf-schema#>\nSELECT ?title WHERE { ?p a <https://orkg.org/class/Paper> ; rdfs:label ?title } ORDER BY ?title LIMIT 100"}, {"family": "Aggregate", "label": "Node types in the graph", "view": "table", "tip": "Count subjects per rdf:type - papers vs contributions vs lists vs problems.", "q": "SELECT ?type (COUNT(?s) AS ?n) WHERE { ?s a ?type } GROUP BY ?type ORDER BY DESC(?n) LIMIT 20"}]
  },
  shacl: {
    scholar: [
      {
        label: "Paper integrity",
        tip: "Every paper needs exactly one title, a venue, and a double-typed novelty score - the clean graph conforms.",
        shape: `@prefix ex: <http://ex/> .
@prefix dct: <http://purl.org/dc/terms/> .
@prefix sh: <http://www.w3.org/ns/shacl#> .
@prefix xsd: <http://www.w3.org/2001/XMLSchema#> .

ex:PaperShape
  a sh:NodeShape ;
  sh:targetClass ex:Paper ;
  sh:property [ sh:path dct:title ; sh:minCount 1 ; sh:maxCount 1 ] ;
  sh:property [ sh:path ex:publishedIn ; sh:minCount 1 ] ;
  sh:property [ sh:path ex:noveltyScore ; sh:datatype xsd:double ] .`
      },
      {
        label: "Single keyword only",
        tip: "Intentional violation: papers carry 2-5 keywords by design, so maxCount 1 flags all 240 of them.",
        shape: `@prefix ex: <http://ex/> .
@prefix sh: <http://www.w3.org/ns/shacl#> .

ex:SingleKeywordShape
  a sh:NodeShape ;
  sh:targetClass ex:Paper ;
  sh:property [
    sh:path ex:keyword ;
    sh:maxCount 1 ;
    sh:message "Papers are multi-keyword by design - intentional violation."
  ] .`
      }
    ],
    "scholar-noisy": [
      {
        label: "Author completeness",
        tip: "The noise knob dropped ORCIDs and h-indexes - 27 violations here; the clean graph conforms.",
        shape: `@prefix ex: <http://ex/> .
@prefix sh: <http://www.w3.org/ns/shacl#> .

ex:PersonCompleteShape
  a sh:NodeShape ;
  sh:targetClass ex:Person ;
  sh:property [ sh:path ex:orcid ; sh:minCount 1 ; sh:message "Author has no ORCID." ] ;
  sh:property [ sh:path ex:hIndex ; sh:minCount 1 ; sh:message "Author has no h-index." ] .`
      },
      {
        label: "Journals need an ISSN",
        tip: "Four journals lost their ISSN to the noise knob.",
        shape: `@prefix ex: <http://ex/> .
@prefix sh: <http://www.w3.org/ns/shacl#> .

ex:JournalShape
  a sh:NodeShape ;
  sh:targetClass ex:Journal ;
  sh:property [
    sh:path ex:issn ;
    sh:minCount 1 ;
    sh:message "Journal lost its ISSN."
  ] .`
      }
    ],
    citations: [
      {
        label: "Citing paper metadata",
        tip: "Checks title, date, and discipline for papers with citation edges.",
        shape: `@prefix cito: <http://purl.org/spar/cito/> .
@prefix dct: <http://purl.org/dc/terms/> .
@prefix ex: <http://ex/> .
@prefix sh: <http://www.w3.org/ns/shacl#> .

ex:CitingPaperMetadataShape
  a sh:NodeShape ;
  sh:targetSubjectsOf cito:cites ;
  sh:property [ sh:path dct:title ; sh:minCount 1 ] ;
  sh:property [ sh:path dct:date ; sh:minCount 1 ] ;
  sh:property [ sh:path ex:discipline ; sh:minCount 1 ] .`
      }
    ],
    typed: [
      {
        label: "Employment is an org",
        tip: "Every worksAt value must be an organization.",
        shape: `@prefix ex: <http://ex/> .
@prefix sh: <http://www.w3.org/ns/shacl#> .

ex:PersonEmploymentShape
  a sh:NodeShape ;
  sh:targetClass ex:Person ;
  sh:property [
    sh:path ex:worksAt ;
    sh:class ex:Org ;
    sh:maxCount 1
  ] .`
      },
      {
        label: "Every person needs a name",
        tip: "Intentional MinCount violation in the tiny graph.",
        shape: `@prefix ex: <http://ex/> .
@prefix sh: <http://www.w3.org/ns/shacl#> .

ex:PersonNameShape
  a sh:NodeShape ;
  sh:targetClass ex:Person ;
  sh:property [
    sh:path ex:name ;
    sh:minCount 1 ;
    sh:message "Every Person must have a name."
  ] .`
      }
    ],
    deps: [
      {
        label: "Application dependencies",
        tip: "The application should have at least one dependency.",
        shape: `@prefix ex: <http://ex/> .
@prefix sh: <http://www.w3.org/ns/shacl#> .

ex:ApplicationDependencyShape
  a sh:NodeShape ;
  sh:targetClass ex:Application ;
  sh:property [
    sh:path ex:dependsOn ;
    sh:minCount 1
  ] .`
      }
    ]
  },
  reach: {
    scholar: {
      pred: "<http://ex/coauthor>",
      seeds: "<http://ex/author/105>",
      examples: [
        { label: "Hub author's coauthor closure", pred: "<http://ex/coauthor>", seeds: "<http://ex/author/105>", reverse: false },
        { label: "Who cites the most-cited paper", pred: "<http://purl.org/spar/cito/cites>", seeds: "<http://ex/paper/15>", reverse: true },
        { label: "Citation closure of paper 245", pred: "<http://purl.org/spar/cito/cites>", seeds: "<http://ex/paper/245>", reverse: false }
      ]
    },
    "scholar-noisy": {
      pred: "<http://purl.org/spar/cito/cites>",
      seeds: "<http://ex/paper/249>",
      examples: [
        { label: "Noise-inflated citation closure", pred: "<http://purl.org/spar/cito/cites>", seeds: "<http://ex/paper/249>", reverse: false },
        { label: "Who cites paper 14 (transitively)", pred: "<http://purl.org/spar/cito/cites>", seeds: "<http://ex/paper/14>", reverse: true },
        { label: "Hub author's coauthor closure", pred: "<http://ex/coauthor>", seeds: "<http://ex/author/120>", reverse: false }
      ]
    },
    citations: {
      pred: "<http://ex/coauthor>",
      seeds: "<http://ex/author/1235>",
      examples: [
        { label: "Author 1235 network", pred: "<http://ex/coauthor>", seeds: "<http://ex/author/1235>", reverse: false },
        { label: "Who cites AlphaFold", pred: "<http://purl.org/spar/cito/cites>", seeds: "<https://doi.org/10.1038/s41586-021-03819-2>", reverse: true }
      ]
    },
    typed: {
      pred: "<http://ex/knows>",
      seeds: "<http://ex/Alice>",
      examples: [
        { label: "Alice knows", pred: "<http://ex/knows>", seeds: "<http://ex/Alice>", reverse: false },
        { label: "Who knows Bob", pred: "<http://ex/knows>", seeds: "<http://ex/Bob>", reverse: true }
      ]
    },
    deps: {
      pred: "<http://ex/dependsOn>",
      seeds: "<http://ex/app>",
      examples: [
        { label: "App dependency closure", pred: "<http://ex/dependsOn>", seeds: "<http://ex/app>", reverse: false },
        { label: "Log4x blast radius", pred: "<http://ex/dependsOn>", seeds: "<http://ex/log4x>", reverse: true }
      ]
    }
  },
  provenance: {
    scholar: {
      predicate: "<http://purl.org/spar/cito/cites>",
      object: "<http://ex/paper/15>",
      examples: [
        {
          label: "Who cites the most-cited paper",
          tip: "Object-bound pattern: routed to the OSP permutation; each match shows its tile and byte range.",
          predicate: "<http://purl.org/spar/cito/cites>",
          object: "<http://ex/paper/15>"
        },
        {
          label: "Everything about the hub author",
          tip: "Subject-bound pattern: routed to SPO — one author's facts live in one a-group of one tile.",
          subject: "<http://ex/author/105>"
        },
        {
          label: "All coauthor edges",
          tip: "Predicate-bound pattern: routed to POS — the whole relation is one contiguous run.",
          predicate: "<http://ex/coauthor>"
        }
      ]
    },
    "scholar-noisy": {
      predicate: "<http://purl.org/spar/cito/cites>",
      object: "<http://ex/paper/14>",
      examples: [
        {
          label: "Who cites paper 14",
          tip: "Object-bound: OSP permutation, with the matching tile and byte range per row.",
          predicate: "<http://purl.org/spar/cito/cites>",
          object: "<http://ex/paper/14>"
        },
        {
          label: "Everything about the hub author",
          tip: "Subject-bound: SPO routing to a single tile.",
          subject: "<http://ex/author/120>"
        }
      ]
    },
    citations: {
      predicate: "<http://purl.org/spar/cito/cites>",
      object: "<https://doi.org/10.1038/s41586-021-03819-2>",
      examples: [
        {
          label: "Who cites AlphaFold",
          tip: "Object-bound over ~539k triples: OSP routes to the one tile holding the DOI's a-group.",
          predicate: "<http://purl.org/spar/cito/cites>",
          object: "<https://doi.org/10.1038/s41586-021-03819-2>"
        },
        {
          label: "One author's facts",
          tip: "Subject-bound: SPO — compare the byte ranges with the predicate-bound example.",
          subject: "<http://ex/author/1235>"
        }
      ]
    },
    typed: {
      predicate: "<http://ex/knows>",
      examples: [
        {
          label: "All knows edges",
          tip: "Predicate-bound: POS permutation; a tiny file is still one tile per permutation.",
          predicate: "<http://ex/knows>"
        },
        {
          label: "Everything about Alice",
          tip: "Subject-bound: SPO routing.",
          subject: "<http://ex/Alice>"
        }
      ]
    },
    deps: {
      predicate: "<http://ex/dependsOn>",
      examples: [
        {
          label: "All dependency edges",
          tip: "Predicate-bound: POS permutation.",
          predicate: "<http://ex/dependsOn>"
        },
        {
          label: "Who depends on log4x",
          tip: "Object-bound: OSP — the impact-analysis pattern at the byte level.",
          predicate: "<http://ex/dependsOn>",
          object: "<http://ex/log4x>"
        }
      ]
    }
  }
};
