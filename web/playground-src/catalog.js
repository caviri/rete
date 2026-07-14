// Shared IRI -> human-label hints for the Wikidata datasets. The editor's
// "Labels" decode toggle reads CATALOG.labelHints[dataset]; wikidata and
// wikidata-100mb use the same Wikidata vocabulary, so they share this map.
const RETE_WD_LABELS = {
  "http://www.wikidata.org/entity/Q100001": "Bemelen (village)",
  "http://www.wikidata.org/entity/Q515": "city",
  "http://www.wikidata.org/entity/Q5": "human",
  "http://www.wikidata.org/entity/Q859": "Plato",
  "http://www.wikidata.org/entity/Q169470": "physicist",
  "http://www.wikidata.org/entity/Q4964182": "philosopher",
  "http://www.wikidata.org/entity/Q36180": "writer",
  "http://www.wikidata.org/entity/Q82955": "politician",
  "http://www.wikidata.org/prop/direct/P279": "subclass of",
  "http://www.wikidata.org/prop/direct/P106": "occupation",
  "http://www.wikidata.org/prop/direct/P625": "coordinate location",
  "http://www.wikidata.org/prop/direct/P737": "influenced by",
  "http://www.wikidata.org/prop/direct/P569": "date of birth",
  "http://www.wikidata.org/prop/direct/P31": "instance of",
  "http://www.wikidata.org/prop/direct/P27": "country of citizenship",
  "http://www.wikidata.org/prop/direct/P19": "place of birth",
  "http://www.wikidata.org/prop/direct/P21": "sex or gender",
  "http://www.wikidata.org/prop/direct/P735": "given name",
  "http://www.wikidata.org/prop/direct/P18": "image",
  "http://www.wikidata.org/entity/Q1028181": "painter",
  "http://www.wikidata.org/entity/Q36834": "composer",
  "http://www.wikidata.org/entity/Q901": "scientist",
  "http://www.wikidata.org/entity/Q90": "Paris",
  "http://www.wikidata.org/entity/Q142": "France",
  "http://www.wikidata.org/entity/Q183": "Germany",
  "http://www.wikidata.org/entity/Q6581072": "female",
  "http://www.wikidata.org/entity/Q6581097": "male"
};

window.RETE_PLAYGROUND_CATALOG = {
  defaultDataset: "scholar",
  // Every dataset is mirrored in Cloudflare R2 at <key>/<key>.rete, so any of
  // them can be cached (downloaded once) or range-queried lazily — not only the
  // few that are too big to embed. Remote-only datasets carry their own `url`;
  // for the rest the URL is derived as remoteBase/<key>/<key>.rete.
  // Graph .rete files are served DIRECTLY from Cloudflare R2 (data.graphplaza.com):
  // 206 range + CORS + no redirect + no token, so the default sync-XHR reader works and
  // One folder per dataset: <base>/<key>/<key>.rete.
  remoteBase: "https://data.graphplaza.com",
  remoteToken: "",
  // SQL Explore companions (parquet/duckdb/sqlite) use their own R2 paths.
  companionBase: "https://data.graphplaza.com",
  companionToken: "",
  // Optional vector-tile basemap PAIRED with a dataset (option B of the geo-LOD work):
  // a PMTiles archive (tippecanoe, true per-zoom LOD, HTTP-range-served) rendered in
  // the "Tiles" output by protomaps-leaflet, with the SPARQL result features highlighted
  // on top. The geometry rendering goes through PMTiles; rete stays the graph next to it.
  pmtiles: {
    "geoadmin": {
      url: "https://data.graphplaza.com/geoadmin/geoadmin.pmtiles",
      label: "geoadmin (OSM admin areas)",
      size: "113 MB",
      // tippecanoe layer → the feature property carrying the human name we join on
      layers: { countries: "shapeName", regions: "shapeName", districts: "shapeName", places: "NAME" }
    },
    // Option C: the tiles live INSIDE the .rete (a kind-7 section). `embedded` tells the
    // Tiles view to parse the .rete header for the section offset and range-read tiles
    // from this same URL — one file = graph + tiles. `url` is the .rete, not a .pmtiles.
    "geoadmin-tiles": {
      url: "https://data.graphplaza.com/geoadmin-tiles/geoadmin-tiles.rete",
      embedded: true,
      label: "embedded in geoadmin-tiles.rete",
      size: "113 MB section",
      layers: { countries: "shapeName", regions: "shapeName", districts: "shapeName", places: "NAME" }
    }
  },
  families: ["Summary", "Select", "Path", "Aggregate", "Geo", "Construct"],
  // Median local (in-memory) query time per example, in ms — benchmarked offline
  // by dev/bench_examples.cjs (5 runs each). Drives the speed badge on each
  // example. Remote-lazy datasets are network-dependent and not listed here.
  perf: {
    "scholar": { "One query, all three engines (Whole · Progressive · Community)": 1, "Predicate totals": 1, "Author profiles": 2, "High-novelty papers": 2, "High-novelty, split by community": 9, "Citation closure (Whole index only)": 1, "Most-cited papers": 2, "Papers per field": 1, "Authors above the mean h-index": 1, "Novelty tiers (IF)": 1, "Title fingerprints (SHA-256)": 1, "Everything but citations": 2, "Coauthor ego network": 1 },
    "scholar-noisy": { "Predicate totals": 1, "Mangled titles": 1, "Authors missing ORCID": 1, "Noise-inflated closure": 1, "Temporal violations": 2, "Cross-field citation pairs": 2, "Cross-field cites from genomics": 2 },
    "causal": { "What's in the model": 1, "Everything that leads to a heart attack": 1, "Downstream effects of obesity": 1, "Feedback loops (vicious cycles)": 1, "Confounders (a common cause of two factors)": 1, "Colliders (two causes, one effect)": 1, "How obesity leads to diabetes (mediators)": 1, "Biggest causal footprint": 1, "What lowers the risk of a heart attack": 1, "Exogenous root causes": 1 },
    "history": { "Map: territories of 1914": 21, "Time: territories per year": 22, "Who ruled Paris in 1914?": 22, "Empires over Beijing through time": 56, "Territories around the British Isles (1815)": 23, "Nearest neighbours of London, 1914": 24, "Bounding box of each 1492 territory": 19, "Territories per era": 19 },
    "linked-jazz": { "Relationship-type totals": 3, "Everything Mary Lou Williams said about people": 2, "Most talked-about musicians": 8, "Who Count Basie reaches by word of mouth": 2, "Most-cited influences": 2, "Mary Lou Williams collaboration ego-network": 2 },
    "nomisma": { "Shape of the corpus": 9, "Silver tetradrachms of Alexander the Great": 8, "Most prolific mints": 6, "Coin types per issuing authority": 5, "Mints used by 3+ successive rulers": 8, "Who else struck at Cassander's mints": 46 },
    "mira": { "Manuscripts you can SEE (IIIF)": 2, "Where early Irish books were made": 2, "The scholars, linked to Wikidata": 1, "Cross-source JOIN: MIrA people × live Wikidata": 1, "Datable manuscripts over time": 2, "The largest manuscripts": 1, "Search the contents": 1, "What the catalogue describes": 1, "Why each book is in the corpus": 1, "The Old Irish glossed books (with images)": 1, "Same join, but via a shareable mapping linkset": 1 },
    "mira-wikidata": { "The mappings": 1, "A mapping with its SSSOM provenance": 1, "The mapping set's metadata": 1 },
    "causalgraph": { "The causal model (cause → effect, confidence, lag)": 1, "What causes a defect?": 1, "Downstream effects of injection pressure": 1, "Human knowledge vs algorithm-discovered": 1, "The ontology: classes & definitions": 1, "The class hierarchy": 1 },
    "mimotext": { "What is in this graph?": 3, "Most common themes across the novels": 4, "Baculard d'Arnaud's novels, by year, with genre": 8, "Novels that share the most themes": 171, "Stylometrically closest novels": 3, "Author ego-network: Baculard d'Arnaud -> novels -> genre": 5 },
    "openalex-astrocytes": { "What's in the graph": 6, "Most-cited astrocyte papers": 4, "Most prolific astrocyte authors": 8, "Leading institutions": 7, "Adjacent sub-topics": 6, "Who cites the field's landmark paper": 4, "Citation network of the top papers": 5 },
    "antarctic-expeditions": { "Shape of the expedition graph": 1, "The crew of the Endurance": 1, "Crew who served on more than one expedition": 1, "Largest crews": 1, "Expedition -> leader + ship + crew ego-network": 1, "Time: expeditions by start year": 1 },
    "factgrid-illuminati": { "What's in the graph": 9, "Members of the Illuminati": 7, "Which properties describe members": 16 },
    "theographic-graph": { "What's in the graph": 7, "Descendants of Abraham": 5, "Who had the most children": 6, "Abraham's children": 5 },
    "monarch": { "What's in the graph": 3, "Phenotypes in the graph": 2, "Most-connected genes": 2 },
    "opencitations": { "What's in the graph": 3, "Most-cited works": 2, "Publications per year": 2, "Citation closure of a seed paper": 2 },
    "orkg": { "What's in the graph": 15, "Papers": 10, "Node types in the graph": 9 }
  },
  datasets: [
    {
      key: "scholar",
      label: "scholar.rete - synthetic scholarly world",
      description: "250 papers, 137 authors, 36 venues from scripts/synth_graph.py (seed 42): power-law citations, field communities, Zipfian venues, and typed literals."
    },
    {
      key: "wikidata",
      kind: "remote-lazy",
      url: "https://data.graphplaza.com/wikidata-1GB/wikidata.rete",
      // Wikidata types entities with "instance of" (wdt:P31), NOT rdf:type — tell
      // the Explore tab so it groups entities into tables by their real classes.
      typePredicate: "<http://www.wikidata.org/prop/direct/P31>",
      label: "wikidata-1GB.rete - a real 1 GB Wikidata graph (remote, lazy)",
      description: "A real ~1.04 GB slice of Wikidata, queried entirely in the browser over HTTP range requests - the point is that a GIGABYTE-scale knowledge graph stays interactive because a selective SPARQL query only faults in the bytes it needs (a typical query reads ~40 MB of the 1.04 GB, never the whole file). Holds both PLACES (e.g. wd:Q100001 Bemelen, with multilingual schema:description) and PEOPLE (wd:Q5 humans with wdt:P106 occupation, wdt:P569 birth date, wdt:P737 influenced-by). Entity/property IRIs stay as wikidata.org/{entity,prop/direct}/* so nodes round-trip to live Wikidata. Use bound subjects/objects and occupation intersections for snappy reads; avoid full scans. CC0 (Wikidata). See also the dedicated 100 MB / 1 GB explorer."
    },
    {
      key: "wikidata-zenodo",
      kind: "remote-lazy",
      url: "https://data.graphplaza.com/wikidata-zenodo/wikidata-zenodo.rete",
      typePredicate: "<http://www.wikidata.org/prop/direct/P31>",
      label: "wikidata with Zenodo DOI (1 GB) - citable source, stable R2 delivery",
      description: "The same ~1.49 GB Wikidata graph deposited as the citable Zenodo record 21168821 (DOI 10.5281/zenodo.21168821), served here as its stable-format R2 twin for browser range queries. The deposited pre-1.0 artifact remains the provenance record; this release copy carries stable format generation 1 and browser-visible Content-Range. A selective query faults ~27 MB rather than downloading the whole file. CC0 (Wikidata)."
    },
    {
      key: "scholar-noisy",
      label: "scholar-noisy.rete - same world, 25% noise",
      description: "The same generator at --noise 0.25: rewired citations (incl. temporal violations), missing ORCIDs and ISSNs, and whitespace-mangled titles - for SHACL and data-quality demos."
    },
    {
      key: "causal",
      label: "causal.rete - cardiometabolic causal model (confounders, mediators, colliders, loops)",
      description: "A cardiometabolic causal model: ~30 typed factors (risk factors, conditions, diseases, symptoms, outcomes, treatments) wired by a transitive :causes relation, plus a protective :reduces relation. Built so the Query examples discover causal structure - confounders (forks), mediators (chains), colliders (common effects), feedback loops (?x :causes+ ?x), and exogenous root causes. The SHACL examples check data quality (three planted defects). And the Coherence tab still proves the schema defect: :Relapsed is a subclass of two owl:disjointWith classes (HealthyState, DiseaseState) so it is UNSATISFIABLE, and patient :p is typed as both states. ~170 triples."
    },
    {
      key: "history",
      kind: "remote-lazy",
      url: "https://data.graphplaza.com/history/history.rete",
      label: "history.rete - historical world borders (GeoSPARQL, remote, lazy)",
      description: "World territorial borders at 7 snapshots from 323 BCE to 1994 CE (aourednik/historical-basemaps, GPL-3.0), each polygon stored as a geo:wktLiteral with an integer year. Query it with GeoSPARQL: point-in-polygon containment, bbox intersection, and distance — combined with temporal filters. Coordinates are CRS84 lon/lat, simplified to ~1 km."
    },
    {"key": "linked-jazz", "label": "linked-jazz.rete - jazz musician social network", "description": "Linked Jazz - a social network of jazz musicians reconstructed from oral-history transcripts. 54 interviewed musicians (the ego hubs) connect outward to ~1,940 people they mention, with who-knows-whom ties typed by the REL vocab (knowsOf, friendOf, hasMet, influencedBy, mentorOf), Music Ontology (collaborated_with) and the project's own ontology (playedTogether, inBandTogether, touredWith, bandLeaderOf). 40 of the 54 hubs also appear as objects, so genuine multi-hop paths exist (transitive knowsOf+ / mentorOf+ chains). Each person carries a foaf:name and usually a dbo:thumbnail. ~9,470 triples: 3,649 knowsOf, 2,009 names, 1,555 thumbnails, plus ~1,800 typed ties. Person IRIs are mostly dbpedia.org/resource, so nodes link out to DBpedia. CC BY-SA 3.0 (Pratt Institute; person data from DBpedia)."},
    {"key": "getty-ulan", "kind": "remote-lazy", "url": "https://data.graphplaza.com/getty-ulan/getty-ulan.rete", "label": "getty-ulan.rete - artist mentorship lineage (remote, lazy)", "description": "Getty ULAN \"who-taught-whom\" lineage: a directed social graph of ~28,300 artists/agents from the Union List of Artist Names. Each person carries a preferred name (skos:prefLabel), an English one-line biography (schema:description, e.g. \"Dutch painter, printmaker, 1606-1669\"), nationality, and birth/death years (xsd:gYear). Persons are connected by gvp:teacherOf (34,561 master->pupil edges) and gvp:influenced (534 edges). Densely connected and deeply transitive - Rembrandt taught ~38 pupils and has ~369 artistic descendants via teacherOf+. IRIs stay as vocab.getty.edu/ulan/NNN so nodes round-trip to live Getty LOD. ~205k triples after de-dup. ODC-BY 1.0 (attribute The J. Paul Getty Trust)."},
    {"key": "causalgraph", "kind": "remote-lazy", "url": "https://data.graphplaza.com/causalgraph/causalgraph.rete", "label": "causalgraph.rete - causal graphs in knowledge graphs (Fraunhofer IWU ontology + example, remote, lazy)", "description": "The causalgraph ontology (github.com/causalgraph/causalgraph, MIT - a Python package for modelling, persisting and visualising causal graphs as RDF) brought to rete: the full OWL schema (converted from OWL/XML with owlready2) PLUS an authored example causal model. The ONTOLOGY (TBox) defines how a causal graph is represented: a CausalGraph is made of CausalNodes (each an Event, State or Variable, split by origin into Human-input vs Machine) and CausalEdges; every CausalEdge reifies a cause→effect link carrying hasConfidence (0-1), hasTimeLag (the delay before the effect), and a Creator provenance - a Human_Creator (domain knowledge) or a Machine_Creator / LearningAlgorithm_Creator (a causal-discovery run). Class definitions come straight from the OWL (rdfs:comment). The EXAMPLE (ABox, scripts/causalgraph_example.py) is a faithful Industry-4.0 causal graph - INJECTION-MOULDING quality: operator setpoint → injection pressure → mould fill → short-shot/warpage, melt temperature → burn marks, etc., with realistic confidences + time-lags, some edges asserted by the process engineer and others 'discovered' by a PCMCI run. 315 triples (ontology + example), embedded. Explore the schema (Schema view), traverse cause→effect, rank by confidence, or split links by who asserted them. MIT (causalgraph / Fraunhofer IWU)."},
    {"key": "ontoneurolog", "kind": "remote-lazy", "url": "https://data.graphplaza.com/ontoneurolog/ontoneurolog.rete", "label": "ontoneurolog.rete - a neuroimaging data-sharing ontology (OntoNeuroLOG v2.2, remote, lazy)", "description": "OntoNeuroLOG v2.2 - the application ontology of the NeuroLOG project (Temal, Dojat, Kassel & Gibaud, J. Biomed. Inform. 2008), which federated four French neuroscience centres (Grenoble, Rennes, Paris, Sophia Antipolis) to share MRI data across their heterogeneous databases. This is a SCHEMA (a TBox, no instance data): a DOLCE-grounded, multi-layer OWL-Lite ontology shipped as 35 RDF/XML modules and merged here into one graph. Its richest layer is a careful model of MR PHYSICS and ACQUISITION - an MR-protocol specifies the actions of an MR dataset acquisition; an MR-sequence specifies RF pulses, gradients and signal reading (spin-echo, gradient-echo, EPI...); classes like flip-angle, field-of-view, coil, 1H-nucleus and MR-image-contrast carry scholarly rdfs:comment definitions citing DICOM, Hornak and Boyle. Around it sit taxonomies of DATASETS (acquisition vs processing - the dataset-processing branch is a tree of neuroimaging pipeline operations: mesh generation, wavelet transformation, calibration-model estimation), MEDICAL-IMAGE files and formats, EXAMINATION SUBJECTS (human, sex, hemispheric dominance), STUDIES, software SERVICES, and clinical ASSESSMENT INSTRUMENTS (e.g. every item of the BDI-II Beck Depression Inventory). The DOLCE upper layer and an I&DA 'information & discourse acts' core give it its backbone of participant-role object properties (is-agent-of-at, has-for-function, generates, is-capable-to-perform) that wire actions, agents and artefacts together. 17,658 triples, 1,303 classes, 289 object properties, remote-lazy over HTTP range. Explore the schema (Schema view), read the MR definitions, or CONSTRUCT a subclass tree as a graph. Public download from the NeuroLOG namespace; explicit licence unstated - attribute B. Gibaud / IRISA-VISAGES and cite Temal et al. 2008. Merged from the 35 public OWL modules by scripts/ontoneurolog_to_nt.py. Pairs naturally with NIDM (the RDF Neuroimaging Data Model) for actual provenance instances."},
    {"key": "nidm", "kind": "remote-lazy", "url": "https://data.graphplaza.com/nidm/nidm.rete", "label": "nidm.rete - Neuroimaging Data Model: a real cohort + PROV provenance (federates with ontoneurolog)", "description": "NIDM (the Neuroimaging Data Model, INCF NIDASH / BIRN) - the RDF + W3C PROV standard for describing the neuroimaging data lifecycle - carrying a REAL cohort: OpenNeuro ds000030, the UCLA Consortium for Neuropsychiatric Phenomics. 272 subjects (nidm:AcquisitionObject / prov:Entity) each with ncit:age (21-50), ncit:gender and ncit:diagnosis across four groups - 130 CONTROL, 50 SCHZ (schizophrenia), 49 BIPOLAR, 43 ADHD - and full PROV provenance: every acquisition prov:wasGeneratedBy an activity and prov:wasAttributedTo an agent. Bundled with the OntoNeuroLOG v3.0 assessment-instrument taxonomy that NIDM reuses (assessment-instrument, questionnaire, behavioural-instrument, scale-item, numerical-score... each with a skos:definition). THE FEDERATION COMPANION to the ontoneurolog dataset: because NIDM adopted OntoNeuroLOG's instrument ontology (at v3.0, http://neurolog.unice.fr/...) while ontoneurolog holds it at v2.2 (irisa.fr/...), this file ships 45 owl:sameAs links bridging the two versions by shared concept - so a CROSS-SOURCE JOIN (add ontoneurolog as a second source) links each NIDM instrument to its OntoNeuroLOG definition and returns 29 concepts defined on both sides. The honest reading: OntoNeuroLOG is the schema (definitions), NIDM is the instances (a real studied cohort) - the two halves of the NeuroLOG vision. 3,153 triples, remote-lazy over HTTP range. NIDM specs are an open community standard (INCF NIDASH, no explicit licence file); the ds000030 phenotype data derives from the OpenNeuro CC0 dataset (UCLA CNP). Built by scripts/nidm_to_nt.py from github.com/incf-nidash/nidm-specs. Extendable with NIDM-Results (FSL/SPM brain-mapping statistics) - future work."},
    {"key": "mira-wikidata", "kind": "remote-lazy", "url": "https://data.graphplaza.com/mira-wikidata/mira-wikidata.rete", "label": "mira-wikidata.rete - MIrA↔Wikidata mappings (SSSOM linkset, remote, lazy)", "description": "A MAPPING LINKSET, not a dataset: the 13 skos:exactMatch links reconciling MIrA's manuscript-culture entities (people, texts) to Wikidata - re-expressed from MIrA's owl:sameAs and shared the way the community does it, via SSSOM (Simple Standard for Sharing Ontological Mappings). Each link carries provenance as an owl:Axiom reification: sssom:mapping_justification (semapv:ManualMappingCuration), sssom:confidence, sssom:subject_label; the set itself is a void:Linkset / sssom:MappingSet with license, creator and date. The point: mappings are a CLAIM, not a fact, so they live in their OWN small, citable, queryable artifact - decoupled from both datasets - and federate IN to bridge them. See the MIrA example 'Same join, but via a shareable mapping linkset': the cross-source join routes ?p skos:exactMatch ?wd through THIS file, the entity label through MIrA, and the facts through live Wikidata. 125 triples, embedded. Built by scripts/mira_sssom.py (TSV companion: mira-wikidata.sssom.tsv). CC BY-NC-SA 4.0 (MIrA)."},
    {"key": "mira", "kind": "remote-lazy", "url": "https://data.graphplaza.com/mira/mira.rete", "typePredicate": "<http://www.wikidata.org/prop/direct/P31>", "label": "mira.rete - early Irish manuscripts (MIrA, Wikidata-aligned, IIIF, remote, lazy)", "description": "MIrA - Manuscripts with Irish Associations (Pádraic Moran, University of Galway): a catalogue of c. 300 manuscripts written before AD 1000 in Ireland, in Irish script, or with Irish connections (often by Irish 'peregrini' on the Continent). Described with STANDARD Wikidata properties - P31 instance-of, P195 collection, P217 shelfmark, P571 inception (xsd:gYear), P1071 location-of-creation, P2048/P2049 height/width, P1574 exemplar-of - over wd:Q* values, every one of which carries an English rdfs:label fetched from Wikidata (so P1071 reads 'location of creation', Q142 reads 'France'). The named scholars (Eriugena, Sedulius Scottus, Isidore...) carry owl:sameAs to Wikidata, so the graph FEDERATES with the Wikidata datasets. 189 of the manuscripts are digitised: their IIIF manifests (wdt:P6108, from the British Library, Trinity College Dublin, e-codices, IRHT...) render as an in-table image viewer - click a folio for the lightbox. Plus mira: name / script / folios / contents (full-text indexed). Each manuscript is also tagged with MIrA's INCLUSION CRITERIA (mira:category, straight from the project's /about page) - Script: Irish (148), Origin: Ireland (106), Vernacular Old-Irish content (168 glossed pages across 84 manuscripts), Named Irish scribe (41), Exemplar: Irish, Text of Irish origin, plus the 'Insular (Irish?)' outline categories - so you can query exactly why each book is in the corpus. The catalogue draws on the standard palaeographical reference works (Bernhard Bischoff's Katalog der festländischen Handschriften and Südostdeutschen Schreibschulen, E. A. Lowe's Codices Latini Antiquiores); the RDF data was developed by Sudhansu Bala Das, funded by a Research Ireland Laureate award and the Insight Research Ireland Centre for Data Analytics. ~5k triples, embedded (instant, offline). Built from the padraicmoran/MIrA RDF dump + per-manuscript XML by scripts/mira_to_nt.py. CC BY-NC-SA 4.0 - non-commercial; cite as Pádraic Moran, Manuscripts with Irish Associations (MIrA), v1.0 (2026), https://mira.ie."},
    {"key": "bioexplora", "kind": "remote-lazy", "url": "https://data.graphplaza.com/bioexplora/bioexplora.rete", "label": "bioexplora.rete - 207k natural-history specimens (Museu de Ciencies Naturals de Barcelona, Darwin Core, remote, lazy)", "description": "The OPEN natural-history collections of the Museu de Ciencies Naturals de Barcelona (bioexplora.cat) as one queryable graph - 207,163 SPECIMENS in Darwin Core across the museum's six collections: arthropods (MCNB-Art, 82,900), molluscs (MCNB-Malac, 50,083), paleontology (MGB, 32,581), vertebrates (MCNB-Cord, 31,954), the tissue bank and general zoology. Every specimen carries its full taxonomy (kingdom -> species), catalogue number, collector, date, locality, georeference and type status, expressed with the REAL Darwin Core term IRIs (dwc:scientificName, dwc:family, dwc:decimalLatitude...) - i.e. the museum's own properties. 13,543 specimens are PHOTOGRAPHED: the IIIF images (museum server, CORS-open) render inline; 43,826 are GEOREFERENCED as GeoSPARQL points (switch Output -> Map); 672 are name-bearing TYPE specimens. Plus two media layers: 667 skull and bone 3D SCANS (the Atles osteologic) linking out to their Sketchfab viewers, and 173 nature SOUND recordings that play inline (CC BY-NC-ND, by Eloisa Matheu via Xeno-canto - NOT the MCNB). 7.19M triples, remote-lazy over HTTP range from the bucket. Harvested KEYLESS from GBIF (the museum publishes Darwin Core archives on ipt.gbif.es), the Sketchfab account laboratorinatura, and Xeno-canto, by scripts/bioexplora_to_nt.py. CC BY 4.0 - credit the Museu de Ciencies Naturals de Barcelona (the audio layer excepted)."},
    {"key": "geoadmin", "kind": "remote-lazy", "url": "https://data.graphplaza.com/geoadmin/geoadmin.rete", "label": "geoadmin.rete - world admin boundaries (geoBoundaries/OSM, 52k polygons, GeoSPARQL, remote, lazy)", "description": "World administrative boundaries with GeoSPARQL geometries, built for FEDERATED geospatial joins — 52k OSM-derived polygons (simplified to ~1 km), a lean 15 MB range-queried lazily. 213 COUNTRIES (ADM0), 3,133 REGIONS/states (ADM1) and 48,362 DISTRICTS (ADM2) as POLYGONS from geoBoundaries (OSM-derived, CC-BY), plus 1,251 populated PLACES as POINTS (Natural Earth) — three admin levels, points and polygons. Each entity carries a name, an ISO 3166-1 code (p:iso/p:country, alpha-2 mapped from geoBoundaries' alpha-3), an admin level (p:adminLevel) and its geometry (geo:asWKT). Add it as a federation source to ANY dataset and JOIN on the country code to pull in the boundary polygon — e.g. attach to every bioexplora specimen the polygon of the country it was collected in, then switch Output → Map. geoBoundaries CC-BY 4.0 (geoboundaries.org); places CC0 (Natural Earth). Built by scripts/geoboundaries_to_nt.py."},
    {"key": "geoadmin-tiles", "kind": "remote-lazy", "url": "https://data.graphplaza.com/geoadmin-tiles/geoadmin-tiles.rete", "label": "geoadmin-tiles.rete - the admin-boundaries graph AND a PMTiles vector basemap in ONE file", "description": "The SAME world-admin graph as geoadmin, but with a PMTiles vector-tile archive EMBEDDED as a section inside the one .rete file (geo-LOD 'option C'). One immutable, content-hashed, HTTP-range-queryable file holds BOTH: run SPARQL on the RDF graph, then switch Output → Tiles and the map range-reads its vector tiles from a kind-7 section of the very same file (no separate .pmtiles, no second request target). The .rete reader ignores the unknown section, so the graph opens exactly as before; the browser parses the 1 KB header to find the tile section's byte offset and faults tiles from there. GeoSPARQL has no native tiling — this is the experiment of carrying pre-tiled vector geometry next to the graph in one range-served file. Tiles built with tippecanoe (z0-9, true per-zoom LOD) and embedded by scripts/embed_tiles.py."},
    {"key": "smithsonian3d", "kind": "remote-lazy", "url": "https://data.graphplaza.com/smithsonian3d/smithsonian3d.rete", "label": "smithsonian3d.rete - 2,199 interactive 3D models (Smithsonian Open Access, CC0, remote, lazy)", "description": "2,199 interactive 3D MODELS from the Smithsonian Open Access program (CC0, public domain) - every object streams a Draco-compressed .glb you can rotate, zoom and inspect right in the results table (the 🧊 3D viewer). The collection spans the Institution: the National Museum of Natural History dominates (primate and animal CRANIA, mandibles, fossils and zoological specimens - Hylobates, Pongo, fossil whales), alongside famous objects from the National Air and Space Museum (the Apollo Command Module Columbia, modelled inside and out), American History, the National Portrait Gallery (Abraham Lincoln's life masks), Cooper Hewitt, African American History and the Freer|Sackler. Each model carries its title, the holding museum unit, its catalogue number, and a link (?record) to the full Smithsonian record. Built KEYLESS from the public smithsonian-open-access S3 bucket (the 3d/ prefix - each model a Voyager scene.svx.json listing Low/Medium/High Draco .glb derivatives) by scripts/smithsonian3d_to_nt.py, taking the Medium-quality mesh for streaming; the .glb files are served with CORS from Amazon S3 so they render inline. 15k triples, remote-lazy from the bucket, with lossless Parquet / DuckDB / SQLite table companions beside it for the Explore tab (the Model3D table holds every object). CC0 1.0 - free to use, share and remix; credit the Smithsonian."},
    {"key": "lineara", "kind": "remote-lazy", "url": "https://data.graphplaza.com/lineara/lineara.rete", "label": "lineara.rete - the Linear A corpus (undeciphered Minoan script, remote, lazy)", "description": "The complete surviving corpus of LINEAR A - the still-undeciphered script of Bronze-Age Minoan Crete (c. 1800-1450 BC), used for palace administration and religious dedications. 1,721 inscriptions (clay tablets, roundels, sealings, libation vessels) from Haghia Triada, Khania, Phaistos, Knossos, Zakros and beyond, each linked to its findspot, scribe, support type and Minoan period, with the Linear A Unicode transcription and the Latin transliteration. The analytical heart is the SIGN and WORD layer: every document is connected to the 375 distinct signs and 1,314 word-sequences it carries (6,039 sign-attestations, 4,100 word-attestations), so you can study sign frequencies, recurring sequences and sign co-occurrence - the main tools for probing an undeciphered script. KU-RO ('total') and KI-RO ('deficit'), the two Linear A words scholars do agree on, are right there at the bottom of the accounts. 67 of the artifacts also LINK (prop:model3d) to the ERC INSCRIBE project's high-resolution interactive 3D scans (3DHOP viewers, University of Bologna) - HT 29, the Khania (KH) and Phaistos (PH) tablets, plus nodules, roundels and a vessel - so you can rotate and measure the actual clay. A second archive, the PAITO Project (Sapienza, Prof. A. Greco; prop:paito), is linked for the Haghia Triada sealings (per-artifact) and the Phaistos corpus (catalogue). Text after the transcriptions of Louis Godart & Jean-Pierre Olivier (GORILA, Ecole Francaise d'Athenes) and the tabulation of George Douros; compiled by mwenge as the LinearA Explorer (github.com/mwenge/lineara.xyz). 38k triples, embedded (instant, offline). Built by scripts/lineara_to_nt.js + scripts/lineara_inscribe.js. Scholarly/educational derivative with full attribution - inscription IMAGES are (c) Ecole Francaise d'Athenes and the 3D MODELS are (c) INSCRIBE / Universita di Bologna (all rights reserved, non-profit scientific use); NEITHER is included here, only references/links; the source repo states no explicit license, so attribute GORILA / Douros / mwenge / INSCRIBE and treat as non-commercial research data."},
    {"key": "nomisma", "label": "nomisma.rete - coinage of Alexander the Great (PELLA)", "description": "PELLA - Coinage of Alexander the Great and the Macedonian kingdom, from Nomisma.org. 7,228 ancient Greek coin TYPES struck under Philip II, Alexander III, Philip III Arrhidaeus and the Diadochi (Cassander, Lysimachus, Ptolemy I), spanning 359-65 BC. Each type links by real IRIs to its mint (~150 cities from Pella and Amphipolis to Babylon, Sardis, Sidon and Susa), issuing authority, material (silver/gold/bronze), denomination (tetradrachm, drachm, stater...), region, and start/end dates as xsd:gYear. Every mint/authority/material/denomination/region carries an English rdfs:label, so the graph is fully self-describing. ~53,535 triples, embeds to ~150 KB. ODbL 1.0 (PELLA coin data) + CC-BY 3.0 (Nomisma vocabulary); attribute the American Numismatic Society / Nomisma.org."},
    {"key": "mimotext", "typePredicate": "<http://data.mimotext.uni-trier.de/prop/direct/P2>", "label": "mimotext.rete - French Enlightenment novels + stylometry", "description": "MiMoTextBase, a Wikibase of French Enlightenment novels (c. 1751-1800) from the Trier Center for Digital Humanities. A self-contained literary graph: 1,774 works linked to authors (956 people), publication dates and places, genres, languages, narrative form and location, and 375 thematic concepts (4,096 work->theme edges). Its distinctive layer is computational: 520 work-to-work STYLOMETRIC SIMILARITY edges (a Burrows-Delta neighbourhood of which novels read alike), plus 191 scholarship-mention edges. English labels for every entity. ~25,155 triples, ~828 KB Turtle -> embeds. A browsable network of who wrote what, which novels are thematically and stylistically close, and which scholarship discusses them together. CC0 (public domain)."},
    {"key": "mmm", "kind": "remote-lazy", "url": "https://data.graphplaza.com/mmm/mmm.rete", "label": "mmm.rete - the full Mapping Manuscript Migrations provenance graph (23.3M triples, remote, lazy)", "description": "The ENTIRE Mapping Manuscript Migrations (MMM) graph as one range-queried .rete: 23,349,356 triples over CIDOC-CRM / FRBRoo, unifying the Schoenberg Database (SDBM), Bibale (IRHT) and Medieval Manuscripts in Oxford (Bodleian) into one provenance graph of medieval and Renaissance manuscripts. Each manuscript is an frbroo:F4_Manifestation_Singleton with its production event (E12, with time-span and place), former/current owners (crm:P51 -> E21 persons and E40 organisations), the works/expressions it carries (mmm:manuscript_work), physical detail (material, folios, lines), a shelfmark (mmm:catalog_or_lot_number) and the catalogue/source records that document it (crm:P70i). Places are TGN entities carrying WGS84 coordinates. The MMM metadata ontology is merged in (class/property labels for the Schema view) and a type pyramid + Dataset Card are embedded. Queried lazily over HTTP range - the browser fetches only the dictionary chunks and index tiles each query touches, never the 141 MB whole. Featured here: Cambridge, University Library, MS Gg.1.1 (SDBM entry 212926), the 14th-century trilingual compendium ('A large collection of poetry', parchment, 633 folios). CC BY-NC 4.0 - non-commercial, attribute MMM (seco.cs.aalto.fi). Pick SELECTIVE shapes (a bound manuscript, a bound owner/collection, a shelfmark) for snappy lazy reads; whole-predicate aggregates over 23M triples scan far more."},
    {"key": "openalex-astrocytes", "label": "openalex-astrocytes.rete - astrocyte research graph (OpenAlex)", "description": "The 500 most-cited works on astrocytes (the star-shaped glial cells of the brain) from OpenAlex (CC0), as a connected citation core: 4,113 cito:cites edges linking 500 papers to 2,074 authors, 537 institutions and 875 sub-topics. Explore the most-cited papers, the most prolific labs, and which fields astrocyte research bridges (reactive astrocytes, blood-brain barrier, neuroinflammation, stem cells)."},
    {"key": "antarctic-expeditions", "label": "antarctic-expeditions.rete - Heroic-Age expeditions, crews & ships", "description": "Heroic-Age Antarctic exploration as an explorable social graph: 6 landmark expeditions (Discovery, Nimrod, Terra Nova, Endurance, Australasian, Belgian, 1897-1917) linked by ex:participant to ~76 crew, by ex:vessel to their 5 ships, and by ex:leader to their commanders (Scott, Shackleton, Mawson, de Gerlache). Each expedition carries ex:startYear/ex:endYear. Because expeditions share personnel and ships, genuine multi-hop paths exist (shared-crew bridges, leaders who served on earlier voyages). IRIs stay as wikidata.org/entity so nodes round-trip to live Wikidata. CC0. Pairs with the atlas 'Heroic-Age Sites' overlay: the huts and deaths on the map are where these crews lived and died."},
    {"key": "factgrid-illuminati", "kind": "remote-lazy", "url": "https://data.graphplaza.com/factgrid-illuminati/factgrid-illuminati.rete", "typePredicate": "<https://database.factgrid.de/prop/direct/P2>", "label": "factgrid-illuminati.rete - Order of the Illuminati prosopography (remote, lazy)", "description": "The 18th-century secret society as a prosopographical graph from FactGrid (CC0), an independent historical Wikibase: ~1,300 members of the Order of the Illuminati (Q10677) with their FactGrid properties and English labels. Property and object labels are resolved so the opaque P-numbers read in plain language."},
    {"key": "theographic-graph", "label": "theographic-graph.rete - biblical narrative graph", "description": "Theographic Bible (CC BY-SA) as a narrative/genealogy graph (distinct from the atlas geo-events layer): ~3,000 people linked by tg:father/mother/child/sibling/partner, born/died places, group memberships, and events with participants. Walk genealogies and event chains."},
    {"key": "monarch", "label": "monarch.rete - disease/gene/phenotype graph", "description": "A bounded slice of the Monarch Initiative biomedical KG (CC-BY): a disease neighbourhood linking genes (biolink:Gene), phenotypes (biolink:has_phenotype), gene-gene interactions (biolink:interacts_with) and taxa, with rdfs:labels and skos:exactMatch cross-references."},
    {"key": "opencitations", "label": "opencitations.rete - a citation neighborhood", "description": "A citation neighbourhood from OpenCitations (CC0) around a seed paper: cito:cites edges plus dct:title / dct:date / dct:creator (foaf:name) / dct:publisher bibliographic metadata. Distinct from the small AlphaFold sample already in the catalog."},
    {"key": "orkg", "kind": "remote-lazy", "url": "https://data.graphplaza.com/orkg/orkg.rete", "label": "orkg.rete - research contributions (remote, lazy)", "description": "A slice of the Open Research Knowledge Graph (CC-BY): papers (orkg:Paper) and their structured contributions (orkg:Contribution), research problems, methods and results, with rdfs:labels - scholarly knowledge as data, not prose."},
    {"key": "ohm-full", "kind": "remote-lazy", "url": "https://data.graphplaza.com/ohm-full/ohm-full.rete", "label": "ohm-full.rete - all of OpenHistoricalMap (remote, lazy)", "description": "The ENTIRE current OpenHistoricalMap planet (daily snapshot, 2026-06-14) as one range-queried .rete: 1,021,295 named + dated + geolocated historical features (~6.1M triples, 150 MB), queried lazily over HTTP - only the dictionary chunks and index tiles each query touches are fetched, never the whole file. Each feature is openhistoricalmap.org/{node,way,relation}/<id> with rdfs:label, ex:startYear/ex:endYear (signed integers, -10000..2100; 2100 = still present) and GeoSPARQL geometry (176k points, 690k lines, 155k polygons; admin boundaries assembled from multipolygon/boundary relations, simplified to ~50 m). Built from planet.openhistoricalmap.org with PyOsmium (scripts/fetch_ohm_planet.sh). CC0 1.0 - credit OpenHistoricalMap contributors. Pick selective shapes (a bound subject, a name, a point-in-polygon) for snappy lazy reads."},
    {"key": "wikidata-100mb", "kind": "remote-lazy", "url": "https://data.graphplaza.com/wikidata-100MB/wikidata.rete", "typePredicate": "<http://www.wikidata.org/prop/direct/P31>", "label": "wikidata-100MB.rete - a real 100 MB Wikidata slice (remote, lazy)", "description": "A real ~104 MB slice of Wikidata, range-queried directly from Cloudflare R2 - the browser fetches only the dictionary chunks and index tiles each query touches (a typical selective query reads ~10 MB of the 104 MB), never the whole file. People (wd:Q5 humans) carry rdfs:label (multilingual), occupation (wdt:P106 -> e.g. physicist Q169470, philosopher Q4964182, writer Q36180, politician Q82955), date of birth (wdt:P569), place of birth (wdt:P19), citizenship (wdt:P27) and 'influenced by' (wdt:P737). Entity/property IRIs stay as wikidata.org/{entity,prop/direct}/* so nodes round-trip to live Wikidata. Pick SELECTIVE shapes (a bound subject/object, an occupation intersection) for snappy reads; aggregates over a whole predicate scan more. CC0 (Wikidata). The 1 GB version (key: wikidata) is the same idea at 10x the data."},
    {"key": "wikidata-xxl", "kind": "remote-lazy", "typePredicate": "<http://www.wikidata.org/prop/direct/P31>", "url": "https://data.graphplaza.com/wikidata-xxl/shard_0000.rete", "shards": ["https://data.graphplaza.com/wikidata-xxl/shard_0000.rete", "https://data.graphplaza.com/wikidata-xxl/shard_0001.rete", "https://data.graphplaza.com/wikidata-xxl/shard_0002.rete", "https://data.graphplaza.com/wikidata-xxl/shard_0003.rete", "https://data.graphplaza.com/wikidata-xxl/shard_0004.rete", "https://data.graphplaza.com/wikidata-xxl/shard_0005.rete"], "label": "wikidata-xxl - 600M-triple Wikidata as 6 federated .rete shards (ONE logical graph, sharded)", "description": "The SHARDED-RETE model at scale: one logical Wikidata graph too big to build as a single file, split into 6 INDEPENDENT --no-pyramid .rete shards (100M triples / ~906 MB each, ~5.4 GB total) that the playground AUTO-FEDERATES — every query fans across all 6 shards (UNION) and the rows merge, so you query it like one dataset. A genuine cross-section of the Wikidata truthy dump (people, places, works, taxa, properties) streamed partition-by-partition from the piebro/wikidata-extraction Parquet (DuckDB httpfs, no full download) and built shard-by-shard with bounded RAM/disk (stream → rete build --no-pyramid → drop the 10 GB N-Triples → next). Each entity carries multilingual rdfs:label / skos:prefLabel / schema:name + schema:description, rdf:type / wdt:P31, and truthy wdt:* statements (occupation P106, etc.); IRIs stay wikidata.org/{entity,prop/direct}/* so nodes round-trip to live Wikidata. This proves the .rete format + client-side federation scale to a dataset BIGGER THAN ANY SINGLE FILE — the path to full Wikidata (~80 shards, ~80 GB) is just more shards. Datatypes are heuristic (typed dateTime + WKT; numbers plain) since WDQS rate-limits the property-datatype map. The fed bar shows '⛓ 6 shards'. Built by scripts/build_wikidata_xxl.sh. CC0 (Wikidata)."},
    {"key": "chemotion", "kind": "remote-lazy", "url": "https://data.graphplaza.com/chemotion/chemotion.rete", "label": "chemotion.rete - the Chemotion chemistry-ELN knowledge graph (remote, lazy)", "description": "The real Chemotion Electronic-Lab-Notebook knowledge graph (FIZ Karlsruhe, 2025) merged with the two ontologies the ELN annotates with - CHMO (Chemical Methods Ontology) and RXNO (Name Reaction Ontology). 1.53M triples queried lazily over HTTP range: 87.7k instances (20.7k datasets, 20.6k studies/processes typed obo:BFO_0000015, 3.7k molecules obo:CHEBI_23367, 4.9k substances obo:CHEBI_59999, 250 creators) aligned to BFO / NFDICore / ChEBI, plus the full CHMO method taxonomy (colorimetry, amperometry, NMR/MS/IR/Raman spectroscopy ...) and RXNO reaction types as an rdfs:subClassOf DAG. Molecules carry SMILES / InChI / InChIKey / chebi:formula. Built from github.com/ISE-FIZKarlsruhe/chemotion-kg (239 MB TTL) + purl.obolibrary.org/obo/{chmo,rxno}.owl with rete build --card. CC BY 4.0. Pick selective shapes (a bound class, a formula, a subClassOf path) for snappy lazy reads; whole-predicate aggregates scan more."},
    {"key": "chebi-full", "kind": "remote-lazy", "url": "https://data.graphplaza.com/chebi-full/chebi-full.rete", "label": "chebi-full.rete - the complete ChEBI chemical ontology (remote, lazy)", "description": "The ENTIRE ChEBI ontology (Chemical Entities of Biological Interest, EMBL-EBI) as one range-queried .rete: 8.83M triples over ~205k chemical classes, their rdfs:subClassOf classification DAG (380k edges), textual definitions (obo:IAO_0000115), exact/related synonyms, ~900k database cross-references (as reified owl:Axiom annotations), and the structural data chemistry needs - molecular formula, charge, average & monoisotopic mass, InChI, InChIKey and SMILES, all under the ChemROF vocabulary (chemrof: https://w3id.org/chemrof/). Queried lazily over HTTP range: the browser fetches only the dictionary chunks and index tiles each query touches, never the 120 MB whole. CC BY 4.0 (EMBL-EBI). It shares its CHEBI_* class IRIs and rdfs:label / rdfs:subClassOf structure with the chemotion dataset, so the two FEDERATE: pick chebi-full, add chemotion as a second source (the 'Federation' example does this for you), and one SPARQL query resolves terms across both ontologies. Lossless Parquet / DuckDB / SQLite table companions sit beside it in the bucket. Pick selective shapes (a bound molecule, a formula, a subClassOf path) for snappy lazy reads; whole-predicate aggregates scan more."},
    {"key": "causenet", "kind": "remote-lazy", "url": "https://data.graphplaza.com/causenet-full-typed/causenet-full-typed.rete", "typePredicate": "<http://www.w3.org/1999/02/22-rdf-syntax-ns#type>", "label": "causenet.rete - a causality graph mined from the web (256M triples, typed + full-text, remote, lazy)", "description": "CauseNet-Full (Heindorf et al., CIKM 2020): the complete causality graph mined from Wikipedia and ClueWeb12 - 11,609,890 cause->effect relations over 12,186,310 concepts, with full extraction provenance. Each relation keeps every one of its 24,443,421 source records: the exact sentence the cause/effect pair was found in, the dependency path-pattern it was matched by (e.g. '[[cause]]/N -nsubj caused/VBD +dobj [[effect]]/N'), and the Wikipedia page or ClueWeb12 web page it came from. Concepts are cn:Concept nodes (cn: = https://causenet.org/ontology#) carrying rdfs:label; cn:causes is the direct cause->effect edge (transitive - follow it with Reach for causal chains); each relation is reified as a cn:CausalRelation with cn:support (its true source count) and cn:hasSource links to the source records. 256,133,680 triples (6.39 GB), queried lazily over HTTP range - the browser fetches only the dictionary chunks and index tiles each query touches, never the whole file. CC BY 4.0 (credit Heindorf et al.). Pick SELECTIVE shapes (a bound concept, a bound relation, the evidence for one claim) for snappy lazy reads; whole-graph aggregates scan more. Lossless relations/sources/concepts Parquet/DuckDB/SQLite companions sit beside it in the bucket."},
    {"key": "jonas", "kind": "remote-lazy", "url": "https://data.graphplaza.com/jonas/jonas.rete", "label": "jonas.rete - medieval texts & their manuscript witnesses (LostMa-ERC / Jonas)", "description": "A knowledge graph of medieval European texts and the manuscripts that transmit them, built from the LostMa-ERC Heurist database derived from Jonas (IRHT-CNRS, the repertory of texts and manuscripts in langue d'oc and d'oil). 234,017 triples over a Heurist record model: 3,738 witnesses (a text attested in a manuscript - with siglum, status, date), 1,128 texts (Lancelot, Tristan, Wolfram's Parzival, the chansons de geste; with language and literary form), 3,153 documents/manuscripts (shelfmark, collection, holding place), 4,253 text parts, 860 physical descriptions, 1,152 places (linked to GeoNames), 575 repositories (linked to VIAF), genres, stories/storyverses, and 71 reconstructed stemmata (openStemmata), over a 4,506-term controlled vocabulary. Every record links back to its live Jonas page via prop:described_at_URL. Queried lazily over HTTP range; the whole file is only ~2 MB, so reads are near-instant. CC0 1.0 (LostMa-ERC; underlying data from Jonas / IRHT-CNRS). Entity IRIs are lostma-erc.github.io/jonas/{id,prop,type,term}/*. This is a curated research subset (the LostMa corpus), not the full ~77k-witness Jonas repertory."},
    {"key": "postscriptum", "kind": "remote-lazy", "url": "https://data.graphplaza.com/postscriptum/postscriptum.rete", "typePredicate": "<http://www.w3.org/1999/02/22-rdf-syntax-ns#type>", "label": "Post Scriptum - Portuguese & Spanish everyday letters, 1500-1800 (CLUL)", "description": "Post Scriptum (CLUL, Universidade de Lisboa), via the TEITOK TEI-P5 edition: 3,649 everyday letters in Portuguese & Spanish, 1500-1800 - a historical correspondence network. Each letter (ps:Letter) carries its sender and recipient (ps:sentBy / ps:receivedBy -> 3,265 person nodes), origin & destination places (ps:sentFrom / ps:sentTo), a date (ps:date, reaching back to 1500), language, the modernised body text (full-text indexed for content search), a social/pragmatic classification (ps:letterType: personal / family / friendship / love..., ps:pragmatics: request..., ps:keyword), and foaf:page back to the live TEITOK letter page. Built from the freely-downloadable XML-TEI_P5 bundles (ES/PT x 1500-1800), the TEI source parsed by scripts/postscriptum_to_nt.py. ~63k triples, 5.3 MB, queried lazily over HTTP range. Free (CLUL recognised as author; ELRA licences). Entity IRIs are teitok.clul.ul.pt/postscriptum/{letter,person}/*."},
    {"key": "databnf", "kind": "remote-lazy", "url": "https://data.graphplaza.com/databnf-full/databnf-full.rete", "typePredicate": "<http://www.w3.org/1999/02/22-rdf-syntax-ns#type>", "label": "data.bnf.fr - the whole BnF linked open data (716M triples, ONE file)", "description": "The COMPLETE data.bnf.fr - the Bibliothèque nationale de France's linked open data (CC0) - as ONE range-queryable .rete (it was 10 federated shards). The official dump's 726,700,333 triples dedup to 673,519,782 unique (matching the BnF's live ~672M store), 194.8M terms, 7.16 GB. Every perimeter in one graph: 4.9M author/person authorities (foaf:Person) with 8.8M owl:sameAs alignments to VIAF / ISNI / IdRef (the authority hub overlapping MMM, Jonas and Biblissima), 2.6M FRBR works, the RAMEAU subject thesaurus, role-typed contributions, places (GeoNames), periodicals, and the 410M-triple modern printed-book catalogue (editions). Carries a types pyramid + planner query_stats, so summary / schema / community queries are answered index-free. Entity IRIs stay as data.bnf.fr/ark:/* so nodes round-trip to the live BnF. Built as a single file with the low-RAM pipeline that made the 726M monolith feasible (it was sharded because the monolith OOM'd a 62 GB host - see dev/low-mem-build.md). Queried lazily over HTTP range: a selective query fetches only the dict chunks and index tiles it touches, never the 7 GB whole; whole-graph aggregates scan more. CC0 1.0 (BnF)."},
    {"key": "bne", "kind": "remote-lazy", "url": "https://data.graphplaza.com/bne-full/bne-full.rete", "typePredicate": "<http://www.w3.org/1999/02/22-rdf-syntax-ns#type>", "label": "datos.bne.es - the whole Spanish National Library LOD (267M triples, ONE file)", "description": "The COMPLETE datos.bne.es - the Biblioteca Nacional de España's linked open data (CC0) - as ONE range-queryable .rete: 267,052,840 triples (the three official RDF dumps merged - autoridades 47M, bibliográficos 208M, materias 6M), 84M terms, 3.4 GB. The Spanish counterpart of data.bnf.fr. Entities at datos.bne.es/resource/* over the BNE's own ontology (datos.bne.es/def/C* classes, P* properties) plus the RDA registry; subjects are SKOS. The authority layer (persons, corporate bodies, works) carries owl:sameAs alignments to VIAF, ISNI, GND, LoC, IdRef, DBpedia AND data.bnf.fr - so it FEDERATES with the databnf dataset: add databnf as a second source ('+ Add source') and one query joins Spanish to French authorities on their shared VIAF / data.bnf.fr ids. Carries a types pyramid + planner query_stats, so summary / schema / community queries are answered index-free. Built as a single file with the low-RAM pipeline (peak ~25.6 GiB); the dump's ~64k malformed lines (IRIs with a stray '>' or a trailing space) were dropped. Entity IRIs stay datos.bne.es/resource/* so nodes round-trip to the live BNE. Queried lazily over HTTP range. CC0 1.0 (BNE)."},
    {"key": "biblissima", "kind": "remote-lazy", "url": "https://data.graphplaza.com/biblissima-full/biblissima-full.rete", "typePredicate": "<https://data.biblissima.fr/prop/direct/P2>", "label": "Biblissima+ - medieval written heritage (full Wikibase, ONE file)", "description": "Biblissima+, the French aggregator of medieval & early-modern written heritage, as ONE range-queryable .rete (it was 3 federated shards): 254,203,131 triples over 867,838 Wikibase entities - manuscripts, works, persons, places, printed editions, iconography - harvested from its open per-entity RDF export (Special:EntityData; Biblissima has no bulk dump and its SPARQL is gated). The manuscript / authority hub of this whole constellation; it overlaps the BnF, MMM and Jonas on persons & places. Wikibase model: entities at data.biblissima.fr/entity/Q*, truthy statements via prop/direct/P*, P2 = 'instance of', multilingual rdfs:label / skos:prefLabel. Now a single 667 MB file with a P2-keyed types pyramid (4 semantic-zoom levels) + planner query_stats, so it answers summary / schema / community queries index-free - built with the low-RAM pipeline that made the monolith feasible (it was sharded because the 254M build OOM'd a 62 GB host). Queried lazily over HTTP range. CC-BY 4.0 (Biblissima+, Campus Condorcet)."},
    {"key": "albala", "kind": "remote-lazy", "url": "https://data.graphplaza.com/albala/albala.rete", "typePredicate": "<http://www.w3.org/1999/02/22-rdf-syntax-ns#type>", "label": "Institución Colombina (ARCAS, Sevilla) - Seville cathedral + archdiocese archives, 69,900 ISAD records (remote-lazy)", "description": "The archival catalogue (ISAD) of the Institución Colombina, Seville: 69,900 records from the Archivo Catedral de Sevilla (ACS, 27,970) and the Archivo General del Arzobispado de Sevilla (AGAS, 41,930) - the Seville cathedral chapter and diocesan archives, with records reaching back to 1424 (Cathedral liturgical codices) and 1510. 927,788 triples queried lazily over HTTP range. Each record (arcas:ArchivalRecord) carries a title (dcterms:title), one or more dates (dcterms:date), its signatura / call number (arcas:signatura - boxes, 'Caja / NNNN'), the ISAD reference and classification codes (arcas:referenceCode, arcas:classificationCode), its archival level (arcas:level, e.g. 'Unidad documental compuesta'), the archive it belongs to (arcas:inArchive) and its parent in the description hierarchy (dcterms:isPartOf - so the fonds/series/file/item tree reconstructs). A full-text index over the titles powers content search (the marriage-licence series of the Archdiocese, liturgy, capellanías...). Harvested anonymously from the public ARCAS consultation (Baratz 'Albalá 7', a Solr-backed JS portal at albala.icolombina.es) - metadata only; the digital images are not publicly available. Entity IRIs are albala.icolombina.es/arcas/{record,archive}/*. © Institución Colombina (public catalogue metadata)."},
    {"key": "memoria", "kind": "remote-lazy", "url": "https://data.graphplaza.com/memoria/memoria.rete", "typePredicate": "<http://www.w3.org/1999/02/22-rdf-syntax-ns#type>", "label": "Memòria - Spanish Civil War victims & mass graves (open data)", "description": "An aggregation of openly-licensed Spanish Civil War 'memoria histórica' datasets into ONE knowledge graph: 99,542 named victims/repressed persons and 2,872 mass graves (fosas), tied together through provinces. 1.21M triples, remote-lazy over HTTP range. Persons (mc:Victim) come from three regional registries - Catalonia's reparació jurídica de víctimes del franquisme (69,834), Catalonia's desapareguts de la Guerra Civil (8,339) and Euskadi's víctimas mortales (21,369) - each carrying name (rdfs:label), sex (mc:sex), age (mc:age), birth & residence municipality/province (mc:bornInMunicipality / mc:bornInProvince / mc:residedIn*), cause/manner of death (mc:cause, e.g. 'Gudari/miliciano muerto en combate', 'Ejecutado por Consejo de Guerra'), sentence (mc:sentence), profession, military unit, and an executed flag (mc:executed). Mass graves (mc:MassGrave) come from the Catalan (1,027, with WGS84 mc:lat/long + GeoSPARQL geometry), Andalusian (615, with historical narratives mc:narrative), Castilla y León (701, with repressor side) and Valencian (529) fosas registries - each with municipality, province (mc:province), victim count (mc:victimCount), status and a link to its official record (foaf:page). 88 provinces (mc:Province) link victims and graves, so you can ask 'which provinces have both the most victims and the most graves'. A full-text index over names powers person search. IRIs are memoria.rete/{persona,fosa,provincia}/*. Aggregated from regional open-data portals (transparenciacatalunya.cat, opendata.euskadi.eus, juntadeandalucia.es, jcyl.es, gva.es) - all CC-BY-style open data; sensitive personal/historical data, attribute the source administrations. NB this is the open regional data, NOT a scrape of any search portal."},
    {"key": "bph", "kind": "remote-lazy", "url": "https://data.graphplaza.com/bph/bph.rete", "typePredicate": "<http://www.w3.org/1999/02/22-rdf-syntax-ns#type>", "label": "Embassy of the Free Mind - 2,307 hermetic & esoteric books (Bibliotheca Philosophica Hermetica, remote, lazy)", "description": "The digitized core of the Bibliotheca Philosophica Hermetica - the Ritman Library / Embassy of the Free Mind in Amsterdam, the world's foremost collection on Hermetica, alchemy, Rosicrucianism, Kabbalah, mysticism and Western esotericism (partly funded by Dan Brown). 2,307 rare books (15th-19th c.) harvested KEYLESS from Source Library (sourcelibrary.org, the collection's own reading platform) with full cataloguing: original + English title, author (a linked schema:Person), year (schema:datePublished), publisher / place of publication, language (German 929, Latin 585, English, French, Dutch, Italian, Hebrew...), shelfmark, bibliography references and USTC catalogue links. Every book also carries an AI-written summary + reading-summary and four significance scores (historical / scholarly / visual / accessibility, 0-25 each) plus a 0-100 quality score, a subject index (126,752 dcterms:subject links to SKOS concepts), chapter structure, geocoded publication places (GeoSPARQL points - switch Output to Map) and related-book links. 764,708 page-image URLs render inline from images.sourcelibrary.org (CORS-open), plus one cover thumbnail per book. 5.77M triples, remote-lazy over HTTP range. Modeled with schema.org / Dublin Core / SKOS / GeoSPARQL by scripts/fetch_bph.py + scripts/bph_to_nt.py. CC BY-SA 4.0 - credit the Embassy of the Free Mind / BPH. Full per-page OCR + English translation text (also CC BY-SA 4.0) is gated behind a free Source Library API key and can be added as a second layer."},
    {"key": "ustc", "kind": "remote-lazy", "url": "https://data.graphplaza.com/ustc/ustc.rete", "typePredicate": "<http://www.w3.org/1999/02/22-rdf-syntax-ns#type>", "label": "USTC - Universal Short Title Catalogue (727 Embassy/BPH-cited editions, remote, lazy)", "description": "A bounded, federation-ready extract of the Universal Short Title Catalogue (ustc.ac.uk, University of St Andrews) - the authoritative bibliography of early-modern European print. Exactly the 727 editions the Embassy of the Free Mind / BPH collection cites (by USTC number), harvested from the public edition pages as a FAIR-USE research subset: USTC is a copyrighted scholarly database with no open licence and no bulk API, so this is 727 of ~1M records - NOT a copy of the catalogue (© USTC / University of St Andrews). Each edition (ustc:Edition, IRI = https://www.ustc.ac.uk/editions/{sn}) carries its standardized title, imprint, place/country/region, year, format (2°/4°/8°/12°...), pagination, authors (with roles) and printers (Lukas Jennis, Lazarus Zetzner's heirs and the other great alchemical presses), USTC classifications (Religious, Science, Medical, Philosophy, Witchcraft & Demonology...), language(s) (Latin, German, French, Dutch, English, Greek, Hebrew), 10,779 library HOLDINGS across 1,146 institutions (Herzog August Bibliothek Wolfenbüttel, British Library, Bayerische Staatsbibliothek, SBB Berlin...) each with shelfmark, and 891 links to DIGITAL copies (Google Books, ProQuest EEB, Cervantes Virtual...). 88,452 triples, remote-lazy. Built to FEDERATE with the bph dataset - the edition IRI is exactly bph's rdfs:seeAlso target and every edition back-links u:embassyBook -> the Embassy book, so one cross-source query joins the Embassy's cover + AI summary to USTC's imprint, printers and library holdings. schema.org / Dublin Core / SKOS. Attribute USTC / University of St Andrews."},
    {"key": "fuero_juzgo", "kind": "remote-lazy", "url": "https://data.graphplaza.com/fuero_juzgo/fuero_juzgo.rete", "typePredicate": "<http://www.w3.org/1999/02/22-rdf-syntax-ns#type>", "label": "Fuero Juzgo — the manuscript tradition: 52 witnesses, IIIF, Wikidata & Biblissima (remote, lazy)", "description": "The manuscript tradition of the FUERO JUZGO — the 13th-century Romance (Castilian / Leonese / Asturian) translation of the Visigothic Liber Iudiciorum, made c. 1260 under Fernando III. Compiled from Mónica Castillo Lluch's Índice de fueros (UNIL) and PhiloBiblon (BETA, texid 1191): all 46 PhiloBiblon witnesses + the Galician López Ferreiro fragment + 3 BOOST-only manuscripts + 2 twelfth-century Catalan fragments of the Libre jutge = 52 testimonies, each with shelfmark (fjo:siglum), holding institution, dialect (leonés occidental/oriental, asturiano, castellano, agallegado…), an integer date range (fjo:notBefore/notAfter, so you can query 'all 13th-c. witnesses'), PhiloBiblon manid/anaid, and — where recorded — its incipit, prologue incipit, explicit and scribal colophon (the shared prologue 'Con cuidado del amor de Cristo… de don Sisnando' and the 'Pedro Martiz Gualego me scripso' colophon thread through the tradition). The manuscript STEMMA is modelled (fjo:copyOf links the 18th-c. antiquarian copies BNE 683 & 1681 back to their medieval exemplars — the Códice Murciano, Escorial d.III.18 & P.II.17). 12 EDITIONS (Villadiego 1600 → Llorente 1792 → Reguera 1798 → the RAE 1815[1817] academic edition promoted by Jovellanos → Orazi, Perona, Romero Cambrón…) link by fjo:editsWitness to the exact codices they establish; 11 STUDIES (Menéndez Pidal, Gessner, Fernández Llera, Castillo Lluch, García Martín…) link by fjo:studies. 12 DIGITIZED witnesses carry a IIIF manifest (fjo:iiifManifest) that renders as an in-table facsimile viewer — both Munich BSB codices, the BnF Espagnol 256 (Gallica), the Bodleian Holkham misc. 46 and all 8 Escorial codices (Real Biblioteca Digital); the flagship is BSB Cod.hisp. 28 (365 canvases, CC Public Domain Mark). Works, institutions, persons and places carry owl:sameAs to Wikidata (so the graph FEDERATES with the wikidata datasets and, via the cross-source join, with live Wikidata — see the map example), and two witnesses are owl:sameAs Biblissima+ entities (BnF Esp. 256 = Q44277, BSB Cod.hisp. 6 = Q264558), bridging to the `biblissima` dataset. A small purpose-built ontology (fjo:, Work/Witness/Edition/Study/Person/Institution/Place, aligned to Dublin Core, FOAF, SKOS, schema.org) travels inside the file. ~1.5k triples, remote-lazy over HTTP range (served from Cloudflare R2, data.graphplaza.com). Built by scripts/fuero_juzgo_to_nt.py from the two curated sources + verified IIIF/Wikidata/Biblissima lookups — no scraping. Scholarly compilation; credit Mónica Castillo Lluch (Índice de fueros, UNIL) and PhiloBiblon; BSB Cod.hisp. 28 images CC PDM 1.0."},
    {"key": "ramon_llull", "kind": "remote-lazy", "url": "https://data.graphplaza.com/ramon_llull/ramon_llull.rete", "typePredicate": "<http://www.w3.org/1999/02/22-rdf-syntax-ns#type>", "label": "BVPB - Ramón Llull: 142 digitised editions & manuscripts (remote, lazy)", "description": "The complete Ramón Llull collection of the Biblioteca Virtual del Patrimonio Bibliográfico (BVPB, Spanish Ministry of Culture): 142 digitised works by the Mallorcan polymath Ramón Llull (Doctor Illuminatus, ca. 1232-1315) - manuscripts and early printed editions (14th-18th c.) of the Ars, Arbor Scientiae, Ars Brevis, Ars generalis ultima, the Liber magnus contemplationis in Deum and the rest of the Lullian corpus, in Latin, Catalan and Castilian. Each work is an edm:ProvidedCHO carrying full Dublin Core - title, creator/contributor, date, publisher & place of printing, language, subject, physical description (dc:format), coverage and rights - plus its BVPB control number, edm:isShownAt (the catalogue landing page), edm:isShownBy (the whole digitised object as a single PDF, served from bvpb.mcu.es) and foaf:page (the page-image viewer). The 71 PRINTED editions are additionally FULL-TEXT SEARCHABLE: every one was OCR'd page by page (Tesseract lat + a dictionary-guided long-ſ correction that turns 'fapientia' back into 'sapientia'), ~8.7M words held as bvpb:fulltext and backed by the .rete text index, and given a IIIF v3 manifest (bvpb:iiifManifest) that opens a page-turning viewer with the OCR word layer over the facsimile - try the '🔎 Full-text search' and '📖 IIIF viewers' examples. Manuscripts are metadata-only (Tesseract can't read the hands - that needs HTR). 2,877 triples (25.7 MB with the text layer), remote-lazy over HTTP range. Harvested from the BVPB Ramón Llull microsite by scripts/bvpb_ramonllull.py, modelled by scripts/bvpb_to_nt.py, OCR/IIIF by scripts/bvpb_ocr_iiif.py. CC BY 4.0 - credit the Biblioteca Virtual del Patrimonio Bibliográfico (Ministerio de Cultura, España). Shares the early-modern rare-book domain with the bph and ustc collections."},
    {"key": "peirce", "kind": "remote-lazy", "url": "https://data.graphplaza.com/peirce/peirce.rete", "typePredicate": "<http://www.w3.org/1999/02/22-rdf-syntax-ns#type>", "label": "Charles S. Peirce papers - the Houghton finding aid as a graph, with 45k IIIF pages (remote, lazy)", "description": "The complete finding aid of the Charles S. Peirce papers at Houghton Library, Harvard (MS Am 1632, finding aid hou02614): the working papers of the philosopher, logician, scientist and founder of pragmatism (1839-1914), 74 linear feet in 166 boxes. All 2,527 archival components as a graph: the finding-aid tree (Collection > Series > Subseries > Component via dcterms:isPartOf - I. Manuscripts with Richard Robin's topical arrangement Mathematics/Pragmatism/Phenomenology/Logic/Metaphysics/..., II. Correspondence, III. Supplementary, IV. Microfilm), each component with its full title, parsed short title, genre ('autograph manuscript', 'typescript'...), date text + integer year range parsed from the title tail, Robin arrangement number (a:msNumber - the 'R numbers' Peirce scholars cite), physical description, and its Box (127 box entities - shelf archaeology). The CORRESPONDENCE parses into a directed network: ~690 letter components with a:sender / a:addressee / a:letterCount over 729 Person nodes - W. P. Garrison of The Nation (216 letters), Paul Carus of The Monist (112), brother James Mills Peirce (87), William James, the Smithsonian... Peirce himself is owl:sameAs Wikidata Q187520. 423 components are DIGITIZED end to end: a:iiifManifest (IIIF Presentation v3, 45,423 page images served by Harvard's image servers) renders as the in-table page viewer, with a:pageCount and a a:thumbnail per object - read 'The Simplest Mathematics' (822 pp.) in the results table. 36,179 triples, remote-lazy over HTTP range, full-text index over the titles. Harvested as ONE official CSV export the catalog page itself offers (no crawling) + the public IIIF manifests; metadata CC0 under Harvard Library's open-metadata policy, finding aid (c) President and Fellows of Harvard College."},
    {"key": "boe", "kind": "remote-lazy", "url": "https://data.graphplaza.com/boe/boe.rete", "typePredicate": "<http://www.w3.org/1999/02/22-rdf-syntax-ns#type>", "label": "BOE — Legislación Consolidada: every in-force Spanish law (12,330) as an ELI citation graph (remote, lazy)", "description": "The ENTIRE consolidated-legislation corpus of Spain's Boletín Oficial del Estado — every actively-maintained norm in force (State + the autonomous communities + the foral regimes), 12,330 in all, as a European-Legislation-Identifier (ELI) knowledge graph. Each norm sits at its REAL canonical ELI IRI (https://www.boe.es/eli/es/l/2015/10/01/40 = Ley 40/2015 de Régimen Jurídico del Sector Público), typed eli:LegalResource and by its rango (bx:Ley, bx:LeyOrganica, bx:RealDecreto, bx:RealDecretoLey, bx:Orden, bx:Reglamento… — 19 classes rdfs:subClassOf eli:LegalResource, so `rete`'s type-pyramid communities ARE the kinds of norm), carrying eli:title, eli:id_local (the BOE-A-… identifier), eli:number, eli:date_document / date_publication / first_date_entry_in_force (typed xsd:date), eli:passed_by → its issuing department (149 organisations — Jefatura del Estado & the ministries + the CCAA governments), eli:jurisdiction (Estatal/Autonómico) and eli:is_about → its subjects (drawn from the BOE's 6,829-term SKOS materia thesaurus). The heart of it is the NORM-TO-NORM CITATION / DEROGATION NETWORK mined from the BOE's own análisis: ~100,000 edges typed with the STANDARD ELI relations — eli:repeals (deroga, 19.5k), eli:amends (modifica, 32.9k), eli:corrects (corrige), eli:cites, eli:transposes (1,042 EU-directive transpositions), eli:based_on (dictada en virtud de) — plus a small bx: extension for BOE-specific acts (publishes / approves / authorizes / ratifies / challenges / declares / interprets). So you can ask which laws have been reformed most (the Ley de Demarcación y Planta Judicial — 134 amendments; the IRPF and IVA tax codes; the Ley de Enjuiciamiento Civil), what the 1978 Constitution repealed (the Franco-era Leyes Fundamentales: the Ley de Sucesión en la Jefatura del Estado, the Fuero de los Españoles, the Ley Orgánica del Estado, the Ley 1/1977 de Reforma Política…), or which norms have been most challenged before the Constitutional Court (the Código Penal — 51 recursos/cuestiones). Referenced norms outside the in-force set (already-repealed or pre-consolidation, including Gaceta-era laws back to the 19th c.) become 28,796 lightweight labelled nodes so no derogation edge dangles. A full-text index over all 41k titles finds every norm on 'inteligencia artificial', 'cambio climático' or 'protección de datos'. 465k triples, 7.9 MB, remote-lazy over HTTP range on Cloudflare R2. Built from the OFFICIAL BOE Open-Data API (the legislación-consolidada enumeration + a per-norm /análisis call + the /datos-auxiliares controlled vocabularies) by scripts/boe_to_nt.py — no scraping. Because the norms carry real ELI IRIs and the ELI ontology, the graph federates with EUR-Lex and the other national ELI publishers. Reuse under Ley 37/2007 + the BOE standard reuse licence (Resolución 27-jun-2024): credit 'Basado en datos de la Agencia Estatal Boletín Oficial del Estado'."},
    {"key": "scrolls", "kind": "remote-lazy", "url": "https://data.graphplaza.com/scrolls/scrolls.rete", "typePredicate": "<http://www.w3.org/1999/02/22-rdf-syntax-ns#type>", "label": "Vesuvius Challenge - Herculaneum scrolls: the data browser as a graph, with download links (remote, lazy)", "description": "The open-data registry that powers the scrollprize.org Data Browser, turned into a provenance graph: the carbonised Herculaneum papyri buried by Vesuvius in AD 79 and now read by X-ray CT + machine learning (the Vesuvius Challenge). 45 physical samples - 27 Scrolls (PHerc0332 = Scroll 3, PHercParis4 = Scroll 1, PHerc0172 = Scroll 5...) and 9 Fragments - each linked through the full digitisation pipeline: Sample <- Scan (63 synchrotron micro-CT sessions at Diamond Light Source and ESRF Grenoble, with beam energy in keV and voxel size in µm) <- Volume (66 reconstructed OME-Zarr CT volumes, with voxel shape and inter-volume registration edges vs:alignsTo) <- Segment (204 traced papyrus surfaces, with bounding boxes and vs:coversVolume overlap) <- DataAsset (2,336 downloadable files). EVERY DataAsset IRI IS ITS PUBLIC DOWNLOAD URL on the open-data bucket - the OME-Zarr volumes, OBJ meshes, tifxyz coordinate maps, surface-volume layer zarrs and the ink-detection prediction TIFFs - so a query result is a working link (the downsampled ink previews are .jpg and render inline as thumbnails). 9 ML Models (TimeSformer, nnU-Net, ResNet3D, DINO-3D for ink detection / surface prediction) are joined to the predictions they produced via vs:usedModel. Sample photographs render as thumbnails via the same Thumbor service the data browser uses. 18,125 triples, remote-lazy over HTTP range. Built from the bucket's metadata.json (the exact registry the React data browser consumes) by scripts/scrolls_to_nt.py - no scraping. CC BY-NC 4.0 (a few older EduceLab volumes under the EduceLab Data License) - cite EduceLab-Scrolls / the Vesuvius Challenge dataset per scrollprize.org/data#how-to-cite."},
    {"key": "bcul", "kind": "remote-lazy", "url": "https://data.graphplaza.com/bcul/bcul.rete", "label": "bcul.rete — BCU Lausanne: a digital twin of the whole library, incl. where every book is held (remote, lazy)", "description": "A range-queryable graph twin of the Bibliothèque cantonale et universitaire de Lausanne, aggregating four of its public interfaces into one 117.5M-triple graph. The ENTIRE Renouvaud catalogue — 3.55M bibliographic records mirrored from the Alma SRU endpoint of the Vaud library network (zone 41BCULAUSA_LIB) — carries full physical HOLDINGS: every book resolves to WHICH library holds it (BCUL sites Unithèque / Riponne / Internef, the Archives cantonales vaudoises, the Musée des beaux-arts, and dozens of faculty, gymnasium and municipal libraries), its shelving location, call number and live availability (available / unavailable / check_holdings) via 4.9M b:heldAt edges to shared Library nodes. Plus the Patrinum digital-heritage repository (332k digitized books, archives, iconography, Fonds Roud, Personnalités Vaudoises — OAI-PMH), the 15 medieval manuscripts BCUL has on e-codices (IIIF + TEI + DOI), and 331 digitized-press titles from Scriptorium (24 Heures & the Feuille d'avis de Lausanne back to 1762). Modelled with schema.org + Dublin Core and connected through 2.7M author/agent nodes, 2.44M subject concepts, places and the Library nodes; 334k items link to a digitized thumbnail, 15 to a IIIF manifest. Harvested entirely from the library's public APIs (no scraping); rights vary per source (dcterms:rights per record). Engineering: Alma SRU caps every query at 10,000 records → a recursive mms_id-space bisection (594 leaves) yields a provably-complete mirror; ~3s SRU latency → a 12-way concurrent harvest."},
    {"key": "ecal", "kind": "remote-lazy", "url": "https://data.graphplaza.com/ecal/ecal.rete", "label": "ecal.rete — ECAL: the art & design school library, as a graph (remote, lazy)", "description": "A range-queryable graph twin of the ECAL Library — the library of the École cantonale d'art de Lausanne, one of the world's leading art & design schools. 28,741 records (1.02M triples) across Art visuel, Photographie, Design graphique & industriel, Cinéma and Théorie: a tightly-curated collection heavy on Le Corbusier, the International Typeface Corporation, artist's books, auction catalogues (Sotheby's) and film. 93% carry a book cover, rendered inline. Every item links to shared author / subject / place nodes and to its section + call number (the shelf), and 3,014 films sit alongside 25,700 books. Same schema.org + Dublin Core ontology as the BCU twin, so the two federate. Harvested politely from the BiblioMaker OPAC (metadata only); the catalogue asks not to be crawled, so this was a gentle, low-rate, authorized harvest."},
    {"key": "gbif-birds", "kind": "remote-lazy", "url": "https://data.graphplaza.com/gbif-birds/gbif-birds.rete", "typePredicate": "<http://www.w3.org/1999/02/22-rdf-syntax-ns#type>", "label": "GBIF Birds — Spain & Switzerland: 50M sightings with a taxonomic zoom + map (remote, lazy)", "description": "Fifty MILLION georeferenced bird (Aves) occurrences — Spain 47.9M, Switzerland 2.26M, 1,104 species — from the GBIF Open Data snapshot (2026-07-01), as one 334M-triple range-queryable file (1.53 GB, remote-lazy over HTTP range). This is the SEMANTIC-ZOOM dataset: every occurrence is rdf:type its species, and each species carries a full taxonomic subClassOf chain Species ⊑ Genus ⊑ Family ⊑ Order ⊑ Aves ⊑ Chordata ⊑ Animalia — so the schema pyramid rolls the 50M sightings up into a 6-LEVEL zoom you can read WITHOUT touching the triple index (open the Schema tab): level 3 = Orders (Passeriformes 29.7M, Charadriiformes 5.1M, Accipitriformes/raptors 2.9M, Anseriformes 2.2M …), level 4 = Families (Fringillidae, Muscicapidae, Accipitridae, Laridae …), level 5 = the 1,104 species. Ask a question about birds at the Order or Family level and you never descend to a single record — the abstraction is the point. ENRICHED build: every species now also carries its COMMON NAME in up to 47 European languages (27,922 language-tagged dwc:vernacularName — French, Catalan, Basque, Welsh, Breton … all present), its IUCN Red List category (967 species; 165 flagged v:sensitive as near-threatened or worse), and precomputed occurrence stats (recordCount, estimatedIndividuals, first/last year). And it is an RDF-STAR showcase: each ADDED fact is a quoted triple annotated with its provenance — << species dwc:vernacularName '…' >> dct:source 'Catalogue of Life' ; prov:generatedAtTime '2026-07-11' — statements about statements, the natural home for who-said-it-and-when. Every occurrence also carries a GeoSPARQL POINT (map view), its country, year, basis-of-record (mostly citizen-science HUMAN_OBSERVATION — eBird & friends), source dataset and its dwc:individualCount where the observer recorded one (33.4M records). IRIs round-trip to live GBIF: occurrence = gbif.org/occurrence/<id>, species owl:sameAs gbif.org/species/<key>. Built KEYLESS from the GBIF AWS Parquet snapshot via DuckDB (scripts/gbif_birds_extract.py + gbif_birds_to_nt.py); names from the GBIF vernacular API, conservation from the IUCN Red List via Wikidata; enrichment emitted as N-Triples-star (scripts/gbif_birds_enrich_nt.py). CC BY-NC 4.0 (GBIF); cite GBIF.org. Selective queries (a species, a small area), the Schema pyramid, and every enrichment lookup are cheap over HTTP range; a full 50M-row aggregate scans the file, so lean on the pyramid for the big picture."},
    {"key": "worldcup", "kind": "remote-lazy", "url": "https://data.graphplaza.com/worldcup/worldcup.rete", "label": "worldcup.rete — FIFA World Cup 2022: stats, player careers & multi-source predictions (remote, lazy)", "description": "The 2022 FIFA World Cup as one queryable knowledge graph, weaving together three things you rarely see in the same place. STATS: all 64 matches (group + full knockout) with scores, penalty shootouts, ~170 goals and their scorers, 8 stadiums with coordinates, and the final standings — Argentina champions, France runners-up. PLAYER EVOLUTION: club-by-club career timelines with start/end years (linked to Wikidata) for the marquee players, so you can trace Messi from Newell's Old Boys to Inter Miami. PREDICTIONS: what different forecasters said would happen, as timestamped, source-attributed claims — FiveThirtyEight's SPI model across 6 snapshots (pre-tournament through the quarter-finals, so you can watch Argentina's title probability dip after the shock Saudi Arabia loss and climb back), plus pre-tournament bookmaker and Opta champion odds. Each prediction is an n-ary node carrying probability + source (PROV-O) + forecast date, so you can compare parties, track how the odds moved, and score who called it best against the real result. Modelled with schema.org + Dublin Core + GeoSPARQL + a small wc: vocab on w3id.org/rete. Open data (OpenFootball, FiveThirtyEight CC-BY, Wikidata CC0)."},
    {"key": "worldcup2026", "kind": "remote-lazy", "url": "https://data.graphplaza.com/worldcup2026/worldcup2026.rete", "label": "worldcup2026.rete — FIFA World Cup 2026: a LIVE snapshot, cross-checked vs live sources (results + player careers)", "description": "The 2026 FIFA World Cup (Canada · USA · Mexico; 48 teams, 12 groups, Round of 32) as a LIVE, IN-PROGRESS knowledge graph. ⚠ Snapshot: the group stage, Round of 32, Round of 16 and quarter-finals are played (100 matches) — the semi-finals (France v Spain, England v Argentina) and final are still to come, modelled as scheduled fixtures. ✓ Cross-checked against live sources (Yahoo Sports, CBS, FIFA) on 2026-07-12: the results, the four semi-finalists (France, Spain, England, Argentina) and the bracket all match — though OpenFootball's community data can carry the odd scorer-level error (it credits Lisandro rather than Lautaro Martínez for Argentina's late QF goal). It holds every played match with scores, penalty shootouts, goals and scorers (Messi & Mbappé lead the race on 8), 16 stadiums across three countries with coordinates, and club career timelines (linked to Wikidata) for the top scorers. Unlike the 2022 twin there is NO prediction layer — FiveThirtyEight is retired and no 2026 forecast time-series is available. schema.org + Dublin Core + GeoSPARQL + a wc: vocab on w3id.org/rete."},
    {"key": "tracking", "kind": "remote-lazy", "url": "https://data.graphplaza.com/tracking/tracking.rete", "label": "tracking.rete — a full match as a spatiotemporal graph (player + ball 2D positions over time)", "description": "A full football match as a SPATIOTEMPORAL graph: the 2D position of every player and the ball, ~5 frames/second across ~94 minutes, on a 105x68 m pitch — 2.55M triples, ~637k position rows. From Metrica Sports' open sample data (anonymised). Each position carries tr:t (seconds), tr:x, tr:y and tr:object, so you can query any object's location in any time window — the basis for a 2D movement replay (a query returns a frame; interpolate between frames to animate). The 'movement over time' at the finest grain, complementing the tournament-scale forecasts and career-scale transfers in the World Cup graphs. tr: vocab on w3id.org/rete."},
    {"key": "subtitles", "kind": "remote-lazy", "url": "https://data.graphplaza.com/subtitles/tears_of_steel.rete", "label": "subtitles.rete — one film, 20 languages, over time (Tears of Steel · CC-BY)", "description": "Tears of Steel — the Blender Foundation's open sci-fi short — subtitled in TWENTY languages, as one temporal + multilingual knowledge graph (14,376 triples). It's the 'over time' idea applied to language. Every su:Line is a MOMENT in the film (su:start / su:end in seconds) carrying the SAME utterance in all 20 tongues as language-tagged literals: English, Spanish, German, French, Italian, Dutch, Russian, Polish, Czech, Hungarian, Greek, Portuguese (BR), Danish, Indonesian, Norwegian, Japanese, Chinese (Simplified + Traditional), and right-to-left Persian & Hebrew. Each su:Cue is one language track's own timed segment (its real start/end + su:text \"…\"@lang), aligned to the English original by time-overlap — 1,536 cues over 76 lines. So you can scrub a timeline and watch the dialogue appear in every language at once, ask 'what's on screen at 1:05 in Japanese and Persian', diff how one line was rendered across 20 cultures, or full-text-search the dialogue (a TEXT_INDEX is baked in). Subtitles are all timed to the same audio, so the shared timeline IS the cross-language alignment. Open data: Tears of Steel (CC BY 3.0, Blender Foundation, mango.blender.org), subtitle .srt files from media.xiph.org/mango. su: vocab + schema.org + Dublin Core on w3id.org/rete."},
    {"key": "mtg", "kind": "remote-lazy", "url": "https://data.graphplaza.com/mtg/mtg.rete", "label": "mtg.rete — Magic: The Gathering: the whole card pool as a graph (34,633 cards, with images)", "description": "The entire Magic: The Gathering card pool as a queryable knowledge graph — 34,633 unique cards (Oracle-level concepts, from MTGJSON's AtomicCards, MIT-licensed) richly interlinked with their colors, card types, subtypes, keywords, mana, formats and named+dated sets, each carrying its Scryfall card IMAGE, full Oracle text and official rulings (76,990 of them). Card types are OWL classes (Creature, Instant, Sorcery, Land, Artifact, Enchantment, Planeswalker … rdfs:subClassOf mtg:Card) and every card rdf:type its types, so the Schema tab is a card-type zoom and the reasoner has a real TBox. Oracle text lives in a TEXT_INDEX, so you can find cards by what they DO ('draw two cards', 'destroy target creature'). Ask for red creatures with flying legal in Modern (and see the cards), every Goblin or Dragon ranked by EDHREC popularity, the colour pie, legendary planeswalkers, cards on the Reserved List, or what a set contains. This is the card-semantics layer from the semantic-MTG-engine plan (building on cmdoret/mtg_ontology): the executable rules engine is a separate deterministic system, but the cards ARE a graph — a big, fun, richly-joined one. mtg: vocab on w3id.org/rete + schema.org; card data MIT (MTGJSON), images © Wizards of the Coast served via Scryfall's CDN. Cyril Matthey-Doret's mtg_ontology is merged in as the TBox foundation — each card type is owl:equivalentClass its class in his ontology, and colours/keywords link to his concepts, so you can query the cards THROUGH his vocabulary."},
    {"key": "mtg-game", "kind": "remote-lazy", "url": "https://data.graphplaza.com/mtg-game/mtg_game.rete", "label": "mtg-game.rete — a live Magic game state as OWL + SPARQL (no engine)", "description": "The DECLARATIVE half of the semantic-MTG-engine plan, in miniature: a runtime OWL ontology (GameState, Player, Zone, ZoneMembership, CardInstance, SpellInstance, the Stack), a handful of cards modelled with their real STRUCTURED abilities (cost / condition / effect / action — the way cmdoret/mtg_ontology intends, e.g. Bonders' Enclave's “{3},{T}: draw a card, only if you control a creature with power ≥ 4”), and one concrete match — Alice vs Bob — captured as two immutable snapshots: state S1 (Alice's turn 3, main phase) and its successor S2 after she casts Lightning Bolt at Bob's Grizzly Bears (the spell now sits on the stack, a Mountain is tapped). There is NO game engine here — just OWL classes and data — yet plain SPARQL answers the plan's question families: DESCRIBE the board, VALIDATE that every card instance is in exactly one zone, decide LEGALITY (which instants Alice can actually pay for from her untapped lands), read what each card DOES from its ability structure, and diff S1→S2. Card concepts reuse the 34k-catalogue IRIs (the `mtg` dataset), so the two graphs are one entity space. mtgg: runtime vocab on w3id.org/rete, building on Cyril Matthey-Doret's mtg_ontology (mtgo: structural classes + MTG-wiki action classes). CC BY 4.0. Its companion `mtg` is the whole card pool; this is one match modelled in full."},
    {"key": "arxiu", "kind": "remote-lazy", "url": "https://data.graphplaza.com/arxiu/arxiu.rete", "label": "arxiu.rete — Arxius en Línia: complete Catalan archives, with scanned documents", "description": "Complete Catalan archives from the Generalitat de Catalunya's online archives (Arxius en Línia), harvested one whole archive at a time and merged into a single graph. Four complete archives so far — the Arxiu Nacional de Catalunya (431,776 units), the Arxiu Comarcal de l'Anoia (137,054), the Arxiu Comarcal del Baix Llobregat (53,644) and the Arxiu Històric de Tarragona (7,401) — 629,875 archival units across 507 fonds. Each archival unit carries its holding archive, fonds, reference code, dates, description AND its scanned document: non-reserved scans are shown as a downsized WebP (~180 KB, ~15% of the ~1.5 MB original) mirrored to R2 so they render inline in query results, and every unit keeps arx:originalImage → the original full-res JPEG on Gencat's own server (the source of record). Reserved documents — 56% of Tarragona, 93% of Baix Llobregat, 99% of Anoia, 73% of the ANC — keep only the link, respecting the holding archives' rights (so ~117,940 units render a WebP — photographs plus PDF documents shown by their rendered first page; the rest are metadata + the original Gencat link). The same open API holds ~1.43M image-records across ~65 archives (~1.5 TB); this graph grows archive by archive. Modelled with schema.org archival vocabulary (schema:ArchiveComponent held by an ArchiveOrganization, part of a Collection/fonds) on w3id.org/rete. Descriptions are openly reusable (Llicència oberta d'ús d'informació / Llei 37/2007, Generalitat de Catalunya, with attribution)."},
  ],
  // Structured metadata for the dataset table. `triples`: exact for bundled
  // graphs (from `rete info`), approximate string for the big remote ones, null
  // when unknown. `source`: where the data came from ("" = synthetic / internal).
  // Type (Bundled vs Remote) is derived from each dataset's `kind`.
  datasetMeta: {
    "tracking":              { triples: "2.55 M", size: "16 MB", license: "Metrica Sports open sample data", source: "https://github.com/metrica-sports/sample-data", provenance: "Metrica Sports open sample data (Sample Game 2), 25 fps raw tracking downsampled to 5 fps, scaled to a 105x68 m pitch. scripts/tracking/build_tracking_graph.py -> N-Triples (tr: vocab) -> rete build. A 2D replay viewer (web/pitch.template.html) plays it back on a canvas pitch." },
    "subtitles":             { triples: "14.4 K", size: "187 KB", license: "CC BY 3.0 (Blender Foundation)", source: "https://media.xiph.org/mango/subtitles/", provenance: "Built 2026-07-13 from the 20 official .srt subtitle files at media.xiph.org/mango/subtitles/ (Tears of Steel, Blender 'Mango' open movie, CC BY 3.0). scripts/subtitles/build_subtitles_graph.py parses each .srt (BOM/encoding auto-detect incl. UTF-16, tolerant timecode regex, handles double-spaced files), emits per-language su:Cue nodes keyed by zero-padded start-ms (time-ordered = contiguous range reads) plus su:Line 'moment' nodes (English pivot) carrying every language's overlapping text as @lang literals; foreign cues aligned to lines by max time-overlap. rete build --pyramid-algo types --text-index --card. 20 languages, 1,536 cues, 76 lines." },
    "mtg":                   { triples: "1.23 M", size: "16.3 MB", license: "Card data MIT (MTGJSON); images © WotC via Scryfall; ontology GPL-3.0 (cmdoret)", source: "https://mtgjson.com/", provenance: "Built 2026-07-13 from MTGJSON AtomicCards (Oracle-level, 34,633 cards) + SetList (named/dated sets) + AtomicCards rulings (76,990) + Scryfall oracle-cards bulk (card images), with cmdoret/mtg_ontology (GPL-3.0) merged in as the TBox. scripts/mtg/build_mtg_graph.py maps each card to colors, card types (OWL classes rdfs:subClassOf mtg:Card, owl:equivalentClass the cmdoret class), subtypes, keywords (bridged to Cyril's concepts), legalities, sets, rulings and Oracle text. rete build (mtg.nt + mtg_ontology.owl.ttl) --pyramid-algo types --text-index --card. The card-semantics layer of a semantic MTG engine." },
    "mtg-game":              { triples: "341", size: "55 KB", license: "CC BY 4.0; builds on cmdoret/mtg_ontology (GPL-3.0)", source: "", provenance: "Hand-authored 2026-07-13 as the declarative counterpart to the MTG card catalogue — scripts/mtg/mtg_game.ttl: a runtime OWL ontology (GameState/Zone/ZoneMembership/CardInstance/SpellInstance/Stack), cards with structured abilities (cost/condition/effect/action, à la cmdoret/mtg_ontology, incl. Bonders' Enclave), and a two-snapshot game state (Alice vs Bob; S2 = Lightning Bolt on the stack). rete build (Turtle) --pyramid-algo types --text-index --card. Card concepts reuse the `mtg` catalogue IRIs. No engine — the plan's Describe/Validate/Legality answered in pure SPARQL." },
    "arxiu":                 { triples: "7.28 M", size: "72.2 MB", license: "Descriptions: Llicència oberta / CC0 (Llei 37/2007); images © holding archives", source: "https://arxiusenlinia.cultura.gencat.cat/", provenance: "Harvested 2026-07-13/14, whole archive by whole archive, from the open backend API (backend.arxiusenlinia.cultura.gencat.cat/unitat/search/advanced, mostrar=AMB_FITXER, sort=id for stable complete pagination): Arxiu Nacional de Catalunya (1, 431,776 units) + Arxiu Comarcal de l'Anoia (10, 137,054) + Arxiu Comarcal del Baix Llobregat (50, 53,644) + Arxiu Històric de Tarragona (330, 7,401) = 629,875 units, 507 fonds, 117,940 WebP scans (non-reserved photos + PDF first-pages rendered via PyMuPDF; the ANC alone contributes 109,306 image scans). scripts/arxiu/harvest_and_mirror.py pages each archive + mirrors non-reserved images as downsized WebP (max 1400px, q80) to R2 while keeping the original Gencat link; scripts/arxiu/build_arxiu_graph.py (accumulative over records_*.jsonl, deduped) → schema.org archival N-Triples → rete build --pyramid-algo types --text-index --card. GEO-ENRICHED (scripts/arxiu/enrich_geo.py): 6,543 units carry a geo:asWKT municipal POINT (titles’ “Terme municipal de X” matched against 951 Catalan municipalities from Wikidata P625) and 5,350 a schema:contentLocation link to their PROVINCE’s geoadmin IRI — the cross-dataset join key. Full archive ~1.43M image-records across ~65 archives." },
    "worldcup2026":          { triples: "4.6 K", size: "107 KB", license: "Open data (OpenFootball PD, Wikidata CC0) — LIVE, unverified", source: "https://github.com/openfootball/worldcup", provenance: "Built 2026-07-12 from OpenFootball (github.com/openfootball/worldcup/2026--usa: groups, matches, scores, scorers, 16 stadiums) + star-player careers from Wikidata (P54 with start/end qualifiers). LIVE tournament, so incomplete (100/104 matches) and unverified — treat as 'as recorded by OpenFootball'. scripts/worldcup/build_wc2026_graph.py + fetch_wc2026_players.py -> N-Triples -> rete build --card. Sibling of the completed `worldcup` (2022) twin; no prediction layer (538 retired)." },
    "worldcup":              { triples: "8.1 K", size: "108 KB", license: "Open data (OpenFootball PD, 538 CC-BY, Wikidata CC0)", source: "https://github.com/openfootball/worldcup", provenance: "Built 2026-07-12 from open data: match backbone (groups, all 64 matches, scores, scorers, stadiums) from OpenFootball (github.com/openfootball/worldcup); the FiveThirtyEight SPI forecast (wc_forecasts.csv, 6 snapshots, via the Wayback Machine since 538's API is retired) + documented pre-tournament Opta & bookmaker champion odds; star-player club careers from Wikidata (P54 with start/end qualifiers). scripts/worldcup/*.py -> N-Triples (schema.org+DCTerms+GeoSPARQL+wc:) -> rete build --pyramid-algo types --card. Predictions modelled as n-ary PROV-attributed nodes (fully SPARQL-queryable)." },
    "ecal":                  { triples: "1.02 M", size: "13.7 MB", license: "Metadata; ECAL catalogue data", source: "https://library.ecal.ch/", provenance: "Harvested 2026-07-11 from the ECAL BiblioMaker OPAC (cloud7.bibliomaker.ch) by polite single-thread ID-iteration over detail pages (scripts/ecal/harvest_ecal.py, ~1 req/1.2 s, resumable, auto-restart driver): 28,741 records, 93% with covers, full-res covers mirrored. BiblioMaker fields → unified JSON (scripts/ecal, incl. Auteurs/Matières as shared nodes) → schema.org+DCTerms N-Triples (scripts/bcul/jsonl_to_nt.py --base data.ecal.ch --mint) → rete build --pyramid-algo types --card." },
    "gbif-birds":            { triples: "334 M",  size: "1.53 GB", license: "CC BY-NC 4.0 (GBIF)", source: "https://www.gbif.org", provenance: "50,202,483 georeferenced Aves occurrences (Spain 47,938,804 + Switzerland 2,263,679; class='Aves', has-coordinate, identified to species) filtered KEYLESS from the GBIF AWS Open Data Parquet snapshot 2026-07-01 (s3://gbif-open-data-eu-central-1, CC BY-NC) with DuckDB httpfs in resumable batches (scripts/gbif_birds_extract.py). Modelled to N-Triples by scripts/gbif_birds_to_nt.py: each occurrence rdf:type its species (round-trips to gbif.org/occurrence/<id>), a taxonomic subClassOf chain Species⊑Genus⊑Family⊑Order⊑Aves⊑Chordata⊑Animalia over 1,104 species (species owl:sameAs gbif.org/species/<key>), a GeoSPARQL POINT, dwc:year, dwc:basisOfRecord, dwc:individualCount and the source dataset. ENRICHED (scripts/gbif_birds_enrich_nt.py, N-Triples-star): 27,922 multilingual dwc:vernacularName in 47 languages (GBIF vernacular API), IUCN Red List status for 967 species + 165 sensitive flags (Wikidata P141), and per-species recordCount/estimatedIndividuals/first-last-year — EACH added statement annotated with RDF-star provenance (dct:source + prov:generatedAtTime + prov:wasDerivedFrom). Built with rete build --pyramid-algo types --card → a 6-level semantic-zoom schema pyramid + a header quoted-triples flag. No account, no scraping. Cite GBIF.org." },
    "bcul":                  { triples: "117 M",  size: "1.98 GB", license: "Metadata; rights vary per source", source: "https://www.bcu-lausanne.ch/", provenance: "Harvested 2026-07-07 from four public APIs — Renouvaud/Alma SRU (recursive mms_id-space bisection into 594 ≤10k-record leaves for a complete 3.55M mirror, 12-worker concurrency; AVA/AVE physical + electronic holdings extracted), Patrinum OAI-PMH (332k), e-codices IIIF (15 mss), Scriptorium MediaINFO sitemap + /api/item (331 titles + 2,000 sample issues). MARCXML/IIIF → unified JSON (scripts/bcul/marc.py) → schema.org+DCTerms N-Triples with shared agent/subject/place/library nodes (scripts/bcul/jsonl_to_nt.py; clean_iri() sanitizes dirty MARC 856) → rete build --pyramid-algo types --card. Scripts + README + JSON Schema in data/bcul/ & scripts/bcul/." },
    "bph":                   { triples: "5.77 M",  size: "58 MB",   license: "CC BY-SA 4.0",         source: "https://www.embassyofthefreemind.com/",                     provenance: "Harvested KEYLESS from Source Library (GET /api/books/browse?library=bph then /api/books/{id}) by scripts/fetch_bph.py: 2,307 of the ~4,600-book BPH digitization, each a full record (metadata, dublin_core, AI summary + 4 significance scores, subject index, chapters, geocoded places, related books, per-page image URLs). Converted to schema.org/DCterms/SKOS/GeoSPARQL N-Triples by scripts/bph_to_nt.py, built with rete build --card. Remote-lazy. Full per-page OCR/translation text (CC BY-SA 4.0) needs a free API key (sourcelibrary.org/dataset)." },
    "ustc":                  { triples: "88.5 K",  size: "1.1 MB",  license: "© USTC / Univ. of St Andrews (research subset)", source: "https://www.ustc.ac.uk/",                          provenance: "The 727 USTC editions cited by the bph collection (a fair-use research subset of a copyrighted database - NOT a bulk copy of the ~1M-record catalogue). Harvested from the public Inertia.js edition pages (the data-page JSON: edition + copies + digitisations + references) by scripts/fetch_ustc.py, converted to schema.org/DCterms/SKOS N-Triples by scripts/ustc_to_nt.py (edition IRI = ustc.ac.uk/editions/{sn} = bph's rdfs:seeAlso target; u:embassyBook back-link for federation), built with rete build --card. Remote-lazy. Attribute USTC / University of St Andrews." },
    "fuero_juzgo":           { triples: 1539,      size: "103 KB",  license: "Scholarly compilation (see sources); BSB Cod.hisp. 28 images CC Public Domain Mark 1.0", source: "https://people.unil.ch/monicacastillolluch/indice-de-fueros/fuero-juzgo/", provenance: "Compiled from Mónica Castillo Lluch's Índice de fueros / Fuero Juzgo (UNIL) and its manuscript-list PDF (Annexe 1 + Annexe 2 = PhiloBiblon BETA, texid 1191), plus verified IIIF manifests (both BSB codices, BnF Espagnol 256/Gallica, Bodleian Holkham misc. 46, all 8 Escorial codices/RBME) and verified Wikidata QIDs + Biblissima entity links. Modelled to a purpose-built ontology (fjo:) by scripts/fuero_juzgo_to_nt.py, built with rete build --pyramid-algo types --text-index --card. No crawling: two curated documents + public IIIF / Wikidata / Biblissima APIs. Remote-lazy on Cloudflare R2 (data.graphplaza.com/fuero_juzgo/fuero_juzgo.rete). Credit M. Castillo Lluch (Índice de fueros, UNIL) and PhiloBiblon." },
    "ramon_llull":           { triples: 2877,      size: "25.7 MB", license: "CC BY 4.0",            source: "https://bvpb.mcu.es/",                                      provenance: "The complete Ramón Llull microsite of the Biblioteca Virtual del Patrimonio Bibliográfico (Ministerio de Cultura, España): 142 records enumerated via the author-authority navigation (BVPB20110039301) and harvested per-record (raw catalogue HTML + RDF Dublin Core export + the digitised-object PDF) by scripts/bvpb_ramonllull.py, modelled to EDM/Dublin Core N-Triples by scripts/bvpb_to_nt.py. The 71 PRINTED editions (26,061 pages; manuscripts excluded) were then OCR'd - page images via PyMuPDF, Tesseract (lat) + dictionary-guided long-ſ correction (scripts/latin_ocr_fix.py) over ~8.7M words - emitted as bvpb:fulltext (full-text searchable via rete --text-index) and one IIIF v3 manifest per book (bvpb:iiifManifest: page images + OCR word overlay, served from R2) by scripts/bvpb_ocr_iiif.py + bvpb_iiif_to_nt.py. Built with rete build --pyramid-algo types --card --text-index. The 142 source PDFs (~13 GB) and 26k page images live in the bucket/local, not inline. Remote-lazy. CC BY 4.0 - credit the BVPB." },
    "peirce":                { triples: 36179,     size: "912 KB",  license: "Metadata CC0 (Harvard Library open-metadata policy); finding aid © Harvard", source: "https://hollisarchives.lib.harvard.edu/catalog/hou02614", provenance: "The official CSV export offered on the HOLLIS for Archival Discovery finding-aid page (one GET, no crawling; 2,527 components) parsed by scripts/peirce_to_nt.py - hierarchy from the Level columns, box entities, Robin numbers, dates/genres/short titles parsed from the title tails, and the correspondence titles ('X. Two letters to Charles S. Peirce, ...') into a directed sender/addressee network with letter counts. IIIF: each digitized component's NRS URN + ':MANIFEST:3' = its IIIF v3 manifest (mps.lib.harvard.edu); 423 manifests fetched by scripts/peirce_fetch_iiif.py and joined in as a:iiifManifest/a:pageCount/a:thumbnail by scripts/peirce_enrich_iiif.py. rete build --pyramid-algo types --text-index --card. Remote-lazy." },
    "boe":                   { triples: "465 K",   size: "7.9 MB",  license: "Ley 37/2007 + BOE standard reuse licence (Res. 27-jun-2024)", source: "https://www.boe.es/datosabiertos/", provenance: "The complete legislación-consolidada corpus of the Agencia Estatal Boletín Oficial del Estado (12,330 in-force norms, State + autonomous + foral) via the OFFICIAL Open-Data API — the paginated enumeration for node metadata, one /legislacion-consolidada/id/{id}/analisis call per norm for the subject links + the norm-to-norm relation graph, and /datos-auxiliares/{rangos,materias,departamentos,ambitos,relaciones-*} for the controlled vocabularies. Modelled to an ELI knowledge graph by scripts/boe_to_nt.py: norms at their canonical ELI IRIs (eli:LegalResource + a bx:<Rango> subclass), ~100k citation/derogation edges mapped to the standard ELI relations (repeals/amends/corrects/cites/transposes/based_on) + a bx: extension, SKOS materia/rango/ambito/estado schemes and org:Organization departments; a hand-authored OWL 2 QL ontology travels INSIDE the file — the 19 rango classes grouped into a two-tier taxonomy (bx:NormaConRangoDeLey vs bx:NormaReglamentaria under eli:LegalResource), domain/range on the citation/derogation relations, owl:inverseOf partners (bx:amendedBy/repealedBy/citedBy…) so the network reads both ways, and disjointness for coherence — so 🧠 Reason answers e.g. 'every norm with the force of law' (0→4,967) or 'what amended this law' (via the entailed inverse), and rete build --reason stamps the coherence verdict into the card. Built with rete build --pyramid-algo types --card --reason. No scraping. Remote-lazy on Cloudflare R2. Reuse under Ley 37/2007 — credit 'Basado en datos de la Agencia Estatal Boletín Oficial del Estado'." },
    "scrolls":               { triples: 18125,     size: "365 KB",  license: "CC BY-NC 4.0 (some EduceLab volumes: EduceLab Data License)", source: "https://scrollprize.org/data_browser", provenance: "The Vesuvius Challenge open-data registry (metadata.json at vesuvius-challenge-open-data.s3.us-east-1.amazonaws.com - the exact file the scrollprize.org React Data Browser consumes, resolved via villa src/data/atlasDataAccess.json), plus a live ListObjectsV2 of each sample's photos/ prefix. Modelled to a provenance graph (vs: = https://scrollprize.org/vocab#) by scripts/scrolls_to_nt.py: Sample(Scroll/Fragment) <- Scan <- Volume <- Segment <- DataAsset, with vs:usedModel links to the 9 ML models and s3:// -> https rewrites so every DataAsset IRI is a working public download URL; sample photos + image assets get Thumbor thumbnails. Built with rete build --pyramid-algo types --card. No scraping - one public JSON + S3 listings. Remote-lazy on Cloudflare R2. CC BY-NC 4.0; cite EduceLab-Scrolls / the Vesuvius Challenge dataset." },
    "scholar":               { triples: 6954,      size: "51 KB",   license: "synthetic",            source: "",                                                          provenance: "Generated by scripts/synth_graph.py (--papers 250 --seed 42): a power-law citation/author/venue world emitted to N-Triples, then assembled with rete build." },
    "scholar-noisy":         { triples: 6671,      size: "50 KB",   license: "synthetic",            source: "",                                                          provenance: "Same scripts/synth_graph.py generator at --noise 0.25 (rewired citations, dropped ORCIDs/ISSNs, mangled titles), emitted to N-Triples and assembled with rete build." },
    "causal":                { triples: 170,       size: "3.5 KB",  license: "example",              source: "",                                                          provenance: "Hand-curated cardiometabolic model emitted by scripts/synth_causal.py to examples/causal.nt (with planted coherence/SHACL defects), assembled with rete build." },
    "history":               { triples: 14430,     size: "1.5 MB",  license: "GPL-3.0",              source: "https://github.com/aourednik/historical-basemaps",         provenance: "World border GeoJSON at 7 era snapshots (aourednik/historical-basemaps) converted to GeoSPARQL wktLiteral N-Triples by scripts/geo_to_rete.py basemaps, assembled with rete build." },
    "linked-jazz":           { triples: 9466,      size: "97 KB",   license: "CC BY-SA 3.0",         source: "https://linkedjazz.org",                                   provenance: "Linked Jazz people + relationship N-Triples pulled from its REST API, well-formedness-filtered and deduped by scripts/fetch_playground_kgs.sh, then assembled with rete build." },
    "getty-ulan":            { triples: "~205,000", size: "2.8 MB",  license: "ODC-BY 1.0",           source: "https://www.getty.edu/research/tools/vocabularies/ulan/",   provenance: "CONSTRUCTed from the Getty vocab.getty.edu SPARQL endpoint (teacherOf/influenced edges + names/bios/dates) by scripts/fetch_playground_kgs.sh getty-ulan, built with rete build (remote-lazy)." },
    "bioexplora":            { triples: "7.89 M",  size: "45 MB",   license: "CC BY 4.0 (specimens/images; audio CC BY-NC-ND)", source: "https://www.bioexplora.cat", provenance: "The 6 MCNB Darwin Core archives from GBIF/ipt.gbif.es (207,163 specimens, real dwc: term IRIs). A CONNECTED-GRAPH layer turns the flat Darwin Core into a navigable graph: 33,837 Taxon nodes wired into a tree by p:parentTaxon (specimen → p:taxon → species → genus → family → … ), 3,876 shared Collector (Agent) nodes via p:collectedBy, and 193 Place nodes via p:foundIn, with an OWL TBox (classes + object properties carrying rdfs:domain/range, so the Schema view draws the real graph). Media: ~9,000 specimen photos mirrored to the bucket as fast WebP (p:preview; source links normalised to the working coeli portraitMedia as p:image) — 43,826 GeoSPARQL points — 672 type specimens — 824 Atles-osteologic 3D scans (92 mirrored to the bucket as Draco+WebP .glb, inline via p:mesh) — and 173 Xeno-canto audio recordings (Eloisa Matheu, CC BY-NC-ND, not MCNB). Built KEYLESS by scripts/bioexplora_to_nt.py + rete build --pyramid-algo types --text-index --card. Remote-lazy. CC BY 4.0 (MCNB) except the audio." },
    "geoadmin":              { triples: "372.5 K", size: "39 MB",   license: "CC-BY 4.0 (geoBoundaries) + CC0 (Natural Earth places)", source: "https://www.geoboundaries.org", provenance: "geoBoundaries (OSM-derived, CC-BY 4.0) global admin boundaries CGAZ + Natural Earth places, converted to GeoSPARQL by scripts/geoboundaries_to_nt.py: 213 Country polygons (ADM0), 3,133 Region polygons (ADM1), 48,362 District polygons (ADM2), 1,251 populated-Place points. GeoJSON → WKT by hand (no GDAL); each entity with p:name, p:iso/p:country (alpha-2 mapped from geoBoundaries' alpha-3 via datasets/country-codes), p:adminLevel, p:partOf and geo:asWKT. The detailed OSM boundaries (Spain alone was ~1 MB of WKT, the biggest ~21 MB) are SIMPLIFIED with Douglas–Peucker: the default geo:asWKT at ~1 km (coarse, instant cells), plus a MULTI-LOD g:geomFine at ~110 m on the 3,206 large features (Country + Region). The playground fetches g:geomFine for JUST the feature you open in the map modal — zoom in → fetch only that one polygon's detail. 1.1 GB of raw N-Triples → a 39 MB .rete. Built as a FEDERATION partner: join another dataset's country code (e.g. dwc:countryCode) to attach the boundary geometry. rete build --pyramid-algo types --card." },
    "geoadmin-tiles":        { triples: "372.5 K", size: "151 MB", license: "CC-BY 4.0 (geoBoundaries) + CC0 (Natural Earth places)", source: "https://www.geoboundaries.org", provenance: "The geoadmin RDF graph (39 MB) with a 113 MB PMTiles vector-tile archive EMBEDDED as a kind-7 section inside the one .rete file (geo-LOD 'option C': one file = graph + tiles, both HTTP-range-queryable). Tiles built from the full-detail geoBoundaries GeoJSON with tippecanoe (z0-9, 4 layers countries/regions/districts/places, true per-zoom LOD) by scripts/geoadmin_pmtiles.sh, then embedded by scripts/embed_tiles.py — which rewrites the .rete's 1 KB header to add the tile section's (offset, length) just before the footer, leaving every graph section byte-identical so the content hash and SPARQL are unchanged. The reader ignores the unknown section (Rete::open reads only known kinds); the browser parses the header to find the section offset and range-reads tiles from the same URL. Verified: rete query still answers the graph; the Tiles view faults the embedded tiles." },
    "smithsonian3d":         { triples: 15639,     size: "381 KB",  license: "CC0 1.0 (public domain)", source: "https://3d.si.edu", provenance: "The Smithsonian Open Access 3D collection, harvested KEYLESS from the public smithsonian-open-access S3 bucket (3d/ prefix; each model a Voyager scene.svx.json -> Medium-quality Draco .glb + title + EDAN record id) by scripts/smithsonian3d_to_nt.py. 2,199 models, each with prop:mesh (the streamable .glb, rendered inline by the 3D cell), prop:unit (museum), prop:catalogNumber and prop:record (link to si.edu). 200 of them also carry prop:spinVideo/prop:spinGif — a lightweight pre-rendered Blender Cycles turntable (webm/gif in the bucket *-spin/ prefix, scripts/render_turntables.sh) that loops in the cell without WebGL. rete build --pyramid-algo types --text-index --card. Remote-lazy from the bucket; lossless Parquet/DuckDB/SQLite companions for the Explore tab via scripts/rdf_to_entity_tables.py --nt --type-predicate rdf:type. CC0 - credit the Smithsonian." },
    "lineara":               { triples: 38306,     size: "355 KB",  license: "No explicit license (scholarly; attribute GORILA/Douros/mwenge/INSCRIBE/PAITO)", source: "https://lineara.xyz · 3D: inscribercproject.com · PAITO: paitoproject.it", provenance: "The LinearA Explorer corpus (github.com/mwenge/lineara.xyz, LinearAInscriptions.js — text after GORILA/Godart-Olivier + George Douros's tabulation) converted to RDF by scripts/lineara_to_nt.js: Inscription → site/scribe/support/period + Linear A transcription + Latin transliteration, plus the sign/word layer (Inscription → Word → Sign and Inscription → Sign). scripts/lineara_inscribe.js adds 91 prop:model3d LINKS to the ERC INSCRIBE 3D viewer pages (67 artifacts, each face linked); scripts/lineara_paito.js adds 89 prop:paito LINKS to the PAITO Project (Sapienza) — 23 Haghia Triada sealings per-artifact + 66 Phaistos documents to their PAITO sub-catalogue. Inscription images © EFA and the 3D/2D+ models © INSCRIBE & PAITO are NOT included — links only. rete build --pyramid-algo types --text-index --card." },
    "nomisma":               { triples: 53535,     size: "76 KB",   license: "ODbL 1.0 + CC-BY 3.0", source: "http://numismatics.org/pella/",                            provenance: "CONSTRUCTed from the Nomisma.org SPARQL endpoint (PELLA coin types + mints/authorities/materials/labels) by scripts/fetch_playground_kgs.sh nomisma, built with rete build." },
    "causalgraph":           { triples: 315,       size: "57 KB",   license: "MIT",                  source: "https://github.com/causalgraph/causalgraph",                provenance: "The causalgraph OWL ontology (Fraunhofer IWU) converted OWL/XML→N-Triples with owlready2, merged with an authored example injection-moulding causal graph (scripts/causalgraph_example.py: 8 nodes, 8 CausalEdges with confidence/time-lag + Human/LearningAlgorithm creators). rete build --pyramid-algo types --type-predicate rdf:type --text-index --card." },
    "ontoneurolog":          { triples: 17658,     size: "344 KB",  license: "Public download; licence unstated — attribute B. Gibaud / IRISA-VISAGES, cite Temal et al. 2008", source: "https://neurolog.i3s.unice.fr/public_namespace/ontology", provenance: "OntoNeuroLOG v2.2 (the NeuroLOG project's DOLCE-grounded neuroimaging application ontology), downloaded as the public 35-module RDF/XML package (ontoneurologv2.2.zip) and merged into one graph by scripts/ontoneurolog_to_nt.py (rdflib, imports NOT dereferenced — every module is local). 1,303 owl:Class, 289 owl:ObjectProperty, 6 owl:DatatypeProperty; 155 classes carry a scholarly rdfs:comment definition. rete build --pyramid-algo types --card. Remote-lazy over HTTP range from R2." },
    "nidm":                  { triples: 3153,      size: "86 KB",   license: "NIDM specs: open community standard (INCF NIDASH); ds000030 phenotype data CC0 (OpenNeuro/UCLA CNP)", source: "https://github.com/incf-nidash/nidm-specs", provenance: "NIDM-Experiment documents merged by scripts/nidm_to_nt.py from github.com/incf-nidash/nidm-specs: the ds000030 cohort (OpenNeuro UCLA CNP — 272 subjects, age/gender/diagnosis, PROV chains), a second example (CENTRAL_E03925), and the OntoNeuroLOG v3.0 instrument taxonomy NIDM reuses. The converter mints 45 owl:sameAs links from NIDM's v3.0 instrument IRIs to the ontoneurolog dataset's v2.2 IRIs (matched by shared concept local-name) — the federation bridge; a cross-source join returns 29 concepts defined on both sides. The generic NIDM-Experiment terms file is deliberately omitted so rdfs:comment stays NIDM-absent and the playground's cross-source splitter routes the ontoneurolog side correctly (see scripts/nidm_to_nt.py). rete build --pyramid-algo types --card. Remote-lazy from R2." },
    "mira-wikidata":         { triples: 125,       size: "30 KB",   license: "CC BY-NC-SA 4.0",      source: "https://www.mira.ie",                                      provenance: "MIrA's owl:sameAs links re-expressed as a standalone SSSOM mapping linkset (skos:exactMatch + owl:Axiom provenance with mapping_justification/confidence + void:Linkset metadata) by scripts/mira_sssom.py. The shareable, federatable form of a mapping set; TSV companion mira-wikidata.sssom.tsv. See dev/mapping-rete.md." },
    "mira":                  { triples: 5044,      size: "127 KB",  license: "CC BY-NC-SA 4.0",      source: "https://www.mira.ie",                                      provenance: "MIrA (Manuscripts with Irish Associations, Pádraic Moran / University of Galway): the Wikidata-aligned RDF dump from github.com/padraicmoran/MIrA, enriched by scripts/mira_to_nt.py with IIIF manifests (wdt:P6108), name/script/folios/contents, and the inclusion-criterion categories (mira:category, from data/other/categories.xml — the project's /about 'Criteria for inclusion') from the per-manuscript XML, plus rdfs:labels for every wd:Q*/wdt:P* code fetched from the Wikidata API. rete build --pyramid-algo types --type-predicate P31 --text-index --card." },
    "mimotext":              { triples: 27389,     size: "126 KB",  license: "CC0",                  source: "https://www.mimotext.uni-trier.de",                        provenance: "CONSTRUCTed from the MiMoText Wikibase SPARQL endpoint (works, themes, stylometric-similarity edges, English labels) by scripts/fetch_playground_kgs.sh mimotext, built with rete build." },
    "mmm":                   { triples: "23.3 M",  size: "141 MB",  license: "CC BY-NC 4.0",         source: "https://mappingmanuscriptmigrations.org",                  provenance: "The full MMM graph (data release v2.1.0, ldf.fi: SDBM + Bibale + Bodleian) merged with the MMM metadata ontology and built with rete build --pyramid-algo types --card - a parallelizable type pyramid + embedded Dataset Card + the merged class/property ontology. Remote-lazy over HTTP range." },
    "openalex-astrocytes":   { triples: 24042,     size: "172 KB",  license: "CC0",                  source: "https://openalex.org",                                     provenance: "The 500 most-cited astrocyte works fetched from the OpenAlex API and wired into a paper/author/institution/topic citation core by scripts/fetch_openalex_astrocytes.py, built with rete build." },
    "antarctic-expeditions": { triples: 275,       size: "3.5 KB",  license: "CC0",                  source: "https://www.wikidata.org",                                 provenance: "Heroic-Age expeditions, crews and ships CONSTRUCTed from the Wikidata Query Service (expedition/participant/vessel/leader edges) to N-Triples, then assembled with rete build." },
    "factgrid-illuminati":   { triples: 34979,     size: "269 KB",  license: "CC0",                  source: "https://database.factgrid.de",                             provenance: "~1,300 Order-of-the-Illuminati members CONSTRUCTed from the FactGrid Wikibase SPARQL endpoint with property/object labels resolved to N-Triples, then assembled with rete build." },
    "theographic-graph":     { triples: 31945,     size: "170 KB",  license: "CC BY-SA",             source: "https://github.com/robertrouse/theographic-bible-metadata", provenance: "Theographic Bible metadata (people, kin ties, places, events) from robertrouse/theographic-bible-metadata converted to a narrative/genealogy N-Triples graph and assembled with rete build." },
    "monarch":               { triples: 7811,      size: "76 KB",   license: "CC-BY",                source: "https://monarchinitiative.org",                            provenance: "A bounded disease/gene/phenotype neighbourhood from the Monarch Initiative KG (biolink classes + labels + xrefs) exported to N-Triples and assembled with rete build." },
    "opencitations":         { triples: 8103,      size: "86 KB",   license: "CC0",                  source: "https://opencitations.net",                                provenance: "A citation neighbourhood around a seed paper from the OpenCitations API (cito:cites + DC/FOAF bibliographic metadata) exported to N-Triples and assembled with rete build." },
    "orkg":                  { triples: 37314,     size: "393 KB",  license: "CC-BY",                source: "https://orkg.org",                                         provenance: "A slice of the Open Research Knowledge Graph (papers, contributions, problems/methods/results + labels) fetched from ORKG, exported to N-Triples and assembled with rete build." },
    "ohm-full":              { triples: "~6.1 M",  size: "150 MB",   license: "CC0 1.0",              source: "https://www.openhistoricalmap.org",                        provenance: "The entire OpenHistoricalMap daily planet .osm.pbf streamed and simplified with PyOsmium (scripts/fetch_ohm_planet.sh) to GeoSPARQL N-Triples, assembled with rete build (remote-lazy)." },
    "wikidata":              { triples: null,      size: "1.04 GB", license: "CC0",                  source: "https://www.wikidata.org",                                 provenance: "A ~1 GB cross-section of the Wikidata truthy dump read from Parquet via DuckDB by scripts/wikidata_parquet_to_nt.py (datatypes recovered) to N-Triples, built with rete build (remote-lazy)." },
    "wikidata-zenodo":       { triples: null,      size: "1.49 GB", license: "CC0",                  source: "https://zenodo.org/records/21168821",                      provenance: "The same Wikidata cross-section as wikidata-1GB (scripts/wikidata_parquet_to_nt.py → rete build), deposited as Zenodo record 21168821 / DOI 10.5281/zenodo.21168821. The playground serves a stable-generation R2 twin because the deposited pre-1.0 artifact predates the stable format and Zenodo does not expose Content-Range to browser JavaScript." },
    "wikidata-100mb":        { triples: null,      size: "104 MB",  license: "CC0",                  source: "https://www.wikidata.org",                                 provenance: "A ~100 MB slice of the Wikidata truthy dump read from Parquet via DuckDB by scripts/wikidata_parquet_to_nt.py (datatypes recovered) to N-Triples, built with rete build (remote-lazy)." },
    "wikidata-xxl":          { triples: "600 M",   size: "5.4 GB (6 shards)", license: "CC0",         source: "https://www.wikidata.org",                                 provenance: "600M triples of the Wikidata truthy dump (piebro/wikidata-extraction Parquet) as 6 INDEPENDENT --no-pyramid .rete shards (100M / ~906 MB each), one per ~900 MB Parquet partition. Streamed + built shard-by-shard with bounded RAM/disk by scripts/build_wikidata_xxl.sh (--part-index streaming → rete build --no-pyramid → drop the ~10.7 GB N-Triples → next; ~16 min/shard). The playground auto-federates the shards (catalog `shards: [...]` → every query UNIONs across all 6). The sharded-rete model that scales past a single file toward full Wikidata (~80 shards). Heuristic datatypes (WDQS rate-limited the property map). CC0 (Wikidata)." },
    "chemotion":             { triples: "1.53 M",  size: "4.8 MB",   license: "CC BY 4.0",            source: "https://chemotion.net",                                    provenance: "The FIZ-Karlsruhe Chemotion-KG (github.com/ISE-FIZKarlsruhe/chemotion-kg, a 239 MB Git-LFS TTL; BFO/NFDICore/ChEBI-aligned, 87.7k instances) merged with CHMO + RXNO (purl.obolibrary.org/obo/*.owl, RDF/XML converted to N-Triples via rdflib), assembled with rete build --card (remote-lazy)." },
    "chebi-full":            { triples: "8.83 M",  size: "120 MB",   license: "CC BY 4.0",            source: "https://www.ebi.ac.uk/chebi/",                              provenance: "The complete ChEBI ontology (chebi.owl, EMBL-EBI) converted from RDF/XML to N-Triples with rapper (raptor2), assembled with rete build --card (remote-lazy). Lossless Parquet/DuckDB/SQLite per-class table companions generated with scripts/rdf_to_entity_tables.py --nt --type-predicate rdf:type and uploaded alongside it." },
    "causenet":         { triples: "256.1 M", size: "6.39 GB",  license: "CC BY 4.0",            source: "https://causenet.org/",                                    provenance: "CauseNet-Full (causenet-full.jsonl, Heindorf et al., CIKM 2020) converted to N-Triples by scripts/causenet_to_nt.py - every causal relation kept with its full Wikipedia/ClueWeb12 source provenance (24.4M source sentences + dependency patterns) - assembled with rete build --card (remote-lazy). Lossless relations/sources/concepts Parquet/DuckDB/SQLite companions via scripts/causenet_to_tables.py." },
    "jonas":                 { triples: "234 K",   size: "2.1 MB",  license: "CC0 1.0",              source: "https://jonas.irht.cnrs.fr/",                              provenance: "The LostMa-ERC Heurist database (data-pipeline-output, derived from Jonas / IRHT-CNRS) converted from its published DuckDB to N-Triples by scripts/jonas_to_nt.py - Heurist records become entities, '<x> H-ID' columns object relations, '<x> TRM-ID' columns links into the controlled vocabulary - then rete build --pyramid-algo types --card (remote-lazy)." },
    "postscriptum":          { triples: "63 K",    size: "5.3 MB",  license: "Free (CLUL/ELRA)",     source: "http://teitok.clul.ul.pt/postscriptum",                    provenance: "Post Scriptum (CLUL, U. Lisboa) TEI-P5 letters, harvested from the freely-downloadable TEITOK XML-TEI_P5 bundles (ES/PT x 1500-1800, ps.clul.ul.pt/files/*.zip; availability status=free, CLUL recognised as author). 3,649 TEI letter files parsed by scripts/postscriptum_to_nt.py (correspDesc -> sender/recipient/place/date, catRef -> type/pragmatics, tokenised <w>/<reg> -> modernised text), then rete build --pyramid-algo types --text-index --card." },
    "databnf":               { triples: "673.5 M", size: "7.16 GB", license: "CC0 1.0",              source: "https://data.bnf.fr/",                                     provenance: "The complete data.bnf.fr official RDF dump (data.gouv.fr, 22 .tar.gz perimeters, CC0) merged into ONE file: 726,700,333 raw -> 673,519,782 unique triples, 194.8M terms. rete build --pyramid-algo types via the low-RAM pipeline (dict-drop + build_seq + parallel compression, peak ~44.6 GiB / ~2h20m) - was 10 federated shards because the monolith OOM'd a 62 GB host (dev/low-mem-build.md)." },
    "bne":                   { triples: "267 M",   size: "3.4 GB",  license: "CC0 1.0",              source: "https://datos.bne.es/",                                    provenance: "datos.bne.es official RDF dumps (autoridades + bibliograficos + materias, .nt.bz2 from /datadump/, behind Cloudflare - download with a Referer header; CC0) decompressed + concatenated (267,116,572 triples), ~64k malformed lines dropped by a streaming NT validator (IRIs with stray '>' / trailing spaces), then rete build --pyramid-algo types via the low-RAM pipeline (peak ~25.6 GiB, ~45 min). The Spanish data.bnf.fr; federates with databnf on owl:sameAs to VIAF / data.bnf.fr." },
    "biblissima":            { triples: "254 M",   size: "667 MB",  license: "CC-BY 4.0",            source: "https://data.biblissima.fr/",                              provenance: "Biblissima+ Wikibase (867,838 entities, 254,203,131 triples) harvested entity-by-entity via the open Special:EntityData .nt export (scripts/harvest_biblissima.py; no bulk dump, SPARQL gated), then built as ONE file - rete build --pyramid-algo types --type-predicate prop/direct/P2 (P2-keyed pyramid, 4 levels). Was 3 federated shards until the low-RAM build changes (dict-drop + build_seq) let the 254M monolith fit; peak ~4.85 GiB." },
    "albala":                { triples: "927 K",   size: "11.4 MB", license: "© Inst. Colombina",    source: "https://albala.icolombina.es/albala",                      provenance: "Harvested anonymously from the public ARCAS consultation (Baratz Albalá 7, a Solr-backed JS portal): the results endpoint page?pageid=30000&responseType=solr&fq=*:* paginated at rows=1000 over 70 requests (69,900 records, access level ACL2), scripts/harvest_albala.py, converted to N-Triples by scripts/albala_to_nt.py, then rete build --pyramid-algo types --text-index --card. Metadata only - digital images are not publicly available." },
    "memoria":               { triples: "1.21 M",  size: "12.9 MB", license: "CC-BY (regional)",     source: "https://datos.gob.es",                                     provenance: "Aggregation of 7 openly-licensed Spanish Civil War memoria-histórica datasets (Catalonia reparació + desapareguts, Euskadi víctimas, and the Catalan/Andalusian/CyL/Valencian fosas registries) downloaded from regional open-data portals, unified into one model by scripts/memoria_to_nt.py (99,542 victims + 2,872 graves + 88 provinces), then rete build --pyramid-algo types --text-index --card. NOT a scrape of any search portal - bulk open data only." },
  },

  // Companion encodings: the lossless Parquet / DuckDB / SQLite per-class entity
  // tables that `scripts/rdf_to_entity_tables.py` produces alongside a `.rete`.
  // These power the multi-backend lazy explorer (explore-100mb.html?dataset=<key>):
  // the same graph, queried four ways (rete / DuckDB-WASM·Parquet / DuckDB·.duckdb /
  // sql.js-httpvfs·SQLite), so you can compare which backend wins per query shape.
  // Paths are relative to remoteBase; the explorer appends ?token=remoteToken.
  // A dataset with no entry here shows only the rete Graph backend (no companions).
  companions: {
    "bcul": {"rete": "bcul/bcul.rete", "parquetDir": "bcul/bcul-tables", "flat": true, "about": "The full BCU Lausanne twin as a flat <code>triples</code> table (subject, predicate, object token + decoded value / datatype / lang) in Parquet — the same 117.5M facts, queryable in SQL (DuckDB-WASM, HTTP-range over the 2.8&nbsp;GB file) to compare against the rete engine. The standalone .duckdb (16&nbsp;GB) and .sqlite (48&nbsp;GB) aren't hosted — at 117M rows they're disproportionate, and DuckDB-WASM reads the Parquet directly.", "tables": [{"name": "triples", "file": "triples.parquet", "label": "triples — every fact (S, P, O)", "entities": 117472563}], "sqlCols": ["subject", "predicate", "object", "otype", "value", "datatype", "lang"], "examples": [{"label": "Predicate totals", "table": {"file": "triples.parquet", "name": "triples"}, "sparql": "SELECT ?p (COUNT(*) AS ?n) WHERE { ?s ?p ?o } GROUP BY ?p ORDER BY DESC(?n)", "duck": "SELECT predicate, COUNT(*) AS n\nFROM {T}\nGROUP BY predicate\nORDER BY n DESC;", "note": "How often each predicate is used — the shape of the graph (schema:about, heldAt, creator …)."}, {"label": "Availability across all holdings", "table": {"file": "triples.parquet", "name": "triples"}, "sparql": "PREFIX b: <https://data.bcu-lausanne.ch/>\nSELECT ?avail (COUNT(*) AS ?n) WHERE { ?s b:availability ?avail } GROUP BY ?avail ORDER BY DESC(?n)", "duck": "SELECT value AS availability, COUNT(*) AS n\nFROM {T}\nWHERE predicate = 'https://data.bcu-lausanne.ch/availability'\nGROUP BY value\nORDER BY n DESC;", "note": "available / unavailable / check_holdings / online — the live status of every copy in the network."}]},
    "ramon_llull": {
      rete: "ramon_llull/ramon_llull.rete",
      parquetDir: "ramon_llull/ramon_llull-tables",  // ProvidedCHO_.parquet + _manifest.parquet
      duckdb: "ramon_llull/ramon_llull.duckdb",  duckdbSize: "0.80 MB",
      sqlite: "ramon_llull/ramon_llull.sqlite",  sqliteSize: "0.20 MB",
      typePredicate: "rdf:type",
      seed: "https://bvpb.mcu.es/es/consulta/registro.do?id=397825", // Operum Tomus I (868 p.)
      about: "The BVPB Ramón Llull collection (142 works) as relational companions on R2: per-class " +
        "<code>.parquet</code>, one <code>.duckdb</code>, one <code>.sqlite</code> — beside the rete " +
        "<code>.rete</code> graph. Every work is one wide row in the <code>ProvidedCHO</code> table: title, " +
        "creator, date, publisher, subject, language, type, physical description, the digitised PDF " +
        "(<code>pdf</code>), and — for the 71 printed editions — <code>pageCount</code> and the IIIF " +
        "<code>iiifManifest</code>. Multi-valued fields are LIST columns. The ~8.7 M-word OCR full text is " +
        "NOT in the tables (it stays a rete search feature); everything else round-trips losslessly.",
      tables: [
        { name: "ProvidedCHO", file: "ProvidedCHO_.parquet", classIri: "http://www.europeana.eu/schemas/edm/ProvidedCHO", label: "ProvidedCHO — Ramón Llull works", entities: 142 },
      ],
      sqlCols: ["entity", "label", "title", "creator", "date", "publisher", "subject", "language", "type", "format", "pdf", "pageCount", "iiifManifest", "types"],
      examples: [
        { label: "Everything about one work (Operum Tomus I)", table: { file: "ProvidedCHO_.parquet", name: "ProvidedCHO" },
          sparql: `SELECT ?p ?o WHERE {\n  <https://bvpb.mcu.es/es/consulta/registro.do?id=397825> ?p ?o\n}`,
          duck: `SELECT *\nFROM {T}\nWHERE entity = 'https://bvpb.mcu.es/es/consulta/registro.do?id=397825';`,
          sqlite: `SELECT *\nFROM "ProvidedCHO"\nWHERE entity = 'https://bvpb.mcu.es/es/consulta/registro.do?id=397825';`,
          note: "One work, every column — title, creator, imprint, physical description, the PDF, page count and IIIF manifest — in a single wide row (vs the graph's row-per-fact)." },
        { label: "The busiest printers & publishers", table: { file: "ProvidedCHO_.parquet", name: "ProvidedCHO" },
          sparql: `PREFIX dc: <http://purl.org/dc/elements/1.1/>\nSELECT ?publisher (COUNT(?s) AS ?n) WHERE {\n  ?s dc:publisher ?publisher\n}\nGROUP BY ?publisher ORDER BY DESC(?n) LIMIT 15`,
          duck: `SELECT publisher, COUNT(*) AS n\nFROM (SELECT UNNEST(publisher) AS publisher FROM {T} WHERE len(publisher) > 0)\nGROUP BY publisher ORDER BY n DESC LIMIT 15;`,
          sqlite: `SELECT je.value AS publisher, COUNT(*) AS n\nFROM "ProvidedCHO", json_each("ProvidedCHO".publisher) je\nGROUP BY 1 ORDER BY n DESC LIMIT 15;`,
          note: "The publisher column is a LIST — DuckDB UNNESTs it, SQLite walks it with json_each. The early-modern Mallorca presses (Viuda de Guasp, Typis Viduae Frau) surface." },
        { label: "Printed editions with a IIIF viewer, by page count", table: { file: "ProvidedCHO_.parquet", name: "ProvidedCHO" },
          sparql: `PREFIX dc: <http://purl.org/dc/elements/1.1/>\nPREFIX bvpb: <https://bvpb.mcu.es/ns#>\nSELECT ?title ?pages ?manifest WHERE {\n  ?s bvpb:iiifManifest ?manifest ; dc:title ?title ; bvpb:pageCount ?pages .\n}\nORDER BY DESC(?pages) LIMIT 30`,
          duck: `SELECT label AS title, CAST(pageCount[1] AS INT) AS pages, iiifManifest[1] AS manifest\nFROM {T}\nWHERE len(iiifManifest) > 0\nORDER BY pages DESC LIMIT 30;`,
          sqlite: `SELECT label AS title, CAST(json_extract(pageCount,'$[0]') AS INT) AS pages, json_extract(iiifManifest,'$[0]') AS manifest\nFROM "ProvidedCHO"\nWHERE iiifManifest != '[]'\nORDER BY pages DESC LIMIT 30;`,
          note: "The 71 printed editions that were OCR'd and given a IIIF manifest, largest first — the Operum Tomus volumes run past 1,000 pages." },
      ],
    },
    "smithsonian3d": {
      rete: "smithsonian3d/smithsonian3d.rete",
      parquetDir: "smithsonian3d/smithsonian3d-tables",  // *.parquet + _manifest.parquet
      duckdb: "smithsonian3d/smithsonian3d.duckdb",  duckdbSize: "1.3 MB",
      sqlite: "smithsonian3d/smithsonian3d.sqlite",  sqliteSize: "1.2 MB",
      typePredicate: "rdf:type",
      seed: "https://3d.si.edu/object/d8c6457e-4ebc-11ea-b77f-2e728ce88125", // Apollo Command Module Exterior
      about: "The Smithsonian Open Access 3D collection (2,199 CC0 models) in four lossless encodings on " +
        "Hugging Face: the rete <code>.rete</code> graph, per-class <code>.parquet</code> tables, one " +
        "<code>.duckdb</code>, and one <code>.sqlite</code>. Every query fetches only the bytes it touches. " +
        "The objects live in the <code>Model3D</code> table — title, museum unit, catalogue number and the " +
        "streamable <code>mesh</code> (.glb) URL; the <code>Unit</code> table holds the 12 museums.",
      tables: [
        { name: "Model3D", file: "Model3D_.parquet", classIri: "https://3d.si.edu/class/Model3D", label: "Model3D — Smithsonian 3D objects", entities: 2199 },
        { name: "Unit",    file: "Unit_.parquet",    classIri: "https://3d.si.edu/class/Unit",    label: "Unit — museums",                 entities: 12 },
      ],
      sqlCols: ["entity", "label", "types", "mesh", "unit", "catalogNumber", "edanId", "record", "extra"],
      examples: [
        { label: "Everything about one model (Apollo Command Module)", table: { file: "Model3D_.parquet", name: "Model3D" },
          sparql: `SELECT ?p ?o WHERE {\n  <https://3d.si.edu/object/d8c6457e-4ebc-11ea-b77f-2e728ce88125> ?p ?o\n}`,
          duck: `SELECT *\nFROM {T}\nWHERE entity = 'https://3d.si.edu/object/d8c6457e-4ebc-11ea-b77f-2e728ce88125';`,
          sqlite: `SELECT *\nFROM "Model3D"\nWHERE entity = 'https://3d.si.edu/object/d8c6457e-4ebc-11ea-b77f-2e728ce88125';`,
          note: "One object, every column — title, museum unit, catalogue number and the streamable .glb mesh — in a single wide row (vs the graph's row-per-fact)." },
        { label: "Models with their 3D mesh", table: { file: "Model3D_.parquet", name: "Model3D" },
          sparql: `PREFIX p: <https://3d.si.edu/prop/>\nPREFIX rdfs: <http://www.w3.org/2000/01/rdf-schema#>\nSELECT ?object ?mesh WHERE {\n  ?o rdfs:label ?object ; p:mesh ?mesh .\n} LIMIT 60`,
          duck: `SELECT label, mesh[1] AS mesh\nFROM {T}\nLIMIT 60;`,
          sqlite: `SELECT label, json_extract(mesh, '$[0]') AS mesh\nFROM "Model3D"\nLIMIT 60;`,
          note: "The mesh column is a one-element list of the .glb URL; DuckDB indexes it with [1], SQLite stores it as JSON text." },
        { label: "Search the titles (crania)", table: { file: "Model3D_.parquet", name: "Model3D" },
          sparql: `PREFIX rdfs: <http://www.w3.org/2000/01/rdf-schema#>\nSELECT ?object WHERE {\n  ?o rdfs:label ?object . FILTER(CONTAINS(LCASE(STR(?object)), "cranium"))\n} LIMIT 40`,
          duck: `SELECT label\nFROM {T}\nWHERE label ILIKE '%cranium%'\nLIMIT 40;`,
          sqlite: `SELECT label\nFROM "Model3D"\nWHERE lower(label) LIKE '%cranium%'\nLIMIT 40;`,
          note: "The same title search across the three relational engines; swap 'cranium' for 'Apollo', 'whale' or a genus like 'Pongo'." },
      ],
    },
    "chebi-full": {
      rete: "chebi-full/chebi-full.rete",
      parquetDir: "chebi-full/chebi-full-tables",   // *.parquet + _manifest.parquet
      duckdb: "chebi-full/chebi-full.duckdb",  duckdbSize: "123 MB",
      sqlite: "chebi-full/chebi-full.sqlite",  sqliteSize: "586 MB",
      typePredicate: "rdf:type",
      seed: "http://purl.obolibrary.org/obo/CHEBI_27732", // caffeine (owl:Class)
      about: "The complete ChEBI ontology (8.83 M triples) in four lossless encodings on " +
        "Hugging Face: the rete <code>.rete</code> graph, per-class <code>.parquet</code> tables, " +
        "one <code>.duckdb</code>, and one <code>.sqlite</code>. Every query fetches only the bytes " +
        "it touches. The molecules live in the <code>Class</code> table (owl:Class), with structural " +
        "columns — SMILES, InChI, formula, mass.",
      // Tables that have their own columnar table (entity rows, named-property columns).
      // `name` is the DuckDB table identifier; `file` the Parquet basename.
      // `classIri` ties a table to the rdf:type class shown in the Explore tab,
      // so selecting a class can route to its companion table.
      tables: [
        { name: "Class",       file: "Class_.parquet",       classIri: "http://www.w3.org/2002/07/owl#Class",       label: "owl:Class — CHEBI molecules", entities: 224691 },
        { name: "Axiom",       file: "Axiom_.parquet",       classIri: "http://www.w3.org/2002/07/owl#Axiom",       label: "owl:Axiom — xref reification", entities: 897439 },
        { name: "Restriction", file: "Restriction_.parquet", classIri: "http://www.w3.org/2002/07/owl#Restriction", label: "owl:Restriction",              entities: 94902 },
      ],
      // Autocomplete hints for the SQL editor (table + common column names).
      sqlCols: ["entity", "label", "types", "subClassOf", "smiles_string", "inchi_string", "mass",
        "monoisotopic_mass", "generalized_empirical_formula", "hasOBONamespace", "IAO_0000115",
        "hasExactSynonym", "extra"],
      // The same question posed on each backend. `duck` uses {T}: read_parquet('…') for the
      // Tables backend, wd."<name>" for the .duckdb backend. Object values are N-Triples term
      // tokens (IRIs `<…>`, literals `"…"`); SQLite stores list/map columns as JSON text.
      examples: [
        { label: "Everything about caffeine (CHEBI:27732)", table: { file: "Class_.parquet", name: "Class" },
          sparql: `SELECT ?p ?o WHERE {\n  <http://purl.obolibrary.org/obo/CHEBI_27732> ?p ?o\n}`,
          duck: `SELECT *\nFROM {T}\nWHERE entity = 'http://purl.obolibrary.org/obo/CHEBI_27732';`,
          sqlite: `SELECT *\nFROM "Class"\nWHERE entity = 'http://purl.obolibrary.org/obo/CHEBI_27732';`,
          note: "One entity, every fact. The graph is tall (a row per predicate/object); the table is one wide row — the shape difference itself." },

        { label: "Formula, mass & SMILES of caffeine", table: { file: "Class_.parquet", name: "Class" },
          sparql: `PREFIX chemrof: <https://w3id.org/chemrof/>\nSELECT ?formula ?mass ?smiles WHERE {\n  <http://purl.obolibrary.org/obo/CHEBI_27732> chemrof:generalized_empirical_formula ?formula ;\n    chemrof:mass ?mass ;\n    chemrof:smiles_string ?smiles\n}`,
          duck: `SELECT label,\n  generalized_empirical_formula AS formula,\n  mass, smiles_string\nFROM {T}\nWHERE entity = 'http://purl.obolibrary.org/obo/CHEBI_27732';`,
          sqlite: `SELECT label, generalized_empirical_formula, mass, smiles_string\nFROM "Class"\nWHERE entity = 'http://purl.obolibrary.org/obo/CHEBI_27732';`,
          note: "ChemROF structural columns — DuckDB reads only those Parquet column chunks; the graph walks each predicate's tiles." },

        { label: "Direct subclasses of amino acid (CHEBI:33709)", table: { file: "Class_.parquet", name: "Class" },
          sparql: `PREFIX rdfs: <http://www.w3.org/2000/01/rdf-schema#>\nSELECT ?s ?label WHERE {\n  ?s rdfs:subClassOf <http://purl.obolibrary.org/obo/CHEBI_33709> ;\n     rdfs:label ?label\n}\nLIMIT 100`,
          duck: `SELECT entity, label\nFROM {T}\nWHERE list_contains(subClassOf, '<http://purl.obolibrary.org/obo/CHEBI_33709>')\nLIMIT 100;`,
          sqlite: `SELECT entity, label\nFROM "Class"\nWHERE EXISTS (SELECT 1 FROM json_each(subClassOf)\n              WHERE value = '<http://purl.obolibrary.org/obo/CHEBI_33709>')\nLIMIT 100;`,
          note: "Children point up via rdfs:subClassOf — a list-membership test on the column vs a BGP on the graph (~57 direct subclasses)." },

        { label: "Count entities per OBO namespace", table: { file: "Class_.parquet", name: "Class" },
          sparql: `PREFIX oio: <http://www.geneontology.org/formats/oboInOwl#>\nSELECT ?ns (COUNT(?s) AS ?n) WHERE {\n  ?s oio:hasOBONamespace ?ns\n}\nGROUP BY ?ns\nORDER BY DESC(?n)`,
          duck: `SELECT ns, count(*) AS n\nFROM (SELECT unnest(hasOBONamespace) AS ns FROM {T})\nGROUP BY ns\nORDER BY n DESC;`,
          sqlite: `SELECT je.value AS ns, count(*) AS n\nFROM "Class", json_each(hasOBONamespace) je\nGROUP BY ns\nORDER BY n DESC;`,
          note: "An aggregate over one column — Parquet reads just that column's chunks; the graph aggregates a whole predicate." },

        { label: "Name search: “amino acid”", table: { file: "Class_.parquet", name: "Class" },
          sparql: `PREFIX rdfs: <http://www.w3.org/2000/01/rdf-schema#>\nSELECT ?s ?label WHERE {\n  ?s rdfs:label ?label\n  FILTER(CONTAINS(LCASE(STR(?label)), "amino acid"))\n}\nLIMIT 50`,
          duck: `SELECT entity, label\nFROM {T}\nWHERE label ILIKE '%amino acid%'\nLIMIT 50;`,
          sqlite: `SELECT entity, label\nFROM "Class"\nWHERE label LIKE '%amino acid%'\nLIMIT 50;`,
          note: "Substring search — a full column scan in every backend; the closest apples-to-apples comparison." },
      ],
    },

    // wikidata-1GB (dataset key "wikidata"): companions live under wikidata-1GB/ in
    // the bucket (not playground/), shared with explore-100mb.html. Tables are named
    // by Q-id (no trailing underscore in the DuckDB/SQLite tables; Parquet files keep
    // the underscore). Typed by wdt:P31.
    "wikidata": {
      rete: "wikidata-1GB/wikidata.rete",
      parquetDir: "wikidata-1GB/parquet",
      duckdb: "wikidata-1GB/wikidata.duckdb",  duckdbSize: "214 MB",
      sqlite: "wikidata-1GB/wikidata.sqlite",  sqliteSize: "475 MB",
      typePredicate: "<http://www.wikidata.org/prop/direct/P31>",
      seed: "http://www.wikidata.org/entity/Q859", // Plato (a human, table Q5)
      about: "A ~1 GB cross-section of Wikidata (truthy) in four lossless encodings on Hugging Face: " +
        "the rete <code>.rete</code> graph, per-class <code>.parquet</code> tables, one <code>.duckdb</code>, " +
        "and one <code>.sqlite</code>. Entities are typed by <code>wdt:P31</code>; the people live in the " +
        "<code>Q5</code> table (occupation P106, citizenship P27, …). Tables are named by Q-id.",
      tables: [
        { name: "Q5",        file: "Q5_.parquet",        classIri: "http://www.wikidata.org/entity/Q5",        label: "human",                  entities: 219322 },
        { name: "Q4167836",  file: "Q4167836_.parquet",  classIri: "http://www.wikidata.org/entity/Q4167836",  label: "Wikimedia category",     entities: 172724 },
        { name: "Q16521",    file: "Q16521_.parquet",    classIri: "http://www.wikidata.org/entity/Q16521",    label: "taxon",                  entities: 83998 },
        { name: "Q4167410",  file: "Q4167410_.parquet",  classIri: "http://www.wikidata.org/entity/Q4167410",  label: "disambiguation page",    entities: 54598 },
        { name: "Q13406463", file: "Q13406463_.parquet", classIri: "http://www.wikidata.org/entity/Q13406463", label: "Wikimedia list article", entities: 17697 },
        { name: "Q482994",   file: "Q482994_.parquet",   classIri: "http://www.wikidata.org/entity/Q482994",   label: "album",                  entities: 16314 },
        { name: "Q532",      file: "Q532_.parquet",      classIri: "http://www.wikidata.org/entity/Q532",      label: "village",                entities: 13258 },
        { name: "Q11424",    file: "Q11424_.parquet",    classIri: "http://www.wikidata.org/entity/Q11424",    label: "film",                   entities: 13093 },
      ],
      sqlCols: ["entity", "label", "types", "P106", "P27", "P569", "P19", "P21", "P735", "extra"],
      examples: [
        { label: "Everything about Plato", table: { file: "Q5_.parquet", name: "Q5" },
          sparql: `SELECT ?p ?o WHERE {\n  <http://www.wikidata.org/entity/Q859> ?p ?o\n}`,
          duck: `SELECT *\nFROM {T}\nWHERE entity = 'http://www.wikidata.org/entity/Q859';`,
          sqlite: `SELECT *\nFROM "Q5"\nWHERE entity = 'http://www.wikidata.org/entity/Q859';`,
          note: "One entity, every fact — a tall graph vs one wide row." },

        { label: "Most common occupations", table: { file: "Q5_.parquet", name: "Q5" },
          sparql: `PREFIX wdt: <http://www.wikidata.org/prop/direct/>\nSELECT ?occ (COUNT(?p) AS ?n) WHERE {\n  ?p wdt:P106 ?occ\n}\nGROUP BY ?occ\nORDER BY DESC(?n)\nLIMIT 15`,
          duck: `SELECT occ, count(*) AS people\nFROM (SELECT unnest(P106) AS occ FROM {T})\nGROUP BY occ\nORDER BY people DESC\nLIMIT 15;`,
          sqlite: `SELECT je.value AS occupation, count(*) AS people\nFROM "Q5", json_each(P106) je\nGROUP BY occupation\nORDER BY people DESC\nLIMIT 15;`,
          note: "Aggregate over one column (P106) — Parquet reads just that column; the graph aggregates a whole predicate." },

        { label: "Biggest polymaths", table: { file: "Q5_.parquet", name: "Q5" },
          sparql: `PREFIX wdt: <http://www.wikidata.org/prop/direct/>\nPREFIX rdfs: <http://www.w3.org/2000/01/rdf-schema#>\nSELECT ?who (COUNT(?occ) AS ?jobs) WHERE {\n  ?p wdt:P106 ?occ ; rdfs:label ?who .\n  FILTER(LANG(?who) = "en")\n}\nGROUP BY ?who\nORDER BY DESC(?jobs)\nLIMIT 15`,
          duck: `SELECT label, len(P106) AS jobs\nFROM {T}\nWHERE label IS NOT NULL\nORDER BY jobs DESC\nLIMIT 15;`,
          sqlite: `SELECT label, json_array_length(P106) AS jobs\nFROM "Q5"\nWHERE label IS NOT NULL\nORDER BY jobs DESC\nLIMIT 15;`,
          note: "Who wears the most hats? len(P106) off the row vs counting P106 edges in the graph." },

        { label: "Physicists who were also philosophers", table: { file: "Q5_.parquet", name: "Q5" },
          sparql: `PREFIX wdt: <http://www.wikidata.org/prop/direct/>\nPREFIX rdfs: <http://www.w3.org/2000/01/rdf-schema#>\nSELECT ?p ?who WHERE {\n  ?p wdt:P106 <http://www.wikidata.org/entity/Q169470> ;\n     wdt:P106 <http://www.wikidata.org/entity/Q4964182> ;\n     rdfs:label ?who . FILTER(LANG(?who) = "en")\n}`,
          duck: `SELECT entity, label\nFROM {T}\nWHERE list_contains(P106, '<http://www.wikidata.org/entity/Q169470>')\n  AND list_contains(P106, '<http://www.wikidata.org/entity/Q4964182>');`,
          sqlite: `SELECT entity, label\nFROM "Q5"\nWHERE EXISTS (SELECT 1 FROM json_each(P106) WHERE value = '<http://www.wikidata.org/entity/Q169470>')\n  AND EXISTS (SELECT 1 FROM json_each(P106) WHERE value = '<http://www.wikidata.org/entity/Q4964182>');`,
          note: "Occupation intersection (physicist Q169470 AND philosopher Q4964182) — same answer, three engines." },
      ],
    },

    // wikidata-100MB (dataset key "wikidata-100mb"): the 100 MB tier's companions,
    // also under wikidata-100MB/ in the bucket. Same shape as the 1 GB entry; the
    // top P31 classes + counts come from this tier's own manifest.
    "wikidata-100mb": {
      rete: "wikidata-100MB/wikidata.rete",
      parquetDir: "wikidata-100MB/parquet",
      duckdb: "wikidata-100MB/wikidata.duckdb",  duckdbSize: "204 MB",
      sqlite: "wikidata-100MB/wikidata.sqlite",  sqliteSize: "475 MB",
      typePredicate: "<http://www.wikidata.org/prop/direct/P31>",
      seed: "http://www.wikidata.org/entity/Q859", // Plato (a human, table Q5)
      about: "A real ~100 MB slice of Wikidata (truthy) in four lossless encodings: the rete " +
        "<code>.rete</code>, per-class <code>.parquet</code> tables, one <code>.duckdb</code>, one " +
        "<code>.sqlite</code>. Entities typed by <code>wdt:P31</code>; people live in <code>Q5</code> " +
        "(occupation P106, citizenship P27, …). Tables are named by Q-id. The 1 GB tier is key “wikidata”.",
      tables: [
        { name: "Q5",          file: "Q5_.parquet",          classIri: "http://www.wikidata.org/entity/Q5",          label: "human",               entities: 19058 },
        { name: "Q4167410",    file: "Q4167410_.parquet",    classIri: "http://www.wikidata.org/entity/Q4167410",    label: "disambiguation page", entities: 5999 },
        { name: "Q16521",      file: "Q16521_.parquet",      classIri: "http://www.wikidata.org/entity/Q16521",      label: "taxon",               entities: 2837 },
        { name: "Q484170",     file: "Q484170_.parquet",     classIri: "http://www.wikidata.org/entity/Q484170",     label: "commune of France",   entities: 1253 },
        { name: "Q747074",     file: "Q747074_.parquet",     classIri: "http://www.wikidata.org/entity/Q747074",     label: "Italian comune",      entities: 960 },
        { name: "Q11424",      file: "Q11424_.parquet",      classIri: "http://www.wikidata.org/entity/Q11424",      label: "film",                entities: 664 },
        { name: "Q3863",       file: "Q3863_.parquet",       classIri: "http://www.wikidata.org/entity/Q3863",       label: "business",            entities: 600 },
        { name: "Q113145171",  file: "Q113145171_.parquet",  classIri: "http://www.wikidata.org/entity/Q113145171",  label: "scholarly article",   entities: 565 },
      ],
      sqlCols: ["entity", "label", "types", "P106", "P27", "P569", "P19", "P21", "P735", "extra"],
      examples: [
        { label: "Everything about Plato", table: { file: "Q5_.parquet", name: "Q5" },
          sparql: `SELECT ?p ?o WHERE {\n  <http://www.wikidata.org/entity/Q859> ?p ?o\n}`,
          duck: `SELECT *\nFROM {T}\nWHERE entity = 'http://www.wikidata.org/entity/Q859';`,
          sqlite: `SELECT *\nFROM "Q5"\nWHERE entity = 'http://www.wikidata.org/entity/Q859';`,
          note: "One entity, every fact — a tall graph vs one wide row." },

        { label: "Most common occupations", table: { file: "Q5_.parquet", name: "Q5" },
          sparql: `PREFIX wdt: <http://www.wikidata.org/prop/direct/>\nSELECT ?occ (COUNT(?p) AS ?n) WHERE {\n  ?p wdt:P106 ?occ\n}\nGROUP BY ?occ\nORDER BY DESC(?n)\nLIMIT 15`,
          duck: `SELECT occ, count(*) AS people\nFROM (SELECT unnest(P106) AS occ FROM {T})\nGROUP BY occ\nORDER BY people DESC\nLIMIT 15;`,
          sqlite: `SELECT je.value AS occupation, count(*) AS people\nFROM "Q5", json_each(P106) je\nGROUP BY occupation\nORDER BY people DESC\nLIMIT 15;`,
          note: "Aggregate over one column (P106) — Parquet reads just that column; the graph aggregates a whole predicate." },

        { label: "Biggest polymaths", table: { file: "Q5_.parquet", name: "Q5" },
          sparql: `PREFIX wdt: <http://www.wikidata.org/prop/direct/>\nPREFIX rdfs: <http://www.w3.org/2000/01/rdf-schema#>\nSELECT ?who (COUNT(?occ) AS ?jobs) WHERE {\n  ?p wdt:P106 ?occ ; rdfs:label ?who .\n  FILTER(LANG(?who) = "en")\n}\nGROUP BY ?who\nORDER BY DESC(?jobs)\nLIMIT 15`,
          duck: `SELECT label, len(P106) AS jobs\nFROM {T}\nWHERE label IS NOT NULL\nORDER BY jobs DESC\nLIMIT 15;`,
          sqlite: `SELECT label, json_array_length(P106) AS jobs\nFROM "Q5"\nWHERE label IS NOT NULL\nORDER BY jobs DESC\nLIMIT 15;`,
          note: "len(P106) off the row vs counting P106 edges in the graph." },

        { label: "Physicists who were also philosophers", table: { file: "Q5_.parquet", name: "Q5" },
          sparql: `PREFIX wdt: <http://www.wikidata.org/prop/direct/>\nPREFIX rdfs: <http://www.w3.org/2000/01/rdf-schema#>\nSELECT ?p ?who WHERE {\n  ?p wdt:P106 <http://www.wikidata.org/entity/Q169470> ;\n     wdt:P106 <http://www.wikidata.org/entity/Q4964182> ;\n     rdfs:label ?who . FILTER(LANG(?who) = "en")\n}`,
          duck: `SELECT entity, label\nFROM {T}\nWHERE list_contains(P106, '<http://www.wikidata.org/entity/Q169470>')\n  AND list_contains(P106, '<http://www.wikidata.org/entity/Q4964182>');`,
          sqlite: `SELECT entity, label\nFROM "Q5"\nWHERE EXISTS (SELECT 1 FROM json_each(P106) WHERE value = '<http://www.wikidata.org/entity/Q169470>')\n  AND EXISTS (SELECT 1 FROM json_each(P106) WHERE value = '<http://www.wikidata.org/entity/Q4964182>');`,
          note: "Occupation intersection — same answer, three engines." },
      ],
    },

    // chemotion (dataset key "chemotion"): companions generated 2026-06-21 from the
    // bucket .rete (dumped via the shipped wasm → N-Triples → rdf_to_entity_tables.py)
    // and uploaded to playground/chemotion-*. The 78k instances (datasets, studies,
    // molecules, substances) are all owl:NamedIndividual — that table carries the
    // structural columns (smiles/inchikey/formula); the CHMO/RXNO ontology classes
    // are owl:Class. Shares CHEBI IRIs with chebi-full (they federate).
    "chemotion": {
      rete: "chemotion/chemotion.rete",
      parquetDir: "chemotion/chemotion-tables",
      duckdb: "chemotion/chemotion.duckdb",  duckdbSize: "48 MB",
      sqlite: "chemotion/chemotion.sqlite",  sqliteSize: "167 MB",
      typePredicate: "rdf:type",
      about: "The Chemotion electronic-lab-notebook knowledge graph (FIZ Karlsruhe) merged with " +
        "CHMO + RXNO — 1.53 M triples — in four lossless encodings. The 78k instances (datasets, " +
        "studies, molecules, substances) are all <code>owl:NamedIndividual</code>; that table carries " +
        "the structural columns <code>smiles</code> / <code>inchikey</code> / <code>formula</code>. The " +
        "CHMO/RXNO method &amp; reaction classes are <code>owl:Class</code>. Shares CHEBI IRIs with chebi-full.",
      tables: [
        { name: "NamedIndividual", file: "NamedIndividual_.parquet", classIri: "http://www.w3.org/2002/07/owl#NamedIndividual",     label: "instances (molecules, datasets…)", entities: 78764 },
        { name: "NFDI_0000015",    file: "NFDI_0000015_.parquet",    classIri: "https://nfdi.fiz-karlsruhe.de/ontology/NFDI_0000015", label: "NFDI_0000015",                    entities: 4864 },
        { name: "Axiom",           file: "Axiom_.parquet",           classIri: "http://www.w3.org/2002/07/owl#Axiom",                label: "owl:Axiom — xref reification",     entities: 4587 },
        { name: "Class",           file: "Class_.parquet",           classIri: "http://www.w3.org/2002/07/owl#Class",                label: "owl:Class — CHMO/RXNO classes",    entities: 4474 },
        { name: "IAO_0000028",     file: "IAO_0000028_.parquet",     classIri: "http://purl.obolibrary.org/obo/IAO_0000028",         label: "IAO symbol",                       entities: 4247 },
        { name: "Restriction",     file: "Restriction_.parquet",     classIri: "http://www.w3.org/2002/07/owl#Restriction",          label: "owl:Restriction",                  entities: 1171 },
      ],
      sqlCols: ["entity", "label", "types", "smiles", "inchikey", "formula", "subClassOf", "IAO_0000115", "hasExactSynonym", "extra"],
      examples: [
        { label: "Instances with a SMILES structure", table: { file: "NamedIndividual_.parquet", name: "NamedIndividual" },
          sparql: `PREFIX chebi: <http://purl.obolibrary.org/obo/chebi/>\nSELECT ?s ?smiles WHERE {\n  ?s chebi:smiles ?smiles\n}\nLIMIT 50`,
          duck: `SELECT entity, label, smiles, inchikey, formula\nFROM {T}\nWHERE smiles IS NOT NULL\nLIMIT 50;`,
          sqlite: `SELECT entity, label, smiles, inchikey, formula\nFROM "NamedIndividual"\nWHERE smiles IS NOT NULL\nLIMIT 50;`,
          note: "Chemotion's molecules carry SMILES / InChIKey / formula — the same structural columns chebi-full has (the two federate on CHEBI IRIs)." },

        { label: "What types are the instances?", table: { file: "NamedIndividual_.parquet", name: "NamedIndividual" },
          sparql: `SELECT ?t (COUNT(?s) AS ?n) WHERE {\n  ?s a ?t\n}\nGROUP BY ?t\nORDER BY DESC(?n)\nLIMIT 15`,
          duck: `SELECT t, count(*) AS n\nFROM (SELECT unnest(types) AS t FROM {T})\nGROUP BY t\nORDER BY n DESC\nLIMIT 15;`,
          sqlite: `SELECT je.value AS type, count(*) AS n\nFROM "NamedIndividual", json_each(types) je\nGROUP BY type\nORDER BY n DESC\nLIMIT 15;`,
          note: "Every instance is an owl:NamedIndividual; its domain types (molecule CHEBI_23367, substance CHEBI_59999, process BFO_0000015…) live in the `types` column." },

        { label: "Ontology classes & their definitions", table: { file: "Class_.parquet", name: "Class" },
          sparql: `PREFIX rdfs: <http://www.w3.org/2000/01/rdf-schema#>\nPREFIX obo: <http://purl.obolibrary.org/obo/>\nSELECT ?c ?label ?def WHERE {\n  ?c a <http://www.w3.org/2002/07/owl#Class> ;\n     rdfs:label ?label ; obo:IAO_0000115 ?def\n}\nLIMIT 25`,
          duck: `SELECT entity, label, IAO_0000115 AS definition\nFROM {T}\nWHERE IAO_0000115 IS NOT NULL\nLIMIT 25;`,
          sqlite: `SELECT entity, label, IAO_0000115\nFROM "Class"\nWHERE IAO_0000115 IS NOT NULL\nLIMIT 25;`,
          note: "CHMO methods + RXNO reactions as owl:Class with textual definitions (obo:IAO_0000115)." },
      ],
    },

    // Smaller datasets (companions generated 2026-06-21, same wasm-dump pipeline).
    // Tables only — the entity-table selector + SQL tab work without curated examples.
    "getty-ulan": {
      rete: "getty-ulan/getty-ulan.rete", parquetDir: "getty-ulan/getty-ulan-tables",
      duckdb: "getty-ulan/getty-ulan.duckdb", sqlite: "getty-ulan/getty-ulan.sqlite", typePredicate: "rdf:type",
      tables: [
        { name: "Person", file: "Person_.parquet", classIri: "http://xmlns.com/foaf/0.1/Person", label: "artist / agent (teacherOf, influenced)", entities: 28279 },
      ],
    },
    "monarch": {
      rete: "monarch/monarch.rete", parquetDir: "monarch/monarch-tables",
      duckdb: "monarch/monarch.duckdb", sqlite: "monarch/monarch.sqlite", typePredicate: "rdf:type",
      tables: [
        { name: "Gene", file: "Gene_.parquet", classIri: "https://w3id.org/biolink/vocab/Gene", label: "gene", entities: 534 },
        { name: "PhenotypicFeature", file: "PhenotypicFeature_.parquet", classIri: "https://w3id.org/biolink/vocab/PhenotypicFeature", label: "phenotype", entities: 312 },
        { name: "Genotype", file: "Genotype_.parquet", classIri: "https://w3id.org/biolink/vocab/Genotype", label: "genotype", entities: 66 },
        { name: "BiologicalProcess", file: "BiologicalProcess_.parquet", classIri: "https://w3id.org/biolink/vocab/BiologicalProcess", label: "biological process", entities: 31 },
        { name: "CellularComponent", file: "CellularComponent_.parquet", classIri: "https://w3id.org/biolink/vocab/CellularComponent", label: "cellular component", entities: 28 },
        { name: "ChemicalEntity", file: "ChemicalEntity_.parquet", classIri: "https://w3id.org/biolink/vocab/ChemicalEntity", label: "chemical entity", entities: 24 },
      ],
    },
    "opencitations": {
      rete: "opencitations/opencitations.rete", parquetDir: "opencitations/opencitations-tables",
      duckdb: "opencitations/opencitations.duckdb", sqlite: "opencitations/opencitations.sqlite", typePredicate: "rdf:type",
      tables: [
        { name: "JournalArticle", file: "JournalArticle_.parquet", classIri: "http://purl.org/spar/fabio/JournalArticle", label: "journal article", entities: 297 },
        { name: "BookChapter", file: "BookChapter_.parquet", classIri: "http://purl.org/spar/fabio/BookChapter", label: "book chapter", entities: 23 },
        { name: "ReferenceEntry", file: "ReferenceEntry_.parquet", classIri: "http://purl.org/spar/fabio/ReferenceEntry", label: "reference entry", entities: 7 },
      ],
    },
    "orkg": {
      rete: "orkg/orkg.rete", parquetDir: "orkg/orkg-tables",
      duckdb: "orkg/orkg.duckdb", sqlite: "orkg/orkg.sqlite", typePredicate: "rdf:type",
      tables: [
        { name: "Contribution", file: "Contribution_.parquet", classIri: "https://orkg.org/class/Contribution", label: "contribution", entities: 932 },
        { name: "C23008", file: "C23008_.parquet", classIri: "https://orkg.org/class/C23008", label: "C23008", entities: 561 },
        { name: "Paper", file: "Paper_.parquet", classIri: "https://orkg.org/class/Paper", label: "paper", entities: 552 },
        { name: "List", file: "List_.parquet", classIri: "https://orkg.org/class/List", label: "list", entities: 551 },
        { name: "Author", file: "Author_.parquet", classIri: "https://orkg.org/class/Author", label: "author", entities: 493 },
        { name: "Venue", file: "Venue_.parquet", classIri: "https://orkg.org/class/Venue", label: "venue", entities: 412 },
      ],
    },
    // causenet-full: native relational companions (NOT rdf:type entity tables) -
    // the "CauseNet in SQL" form, generated by scripts/causenet_to_tables.py.
    "causenet": {
      rete: "causenet-full/causenet-full.rete",
      parquetDir: "causenet-full/causenet-full-tables",
      duckdb: "causenet-full/causenet-full.duckdb",  duckdbSize: "8.95 GB",
      sqlite: "causenet-full/causenet-full.sqlite",  sqliteSize: "13.9 GB",
      typePredicate: "rdf:type",
      about: "CauseNet-Full as clean relational tables on Hugging Face: <code>relations</code> " +
        "(11.6M cause/effect/support edges), <code>sources</code> (24.4M provenance rows - the " +
        "sentence, dependency pattern and Wikipedia/ClueWeb12 page behind every claim), and " +
        "<code>concepts</code> (12.2M, with in/out causal degree). The same data as the " +
        "<code>.rete</code> graph, queried with SQL; each query fetches only the bytes it touches.",
      sqlCols: ["cause", "effect", "support", "source_type", "sentence", "pattern", "concept",
        "in_degree", "out_degree", "degree", "wikipedia_page_title", "clueweb12_page_reference"],
      tables: [
        { name: "relations", file: "relations.parquet", label: "cause -> effect edges + support", entities: 11609890 },
        { name: "sources",   file: "sources.parquet",   label: "per-sentence provenance (Wikipedia + ClueWeb12)", entities: 24443421 },
        { name: "concepts",  file: "concepts.parquet",  label: "concepts + causal in/out degree", entities: 12186310 },
      ],
      examples: [
        { label: "Strongest causal claims (by evidence count)", duck: "SELECT cause, effect, support FROM \"relations\" ORDER BY support DESC LIMIT 25", sqlite: "SELECT cause, effect, support FROM relations ORDER BY support DESC LIMIT 25" },
        { label: "What does 'smoking' cause?", duck: "SELECT effect, support FROM \"relations\" WHERE cause = 'smoking' ORDER BY support DESC LIMIT 50", sqlite: "SELECT effect, support FROM relations WHERE cause = 'smoking' ORDER BY support DESC LIMIT 50" },
        { label: "The evidence sentences for smoking -> cancer", duck: "SELECT source_type, sentence FROM \"sources\" WHERE cause = 'smoking' AND effect = 'cancer' LIMIT 25", sqlite: "SELECT source_type, sentence FROM sources WHERE cause = 'smoking' AND effect = 'cancer' LIMIT 25" },
        { label: "Most-caused effects (highest in-degree)", duck: "SELECT concept, in_degree FROM \"concepts\" ORDER BY in_degree DESC LIMIT 25", sqlite: "SELECT concept, in_degree FROM concepts ORDER BY in_degree DESC LIMIT 25" },
        { label: "Where does the evidence come from?", duck: "SELECT source_type, COUNT(*) AS n FROM \"sources\" GROUP BY source_type ORDER BY n DESC", sqlite: "SELECT source_type, COUNT(*) AS n FROM sources GROUP BY source_type ORDER BY n DESC" },
      ],
    },
    "albala": {"rete": "albala/albala.rete", "parquetDir": "albala/albala-tables", "duckdb": "albala/albala.duckdb", "duckdbSize": "32.0 MB", "sqlite": "albala/albala.sqlite", "sqliteSize": "314.6 MB", "flat": true, "about": "The albala graph as a flat <code>triples</code> table (subject, predicate, object token + decoded value/datatype/lang) in Parquet, DuckDB and SQLite — the same 927,534 facts, queryable in SQL to compare against the rete engine.", "tables": [{"name": "triples", "file": "triples.parquet", "label": "triples — every fact (S, P, O)", "entities": 927534}], "sqlCols": ["subject", "predicate", "object", "otype", "value", "datatype", "lang"], "examples": [{"label": "Predicate totals", "table": {"file": "triples.parquet", "name": "triples"}, "sparql": "SELECT ?p (COUNT(*) AS ?n) WHERE { ?s ?p ?o } GROUP BY ?p ORDER BY DESC(?n)", "duck": "SELECT predicate, COUNT(*) AS n\nFROM {T}\nGROUP BY predicate\nORDER BY n DESC;", "sqlite": "SELECT predicate, COUNT(*) AS n\nFROM \"triples\"\nGROUP BY predicate\nORDER BY n DESC;", "note": "How often each predicate is used — the shape of the graph, as a GROUP BY over the flat triples table."}, {"label": "Busiest subjects", "table": {"file": "triples.parquet", "name": "triples"}, "sparql": "SELECT ?s (COUNT(*) AS ?n) WHERE { ?s ?p ?o } GROUP BY ?s ORDER BY DESC(?n) LIMIT 50", "duck": "SELECT subject, COUNT(*) AS n\nFROM {T}\nGROUP BY subject\nORDER BY n DESC\nLIMIT 50;", "sqlite": "SELECT subject, COUNT(*) AS n\nFROM \"triples\"\nGROUP BY subject\nORDER BY n DESC\nLIMIT 50;", "note": "The 50 subjects with the most outgoing facts — the hubs of the graph."}]},
    "antarctic-expeditions": {"rete": "antarctic-expeditions/antarctic-expeditions.rete", "parquetDir": "antarctic-expeditions/antarctic-expeditions-tables", "duckdb": "antarctic-expeditions/antarctic-expeditions.duckdb", "duckdbSize": "524 KB", "sqlite": "antarctic-expeditions/antarctic-expeditions.sqlite", "sqliteSize": "92 KB", "flat": true, "about": "The antarctic-expeditions graph as a flat <code>triples</code> table (subject, predicate, object token + decoded value/datatype/lang) in Parquet, DuckDB and SQLite — the same 275 facts, queryable in SQL to compare against the rete engine.", "tables": [{"name": "triples", "file": "triples.parquet", "label": "triples — every fact (S, P, O)", "entities": 275}], "sqlCols": ["subject", "predicate", "object", "otype", "value", "datatype", "lang"], "examples": [{"label": "Predicate totals", "table": {"file": "triples.parquet", "name": "triples"}, "sparql": "SELECT ?p (COUNT(*) AS ?n) WHERE { ?s ?p ?o } GROUP BY ?p ORDER BY DESC(?n)", "duck": "SELECT predicate, COUNT(*) AS n\nFROM {T}\nGROUP BY predicate\nORDER BY n DESC;", "sqlite": "SELECT predicate, COUNT(*) AS n\nFROM \"triples\"\nGROUP BY predicate\nORDER BY n DESC;", "note": "How often each predicate is used — the shape of the graph, as a GROUP BY over the flat triples table."}, {"label": "Busiest subjects", "table": {"file": "triples.parquet", "name": "triples"}, "sparql": "SELECT ?s (COUNT(*) AS ?n) WHERE { ?s ?p ?o } GROUP BY ?s ORDER BY DESC(?n) LIMIT 50", "duck": "SELECT subject, COUNT(*) AS n\nFROM {T}\nGROUP BY subject\nORDER BY n DESC\nLIMIT 50;", "sqlite": "SELECT subject, COUNT(*) AS n\nFROM \"triples\"\nGROUP BY subject\nORDER BY n DESC\nLIMIT 50;", "note": "The 50 subjects with the most outgoing facts — the hubs of the graph."}]},
    "causal": {"rete": "causal/causal.rete", "parquetDir": "causal/causal-tables", "duckdb": "causal/causal.duckdb", "duckdbSize": "524 KB", "sqlite": "causal/causal.sqlite", "sqliteSize": "56 KB", "flat": true, "about": "The causal graph as a flat <code>triples</code> table (subject, predicate, object token + decoded value/datatype/lang) in Parquet, DuckDB and SQLite — the same 170 facts, queryable in SQL to compare against the rete engine.", "tables": [{"name": "triples", "file": "triples.parquet", "label": "triples — every fact (S, P, O)", "entities": 170}], "sqlCols": ["subject", "predicate", "object", "otype", "value", "datatype", "lang"], "examples": [{"label": "Predicate totals", "table": {"file": "triples.parquet", "name": "triples"}, "sparql": "SELECT ?p (COUNT(*) AS ?n) WHERE { ?s ?p ?o } GROUP BY ?p ORDER BY DESC(?n)", "duck": "SELECT predicate, COUNT(*) AS n\nFROM {T}\nGROUP BY predicate\nORDER BY n DESC;", "sqlite": "SELECT predicate, COUNT(*) AS n\nFROM \"triples\"\nGROUP BY predicate\nORDER BY n DESC;", "note": "How often each predicate is used — the shape of the graph, as a GROUP BY over the flat triples table."}, {"label": "Busiest subjects", "table": {"file": "triples.parquet", "name": "triples"}, "sparql": "SELECT ?s (COUNT(*) AS ?n) WHERE { ?s ?p ?o } GROUP BY ?s ORDER BY DESC(?n) LIMIT 50", "duck": "SELECT subject, COUNT(*) AS n\nFROM {T}\nGROUP BY subject\nORDER BY n DESC\nLIMIT 50;", "sqlite": "SELECT subject, COUNT(*) AS n\nFROM \"triples\"\nGROUP BY subject\nORDER BY n DESC\nLIMIT 50;", "note": "The 50 subjects with the most outgoing facts — the hubs of the graph."}]},
    "causalgraph": {"rete": "causalgraph/causalgraph.rete", "parquetDir": "causalgraph/causalgraph-tables", "duckdb": "causalgraph/causalgraph.duckdb", "duckdbSize": "524 KB", "sqlite": "causalgraph/causalgraph.sqlite", "sqliteSize": "132 KB", "flat": true, "about": "The causalgraph graph as a flat <code>triples</code> table (subject, predicate, object token + decoded value/datatype/lang) in Parquet, DuckDB and SQLite — the same 315 facts, queryable in SQL to compare against the rete engine.", "tables": [{"name": "triples", "file": "triples.parquet", "label": "triples — every fact (S, P, O)", "entities": 315}], "sqlCols": ["subject", "predicate", "object", "otype", "value", "datatype", "lang"], "examples": [{"label": "Predicate totals", "table": {"file": "triples.parquet", "name": "triples"}, "sparql": "SELECT ?p (COUNT(*) AS ?n) WHERE { ?s ?p ?o } GROUP BY ?p ORDER BY DESC(?n)", "duck": "SELECT predicate, COUNT(*) AS n\nFROM {T}\nGROUP BY predicate\nORDER BY n DESC;", "sqlite": "SELECT predicate, COUNT(*) AS n\nFROM \"triples\"\nGROUP BY predicate\nORDER BY n DESC;", "note": "How often each predicate is used — the shape of the graph, as a GROUP BY over the flat triples table."}, {"label": "Busiest subjects", "table": {"file": "triples.parquet", "name": "triples"}, "sparql": "SELECT ?s (COUNT(*) AS ?n) WHERE { ?s ?p ?o } GROUP BY ?s ORDER BY DESC(?n) LIMIT 50", "duck": "SELECT subject, COUNT(*) AS n\nFROM {T}\nGROUP BY subject\nORDER BY n DESC\nLIMIT 50;", "sqlite": "SELECT subject, COUNT(*) AS n\nFROM \"triples\"\nGROUP BY subject\nORDER BY n DESC\nLIMIT 50;", "note": "The 50 subjects with the most outgoing facts — the hubs of the graph."}]},
    "factgrid-illuminati": {"rete": "factgrid-illuminati/factgrid-illuminati.rete", "parquetDir": "factgrid-illuminati/factgrid-illuminati-tables", "duckdb": "factgrid-illuminati/factgrid-illuminati.duckdb", "duckdbSize": "1.5 MB", "sqlite": "factgrid-illuminati/factgrid-illuminati.sqlite", "sqliteSize": "11.2 MB", "flat": true, "about": "The factgrid-illuminati graph as a flat <code>triples</code> table (subject, predicate, object token + decoded value/datatype/lang) in Parquet, DuckDB and SQLite — the same 34,979 facts, queryable in SQL to compare against the rete engine.", "tables": [{"name": "triples", "file": "triples.parquet", "label": "triples — every fact (S, P, O)", "entities": 34979}], "sqlCols": ["subject", "predicate", "object", "otype", "value", "datatype", "lang"], "examples": [{"label": "Predicate totals", "table": {"file": "triples.parquet", "name": "triples"}, "sparql": "SELECT ?p (COUNT(*) AS ?n) WHERE { ?s ?p ?o } GROUP BY ?p ORDER BY DESC(?n)", "duck": "SELECT predicate, COUNT(*) AS n\nFROM {T}\nGROUP BY predicate\nORDER BY n DESC;", "sqlite": "SELECT predicate, COUNT(*) AS n\nFROM \"triples\"\nGROUP BY predicate\nORDER BY n DESC;", "note": "How often each predicate is used — the shape of the graph, as a GROUP BY over the flat triples table."}, {"label": "Busiest subjects", "table": {"file": "triples.parquet", "name": "triples"}, "sparql": "SELECT ?s (COUNT(*) AS ?n) WHERE { ?s ?p ?o } GROUP BY ?s ORDER BY DESC(?n) LIMIT 50", "duck": "SELECT subject, COUNT(*) AS n\nFROM {T}\nGROUP BY subject\nORDER BY n DESC\nLIMIT 50;", "sqlite": "SELECT subject, COUNT(*) AS n\nFROM \"triples\"\nGROUP BY subject\nORDER BY n DESC\nLIMIT 50;", "note": "The 50 subjects with the most outgoing facts — the hubs of the graph."}]},
    "fuero_juzgo": {"rete": "fuero_juzgo/fuero_juzgo.rete", "parquetDir": "fuero_juzgo/fuero_juzgo-tables", "duckdb": "fuero_juzgo/fuero_juzgo.duckdb", "duckdbSize": "524 KB", "sqlite": "fuero_juzgo/fuero_juzgo.sqlite", "sqliteSize": "536 KB", "flat": true, "about": "The fuero_juzgo graph as a flat <code>triples</code> table (subject, predicate, object token + decoded value/datatype/lang) in Parquet, DuckDB and SQLite — the same 1,536 facts, queryable in SQL to compare against the rete engine.", "tables": [{"name": "triples", "file": "triples.parquet", "label": "triples — every fact (S, P, O)", "entities": 1536}], "sqlCols": ["subject", "predicate", "object", "otype", "value", "datatype", "lang"], "examples": [{"label": "Predicate totals", "table": {"file": "triples.parquet", "name": "triples"}, "sparql": "SELECT ?p (COUNT(*) AS ?n) WHERE { ?s ?p ?o } GROUP BY ?p ORDER BY DESC(?n)", "duck": "SELECT predicate, COUNT(*) AS n\nFROM {T}\nGROUP BY predicate\nORDER BY n DESC;", "sqlite": "SELECT predicate, COUNT(*) AS n\nFROM \"triples\"\nGROUP BY predicate\nORDER BY n DESC;", "note": "How often each predicate is used — the shape of the graph, as a GROUP BY over the flat triples table."}, {"label": "Busiest subjects", "table": {"file": "triples.parquet", "name": "triples"}, "sparql": "SELECT ?s (COUNT(*) AS ?n) WHERE { ?s ?p ?o } GROUP BY ?s ORDER BY DESC(?n) LIMIT 50", "duck": "SELECT subject, COUNT(*) AS n\nFROM {T}\nGROUP BY subject\nORDER BY n DESC\nLIMIT 50;", "sqlite": "SELECT subject, COUNT(*) AS n\nFROM \"triples\"\nGROUP BY subject\nORDER BY n DESC\nLIMIT 50;", "note": "The 50 subjects with the most outgoing facts — the hubs of the graph."}]},
    "geoadmin": {"rete": "geoadmin/geoadmin.rete", "parquetDir": "geoadmin/geoadmin-tables", "duckdb": "geoadmin/geoadmin.duckdb", "duckdbSize": "292.5 MB", "sqlite": "geoadmin/geoadmin.sqlite", "sqliteSize": "364.2 MB", "flat": true, "about": "The geoadmin graph as a flat <code>triples</code> table (subject, predicate, object token + decoded value/datatype/lang) in Parquet, DuckDB and SQLite — the same 372,508 facts, queryable in SQL to compare against the rete engine.", "tables": [{"name": "triples", "file": "triples.parquet", "label": "triples — every fact (S, P, O)", "entities": 372508}], "sqlCols": ["subject", "predicate", "object", "otype", "value", "datatype", "lang"], "examples": [{"label": "Predicate totals", "table": {"file": "triples.parquet", "name": "triples"}, "sparql": "SELECT ?p (COUNT(*) AS ?n) WHERE { ?s ?p ?o } GROUP BY ?p ORDER BY DESC(?n)", "duck": "SELECT predicate, COUNT(*) AS n\nFROM {T}\nGROUP BY predicate\nORDER BY n DESC;", "sqlite": "SELECT predicate, COUNT(*) AS n\nFROM \"triples\"\nGROUP BY predicate\nORDER BY n DESC;", "note": "How often each predicate is used — the shape of the graph, as a GROUP BY over the flat triples table."}, {"label": "Busiest subjects", "table": {"file": "triples.parquet", "name": "triples"}, "sparql": "SELECT ?s (COUNT(*) AS ?n) WHERE { ?s ?p ?o } GROUP BY ?s ORDER BY DESC(?n) LIMIT 50", "duck": "SELECT subject, COUNT(*) AS n\nFROM {T}\nGROUP BY subject\nORDER BY n DESC\nLIMIT 50;", "sqlite": "SELECT subject, COUNT(*) AS n\nFROM \"triples\"\nGROUP BY subject\nORDER BY n DESC\nLIMIT 50;", "note": "The 50 subjects with the most outgoing facts — the hubs of the graph."}]},
    "history": {"rete": "history/history.rete", "parquetDir": "history/history-tables", "duckdb": "history/history.duckdb", "duckdbSize": "18.3 MB", "sqlite": "history/history.sqlite", "sqliteSize": "19.3 MB", "flat": true, "about": "The history graph as a flat <code>triples</code> table (subject, predicate, object token + decoded value/datatype/lang) in Parquet, DuckDB and SQLite — the same 14,430 facts, queryable in SQL to compare against the rete engine.", "tables": [{"name": "triples", "file": "triples.parquet", "label": "triples — every fact (S, P, O)", "entities": 14430}], "sqlCols": ["subject", "predicate", "object", "otype", "value", "datatype", "lang"], "examples": [{"label": "Predicate totals", "table": {"file": "triples.parquet", "name": "triples"}, "sparql": "SELECT ?p (COUNT(*) AS ?n) WHERE { ?s ?p ?o } GROUP BY ?p ORDER BY DESC(?n)", "duck": "SELECT predicate, COUNT(*) AS n\nFROM {T}\nGROUP BY predicate\nORDER BY n DESC;", "sqlite": "SELECT predicate, COUNT(*) AS n\nFROM \"triples\"\nGROUP BY predicate\nORDER BY n DESC;", "note": "How often each predicate is used — the shape of the graph, as a GROUP BY over the flat triples table."}, {"label": "Busiest subjects", "table": {"file": "triples.parquet", "name": "triples"}, "sparql": "SELECT ?s (COUNT(*) AS ?n) WHERE { ?s ?p ?o } GROUP BY ?s ORDER BY DESC(?n) LIMIT 50", "duck": "SELECT subject, COUNT(*) AS n\nFROM {T}\nGROUP BY subject\nORDER BY n DESC\nLIMIT 50;", "sqlite": "SELECT subject, COUNT(*) AS n\nFROM \"triples\"\nGROUP BY subject\nORDER BY n DESC\nLIMIT 50;", "note": "The 50 subjects with the most outgoing facts — the hubs of the graph."}]},
    "jonas": {"rete": "jonas/jonas.rete", "parquetDir": "jonas/jonas-tables", "duckdb": "jonas/jonas.duckdb", "duckdbSize": "14.5 MB", "sqlite": "jonas/jonas.sqlite", "sqliteSize": "82.6 MB", "flat": true, "about": "The jonas graph as a flat <code>triples</code> table (subject, predicate, object token + decoded value/datatype/lang) in Parquet, DuckDB and SQLite — the same 234,013 facts, queryable in SQL to compare against the rete engine.", "tables": [{"name": "triples", "file": "triples.parquet", "label": "triples — every fact (S, P, O)", "entities": 234013}], "sqlCols": ["subject", "predicate", "object", "otype", "value", "datatype", "lang"], "examples": [{"label": "Predicate totals", "table": {"file": "triples.parquet", "name": "triples"}, "sparql": "SELECT ?p (COUNT(*) AS ?n) WHERE { ?s ?p ?o } GROUP BY ?p ORDER BY DESC(?n)", "duck": "SELECT predicate, COUNT(*) AS n\nFROM {T}\nGROUP BY predicate\nORDER BY n DESC;", "sqlite": "SELECT predicate, COUNT(*) AS n\nFROM \"triples\"\nGROUP BY predicate\nORDER BY n DESC;", "note": "How often each predicate is used — the shape of the graph, as a GROUP BY over the flat triples table."}, {"label": "Busiest subjects", "table": {"file": "triples.parquet", "name": "triples"}, "sparql": "SELECT ?s (COUNT(*) AS ?n) WHERE { ?s ?p ?o } GROUP BY ?s ORDER BY DESC(?n) LIMIT 50", "duck": "SELECT subject, COUNT(*) AS n\nFROM {T}\nGROUP BY subject\nORDER BY n DESC\nLIMIT 50;", "sqlite": "SELECT subject, COUNT(*) AS n\nFROM \"triples\"\nGROUP BY subject\nORDER BY n DESC\nLIMIT 50;", "note": "The 50 subjects with the most outgoing facts — the hubs of the graph."}]},
    "lineara": {"rete": "lineara/lineara.rete", "parquetDir": "lineara/lineara-tables", "duckdb": "lineara/lineara.duckdb", "duckdbSize": "1.3 MB", "sqlite": "lineara/lineara.sqlite", "sqliteSize": "8.5 MB", "flat": true, "about": "The lineara graph as a flat <code>triples</code> table (subject, predicate, object token + decoded value/datatype/lang) in Parquet, DuckDB and SQLite — the same 32,153 facts, queryable in SQL to compare against the rete engine.", "tables": [{"name": "triples", "file": "triples.parquet", "label": "triples — every fact (S, P, O)", "entities": 32153}], "sqlCols": ["subject", "predicate", "object", "otype", "value", "datatype", "lang"], "examples": [{"label": "Predicate totals", "table": {"file": "triples.parquet", "name": "triples"}, "sparql": "SELECT ?p (COUNT(*) AS ?n) WHERE { ?s ?p ?o } GROUP BY ?p ORDER BY DESC(?n)", "duck": "SELECT predicate, COUNT(*) AS n\nFROM {T}\nGROUP BY predicate\nORDER BY n DESC;", "sqlite": "SELECT predicate, COUNT(*) AS n\nFROM \"triples\"\nGROUP BY predicate\nORDER BY n DESC;", "note": "How often each predicate is used — the shape of the graph, as a GROUP BY over the flat triples table."}, {"label": "Busiest subjects", "table": {"file": "triples.parquet", "name": "triples"}, "sparql": "SELECT ?s (COUNT(*) AS ?n) WHERE { ?s ?p ?o } GROUP BY ?s ORDER BY DESC(?n) LIMIT 50", "duck": "SELECT subject, COUNT(*) AS n\nFROM {T}\nGROUP BY subject\nORDER BY n DESC\nLIMIT 50;", "sqlite": "SELECT subject, COUNT(*) AS n\nFROM \"triples\"\nGROUP BY subject\nORDER BY n DESC\nLIMIT 50;", "note": "The 50 subjects with the most outgoing facts — the hubs of the graph."}]},
    "linked-jazz": {"rete": "linked-jazz/linked-jazz.rete", "parquetDir": "linked-jazz/linked-jazz-tables", "duckdb": "linked-jazz/linked-jazz.duckdb", "duckdbSize": "1.0 MB", "sqlite": "linked-jazz/linked-jazz.sqlite", "sqliteSize": "2.9 MB", "flat": true, "about": "The linked-jazz graph as a flat <code>triples</code> table (subject, predicate, object token + decoded value/datatype/lang) in Parquet, DuckDB and SQLite — the same 9,466 facts, queryable in SQL to compare against the rete engine.", "tables": [{"name": "triples", "file": "triples.parquet", "label": "triples — every fact (S, P, O)", "entities": 9466}], "sqlCols": ["subject", "predicate", "object", "otype", "value", "datatype", "lang"], "examples": [{"label": "Predicate totals", "table": {"file": "triples.parquet", "name": "triples"}, "sparql": "SELECT ?p (COUNT(*) AS ?n) WHERE { ?s ?p ?o } GROUP BY ?p ORDER BY DESC(?n)", "duck": "SELECT predicate, COUNT(*) AS n\nFROM {T}\nGROUP BY predicate\nORDER BY n DESC;", "sqlite": "SELECT predicate, COUNT(*) AS n\nFROM \"triples\"\nGROUP BY predicate\nORDER BY n DESC;", "note": "How often each predicate is used — the shape of the graph, as a GROUP BY over the flat triples table."}, {"label": "Busiest subjects", "table": {"file": "triples.parquet", "name": "triples"}, "sparql": "SELECT ?s (COUNT(*) AS ?n) WHERE { ?s ?p ?o } GROUP BY ?s ORDER BY DESC(?n) LIMIT 50", "duck": "SELECT subject, COUNT(*) AS n\nFROM {T}\nGROUP BY subject\nORDER BY n DESC\nLIMIT 50;", "sqlite": "SELECT subject, COUNT(*) AS n\nFROM \"triples\"\nGROUP BY subject\nORDER BY n DESC\nLIMIT 50;", "note": "The 50 subjects with the most outgoing facts — the hubs of the graph."}]},
    "memoria": {"rete": "memoria/memoria.rete", "parquetDir": "memoria/memoria-tables", "duckdb": "memoria/memoria.duckdb", "duckdbSize": "24.3 MB", "sqlite": "memoria/memoria.sqlite", "sqliteSize": "346.7 MB", "flat": true, "about": "The memoria graph as a flat <code>triples</code> table (subject, predicate, object token + decoded value/datatype/lang) in Parquet, DuckDB and SQLite — the same 1,214,111 facts, queryable in SQL to compare against the rete engine.", "tables": [{"name": "triples", "file": "triples.parquet", "label": "triples — every fact (S, P, O)", "entities": 1214111}], "sqlCols": ["subject", "predicate", "object", "otype", "value", "datatype", "lang"], "examples": [{"label": "Predicate totals", "table": {"file": "triples.parquet", "name": "triples"}, "sparql": "SELECT ?p (COUNT(*) AS ?n) WHERE { ?s ?p ?o } GROUP BY ?p ORDER BY DESC(?n)", "duck": "SELECT predicate, COUNT(*) AS n\nFROM {T}\nGROUP BY predicate\nORDER BY n DESC;", "sqlite": "SELECT predicate, COUNT(*) AS n\nFROM \"triples\"\nGROUP BY predicate\nORDER BY n DESC;", "note": "How often each predicate is used — the shape of the graph, as a GROUP BY over the flat triples table."}, {"label": "Busiest subjects", "table": {"file": "triples.parquet", "name": "triples"}, "sparql": "SELECT ?s (COUNT(*) AS ?n) WHERE { ?s ?p ?o } GROUP BY ?s ORDER BY DESC(?n) LIMIT 50", "duck": "SELECT subject, COUNT(*) AS n\nFROM {T}\nGROUP BY subject\nORDER BY n DESC\nLIMIT 50;", "sqlite": "SELECT subject, COUNT(*) AS n\nFROM \"triples\"\nGROUP BY subject\nORDER BY n DESC\nLIMIT 50;", "note": "The 50 subjects with the most outgoing facts — the hubs of the graph."}]},
    "mimotext": {"rete": "mimotext/mimotext.rete", "parquetDir": "mimotext/mimotext-tables", "duckdb": "mimotext/mimotext.duckdb", "duckdbSize": "1.3 MB", "sqlite": "mimotext/mimotext.sqlite", "sqliteSize": "9.9 MB", "flat": true, "about": "The mimotext graph as a flat <code>triples</code> table (subject, predicate, object token + decoded value/datatype/lang) in Parquet, DuckDB and SQLite — the same 27,389 facts, queryable in SQL to compare against the rete engine.", "tables": [{"name": "triples", "file": "triples.parquet", "label": "triples — every fact (S, P, O)", "entities": 27389}], "sqlCols": ["subject", "predicate", "object", "otype", "value", "datatype", "lang"], "examples": [{"label": "Predicate totals", "table": {"file": "triples.parquet", "name": "triples"}, "sparql": "SELECT ?p (COUNT(*) AS ?n) WHERE { ?s ?p ?o } GROUP BY ?p ORDER BY DESC(?n)", "duck": "SELECT predicate, COUNT(*) AS n\nFROM {T}\nGROUP BY predicate\nORDER BY n DESC;", "sqlite": "SELECT predicate, COUNT(*) AS n\nFROM \"triples\"\nGROUP BY predicate\nORDER BY n DESC;", "note": "How often each predicate is used — the shape of the graph, as a GROUP BY over the flat triples table."}, {"label": "Busiest subjects", "table": {"file": "triples.parquet", "name": "triples"}, "sparql": "SELECT ?s (COUNT(*) AS ?n) WHERE { ?s ?p ?o } GROUP BY ?s ORDER BY DESC(?n) LIMIT 50", "duck": "SELECT subject, COUNT(*) AS n\nFROM {T}\nGROUP BY subject\nORDER BY n DESC\nLIMIT 50;", "sqlite": "SELECT subject, COUNT(*) AS n\nFROM \"triples\"\nGROUP BY subject\nORDER BY n DESC\nLIMIT 50;", "note": "The 50 subjects with the most outgoing facts — the hubs of the graph."}]},
    "mira-wikidata": {"rete": "mira-wikidata/mira-wikidata.rete", "parquetDir": "mira-wikidata/mira-wikidata-tables", "duckdb": "mira-wikidata/mira-wikidata.duckdb", "duckdbSize": "524 KB", "sqlite": "mira-wikidata/mira-wikidata.sqlite", "sqliteSize": "60 KB", "flat": true, "about": "The mira-wikidata graph as a flat <code>triples</code> table (subject, predicate, object token + decoded value/datatype/lang) in Parquet, DuckDB and SQLite — the same 125 facts, queryable in SQL to compare against the rete engine.", "tables": [{"name": "triples", "file": "triples.parquet", "label": "triples — every fact (S, P, O)", "entities": 125}], "sqlCols": ["subject", "predicate", "object", "otype", "value", "datatype", "lang"], "examples": [{"label": "Predicate totals", "table": {"file": "triples.parquet", "name": "triples"}, "sparql": "SELECT ?p (COUNT(*) AS ?n) WHERE { ?s ?p ?o } GROUP BY ?p ORDER BY DESC(?n)", "duck": "SELECT predicate, COUNT(*) AS n\nFROM {T}\nGROUP BY predicate\nORDER BY n DESC;", "sqlite": "SELECT predicate, COUNT(*) AS n\nFROM \"triples\"\nGROUP BY predicate\nORDER BY n DESC;", "note": "How often each predicate is used — the shape of the graph, as a GROUP BY over the flat triples table."}, {"label": "Busiest subjects", "table": {"file": "triples.parquet", "name": "triples"}, "sparql": "SELECT ?s (COUNT(*) AS ?n) WHERE { ?s ?p ?o } GROUP BY ?s ORDER BY DESC(?n) LIMIT 50", "duck": "SELECT subject, COUNT(*) AS n\nFROM {T}\nGROUP BY subject\nORDER BY n DESC\nLIMIT 50;", "sqlite": "SELECT subject, COUNT(*) AS n\nFROM \"triples\"\nGROUP BY subject\nORDER BY n DESC\nLIMIT 50;", "note": "The 50 subjects with the most outgoing facts — the hubs of the graph."}]},
    "mira": {"rete": "mira/mira.rete", "parquetDir": "mira/mira-tables", "duckdb": "mira/mira.duckdb", "duckdbSize": "524 KB", "sqlite": "mira/mira.sqlite", "sqliteSize": "1.5 MB", "flat": true, "about": "The mira graph as a flat <code>triples</code> table (subject, predicate, object token + decoded value/datatype/lang) in Parquet, DuckDB and SQLite — the same 5,044 facts, queryable in SQL to compare against the rete engine.", "tables": [{"name": "triples", "file": "triples.parquet", "label": "triples — every fact (S, P, O)", "entities": 5044}], "sqlCols": ["subject", "predicate", "object", "otype", "value", "datatype", "lang"], "examples": [{"label": "Predicate totals", "table": {"file": "triples.parquet", "name": "triples"}, "sparql": "SELECT ?p (COUNT(*) AS ?n) WHERE { ?s ?p ?o } GROUP BY ?p ORDER BY DESC(?n)", "duck": "SELECT predicate, COUNT(*) AS n\nFROM {T}\nGROUP BY predicate\nORDER BY n DESC;", "sqlite": "SELECT predicate, COUNT(*) AS n\nFROM \"triples\"\nGROUP BY predicate\nORDER BY n DESC;", "note": "How often each predicate is used — the shape of the graph, as a GROUP BY over the flat triples table."}, {"label": "Busiest subjects", "table": {"file": "triples.parquet", "name": "triples"}, "sparql": "SELECT ?s (COUNT(*) AS ?n) WHERE { ?s ?p ?o } GROUP BY ?s ORDER BY DESC(?n) LIMIT 50", "duck": "SELECT subject, COUNT(*) AS n\nFROM {T}\nGROUP BY subject\nORDER BY n DESC\nLIMIT 50;", "sqlite": "SELECT subject, COUNT(*) AS n\nFROM \"triples\"\nGROUP BY subject\nORDER BY n DESC\nLIMIT 50;", "note": "The 50 subjects with the most outgoing facts — the hubs of the graph."}]},
    "nidm": {"rete": "nidm/nidm.rete", "parquetDir": "nidm/nidm-tables", "duckdb": "nidm/nidm.duckdb", "duckdbSize": "524 KB", "sqlite": "nidm/nidm.sqlite", "sqliteSize": "1.2 MB", "flat": true, "about": "The nidm graph as a flat <code>triples</code> table (subject, predicate, object token + decoded value/datatype/lang) in Parquet, DuckDB and SQLite — the same 3,153 facts, queryable in SQL to compare against the rete engine.", "tables": [{"name": "triples", "file": "triples.parquet", "label": "triples — every fact (S, P, O)", "entities": 3153}], "sqlCols": ["subject", "predicate", "object", "otype", "value", "datatype", "lang"], "examples": [{"label": "Predicate totals", "table": {"file": "triples.parquet", "name": "triples"}, "sparql": "SELECT ?p (COUNT(*) AS ?n) WHERE { ?s ?p ?o } GROUP BY ?p ORDER BY DESC(?n)", "duck": "SELECT predicate, COUNT(*) AS n\nFROM {T}\nGROUP BY predicate\nORDER BY n DESC;", "sqlite": "SELECT predicate, COUNT(*) AS n\nFROM \"triples\"\nGROUP BY predicate\nORDER BY n DESC;", "note": "How often each predicate is used — the shape of the graph, as a GROUP BY over the flat triples table."}, {"label": "Busiest subjects", "table": {"file": "triples.parquet", "name": "triples"}, "sparql": "SELECT ?s (COUNT(*) AS ?n) WHERE { ?s ?p ?o } GROUP BY ?s ORDER BY DESC(?n) LIMIT 50", "duck": "SELECT subject, COUNT(*) AS n\nFROM {T}\nGROUP BY subject\nORDER BY n DESC\nLIMIT 50;", "sqlite": "SELECT subject, COUNT(*) AS n\nFROM \"triples\"\nGROUP BY subject\nORDER BY n DESC\nLIMIT 50;", "note": "The 50 subjects with the most outgoing facts — the hubs of the graph."}]},
    "nomisma": {"rete": "nomisma/nomisma.rete", "parquetDir": "nomisma/nomisma-tables", "duckdb": "nomisma/nomisma.duckdb", "duckdbSize": "1.8 MB", "sqlite": "nomisma/nomisma.sqlite", "sqliteSize": "16.8 MB", "flat": true, "about": "The nomisma graph as a flat <code>triples</code> table (subject, predicate, object token + decoded value/datatype/lang) in Parquet, DuckDB and SQLite — the same 53,535 facts, queryable in SQL to compare against the rete engine.", "tables": [{"name": "triples", "file": "triples.parquet", "label": "triples — every fact (S, P, O)", "entities": 53535}], "sqlCols": ["subject", "predicate", "object", "otype", "value", "datatype", "lang"], "examples": [{"label": "Predicate totals", "table": {"file": "triples.parquet", "name": "triples"}, "sparql": "SELECT ?p (COUNT(*) AS ?n) WHERE { ?s ?p ?o } GROUP BY ?p ORDER BY DESC(?n)", "duck": "SELECT predicate, COUNT(*) AS n\nFROM {T}\nGROUP BY predicate\nORDER BY n DESC;", "sqlite": "SELECT predicate, COUNT(*) AS n\nFROM \"triples\"\nGROUP BY predicate\nORDER BY n DESC;", "note": "How often each predicate is used — the shape of the graph, as a GROUP BY over the flat triples table."}, {"label": "Busiest subjects", "table": {"file": "triples.parquet", "name": "triples"}, "sparql": "SELECT ?s (COUNT(*) AS ?n) WHERE { ?s ?p ?o } GROUP BY ?s ORDER BY DESC(?n) LIMIT 50", "duck": "SELECT subject, COUNT(*) AS n\nFROM {T}\nGROUP BY subject\nORDER BY n DESC\nLIMIT 50;", "sqlite": "SELECT subject, COUNT(*) AS n\nFROM \"triples\"\nGROUP BY subject\nORDER BY n DESC\nLIMIT 50;", "note": "The 50 subjects with the most outgoing facts — the hubs of the graph."}]},
    "ontoneurolog": {"rete": "ontoneurolog/ontoneurolog.rete", "parquetDir": "ontoneurolog/ontoneurolog-tables", "duckdb": "ontoneurolog/ontoneurolog.duckdb", "duckdbSize": "1.8 MB", "sqlite": "ontoneurolog/ontoneurolog.sqlite", "sqliteSize": "8.0 MB", "flat": true, "about": "The ontoneurolog graph as a flat <code>triples</code> table (subject, predicate, object token + decoded value/datatype/lang) in Parquet, DuckDB and SQLite — the same 17,658 facts, queryable in SQL to compare against the rete engine.", "tables": [{"name": "triples", "file": "triples.parquet", "label": "triples — every fact (S, P, O)", "entities": 17658}], "sqlCols": ["subject", "predicate", "object", "otype", "value", "datatype", "lang"], "examples": [{"label": "Predicate totals", "table": {"file": "triples.parquet", "name": "triples"}, "sparql": "SELECT ?p (COUNT(*) AS ?n) WHERE { ?s ?p ?o } GROUP BY ?p ORDER BY DESC(?n)", "duck": "SELECT predicate, COUNT(*) AS n\nFROM {T}\nGROUP BY predicate\nORDER BY n DESC;", "sqlite": "SELECT predicate, COUNT(*) AS n\nFROM \"triples\"\nGROUP BY predicate\nORDER BY n DESC;", "note": "How often each predicate is used — the shape of the graph, as a GROUP BY over the flat triples table."}, {"label": "Busiest subjects", "table": {"file": "triples.parquet", "name": "triples"}, "sparql": "SELECT ?s (COUNT(*) AS ?n) WHERE { ?s ?p ?o } GROUP BY ?s ORDER BY DESC(?n) LIMIT 50", "duck": "SELECT subject, COUNT(*) AS n\nFROM {T}\nGROUP BY subject\nORDER BY n DESC\nLIMIT 50;", "sqlite": "SELECT subject, COUNT(*) AS n\nFROM \"triples\"\nGROUP BY subject\nORDER BY n DESC\nLIMIT 50;", "note": "The 50 subjects with the most outgoing facts — the hubs of the graph."}]},
    "openalex-astrocytes": {"rete": "openalex-astrocytes/openalex-astrocytes.rete", "parquetDir": "openalex-astrocytes/openalex-astrocytes-tables", "duckdb": "openalex-astrocytes/openalex-astrocytes.duckdb", "duckdbSize": "1.3 MB", "sqlite": "openalex-astrocytes/openalex-astrocytes.sqlite", "sqliteSize": "5.6 MB", "flat": true, "about": "The openalex-astrocytes graph as a flat <code>triples</code> table (subject, predicate, object token + decoded value/datatype/lang) in Parquet, DuckDB and SQLite — the same 24,042 facts, queryable in SQL to compare against the rete engine.", "tables": [{"name": "triples", "file": "triples.parquet", "label": "triples — every fact (S, P, O)", "entities": 24042}], "sqlCols": ["subject", "predicate", "object", "otype", "value", "datatype", "lang"], "examples": [{"label": "Predicate totals", "table": {"file": "triples.parquet", "name": "triples"}, "sparql": "SELECT ?p (COUNT(*) AS ?n) WHERE { ?s ?p ?o } GROUP BY ?p ORDER BY DESC(?n)", "duck": "SELECT predicate, COUNT(*) AS n\nFROM {T}\nGROUP BY predicate\nORDER BY n DESC;", "sqlite": "SELECT predicate, COUNT(*) AS n\nFROM \"triples\"\nGROUP BY predicate\nORDER BY n DESC;", "note": "How often each predicate is used — the shape of the graph, as a GROUP BY over the flat triples table."}, {"label": "Busiest subjects", "table": {"file": "triples.parquet", "name": "triples"}, "sparql": "SELECT ?s (COUNT(*) AS ?n) WHERE { ?s ?p ?o } GROUP BY ?s ORDER BY DESC(?n) LIMIT 50", "duck": "SELECT subject, COUNT(*) AS n\nFROM {T}\nGROUP BY subject\nORDER BY n DESC\nLIMIT 50;", "sqlite": "SELECT subject, COUNT(*) AS n\nFROM \"triples\"\nGROUP BY subject\nORDER BY n DESC\nLIMIT 50;", "note": "The 50 subjects with the most outgoing facts — the hubs of the graph."}]},
    "peirce": {"rete": "peirce/peirce.rete", "parquetDir": "peirce/peirce-tables", "duckdb": "peirce/peirce.duckdb", "duckdbSize": "2.0 MB", "sqlite": "peirce/peirce.sqlite", "sqliteSize": "14.2 MB", "flat": true, "about": "The peirce graph as a flat <code>triples</code> table (subject, predicate, object token + decoded value/datatype/lang) in Parquet, DuckDB and SQLite — the same 36,179 facts, queryable in SQL to compare against the rete engine.", "tables": [{"name": "triples", "file": "triples.parquet", "label": "triples — every fact (S, P, O)", "entities": 36179}], "sqlCols": ["subject", "predicate", "object", "otype", "value", "datatype", "lang"], "examples": [{"label": "Predicate totals", "table": {"file": "triples.parquet", "name": "triples"}, "sparql": "SELECT ?p (COUNT(*) AS ?n) WHERE { ?s ?p ?o } GROUP BY ?p ORDER BY DESC(?n)", "duck": "SELECT predicate, COUNT(*) AS n\nFROM {T}\nGROUP BY predicate\nORDER BY n DESC;", "sqlite": "SELECT predicate, COUNT(*) AS n\nFROM \"triples\"\nGROUP BY predicate\nORDER BY n DESC;", "note": "How often each predicate is used — the shape of the graph, as a GROUP BY over the flat triples table."}, {"label": "Busiest subjects", "table": {"file": "triples.parquet", "name": "triples"}, "sparql": "SELECT ?s (COUNT(*) AS ?n) WHERE { ?s ?p ?o } GROUP BY ?s ORDER BY DESC(?n) LIMIT 50", "duck": "SELECT subject, COUNT(*) AS n\nFROM {T}\nGROUP BY subject\nORDER BY n DESC\nLIMIT 50;", "sqlite": "SELECT subject, COUNT(*) AS n\nFROM \"triples\"\nGROUP BY subject\nORDER BY n DESC\nLIMIT 50;", "note": "The 50 subjects with the most outgoing facts — the hubs of the graph."}]},
    "boe": {"rete": "boe/boe.rete", "parquetDir": "boe/boe-tables", "duckdb": "boe/boe.duckdb", "duckdbSize": "26.0 MB", "sqlite": "boe/boe.sqlite", "sqliteSize": "178.8 MB", "flat": true, "about": "The boe graph as a flat <code>triples</code> table (subject, predicate, object token + decoded value/datatype/lang) in Parquet, DuckDB and SQLite — the same 465,249 facts, queryable in SQL to compare against the rete engine.", "tables": [{"name": "triples", "file": "triples.parquet", "label": "triples — every fact (S, P, O)", "entities": 465249}], "sqlCols": ["subject", "predicate", "object", "otype", "value", "datatype", "lang"], "examples": [{"label": "Norms by rango", "table": {"file": "triples.parquet", "name": "triples"}, "sparql": "SELECT ?r (COUNT(*) AS ?n) WHERE { ?s <http://www.w3.org/1999/02/22-rdf-syntax-ns#type> ?r } GROUP BY ?r ORDER BY DESC(?n)", "duck": "SELECT object AS rango, COUNT(*) AS n\nFROM {T}\nWHERE predicate = 'http://www.w3.org/1999/02/22-rdf-syntax-ns#type'\n  AND object LIKE 'https://graphplaza.com/ns/boe#%'\nGROUP BY object\nORDER BY n DESC;", "sqlite": "SELECT object AS rango, COUNT(*) AS n\nFROM \"triples\"\nWHERE predicate = 'http://www.w3.org/1999/02/22-rdf-syntax-ns#type'\n  AND object LIKE 'https://graphplaza.com/ns/boe#%'\nGROUP BY object\nORDER BY n DESC;", "note": "How many norms of each rango (Ley, Real Decreto, Orden…) — a GROUP BY over the type triples, filtered to the bx: rango classes."}, {"label": "Relationship edges by type", "table": {"file": "triples.parquet", "name": "triples"}, "sparql": "SELECT ?p (COUNT(*) AS ?n) WHERE { ?s ?p ?o } GROUP BY ?p ORDER BY DESC(?n)", "duck": "SELECT predicate, COUNT(*) AS n\nFROM {T}\nWHERE predicate LIKE 'http://data.europa.eu/eli/ontology#%'\n   OR predicate LIKE 'https://graphplaza.com/ns/boe#%'\nGROUP BY predicate\nORDER BY n DESC;", "sqlite": "SELECT predicate, COUNT(*) AS n\nFROM \"triples\"\nWHERE predicate LIKE 'http://data.europa.eu/eli/ontology#%'\n   OR predicate LIKE 'https://graphplaza.com/ns/boe#%'\nGROUP BY predicate\nORDER BY n DESC;", "note": "The shape of the legal-citation graph: how many repeals / amends / cites / transposes / based_on edges, as a GROUP BY over predicates."}]},
    "scrolls": {"rete": "scrolls/scrolls.rete", "parquetDir": "scrolls/scrolls-tables", "duckdb": "scrolls/scrolls.duckdb", "duckdbSize": "1.76 MB", "sqlite": "scrolls/scrolls.sqlite", "sqliteSize": "10.95 MB", "flat": true, "about": "The Vesuvius Challenge scrolls graph as a flat <code>triples</code> table (subject, predicate, object token + decoded value/datatype/lang) in Parquet, DuckDB and SQLite — the same 18,125 facts, queryable in SQL to compare against the rete engine. Every <code>object</code> that is a DataAsset subject is a public download URL.", "tables": [{"name": "triples", "file": "triples.parquet", "label": "triples — every fact (S, P, O)", "entities": 18125}], "sqlCols": ["subject", "predicate", "object", "otype", "value", "datatype", "lang"], "examples": [{"label": "Assets by type (what's downloadable)", "table": {"file": "triples.parquet", "name": "triples"}, "sparql": "PREFIX vs: <https://scrollprize.org/vocab#>\nSELECT ?t (COUNT(*) AS ?n) WHERE { ?a vs:assetType ?t } GROUP BY ?t ORDER BY DESC(?n)", "duck": "SELECT value AS asset_type, COUNT(*) AS n\nFROM {T}\nWHERE predicate = 'https://scrollprize.org/vocab#assetType'\nGROUP BY value ORDER BY n DESC;", "sqlite": "SELECT value AS asset_type, COUNT(*) AS n\nFROM \"triples\"\nWHERE predicate = 'https://scrollprize.org/vocab#assetType'\nGROUP BY value ORDER BY n DESC;", "note": "Every downloadable file kind and its count — ome-zarr volumes, obj meshes, tifxyz coordinate maps, ink-detection TIFFs — as a GROUP BY over the flat triples table."}, {"label": "Predicate totals", "table": {"file": "triples.parquet", "name": "triples"}, "sparql": "SELECT ?p (COUNT(*) AS ?n) WHERE { ?s ?p ?o } GROUP BY ?p ORDER BY DESC(?n)", "duck": "SELECT predicate, COUNT(*) AS n\nFROM {T}\nGROUP BY predicate\nORDER BY n DESC;", "sqlite": "SELECT predicate, COUNT(*) AS n\nFROM \"triples\"\nGROUP BY predicate\nORDER BY n DESC;", "note": "How often each predicate is used — the shape of the graph, as a GROUP BY over the flat triples table."}]},
    "postscriptum": {"rete": "postscriptum/postscriptum.rete", "parquetDir": "postscriptum/postscriptum-tables", "duckdb": "postscriptum/postscriptum.duckdb", "duckdbSize": "30.0 MB", "sqlite": "postscriptum/postscriptum.sqlite", "sqliteSize": "39.3 MB", "flat": true, "about": "The postscriptum graph as a flat <code>triples</code> table (subject, predicate, object token + decoded value/datatype/lang) in Parquet, DuckDB and SQLite — the same 63,087 facts, queryable in SQL to compare against the rete engine.", "tables": [{"name": "triples", "file": "triples.parquet", "label": "triples — every fact (S, P, O)", "entities": 63087}], "sqlCols": ["subject", "predicate", "object", "otype", "value", "datatype", "lang"], "examples": [{"label": "Predicate totals", "table": {"file": "triples.parquet", "name": "triples"}, "sparql": "SELECT ?p (COUNT(*) AS ?n) WHERE { ?s ?p ?o } GROUP BY ?p ORDER BY DESC(?n)", "duck": "SELECT predicate, COUNT(*) AS n\nFROM {T}\nGROUP BY predicate\nORDER BY n DESC;", "sqlite": "SELECT predicate, COUNT(*) AS n\nFROM \"triples\"\nGROUP BY predicate\nORDER BY n DESC;", "note": "How often each predicate is used — the shape of the graph, as a GROUP BY over the flat triples table."}, {"label": "Busiest subjects", "table": {"file": "triples.parquet", "name": "triples"}, "sparql": "SELECT ?s (COUNT(*) AS ?n) WHERE { ?s ?p ?o } GROUP BY ?s ORDER BY DESC(?n) LIMIT 50", "duck": "SELECT subject, COUNT(*) AS n\nFROM {T}\nGROUP BY subject\nORDER BY n DESC\nLIMIT 50;", "sqlite": "SELECT subject, COUNT(*) AS n\nFROM \"triples\"\nGROUP BY subject\nORDER BY n DESC\nLIMIT 50;", "note": "The 50 subjects with the most outgoing facts — the hubs of the graph."}]},
    "scholar-noisy": {"rete": "scholar-noisy/scholar-noisy.rete", "parquetDir": "scholar-noisy/scholar-noisy-tables", "duckdb": "scholar-noisy/scholar-noisy.duckdb", "duckdbSize": "780 KB", "sqlite": "scholar-noisy/scholar-noisy.sqlite", "sqliteSize": "1.4 MB", "flat": true, "about": "The scholar-noisy graph as a flat <code>triples</code> table (subject, predicate, object token + decoded value/datatype/lang) in Parquet, DuckDB and SQLite — the same 6,671 facts, queryable in SQL to compare against the rete engine.", "tables": [{"name": "triples", "file": "triples.parquet", "label": "triples — every fact (S, P, O)", "entities": 6671}], "sqlCols": ["subject", "predicate", "object", "otype", "value", "datatype", "lang"], "examples": [{"label": "Predicate totals", "table": {"file": "triples.parquet", "name": "triples"}, "sparql": "SELECT ?p (COUNT(*) AS ?n) WHERE { ?s ?p ?o } GROUP BY ?p ORDER BY DESC(?n)", "duck": "SELECT predicate, COUNT(*) AS n\nFROM {T}\nGROUP BY predicate\nORDER BY n DESC;", "sqlite": "SELECT predicate, COUNT(*) AS n\nFROM \"triples\"\nGROUP BY predicate\nORDER BY n DESC;", "note": "How often each predicate is used — the shape of the graph, as a GROUP BY over the flat triples table."}, {"label": "Busiest subjects", "table": {"file": "triples.parquet", "name": "triples"}, "sparql": "SELECT ?s (COUNT(*) AS ?n) WHERE { ?s ?p ?o } GROUP BY ?s ORDER BY DESC(?n) LIMIT 50", "duck": "SELECT subject, COUNT(*) AS n\nFROM {T}\nGROUP BY subject\nORDER BY n DESC\nLIMIT 50;", "sqlite": "SELECT subject, COUNT(*) AS n\nFROM \"triples\"\nGROUP BY subject\nORDER BY n DESC\nLIMIT 50;", "note": "The 50 subjects with the most outgoing facts — the hubs of the graph."}]},
    "scholar": {"rete": "scholar/scholar.rete", "parquetDir": "scholar/scholar-tables", "duckdb": "scholar/scholar.duckdb", "duckdbSize": "780 KB", "sqlite": "scholar/scholar.sqlite", "sqliteSize": "1.4 MB", "flat": true, "about": "The scholar graph as a flat <code>triples</code> table (subject, predicate, object token + decoded value/datatype/lang) in Parquet, DuckDB and SQLite — the same 6,954 facts, queryable in SQL to compare against the rete engine.", "tables": [{"name": "triples", "file": "triples.parquet", "label": "triples — every fact (S, P, O)", "entities": 6954}], "sqlCols": ["subject", "predicate", "object", "otype", "value", "datatype", "lang"], "examples": [{"label": "Predicate totals", "table": {"file": "triples.parquet", "name": "triples"}, "sparql": "SELECT ?p (COUNT(*) AS ?n) WHERE { ?s ?p ?o } GROUP BY ?p ORDER BY DESC(?n)", "duck": "SELECT predicate, COUNT(*) AS n\nFROM {T}\nGROUP BY predicate\nORDER BY n DESC;", "sqlite": "SELECT predicate, COUNT(*) AS n\nFROM \"triples\"\nGROUP BY predicate\nORDER BY n DESC;", "note": "How often each predicate is used — the shape of the graph, as a GROUP BY over the flat triples table."}, {"label": "Busiest subjects", "table": {"file": "triples.parquet", "name": "triples"}, "sparql": "SELECT ?s (COUNT(*) AS ?n) WHERE { ?s ?p ?o } GROUP BY ?s ORDER BY DESC(?n) LIMIT 50", "duck": "SELECT subject, COUNT(*) AS n\nFROM {T}\nGROUP BY subject\nORDER BY n DESC\nLIMIT 50;", "sqlite": "SELECT subject, COUNT(*) AS n\nFROM \"triples\"\nGROUP BY subject\nORDER BY n DESC\nLIMIT 50;", "note": "The 50 subjects with the most outgoing facts — the hubs of the graph."}]},
    "theographic-graph": {"rete": "theographic-graph/theographic-graph.rete", "parquetDir": "theographic-graph/theographic-graph-tables", "duckdb": "theographic-graph/theographic-graph.duckdb", "duckdbSize": "1.3 MB", "sqlite": "theographic-graph/theographic-graph.sqlite", "sqliteSize": "8.2 MB", "flat": true, "about": "The theographic-graph graph as a flat <code>triples</code> table (subject, predicate, object token + decoded value/datatype/lang) in Parquet, DuckDB and SQLite — the same 31,945 facts, queryable in SQL to compare against the rete engine.", "tables": [{"name": "triples", "file": "triples.parquet", "label": "triples — every fact (S, P, O)", "entities": 31945}], "sqlCols": ["subject", "predicate", "object", "otype", "value", "datatype", "lang"], "examples": [{"label": "Predicate totals", "table": {"file": "triples.parquet", "name": "triples"}, "sparql": "SELECT ?p (COUNT(*) AS ?n) WHERE { ?s ?p ?o } GROUP BY ?p ORDER BY DESC(?n)", "duck": "SELECT predicate, COUNT(*) AS n\nFROM {T}\nGROUP BY predicate\nORDER BY n DESC;", "sqlite": "SELECT predicate, COUNT(*) AS n\nFROM \"triples\"\nGROUP BY predicate\nORDER BY n DESC;", "note": "How often each predicate is used — the shape of the graph, as a GROUP BY over the flat triples table."}, {"label": "Busiest subjects", "table": {"file": "triples.parquet", "name": "triples"}, "sparql": "SELECT ?s (COUNT(*) AS ?n) WHERE { ?s ?p ?o } GROUP BY ?s ORDER BY DESC(?n) LIMIT 50", "duck": "SELECT subject, COUNT(*) AS n\nFROM {T}\nGROUP BY subject\nORDER BY n DESC\nLIMIT 50;", "sqlite": "SELECT subject, COUNT(*) AS n\nFROM \"triples\"\nGROUP BY subject\nORDER BY n DESC\nLIMIT 50;", "note": "The 50 subjects with the most outgoing facts — the hubs of the graph."}]},
    "ustc": {"rete": "ustc/ustc.rete", "parquetDir": "ustc/ustc-tables", "duckdb": "ustc/ustc.duckdb", "duckdbSize": "4.8 MB", "sqlite": "ustc/ustc.sqlite", "sqliteSize": "28.1 MB", "flat": true, "about": "The ustc graph as a flat <code>triples</code> table (subject, predicate, object token + decoded value/datatype/lang) in Parquet, DuckDB and SQLite — the same 88,157 facts, queryable in SQL to compare against the rete engine.", "tables": [{"name": "triples", "file": "triples.parquet", "label": "triples — every fact (S, P, O)", "entities": 88157}], "sqlCols": ["subject", "predicate", "object", "otype", "value", "datatype", "lang"], "examples": [{"label": "Predicate totals", "table": {"file": "triples.parquet", "name": "triples"}, "sparql": "SELECT ?p (COUNT(*) AS ?n) WHERE { ?s ?p ?o } GROUP BY ?p ORDER BY DESC(?n)", "duck": "SELECT predicate, COUNT(*) AS n\nFROM {T}\nGROUP BY predicate\nORDER BY n DESC;", "sqlite": "SELECT predicate, COUNT(*) AS n\nFROM \"triples\"\nGROUP BY predicate\nORDER BY n DESC;", "note": "How often each predicate is used — the shape of the graph, as a GROUP BY over the flat triples table."}, {"label": "Busiest subjects", "table": {"file": "triples.parquet", "name": "triples"}, "sparql": "SELECT ?s (COUNT(*) AS ?n) WHERE { ?s ?p ?o } GROUP BY ?s ORDER BY DESC(?n) LIMIT 50", "duck": "SELECT subject, COUNT(*) AS n\nFROM {T}\nGROUP BY subject\nORDER BY n DESC\nLIMIT 50;", "sqlite": "SELECT subject, COUNT(*) AS n\nFROM \"triples\"\nGROUP BY subject\nORDER BY n DESC\nLIMIT 50;", "note": "The 50 subjects with the most outgoing facts — the hubs of the graph."}]},
    "bioexplora": {"rete": "bioexplora/bioexplora.rete", "parquetDir": "bioexplora/bioexplora-tables", "duckdb": "bioexplora/bioexplora.duckdb", "duckdbSize": "142.8 MB", "flat": true, "about": "The bioexplora graph as a flat <code>triples</code> table (subject, predicate, object token + decoded value/datatype/lang) — the same 7,892,374 facts in Parquet and DuckDB, queryable in SQL to compare against the rete engine.", "tables": [{"name": "triples", "file": "triples.parquet", "label": "triples — every fact (S, P, O)", "entities": 7892374}], "sqlCols": ["subject", "predicate", "object", "otype", "value", "datatype", "lang"], "examples": [{"label": "Predicate totals", "table": {"file": "triples.parquet", "name": "triples"}, "sparql": "SELECT ?p (COUNT(*) AS ?n) WHERE { ?s ?p ?o } GROUP BY ?p ORDER BY DESC(?n)", "duck": "SELECT predicate, COUNT(*) AS n\nFROM {T}\nGROUP BY predicate\nORDER BY n DESC;", "sqlite": "SELECT predicate, COUNT(*) AS n\nFROM \"triples\"\nGROUP BY predicate\nORDER BY n DESC;", "note": "How often each predicate is used — the shape of the graph, as a GROUP BY over the flat triples table."}, {"label": "Busiest subjects", "table": {"file": "triples.parquet", "name": "triples"}, "sparql": "SELECT ?s (COUNT(*) AS ?n) WHERE { ?s ?p ?o } GROUP BY ?s ORDER BY DESC(?n) LIMIT 50", "duck": "SELECT subject, COUNT(*) AS n\nFROM {T}\nGROUP BY subject\nORDER BY n DESC\nLIMIT 50;", "sqlite": "SELECT subject, COUNT(*) AS n\nFROM \"triples\"\nGROUP BY subject\nORDER BY n DESC\nLIMIT 50;", "note": "The 50 subjects with the most outgoing facts — the hubs of the graph."}]},
    "bph": {"rete": "bph/bph.rete", "parquetDir": "bph/bph-tables", "duckdb": "bph/bph.duckdb", "duckdbSize": "310.8 MB", "flat": true, "about": "The bph graph as a flat <code>triples</code> table (subject, predicate, object token + decoded value/datatype/lang) — the same 5,741,344 facts in Parquet and DuckDB, queryable in SQL to compare against the rete engine.", "tables": [{"name": "triples", "file": "triples.parquet", "label": "triples — every fact (S, P, O)", "entities": 5741344}], "sqlCols": ["subject", "predicate", "object", "otype", "value", "datatype", "lang"], "examples": [{"label": "Predicate totals", "table": {"file": "triples.parquet", "name": "triples"}, "sparql": "SELECT ?p (COUNT(*) AS ?n) WHERE { ?s ?p ?o } GROUP BY ?p ORDER BY DESC(?n)", "duck": "SELECT predicate, COUNT(*) AS n\nFROM {T}\nGROUP BY predicate\nORDER BY n DESC;", "sqlite": "SELECT predicate, COUNT(*) AS n\nFROM \"triples\"\nGROUP BY predicate\nORDER BY n DESC;", "note": "How often each predicate is used — the shape of the graph, as a GROUP BY over the flat triples table."}, {"label": "Busiest subjects", "table": {"file": "triples.parquet", "name": "triples"}, "sparql": "SELECT ?s (COUNT(*) AS ?n) WHERE { ?s ?p ?o } GROUP BY ?s ORDER BY DESC(?n) LIMIT 50", "duck": "SELECT subject, COUNT(*) AS n\nFROM {T}\nGROUP BY subject\nORDER BY n DESC\nLIMIT 50;", "sqlite": "SELECT subject, COUNT(*) AS n\nFROM \"triples\"\nGROUP BY subject\nORDER BY n DESC\nLIMIT 50;", "note": "The 50 subjects with the most outgoing facts — the hubs of the graph."}]},
    "ohm-full": {"rete": "ohm-full/ohm-full.rete", "parquetDir": "ohm-full/ohm-full-tables", "duckdb": "ohm-full/ohm-full.duckdb", "duckdbSize": "1053.8 MB", "flat": true, "about": "The ohm-full graph as a flat <code>triples</code> table (subject, predicate, object token + decoded value/datatype/lang) — the same 6,127,745 facts in Parquet and DuckDB, queryable in SQL to compare against the rete engine.", "tables": [{"name": "triples", "file": "triples.parquet", "label": "triples — every fact (S, P, O)", "entities": 6127745}], "sqlCols": ["subject", "predicate", "object", "otype", "value", "datatype", "lang"], "examples": [{"label": "Predicate totals", "table": {"file": "triples.parquet", "name": "triples"}, "sparql": "SELECT ?p (COUNT(*) AS ?n) WHERE { ?s ?p ?o } GROUP BY ?p ORDER BY DESC(?n)", "duck": "SELECT predicate, COUNT(*) AS n\nFROM {T}\nGROUP BY predicate\nORDER BY n DESC;", "sqlite": "SELECT predicate, COUNT(*) AS n\nFROM \"triples\"\nGROUP BY predicate\nORDER BY n DESC;", "note": "How often each predicate is used — the shape of the graph, as a GROUP BY over the flat triples table."}, {"label": "Busiest subjects", "table": {"file": "triples.parquet", "name": "triples"}, "sparql": "SELECT ?s (COUNT(*) AS ?n) WHERE { ?s ?p ?o } GROUP BY ?s ORDER BY DESC(?n) LIMIT 50", "duck": "SELECT subject, COUNT(*) AS n\nFROM {T}\nGROUP BY subject\nORDER BY n DESC\nLIMIT 50;", "sqlite": "SELECT subject, COUNT(*) AS n\nFROM \"triples\"\nGROUP BY subject\nORDER BY n DESC\nLIMIT 50;", "note": "The 50 subjects with the most outgoing facts — the hubs of the graph."}]},
  },

  // Per-dataset visual identity + descriptive tags for the dataset browser.
  // Semantic-search (RAG) indexes: precomputed doc embeddings (a Float32 matrix,
  // one row per entity) on the bucket + the Transformers.js model that embeds the
  // query. The "Semantic" tab loads these, embeds the query in-browser (WebGPU),
  // and cosine-ranks entirely client-side — no server.
  // Semantic-search (RAG) indexes — precomputed doc embeddings on R2/graphplaza
  // (token-free) + the model that embeds the query. Generated by scripts/rag_*.
  // Semantic-search (RAG) indexes — precomputed doc embeddings on R2/graphplaza
  // (token-free) + the model that embeds the query. Generated by scripts/rag_*.
  rag: {
    "albala": { emb: "https://data.graphplaza.com/albala-rag/albala_emb.f32", index: "https://data.graphplaza.com/albala-rag/albala_emb_index.json", model: "Xenova/multilingual-e5-small", queryPrefix: "query: ", dim: 384, count: 8000 },
    "antarctic-expeditions": { emb: "https://data.graphplaza.com/antarctic-expeditions-rag/antarctic-expeditions_emb.f32", index: "https://data.graphplaza.com/antarctic-expeditions-rag/antarctic-expeditions_emb_index.json", model: "Xenova/multilingual-e5-small", queryPrefix: "query: ", dim: 384, count: 87 },
    "biblissima": { emb: "https://data.graphplaza.com/biblissima-rag/biblissima_emb.f32", index: "https://data.graphplaza.com/biblissima-rag/biblissima_emb_index.json", model: "Xenova/multilingual-e5-small", queryPrefix: "query: ", dim: 384, count: 5269 },
    "bioexplora": { emb: "https://data.graphplaza.com/bioexplora-rag/bioexplora_emb.f32", index: "https://data.graphplaza.com/bioexplora-rag/bioexplora_emb_index.json", model: "Xenova/multilingual-e5-small", queryPrefix: "query: ", dim: 384, count: 8000 },
    "bne": { emb: "https://data.graphplaza.com/bne-rag/bne_emb.f32", index: "https://data.graphplaza.com/bne-rag/bne_emb_index.json", model: "Xenova/multilingual-e5-small", queryPrefix: "query: ", dim: 384, count: 8000 },
    "boe": { emb: "https://data.graphplaza.com/boe-rag/boe_emb.f32", index: "https://data.graphplaza.com/boe-rag/boe_emb_index.json", model: "Xenova/multilingual-e5-small", queryPrefix: "query: ", dim: 384, count: 12330,
      examples: ["protección de datos y privacidad", "violencia de género y contra la mujer", "cambio climático y transición energética", "inteligencia artificial", "vivienda y alquiler", "energías renovables"],
      enrich: {
        cols: { titulo: "Norma", rango: "Rango", fecha: "Fecha", organo: "Órgano / Departamento" },
        q: "# The norms your semantic search just found — now enriched with structured\n# fields (rango, fecha, órgano) straight from the graph. Edit freely: add e.g.\n# ?s eli:repeals ?x . ?x eli:title ?derogada  to see what each one repeals.\nPREFIX eli: <http://data.europa.eu/eli/ontology#>\nPREFIX rdfs: <http://www.w3.org/2000/01/rdf-schema#>\nPREFIX skos: <http://www.w3.org/2004/02/skos/core#>\nSELECT ?titulo ?rango ?fecha ?organo WHERE {\n  VALUES ?s { %IRIS% }\n  ?s eli:title ?titulo .\n  OPTIONAL { ?s eli:type_document ?rc . ?rc skos:prefLabel ?rango }\n  OPTIONAL { ?s eli:date_document ?fecha }\n  OPTIONAL { ?s eli:passed_by ?o . ?o rdfs:label ?organo }\n}\nORDER BY DESC(?fecha)"
      } },
    "bph": { emb: "https://data.graphplaza.com/bph-rag/bph_emb.f32", index: "https://data.graphplaza.com/bph-rag/bph_emb_index.json", model: "Xenova/multilingual-e5-small", queryPrefix: "query: ", dim: 384, count: 2307 },
    "causal": { emb: "https://data.graphplaza.com/causal-rag/causal_emb.f32", index: "https://data.graphplaza.com/causal-rag/causal_emb_index.json", model: "Xenova/multilingual-e5-small", queryPrefix: "query: ", dim: 384, count: 30 },
    "causalgraph": { emb: "https://data.graphplaza.com/causalgraph-rag/causalgraph_emb.f32", index: "https://data.graphplaza.com/causalgraph-rag/causalgraph_emb_index.json", model: "Xenova/multilingual-e5-small", queryPrefix: "query: ", dim: 384, count: 37 },
    "causenet": { emb: "https://data.graphplaza.com/causenet-full-rag/causenet-full_emb.f32", index: "https://data.graphplaza.com/causenet-full-rag/causenet-full_emb_index.json", model: "Xenova/multilingual-e5-small", queryPrefix: "query: ", dim: 384, count: 8000 },
    "chebi-full": { emb: "https://data.graphplaza.com/chebi-full-rag/chebi-full_emb.f32", index: "https://data.graphplaza.com/chebi-full-rag/chebi-full_emb_index.json", model: "Xenova/multilingual-e5-small", queryPrefix: "query: ", dim: 384, count: 8000 },
    "chemotion": { emb: "https://data.graphplaza.com/chemotion-rag/chemotion_emb.f32", index: "https://data.graphplaza.com/chemotion-rag/chemotion_emb_index.json", model: "Xenova/multilingual-e5-small", queryPrefix: "query: ", dim: 384, count: 8000 },
    "databnf": { emb: "https://data.graphplaza.com/databnf-rag/databnf_emb.f32", index: "https://data.graphplaza.com/databnf-rag/databnf_emb_index.json", model: "Xenova/multilingual-e5-small", queryPrefix: "query: ", dim: 384, count: 8000 },
    "factgrid-illuminati": { emb: "https://data.graphplaza.com/factgrid-illuminati-rag/factgrid-illuminati_emb.f32", index: "https://data.graphplaza.com/factgrid-illuminati-rag/factgrid-illuminati_emb_index.json", model: "Xenova/multilingual-e5-small", queryPrefix: "query: ", dim: 384, count: 7020 },
    "fuero_juzgo": { emb: "https://data.graphplaza.com/fuero_juzgo-rag/fuero_juzgo_emb.f32", index: "https://data.graphplaza.com/fuero_juzgo-rag/fuero_juzgo_emb_index.json", model: "Xenova/multilingual-e5-small", queryPrefix: "query: ", dim: 384, count: 156 },
    "geoadmin": { emb: "https://data.graphplaza.com/geoadmin-rag/geoadmin_emb.f32", index: "https://data.graphplaza.com/geoadmin-rag/geoadmin_emb_index.json", model: "Xenova/multilingual-e5-small", queryPrefix: "query: ", dim: 384, count: 8000 },
    "getty-ulan": { emb: "https://data.graphplaza.com/getty-ulan-rag/getty-ulan_emb.f32", index: "https://data.graphplaza.com/getty-ulan-rag/getty-ulan_emb_index.json", model: "Xenova/multilingual-e5-small", queryPrefix: "query: ", dim: 384, count: 8000 },
    "history": { emb: "https://data.graphplaza.com/history-rag/history_emb.f32", index: "https://data.graphplaza.com/history-rag/history_emb_index.json", model: "Xenova/multilingual-e5-small", queryPrefix: "query: ", dim: 384, count: 2059 },
    "jonas": { emb: "https://data.graphplaza.com/jonas-rag/jonas_emb.f32", index: "https://data.graphplaza.com/jonas-rag/jonas_emb_index.json", model: "Xenova/multilingual-e5-small", queryPrefix: "query: ", dim: 384, count: 8000 },
    "lineara": { emb: "https://data.graphplaza.com/lineara-rag/lineara_emb.f32", index: "https://data.graphplaza.com/lineara-rag/lineara_emb_index.json", model: "Xenova/multilingual-e5-small", queryPrefix: "query: ", dim: 384, count: 3594 },
    "linked-jazz": { emb: "https://data.graphplaza.com/linked-jazz-rag/linked-jazz_emb.f32", index: "https://data.graphplaza.com/linked-jazz-rag/linked-jazz_emb_index.json", model: "Xenova/multilingual-e5-small", queryPrefix: "query: ", dim: 384, count: 2005 },
    "memoria": { emb: "https://data.graphplaza.com/memoria-rag/memoria_emb.f32", index: "https://data.graphplaza.com/memoria-rag/memoria_emb_index.json", model: "Xenova/multilingual-e5-small", queryPrefix: "query: ", dim: 384, count: 8000 },
    "mimotext": { emb: "https://data.graphplaza.com/mimotext-rag/mimotext_emb.f32", index: "https://data.graphplaza.com/mimotext-rag/mimotext_emb_index.json", model: "Xenova/multilingual-e5-small", queryPrefix: "query: ", dim: 384, count: 1478 },
    "mira": { emb: "https://data.graphplaza.com/mira-rag/mira_emb.f32", index: "https://data.graphplaza.com/mira-rag/mira_emb_index.json", model: "Xenova/multilingual-e5-small", queryPrefix: "query: ", dim: 384, count: 480 },
    "mmm": { emb: "https://data.graphplaza.com/mmm-rag/mmm_emb.f32", index: "https://data.graphplaza.com/mmm-rag/mmm_emb_index.json", model: "Xenova/multilingual-e5-small", queryPrefix: "query: ", dim: 384, count: 8000 },
    "monarch": { emb: "https://data.graphplaza.com/monarch-rag/monarch_emb.f32", index: "https://data.graphplaza.com/monarch-rag/monarch_emb_index.json", model: "Xenova/multilingual-e5-small", queryPrefix: "query: ", dim: 384, count: 1069 },
    "nomisma": { emb: "https://data.graphplaza.com/nomisma-rag/nomisma_emb.f32", index: "https://data.graphplaza.com/nomisma-rag/nomisma_emb_index.json", model: "Xenova/multilingual-e5-small", queryPrefix: "query: ", dim: 384, count: 7369 },
    "ohm-full": { emb: "https://data.graphplaza.com/ohm-full-rag/ohm-full_emb.f32", index: "https://data.graphplaza.com/ohm-full-rag/ohm-full_emb_index.json", model: "Xenova/multilingual-e5-small", queryPrefix: "query: ", dim: 384, count: 8000 },
    "ontoneurolog": { emb: "https://data.graphplaza.com/ontoneurolog-rag/ontoneurolog_emb.f32", index: "https://data.graphplaza.com/ontoneurolog-rag/ontoneurolog_emb_index.json", model: "Xenova/multilingual-e5-small", queryPrefix: "query: ", dim: 384, count: 710 },
    "openalex-astrocytes": { emb: "https://data.graphplaza.com/openalex-astrocytes-rag/openalex-astrocytes_emb.f32", index: "https://data.graphplaza.com/openalex-astrocytes-rag/openalex-astrocytes_emb_index.json", model: "Xenova/multilingual-e5-small", queryPrefix: "query: ", dim: 384, count: 3986 },
    "opencitations": { emb: "https://data.graphplaza.com/opencitations-rag/opencitations_emb.f32", index: "https://data.graphplaza.com/opencitations-rag/opencitations_emb_index.json", model: "Xenova/multilingual-e5-small", queryPrefix: "query: ", dim: 384, count: 3315 },
    "orkg": { emb: "https://data.graphplaza.com/orkg-rag/orkg_emb.f32", index: "https://data.graphplaza.com/orkg-rag/orkg_emb_index.json", model: "Xenova/multilingual-e5-small", queryPrefix: "query: ", dim: 384, count: 8000 },
    "peirce": { emb: "https://data.graphplaza.com/peirce-rag/peirce_emb.f32", index: "https://data.graphplaza.com/peirce-rag/peirce_emb_index.json", model: "Xenova/multilingual-e5-small", queryPrefix: "query: ", dim: 384, count: 3384 },
    "postscriptum": { emb: "https://data.graphplaza.com/postscriptum-rag/postscriptum_emb.f32", index: "https://data.graphplaza.com/postscriptum-rag/postscriptum_emb_index.json", model: "Xenova/multilingual-e5-small", queryPrefix: "query: ", dim: 384, count: 6914 },
    "ramon_llull": { emb: "https://data.graphplaza.com/ramon_llull-rag/ramon_llull_emb.f32", index: "https://data.graphplaza.com/ramon_llull-rag/ramon_llull_emb_index.json", model: "Xenova/multilingual-e5-small", queryPrefix: "query: ", dim: 384, count: 142 },
    "scholar-noisy": { emb: "https://data.graphplaza.com/scholar-noisy-rag/scholar-noisy_emb.f32", index: "https://data.graphplaza.com/scholar-noisy-rag/scholar-noisy_emb_index.json", model: "Xenova/multilingual-e5-small", queryPrefix: "query: ", dim: 384, count: 387 },
    "scholar": { emb: "https://data.graphplaza.com/scholar-rag/scholar_emb.f32", index: "https://data.graphplaza.com/scholar-rag/scholar_emb_index.json", model: "Xenova/multilingual-e5-small", queryPrefix: "query: ", dim: 384, count: 387 },
    "smithsonian3d": { emb: "https://data.graphplaza.com/smithsonian3d-rag/smithsonian3d_emb.f32", index: "https://data.graphplaza.com/smithsonian3d-rag/smithsonian3d_emb_index.json", model: "Xenova/multilingual-e5-small", queryPrefix: "query: ", dim: 384, count: 2211 },
    "theographic-graph": { emb: "https://data.graphplaza.com/theographic-graph-rag/theographic-graph_emb.f32", index: "https://data.graphplaza.com/theographic-graph-rag/theographic-graph_emb_index.json", model: "Xenova/multilingual-e5-small", queryPrefix: "query: ", dim: 384, count: 4814 },
    "ustc": { emb: "https://data.graphplaza.com/ustc-rag/ustc_emb.f32", index: "https://data.graphplaza.com/ustc-rag/ustc_emb_index.json", model: "Xenova/multilingual-e5-small", queryPrefix: "query: ", dim: 384, count: 727 },
    "wikidata-100mb": { emb: "https://data.graphplaza.com/wikidata-100mb-rag/wikidata-100mb_emb.f32", index: "https://data.graphplaza.com/wikidata-100mb-rag/wikidata-100mb_emb_index.json", model: "Xenova/multilingual-e5-small", queryPrefix: "query: ", dim: 384, count: 786 },
    "wikidata-xxl": { emb: "https://data.graphplaza.com/wikidata-xxl-rag/wikidata-xxl_emb.f32", index: "https://data.graphplaza.com/wikidata-xxl-rag/wikidata-xxl_emb_index.json", model: "Xenova/multilingual-e5-small", queryPrefix: "query: ", dim: 384, count: 594 },
    "wikidata-zenodo": { emb: "https://data.graphplaza.com/wikidata-zenodo-rag/wikidata-zenodo_emb.f32", index: "https://data.graphplaza.com/wikidata-zenodo-rag/wikidata-zenodo_emb_index.json", model: "Xenova/multilingual-e5-small", queryPrefix: "query: ", dim: 384, count: 597 },
    "wikidata": { emb: "https://data.graphplaza.com/wikidata-rag/wikidata_emb.f32", index: "https://data.graphplaza.com/wikidata-rag/wikidata_emb_index.json", model: "Xenova/multilingual-e5-small", queryPrefix: "query: ", dim: 384, count: 597 },
  },

  // `icon` is the tile glyph; `tags` are quick descriptive labels; `reasoning`
  // marks graphs where the Coherence/OWL reasoner is illustrative.
  datasetExtra: {
    "tracking":              { icon: "🎥", tags: ["football","tracking","spatiotemporal","movement","Metrica","x/y","replay"] },
    "subtitles":             { icon: "💬", tags: ["subtitles","multilingual","20 languages","temporal","translation","film","Tears of Steel","CC-BY","RTL","CJK"] },
    "mtg":                   { icon: "🃏", tags: ["Magic: The Gathering","MTG","cards","games","MTGJSON","Scryfall","OWL types","text search","card images","34k cards"], reasoning: true },
    "mtg-game":              { icon: "🎴", tags: ["Magic: The Gathering","game state","OWL","runtime ontology","abilities","stack","SPARQL","no engine","cmdoret"], reasoning: true },
    "arxiu":                 { icon: "🗃️", tags: ["archives","Arxiu Nacional","Catalonia","Generalitat","manuscripts","scanned documents","WebP","schema.org","open data","complete archive"] },
    "worldcup2026":          { icon: "🔴", tags: ["football","soccer","World Cup 2026","live","USA","Canada","Mexico","48 teams","OpenFootball","GeoSPARQL"] },
    "worldcup":              { icon: "⚽", tags: ["football","soccer","World Cup","Qatar 2022","predictions","FiveThirtyEight","provenance","GeoSPARQL","schema.org"] },
    "bph":                   { icon: "📜", tags: ["hermetica", "alchemy", "esoterica", "rare-books", "Amsterdam", "multilingual", "IIIF"] },
    "ustc":                  { icon: "📖", tags: ["bibliography", "early-modern-print", "rare-books", "holdings", "federation", "St-Andrews"] },
    "fuero_juzgo":           { icon: "📜", tags: ["Fuero-Juzgo", "Liber-Iudiciorum", "manuscripts", "medieval", "legal-history", "philology", "leonés", "PhiloBiblon", "IIIF", "Wikidata", "Biblissima"] },
    "ramon_llull":           { icon: "🗝️", tags: ["Ramón-Llull", "manuscripts", "rare-books", "medieval", "Mallorca", "Latin-Catalan", "BVPB", "OCR", "full-text", "IIIF"] },
    "peirce":                { icon: "🖋️", tags: ["Peirce", "philosophy", "logic", "pragmatism", "manuscripts", "correspondence", "Houghton-Harvard", "IIIF"] },
    "boe":                   { icon: "⚖️", reasoning: true, tags: ["Spain", "law", "legislation", "ELI", "BOE", "consolidated", "citation-network", "derogation", "OWL", "reasoning", "SKOS", "open-data", "government"] },
    "scrolls":               { icon: "📜", tags: ["Vesuvius-Challenge", "Herculaneum", "papyri", "X-ray-CT", "virtual-unwrapping", "ink-detection", "machine-learning", "provenance", "OME-Zarr", "downloads"] },
    "scholar":               { icon: "🎓", tags: ["synthetic", "scholarly", "citations"] },
    "scholar-noisy":         { icon: "🎓", tags: ["synthetic", "data-quality", "noisy"] },
    "causal":                { icon: "🫀", tags: ["causal", "ontology", "OWL"], reasoning: true },
    "history":               { icon: "🗺️", tags: ["geospatial", "borders", "temporal"] },
    "linked-jazz":           { icon: "🎷", tags: ["social-network", "music", "DBpedia"] },
    "getty-ulan":            { icon: "🎨", tags: ["art", "lineage", "Getty"] },
    "bioexplora":            { icon: "🦴", tags: ["natural-history", "biodiversity", "Darwin Core", "GBIF", "Barcelona", "IIIF", "3D", "audio"] },
    "geoadmin":              { icon: "🗺", tags: ["geo", "GeoSPARQL", "boundaries", "Natural Earth", "federation", "CC0"] },
    "geoadmin-tiles":        { icon: "🧱", tags: ["geo", "GeoSPARQL", "PMTiles", "tiles-in-rete", "vector tiles", "option-C"] },
    "smithsonian3d":         { icon: "🗿", tags: ["3D", "museum", "Smithsonian", "CC0", "natural-history"] },
    "lineara":               { icon: "🏺", tags: ["Linear A", "Minoan", "epigraphy", "undeciphered", "Aegean"] },
    "nomisma":               { icon: "🪙", tags: ["numismatics", "ancient", "Alexander"] },
    "mira":                  { icon: "☘️", tags: ["manuscripts", "medieval", "Irish", "IIIF", "Wikidata"] },
    "mira-wikidata":         { icon: "🔗", tags: ["mappings", "SSSOM", "linkset", "Wikidata", "federation"] },
    "causalgraph":           { icon: "⛓️", tags: ["causal", "ontology", "OWL", "Industry-4.0", "provenance"] },
    "ontoneurolog":          { icon: "🧠", tags: ["neuroimaging", "ontology", "OWL", "MRI", "DOLCE", "provenance"] },
    "nidm":                  { icon: "🧬", tags: ["neuroimaging", "PROV", "provenance", "cohort", "OpenNeuro", "federation"] },
    "mimotext":              { icon: "📖", tags: ["literature", "Enlightenment", "stylometry"] },
    "mmm":                   { icon: "📜", tags: ["manuscripts", "provenance", "CIDOC-CRM"] },
    "openalex-astrocytes":   { icon: "🧠", tags: ["OpenAlex", "neuroscience", "citations"] },
    "antarctic-expeditions": { icon: "🧭", tags: ["history", "exploration", "Wikidata"] },
    "factgrid-illuminati":   { icon: "🕯️", tags: ["prosopography", "history", "FactGrid"] },
    "theographic-graph":     { icon: "✝️", tags: ["bible", "genealogy", "narrative"] },
    "monarch":               { icon: "🧬", tags: ["biomedical", "genes", "Biolink"] },
    "opencitations":         { icon: "🔗", tags: ["OpenCitations", "citations"] },
    "orkg":                  { icon: "🔬", tags: ["research", "contributions", "scholarly"] },
    "ohm-full":              { icon: "🗺️", tags: ["geospatial", "OpenHistoricalMap", "CC0"] },
    "wikidata":              { icon: "🌐", tags: ["Wikidata", "people", "places"] },
    "wikidata-zenodo":       { icon: "🏛️", tags: ["Wikidata", "Zenodo", "DOI", "citable-data", "HTTP-range", "people", "places"] },
    "wikidata-100mb":        { icon: "🌐", tags: ["Wikidata", "people", "occupations"] },
    "wikidata-xxl":          { icon: "⛓", tags: ["Wikidata", "sharded", "federation", "XXL", "600M", "CC0"] },
    "chemotion":             { icon: "🧪", tags: ["chemistry", "ELN", "ontology", "ChEBI", "CHMO"] },
    "chebi-full":            { icon: "⚗️", tags: ["chemistry", "ontology", "ChEBI", "OBO", "federation"] },
    "causenet":         { icon: "⛓️", tags: ["causality", "provenance", "web-extraction", "NLP", "reach"] },
    "jonas":                 { icon: "📜", tags: ["medieval", "manuscripts", "philology", "witnesses", "stemmata"] },
    "postscriptum":          { icon: "✉️", tags: ["letters", "correspondence", "Portuguese", "Spanish", "TEI"] },
    "databnf":               { icon: "🇫🇷", tags: ["BnF", "authorities", "VIAF", "RAMEAU", "single-file"] },
    "bne":                   { icon: "🇪🇸", tags: ["BNE", "Spain", "authorities", "VIAF", "books"] },
    "biblissima":            { icon: "🗝️", tags: ["Biblissima", "manuscripts", "Wikibase", "heritage", "single-file"] },
    "albala":                { icon: "⛪", tags: ["Seville", "archives", "ISAD", "Colombina", "Spain"] },
    "memoria":               { icon: "🕯️", tags: ["Spain", "Civil War", "memoria histórica", "victims", "open data"] },
    "gbif-birds":            { icon: "🐦", tags: ["GBIF", "birds", "Aves", "biodiversity", "occurrences", "taxonomy", "semantic-zoom", "GeoSPARQL", "Spain", "Switzerland", "citizen-science", "RDF-star", "multilingual", "IUCN", "conservation", "provenance"] },
    "bcul":                  { icon: "🏛️", tags: ["library", "catalogue", "MARC", "holdings", "IIIF", "manuscripts", "Switzerland", "Lausanne", "schema.org"] },
    "ecal":                  { icon: "🎨", tags: ["art", "design", "photography", "cinema", "ECAL", "Lausanne", "covers", "schema.org"] },
  },
  examples: {
    "tracking": [
      {"family": "Summary", "label": "What's in this tracking graph", "view": "table", "cols": {"metric": "Metric", "n": "Value"}, "tip": "The shape of a spatiotemporal match: 27 tracked objects (players + ball), ~28k time frames, ~637k position rows.\nEach position = tr:object + tr:t (s) + tr:x + tr:y on a 105x68 m pitch.", "q": "PREFIX tr: <https://w3id.org/rete/tracking#>\nSELECT (\"positions\" AS ?metric) (COUNT(?p) AS ?n) WHERE { ?p tr:t ?t }"},
      {"family": "Select", "label": "📸 Everyone's position at one instant", "view": "table", "cols": {"obj": "Object", "x": "x (m)", "y": "y (m)"}, "tip": "A single animation frame: every player and the ball at t≈300 s (5:00). A 2D replay is just this query, stepped through time and drawn on a pitch.\nWiden the window or change the time to scrub.", "q": "PREFIX tr: <https://w3id.org/rete/tracking#>\nPREFIX rdfs: <http://www.w3.org/2000/01/rdf-schema#>\nSELECT ?obj ?x ?y WHERE {\n  ?p tr:t ?t ; tr:object ?o ; tr:x ?x ; tr:y ?y .\n  ?o rdfs:label ?obj .\n  FILTER(?t >= 300 && ?t < 300.2)\n} ORDER BY ?obj"},
      {"family": "Select", "label": "🏃 One player's path over 30 seconds", "view": "table", "cols": {"t": "t (s)", "x": "x (m)", "y": "y (m)"}, "tip": "The trajectory of Home #10 across half a minute — the raw material of a movement trail or heatmap. Bounded by object + time window, so it's a cheap lazy read.\nSwap the object IRI (…/obj/home_10) for any player or …/obj/ball.", "q": "PREFIX tr: <https://w3id.org/rete/tracking#>\nSELECT ?t ?x ?y WHERE {\n  ?p tr:object <https://w3id.org/rete/tracking/obj/home_10> ;\n     tr:t ?t ; tr:x ?x ; tr:y ?y .\n  FILTER(?t >= 600 && ?t < 630)\n} ORDER BY ?t"}
    ],
    "subtitles": [
      {"family": "Summary", "label": "The 20 language tracks", "view": "table", "cols": {"language": "Language", "cues": "Cues"}, "tip": "What's in the graph: 20 subtitle tracks for one film, each a full language version. English (en) is the pivot with 76 cues; other tongues merge or split lines slightly, so counts differ.\nEach track is a su:SubtitleTrack of Tears of Steel (Blender, CC BY 3.0).", "q": "PREFIX su: <https://w3id.org/rete/subtitles#>\nPREFIX rdfs: <http://www.w3.org/2000/01/rdf-schema#>\nSELECT ?language ?cues WHERE {\n  ?t a su:SubtitleTrack ; rdfs:label ?language ; su:cueCount ?cues .\n} ORDER BY DESC(?cues) ?language"},
      {"family": "Multilingual", "label": "🌍 One line, every language", "view": "table", "cols": {"language": "Lang", "text": "Subtitle"}, "tip": "The film's first spoken line — “You're a jerk, Thom.” at 0:23 — in every language at once, including right-to-left Persian & Hebrew and CJK Japanese/Chinese.\nA su:Line is one MOMENT; each language hangs off it as a su:text \"…\"@lang literal.", "q": "PREFIX su: <https://w3id.org/rete/subtitles#>\nSELECT ?language ?text WHERE {\n  ?line a su:Line ; su:start ?s ; su:text ?text .\n  FILTER(?s >= 23 && ?s < 24)\n  BIND(LANG(?text) AS ?language)\n} ORDER BY ?language"},
      {"family": "Over time", "label": "🎬 The whole film in one language", "view": "table", "cols": {"start": "Start (s)", "subtitle": "Japanese"}, "tip": "Play one language's subtitle track in order — here Japanese (ja), each su:Cue with its own start time. This is the film as a timeline of text.\nChange \"ja\" to es, fr, ru, fa, zh-Hant … to switch language.", "q": "PREFIX su: <https://w3id.org/rete/subtitles#>\nPREFIX dct: <http://purl.org/dc/terms/>\nSELECT ?start ?subtitle WHERE {\n  ?c a su:Cue ; dct:language \"ja\" ; su:start ?start ; su:text ?subtitle .\n} ORDER BY ?start"},
      {"family": "Over time", "label": "⏱ What's on screen at 1:05", "view": "table", "cols": {"language": "Lang", "text": "Subtitle"}, "tip": "Freeze the film at t = 65 s (1:05) and read the line whose [su:start, su:end] interval covers that instant — in six languages side by side.\nIt's a time-window query: change 65 to any second to scrub the whole film.", "q": "PREFIX su: <https://w3id.org/rete/subtitles#>\nSELECT ?language ?text WHERE {\n  ?line a su:Line ; su:start ?s ; su:end ?e ; su:text ?text .\n  FILTER(?s <= 65 && ?e >= 65)\n  BIND(LANG(?text) AS ?language)\n  FILTER(?language IN (\"en\",\"es\",\"ja\",\"ru\",\"fa\",\"zh-Hans\"))\n} ORDER BY ?language"},
      {"family": "Search", "label": "🔤 Find a word in the dialogue", "view": "table", "cols": {"start": "Start (s)", "language": "Lang", "text": "Subtitle"}, "tip": "Every subtitle mentioning “robot”, across all 20 languages, and exactly when it's said — full-text over the dialogue (a TEXT_INDEX is baked into the .rete).\nSwap 'robot' for any word, or add a dct:language filter to search one language.", "q": "PREFIX su: <https://w3id.org/rete/subtitles#>\nSELECT ?start ?language ?text WHERE {\n  ?c a su:Cue ; su:start ?start ; su:text ?text .\n  FILTER(CONTAINS(LCASE(STR(?text)), \"robot\"))\n  BIND(LANG(?text) AS ?language)\n} ORDER BY ?start ?language"},
      {"family": "Multilingual", "label": "🔀 How one line was translated", "view": "table", "cols": {"time": "t (s)", "english": "English", "translations": "es · fr · de · ru · ja"}, "tip": "Line up the opening lines of dialogue with their Spanish, French, German, Russian and Japanese translations side by side — the same utterance, five ways.\nThe English su:Line is the hub; each language is a su:text literal hanging off it.", "q": "PREFIX su: <https://w3id.org/rete/subtitles#>\nPREFIX rdfs: <http://www.w3.org/2000/01/rdf-schema#>\nSELECT ?time ?english (GROUP_CONCAT(CONCAT(?lg,\"=\",?tx); separator=\"   ·   \") AS ?translations) WHERE {\n  ?line a su:Line ; su:start ?time ; rdfs:label ?english ; su:text ?tx .\n  BIND(LANG(?tx) AS ?lg)\n  FILTER(?lg IN (\"es\",\"fr\",\"de\",\"ru\",\"ja\"))\n} GROUP BY ?line ?time ?english ORDER BY ?time LIMIT 12"}
    ],
    "mtg": [
      {"family": "Summary", "label": "How many cards of each type", "view": "table", "cols": {"type": "Card type", "cards": "Cards"}, "tip": "34,633 cards grouped by card type (Creature, Instant, Land, Artifact …). Each type is an OWL class (rdfs:subClassOf mtg:Card) — open the Schema tab to see the same breakdown as a type zoom.\nCards can have several types, so an Artifact Creature counts under both.", "q": "PREFIX mtg: <https://w3id.org/rete/mtg#>\nPREFIX rdfs: <http://www.w3.org/2000/01/rdf-schema#>\nSELECT ?type (COUNT(?c) AS ?cards) WHERE {\n  ?c a mtg:Card ; a ?t .\n  ?t rdfs:subClassOf mtg:Card ; rdfs:label ?type .\n} GROUP BY ?type ORDER BY DESC(?cards)"},
      {"family": "Deckbuilding", "label": "🔥 Red fliers legal in Modern", "view": "table", "cols": {"image": "Card", "card": "Name", "rank": "EDHREC"}, "tip": "A real deckbuilding query: red Creatures with the Flying keyword that are legal in the Modern format, most-played first (mtg:edhrecRank). The pictures come straight from the graph — mtg:image resolves to Scryfall's card art.\nSwap color/R for /U, flying for haste, or modern for legacy / commander.", "q": "PREFIX mtg: <https://w3id.org/rete/mtg#>\nPREFIX schema: <http://schema.org/>\nSELECT ?image ?card ?rank WHERE {\n  ?c a mtg:Creature ; schema:name ?card ; mtg:image ?image ; mtg:edhrecRank ?rank ;\n     mtg:color <https://w3id.org/rete/mtg/color/R> ;\n     mtg:keyword <https://w3id.org/rete/mtg/keyword/flying> ;\n     mtg:legalIn <https://w3id.org/rete/mtg/format/modern> .\n} ORDER BY ?rank LIMIT 24"},
      {"family": "Search", "label": "🔎 Cards that draw two cards", "view": "table", "cols": {"image": "Card", "card": "Name", "text": "Oracle text"}, "tip": "Full-text over Oracle text (a TEXT_INDEX is baked into the .rete): every card whose rules text says “draw two cards”. This is finding cards by what they DO.\nChange the phrase — 'destroy target creature', 'create a Treasure token', 'gain 3 life' — to search by effect.", "q": "PREFIX mtg: <https://w3id.org/rete/mtg#>\nPREFIX schema: <http://schema.org/>\nSELECT ?image ?card ?text WHERE {\n  ?c schema:name ?card ; mtg:image ?image ; mtg:oracleText ?text .\n  FILTER(CONTAINS(LCASE(?text), \"draw two cards\"))\n} LIMIT 24"},
      {"family": "Explore", "label": "🐉 The most popular Dragons", "view": "table", "cols": {"image": "Card", "card": "Name", "rank": "EDHREC"}, "tip": "Every card with the Dragon creature type, ranked by EDHREC popularity — a picture gallery of the game's dragons.\nSwap subtype/dragon for /goblin, /angel, /zombie, /elf … (there are 613 creature types in the graph).", "q": "PREFIX mtg: <https://w3id.org/rete/mtg#>\nPREFIX schema: <http://schema.org/>\nSELECT ?image ?card ?rank WHERE {\n  ?c mtg:subtype <https://w3id.org/rete/mtg/subtype/dragon> ; schema:name ?card ; mtg:image ?image ; mtg:edhrecRank ?rank .\n} ORDER BY ?rank LIMIT 24"},
      {"family": "Summary", "label": "🎨 The colour pie", "view": "table", "cols": {"color": "Colour", "cards": "Cards"}, "tip": "How many cards fall in each of Magic's five colours (White, Blue, Black, Red, Green). Multicolour cards count under every colour they contain; colourless cards have none.\nmtg:color links a card to a colour node carrying an rdfs:label.", "q": "PREFIX mtg: <https://w3id.org/rete/mtg#>\nPREFIX rdfs: <http://www.w3.org/2000/01/rdf-schema#>\nSELECT ?color (COUNT(?c) AS ?cards) WHERE {\n  ?c mtg:color ?col . ?col rdfs:label ?color .\n} GROUP BY ?color ORDER BY DESC(?cards)"},
      {"family": "Explore", "label": "⭐ The most-played cards", "view": "table", "cols": {"image": "Card", "card": "Name", "rank": "EDHREC"}, "tip": "The most popular cards in the whole game by EDHREC rank (1 = most played) — Sol Ring and friends — as a picture gallery.\nEvery row's picture is a mtg:image edge resolved to Scryfall's CDN; drop the ORDER BY to browse randomly.", "q": "PREFIX mtg: <https://w3id.org/rete/mtg#>\nPREFIX schema: <http://schema.org/>\nSELECT ?image ?card ?rank WHERE {\n  ?c schema:name ?card ; mtg:image ?image ; mtg:edhrecRank ?rank .\n} ORDER BY ?rank LIMIT 24"},
      {"family": "History", "label": "🗓 The first cards ever (Alpha, 1993)", "view": "table", "cols": {"image": "Card", "card": "Name", "rank": "EDHREC"}, "tip": "Every card from Limited Edition Alpha — Magic's very first set, released 5 Aug 1993 — most-played today first. Sets are now named, dated nodes (mtg:printedIn → a Set with schema:name + mtg:releaseDate).\nSwap set/LEA for any set code to browse a set as a gallery.", "q": "PREFIX mtg: <https://w3id.org/rete/mtg#>\nPREFIX schema: <http://schema.org/>\nSELECT ?image ?card ?rank WHERE {\n  ?c mtg:printedIn <https://w3id.org/rete/mtg/set/LEA> ; schema:name ?card ; mtg:image ?image .\n  OPTIONAL { ?c mtg:edhrecRank ?rank }\n} ORDER BY ?rank LIMIT 24"},
      {"family": "Search", "label": "📜 Search the official rulings", "view": "table", "cols": {"card": "Card", "ruling": "Ruling"}, "tip": "Full-text over 76,990 official rulings (the plan's rule-evidence layer): every ruling that mentions “the legend rule”, with the card it clarifies.\nRulings hang off cards as mtg:ruling → a Ruling with dct:date + rdfs:comment; change the phrase to search any rules concept.", "q": "PREFIX mtg: <https://w3id.org/rete/mtg#>\nPREFIX schema: <http://schema.org/>\nPREFIX rdfs: <http://www.w3.org/2000/01/rdf-schema#>\nSELECT ?card ?ruling WHERE {\n  ?c schema:name ?card ; mtg:ruling ?r .\n  ?r rdfs:comment ?ruling .\n  FILTER(CONTAINS(LCASE(?ruling), \"legend rule\"))\n} LIMIT 20"},
      {"family": "Ontology", "label": "🔗 Query cards through Cyril's ontology", "view": "table", "cols": {"image": "Card", "card": "Name"}, "tip": "cmdoret/mtg_ontology is merged in as the TBox: this asks for creatures via HIS class (cmdoret:Creature), reached through owl:equivalentClass back to our mtg:Creature — the two vocabularies are one graph.\nSwap Creature for Instant, Land or Artifact to cross the bridge the other way.", "q": "PREFIX mtg: <https://w3id.org/rete/mtg#>\nPREFIX schema: <http://schema.org/>\nPREFIX owl: <http://www.w3.org/2002/07/owl#>\nSELECT ?image ?card WHERE {\n  <https://cmdoret.net/mtg_ontology/Creature> ^owl:equivalentClass ?t .\n  ?c a ?t ; schema:name ?card ; mtg:image ?image ; mtg:edhrecRank ?rank .\n} ORDER BY ?rank LIMIT 24"}
    ],
    "mtg-game": [
      {"family": "Describe", "label": "🎲 What's on the battlefield (state S1)", "view": "table", "cols": {"controller": "Controls", "card": "Card"}, "tip": "The plan's 'Describe' question, in SPARQL: every card instance on the battlefield in game state S1 and who controls it — Alice has Llanowar Elves + two Mountains, Bob has Grizzly Bears + Serra Angel.\nZoneMembership is state-scoped, so change s1 to s2 to see the board a move later.", "q": "PREFIX mtgg: <https://w3id.org/rete/mtg/game#>\nPREFIX schema: <http://schema.org/>\nPREFIX rdfs: <http://www.w3.org/2000/01/rdf-schema#>\nSELECT ?controller ?card WHERE {\n  ?m a mtgg:ZoneMembership ; mtgg:inState <https://w3id.org/rete/mtg/game/s1> ; mtgg:object ?i ; mtgg:zone <https://w3id.org/rete/mtg/game/battlefield> .\n  ?i mtgg:ofCard ?c ; mtgg:controller ?ctrl .\n  ?c schema:name ?card . ?ctrl rdfs:label ?controller .\n} ORDER BY ?controller ?card"},
      {"family": "Describe", "label": "📜 What does each card DO?", "view": "table", "cols": {"card": "Card", "ability": "Ability", "action": "Action"}, "tip": "Cards aren't text blobs here — each ability is a structured object (cost / condition / effect / action), the way cmdoret/mtg_ontology models them. The action links to the MTG-wiki concept (draw, add_mana …).\nBonders' Enclave shows two abilities incl. its power-≥4 condition.", "q": "PREFIX mtgg: <https://w3id.org/rete/mtg/game#>\nPREFIX mtg: <https://w3id.org/rete/mtg#>\nPREFIX schema: <http://schema.org/>\nPREFIX rdfs: <http://www.w3.org/2000/01/rdf-schema#>\nSELECT ?card ?ability ?action WHERE {\n  ?c a mtg:Card ; schema:name ?card ; mtgg:ability ?ab .\n  ?ab rdfs:label ?ability ; mtgg:effect ?e . ?e mtgg:action ?action .\n} ORDER BY ?card"},
      {"family": "Legality", "label": "✅ Which instants can Alice cast now?", "view": "table", "cols": {"card": "Card", "cost": "Cost", "available": "Mana"}, "tip": "The plan's 'Legality' question answered WITHOUT an engine: an instant in Alice's hand whose mana value she can pay from her untapped mana sources in S1. Lightning Bolt (cost 1) is castable — she has 2 untapped Mountains.\nTap a Mountain (use s2) and re-run to watch it drop out.", "q": "PREFIX mtgg: <https://w3id.org/rete/mtg/game#>\nPREFIX mtg: <https://w3id.org/rete/mtg#>\nPREFIX mtgo: <https://cmdoret.net/mtg_ontology/>\nPREFIX schema: <http://schema.org/>\nSELECT ?card ?cost ?available WHERE {\n  { SELECT (COUNT(?land) AS ?available) WHERE {\n      ?lm mtgg:inState <https://w3id.org/rete/mtg/game/s1> ; mtgg:object ?land ; mtgg:zone <https://w3id.org/rete/mtg/game/battlefield> ; mtgg:tapped false .\n      ?land mtgg:controller <https://w3id.org/rete/mtg/game/alice> ; mtgg:ofCard ?lc . ?lc mtgg:producesMana ?col . } }\n  ?hm mtgg:inState <https://w3id.org/rete/mtg/game/s1> ; mtgg:object ?i ; mtgg:zone <https://w3id.org/rete/mtg/game/aliceHand> .\n  ?i mtgg:ofCard ?c . ?c a mtgo:Instant ; schema:name ?card ; mtg:manaValue ?cost .\n  FILTER(?cost <= ?available)\n}"},
      {"family": "Validate", "label": "🧪 Is the state coherent? (one zone per card)", "view": "table", "cols": {"instance": "Instance", "zones": "# zones"}, "tip": "A SHACL-style coherence check as SPARQL: every card instance must be in exactly one zone in a state. This returns the OFFENDERS — an empty result means S1 is valid.\nA well-formed game state should always answer nothing here.", "q": "PREFIX mtgg: <https://w3id.org/rete/mtg/game#>\nSELECT ?instance (COUNT(?z) AS ?zones) WHERE {\n  ?m a mtgg:ZoneMembership ; mtgg:inState <https://w3id.org/rete/mtg/game/s1> ; mtgg:object ?instance ; mtgg:zone ?z .\n} GROUP BY ?instance HAVING (COUNT(?z) != 1)"},
      {"family": "Counterfactual", "label": "🪄 On the stack: Bolt → Bears (state S2)", "view": "table", "cols": {"spell": "On the stack", "targetCard": "Target", "controller": "Cast by"}, "tip": "State S2 is the successor of S1 after Alice casts Lightning Bolt at Bob's Grizzly Bears — the spell instance now sits on the Stack with its target and controller. Successor states link back with mtgg:previousState.\nThis is a transition captured as data, not run by an engine.", "q": "PREFIX mtgg: <https://w3id.org/rete/mtg/game#>\nPREFIX schema: <http://schema.org/>\nPREFIX rdfs: <http://www.w3.org/2000/01/rdf-schema#>\nSELECT ?spell ?targetCard ?controller WHERE {\n  ?m mtgg:inState <https://w3id.org/rete/mtg/game/s2> ; mtgg:object ?sp ; mtgg:zone <https://w3id.org/rete/mtg/game/stack> .\n  ?sp rdfs:label ?spell ; mtgg:target ?t ; mtgg:controller ?ctrl .\n  ?t mtgg:ofCard ?tc . ?tc schema:name ?targetCard . ?ctrl rdfs:label ?controller .\n}"},
      {"family": "Counterfactual", "label": "🔁 What changed from S1 to S2", "view": "table", "cols": {"card": "Newly tapped"}, "tip": "Diff two snapshots: permanents that were untapped in S1 but tapped in S2 — i.e. what paying for the Bolt cost. Here it's the one Mountain Alice tapped for {R}.\nComparing states is just a join across two mtgg:inState values.", "q": "PREFIX mtgg: <https://w3id.org/rete/mtg/game#>\nPREFIX schema: <http://schema.org/>\nSELECT ?card WHERE {\n  ?m2 mtgg:inState <https://w3id.org/rete/mtg/game/s2> ; mtgg:object ?o ; mtgg:tapped true .\n  ?m1 mtgg:inState <https://w3id.org/rete/mtg/game/s1> ; mtgg:object ?o ; mtgg:tapped false .\n  ?o mtgg:ofCard ?c . ?c schema:name ?card .\n}"}
    ],
    "arxiu": [
      {"family": "Select", "label": "🖼 Browse the scans: image, title, description & dates", "view": "cards", "cols": {"image": "Scan", "title": "Title", "description": "Description", "from": "From", "to": "To", "id": "Record"},
       "tip": "One card per digitized archival unit: the scan, its title, the archivist's description, and the covered date range — best in the Cards output (tap a photo to zoom, tap a card to page through).\nThe ?id column is the archive's own record identifier; the unit IRI itself names the fonds it belongs to (schema:isPartOf walks the hierarchy).",
       "q": "PREFIX schema: <http://schema.org/>\nPREFIX dct: <http://purl.org/dc/terms/>\nSELECT (MIN(STR(?img)) AS ?image) ?title (SAMPLE(?d) AS ?description) (SAMPLE(?f) AS ?from) (SAMPLE(?t) AS ?to) (SAMPLE(?i) AS ?id) WHERE {\n  ?doc schema:name ?title ;\n       schema:image ?img .\n  OPTIONAL { ?doc schema:description ?d }\n  OPTIONAL { ?doc schema:startDate ?f }\n  OPTIONAL { ?doc schema:endDate ?t }\n  OPTIONAL { ?doc dct:identifier ?i }\n} GROUP BY ?doc ?title LIMIT 60"},
      {"family": "Select", "label": "📍 Map the geocoded units", "view": "map", "cols": {"wkt": "Location", "title": "Title"},
       "tip": "6,531 units are geocoded: their titles name a “Terme municipal”, matched to the municipality’s Wikidata coordinates — cemetery surveys, municipal fonds … Switch Output to Map: one pin per unit, hover for the title.\nAdd FILTER(CONTAINS(LCASE(STR(?title)), \"conca de barber\")) to map just one comarca.",
       "q": "PREFIX geo: <http://www.opengis.net/ont/geosparql#>\nPREFIX schema: <http://schema.org/>\nSELECT ?wkt ?title WHERE {\n  ?d geo:asWKT ?wkt ;\n     schema:name ?title .\n} LIMIT 4000"},
      {"family": "Aggregate", "label": "🔗 Federated: files per province (joins geoadmin)", "view": "table", "fed": ["geoadmin"], "cols": {"province": "Province", "files": "# units"},
       "tip": "A TRUE cross-dataset join, live in your browser: each unit’s schema:contentLocation IRI is a PROVINCE in the geoadmin world-boundaries dataset, so ?p’s label (and geometry) come from a DIFFERENT .rete file than the archival unit.\nThe geoadmin source is attached automatically; count how Tarragona’s files dominate thanks to the Conca de Barberà cemetery surveys.",
       "q": "PREFIX schema: <http://schema.org/>\nPREFIX rdfs: <http://www.w3.org/2000/01/rdf-schema#>\nSELECT ?province (COUNT(?d) AS ?files) WHERE {\n  ?d schema:contentLocation ?p .\n  ?p rdfs:label ?province .\n} GROUP BY ?province ORDER BY DESC(?files)"},
      {"family": "Select", "label": "🗺 Files about the Conca de Barberà", "view": "cards", "cols": {"image": "Scan", "title": "Title", "description": "Description", "record": "In the arxiu"},
       "tip": "Full-text place search over the titles: every unit naming the Conca de Barberà (the comarca around Montblanc, Tarragona) — cemetery surveys of Santa Coloma de Queralt, municipal records … one card per unit with its first scan.\nSwap the FILTER text for any place — “priorat”, “montblanc”, “poblet” — to search another area; ?original links the archive's own master file (often a PDF).",
       "q": "PREFIX schema: <http://schema.org/>\nPREFIX arx: <https://w3id.org/rete/arxiu#>\nPREFIX dct: <http://purl.org/dc/terms/>\nSELECT (MIN(STR(?img)) AS ?image) ?title (SAMPLE(?d) AS ?description) (SAMPLE(?link) AS ?record) WHERE {\n  ?doc schema:name ?title ;\n       schema:image ?img ;\n       dct:identifier ?i .\n  FILTER(CONTAINS(LCASE(STR(?title)), \"conca de barber\") || CONTAINS(LCASE(STR(?title)), \"espluga de francol\"))\n  OPTIONAL { ?doc schema:description ?d }\n  BIND(CONCAT(\"https://arxiusenlinia.cultura.gencat.cat/#/cercabasica/resultats?text=\", ENCODE_FOR_URI(STR(?i))) AS ?link)\n} GROUP BY ?doc ?title LIMIT 60"},
      {"family": "Summary", "label": "🏛 The archives (and their size)", "view": "table", "cols": {"archive": "Archive", "units": "Units", "scans": "With scan"}, "tip": "The complete Catalan archives harvested so far, with how many digitised units each holds and how many have a WebP scan (the rest keep only the Gencat link — many are rights-reserved).\nEach unit is a schema:ArchiveComponent held by an ArchiveOrganization.", "q": "PREFIX schema: <http://schema.org/>\nSELECT ?archive (COUNT(?u) AS ?units) (COUNT(?i) AS ?scans) WHERE {\n  ?u a schema:ArchiveComponent ; schema:holdingArchive ?a .\n  ?a schema:name ?archive .\n  OPTIONAL { ?u schema:image ?i }\n} GROUP BY ?archive ORDER BY DESC(?units)"},
      {"family": "Media", "label": "🖼 Scanned documents (gallery)", "view": "table", "cols": {"image": "Scan", "title": "Title", "archive": "Archive"}, "tip": "The scanned documents themselves — each schema:image is the downsized WebP mirrored to R2 (rendered here), ~15% the size of Gencat's original JPEG. Many are medieval charters, notarial and land-registry records.\nDrop the LIMIT to browse all 3,101 scans.", "q": "PREFIX schema: <http://schema.org/>\nSELECT ?image ?title ?archive WHERE {\n  ?u schema:image ?image ; schema:name ?title ; schema:holdingArchive ?a .\n  ?a schema:name ?archive .\n} LIMIT 30"},
      {"family": "Explore", "label": "📅 The oldest documents", "view": "table", "cols": {"image": "Scan", "title": "Title", "start": "From"}, "tip": "Units that carry a parsed start date, earliest first — the deep end of the archive (charters, cartularies).\nDates come from schema:startDate; not every unit has a machine-readable one.", "q": "PREFIX schema: <http://schema.org/>\nSELECT ?image ?title ?start WHERE {\n  ?u schema:startDate ?start ; schema:name ?title .\n  OPTIONAL { ?u schema:image ?image }\n} ORDER BY ?start LIMIT 24"},
      {"family": "Search", "label": "🔎 Documents mentioning Tarragona", "view": "table", "cols": {"image": "Scan", "title": "Title", "desc": "Description"}, "tip": "Full-text over the descriptions (a TEXT_INDEX is built in): units whose description mentions “Tarragona” (848 of them), with their scan.\nChange the word — Reus, Tortosa, cadastre … — to search the catalogue by content.", "q": "PREFIX schema: <http://schema.org/>\nSELECT ?image ?title ?desc WHERE {\n  ?u schema:description ?desc ; schema:name ?title .\n  OPTIONAL { ?u schema:image ?image }\n  FILTER(CONTAINS(LCASE(?desc), \"tarragona\"))\n} LIMIT 24"},
      {"family": "Media", "label": "🔗 WebP copy + original Gencat link", "view": "table", "cols": {"image": "WebP (R2)", "title": "Title", "original": "Original (Gencat)"}, "tip": "Every scan is kept two ways: a downsized WebP on R2 (schema:image, rendered here) and the original full-res JPEG on Gencat's own server (arx:originalImage) — the source of record always points back to the holding archive.\nThe reservat flag is respected: reserved images are linked, never re-hosted.", "q": "PREFIX schema: <http://schema.org/>\nPREFIX arx: <https://w3id.org/rete/arxiu#>\nSELECT ?image ?title ?original WHERE {\n  ?u schema:image ?image ; schema:name ?title ; arx:originalImage ?original .\n} LIMIT 20"},
      {"family": "Media", "label": "📄 PDF documents (rendered first page)", "view": "table", "cols": {"image": "First page", "title": "Title", "original": "Full PDF"}, "tip": "About 3% of the documents are multi-page PDFs, not single images. Their first page is rendered to a WebP (via PyMuPDF) so they still show in the gallery; arx:originalImage links the full PDF on Gencat.\nFilter by arx:contentType to separate PDFs from photographs.", "q": "PREFIX schema: <http://schema.org/>\nPREFIX arx: <https://w3id.org/rete/arxiu#>\nSELECT ?image ?title ?original WHERE {\n  ?u arx:contentType \"PDF\" ; schema:name ?title ; arx:originalImage ?original .\n  OPTIONAL { ?u schema:image ?image }\n} LIMIT 24"}
    ],
    "worldcup2026": [
      {"family": "Select", "label": "👥 A team's full squad", "view": "table", "cols": {"num": "#", "pos": "Pos", "player": "Player", "club": "Club (at the WC)", "dob": "Born"}, "tip": "The complete 26-man roster (from Wikipedia) — shirt number, position, club at the tournament, and date of birth. Here Argentina; swap the team IRI for any nation.\nThis is the FULL squad, not just the goalscorers — every dataset player links to their national team via wc:representedTeam.", "q": "PREFIX wc: <https://w3id.org/rete/worldcup#>\nPREFIX sc: <http://schema.org/>\nPREFIX rdfs: <http://www.w3.org/2000/01/rdf-schema#>\nPREFIX xsd: <http://www.w3.org/2001/XMLSchema#>\nSELECT ?num ?pos ?player ?club ?dob WHERE {\n  <https://w3id.org/rete/worldcup/2026/team/Argentina> wc:squadPlayer ?p .\n  ?p sc:name ?player .\n  OPTIONAL { ?p wc:shirtNumber ?num }\n  OPTIONAL { ?p wc:position ?pos }\n  OPTIONAL { ?p sc:birthDate ?dob }\n  OPTIONAL { ?p wc:clubAtTournament ?c . ?c rdfs:label ?club }\n} ORDER BY xsd:integer(?num)"},
      {"family": "Aggregate", "label": "🏟️ Clubs that sent the most players", "view": "table", "cols": {"club": "Club", "players": "Players"}, "tip": "Now that every squad is in the graph, ask which clubs supplied the most World Cup players — the elite European sides dominate (Barcelona, Bayern, Man City), with the host nation's club a surprise high entry.\nGROUP BY over wc:clubAtTournament across all 32/48 full squads.", "q": "PREFIX wc: <https://w3id.org/rete/worldcup#>\nPREFIX rdfs: <http://www.w3.org/2000/01/rdf-schema#>\nSELECT ?club (COUNT(?p) AS ?players) WHERE {\n  ?p wc:clubAtTournament ?c .\n  ?c rdfs:label ?club .\n} GROUP BY ?club ORDER BY DESC(?players) LIMIT 20"},
      {"family": "Summary", "label": "What's in this LIVE graph", "view": "table", "cols": {"kind": "Kind", "n": "#"}, "tip": "⚠ A snapshot of a tournament in progress (semi-final stage), with results AS RECORDED BY OpenFootball — not verified. Counts by type: 102 Match nodes (100 played + 2 scheduled semis), Goals, 16 Stadiums, CareerStints.\nSee the sibling `worldcup` for the complete 2022 edition + predictions.", "q": "PREFIX wc: <https://w3id.org/rete/worldcup#>\nPREFIX rdfs: <http://www.w3.org/2000/01/rdf-schema#>\nSELECT ?kind (COUNT(?s) AS ?n) WHERE {\n  ?s a ?c . ?c rdfs:label ?kind .\n  VALUES ?c { wc:Match wc:Goal wc:Stadium wc:CareerStint }\n} GROUP BY ?kind ORDER BY DESC(?n)"},
      {"family": "Select", "label": "🔴 Who's still in it — the semi-finals", "view": "table", "cols": {"match": "Fixture", "date": "Date"}, "tip": "The live bracket: the matches with wc:status 'scheduled' are the ties not yet played. As of this snapshot the four semi-finalists are France, Spain, England and Argentina.\nEverything else has a score; flip the filter to \"played\" to see the results so far.", "q": "PREFIX wc: <https://w3id.org/rete/worldcup#>\nPREFIX sc: <http://schema.org/>\nPREFIX rdfs: <http://www.w3.org/2000/01/rdf-schema#>\nSELECT ?match ?date WHERE {\n  ?m a wc:Match ; wc:status \"scheduled\" ; rdfs:label ?match .\n  OPTIONAL { ?m sc:startDate ?date }\n} ORDER BY ?date"},
      {"family": "Aggregate", "label": "🏆 The Golden Boot race", "view": "table", "cols": {"player": "Player", "goals": "Goals"}, "tip": "Who's scoring most so far (own goals excluded). In this snapshot Messi and Mbappé lead on 8, with Haaland, Bellingham and Kane chasing — and the semis still to play.\nNames are normalised so a surname and full name count as one player.", "q": "PREFIX wc: <https://w3id.org/rete/worldcup#>\nPREFIX sc: <http://schema.org/>\nSELECT ?player (COUNT(?g) AS ?goals) WHERE {\n  ?g a wc:Goal ; wc:scoredBy ?p .\n  ?p sc:name ?player .\n  FILTER NOT EXISTS { ?g wc:ownGoal true }\n} GROUP BY ?player ORDER BY DESC(?goals) LIMIT 15"},
      {"family": "Path", "label": "🛣️ A semi-finalist's road to the last four", "view": "table", "cols": {"stage": "Stage", "match": "Match"}, "tip": "Trace one team through the bracket, round by round: every played match involving Argentina, from the group stage through the quarter-final. UNION catches games where they were home OR away.\nSwap the team IRI for …/team/France, /Spain, /England to follow another run.", "q": "PREFIX wc: <https://w3id.org/rete/worldcup#>\nPREFIX sc: <http://schema.org/>\nPREFIX rdfs: <http://www.w3.org/2000/01/rdf-schema#>\nSELECT ?stage ?match WHERE {\n  ?m a wc:Match ; wc:stage ?stage ; rdfs:label ?match ; wc:status \"played\" .\n  { ?m wc:homeTeam <https://w3id.org/rete/worldcup/2026/team/Argentina> } UNION { ?m wc:awayTeam <https://w3id.org/rete/worldcup/2026/team/Argentina> }\n} ORDER BY ?m"},
      {"family": "Path", "label": "⚽ Player evolution: Mbappé's club career", "view": "table", "cols": {"club": "Club", "from": "From", "to": "To"}, "tip": "The temporal dimension, same as the 2022 twin: Kylian Mbappé's club stints with start/end years, from Wikidata (P54 + date qualifiers).\nSwap the IRI for …/player/Lionel_Messi, /Erling_Haaland, /Jude_Bellingham.", "q": "PREFIX wc: <https://w3id.org/rete/worldcup#>\nPREFIX sc: <http://schema.org/>\nPREFIX rdfs: <http://www.w3.org/2000/01/rdf-schema#>\nSELECT ?club ?from ?to WHERE {\n  ?st a wc:CareerStint ;\n      wc:player <https://w3id.org/rete/worldcup/2026/player/Kylian_Mbapp%C3%A9> ;\n      wc:club ?c .\n  ?c rdfs:label ?club .\n  OPTIONAL { ?st sc:startDate ?from }\n  OPTIONAL { ?st sc:endDate ?to }\n} ORDER BY ?from"},
      {"family": "Select", "label": "🗺️ 16 stadiums across three countries", "view": "map", "cols": {"name": "Stadium", "city": "City", "wkt": "Location"}, "tip": "The first tri-nation World Cup — switch Output → Map to see the venues spread from Vancouver and Seattle down to Mexico City. Each carries capacity and a GeoSPARQL point.\nThe Estadio Azteca and MetLife (the final's venue) are the biggest.", "q": "PREFIX wc: <https://w3id.org/rete/worldcup#>\nPREFIX sc: <http://schema.org/>\nPREFIX geo: <http://www.opengis.net/ont/geosparql#>\nSELECT ?name ?city ?wkt WHERE {\n  ?s a wc:Stadium ; sc:name ?name ; wc:city ?city ; geo:asWKT ?wkt .\n} ORDER BY ?name"},
      {"family": "Select", "label": "💥 Penalty shootouts (the knockout drama)", "view": "table", "cols": {"match": "Match", "hp": "Home pens", "ap": "Away pens"}, "tip": "Every knockout tie decided from the spot — matches carrying wc:homePenalties / wc:awayPenalties. Big names went out this way (Germany and the Netherlands both fell in the Round of 32).\nThe label shows the 120-minute score; the penalty columns show the shootout.", "q": "PREFIX wc: <https://w3id.org/rete/worldcup#>\nPREFIX rdfs: <http://www.w3.org/2000/01/rdf-schema#>\nSELECT ?match ?hp ?ap WHERE {\n  ?m a wc:Match ; rdfs:label ?match ;\n     wc:homePenalties ?hp ; wc:awayPenalties ?ap .\n} ORDER BY ?m"},
      {"family": "Aggregate", "label": "🥅 Goals per group stage vs knockout", "view": "table", "cols": {"stage": "Stage", "goals": "Goals"}, "tip": "How the goals spread across the rounds — the 48-team group stage is high-scoring, the knockouts tighten up.\nGroup by wc:stage over the goals, joined through each goal's match.", "q": "PREFIX wc: <https://w3id.org/rete/worldcup#>\nSELECT ?stage (COUNT(?g) AS ?goals) WHERE {\n  ?g a wc:Goal ; wc:inMatch ?m .\n  ?m wc:stage ?stage .\n} GROUP BY ?stage ORDER BY DESC(?goals)"}
    ],
    "worldcup": [
      {"family": "Select", "label": "👥 A team's full squad", "view": "table", "cols": {"num": "#", "pos": "Pos", "player": "Player", "club": "Club (at the WC)", "dob": "Born"}, "tip": "The complete 26-man roster (from Wikipedia) — shirt number, position, club at the tournament, and date of birth. Here Argentina; swap the team IRI for any nation.\nThis is the FULL squad, not just the goalscorers — every dataset player links to their national team via wc:representedTeam.", "q": "PREFIX wc: <https://w3id.org/rete/worldcup#>\nPREFIX sc: <http://schema.org/>\nPREFIX rdfs: <http://www.w3.org/2000/01/rdf-schema#>\nPREFIX xsd: <http://www.w3.org/2001/XMLSchema#>\nSELECT ?num ?pos ?player ?club ?dob WHERE {\n  <https://w3id.org/rete/worldcup/team/Argentina> wc:squadPlayer ?p .\n  ?p sc:name ?player .\n  OPTIONAL { ?p wc:shirtNumber ?num }\n  OPTIONAL { ?p wc:position ?pos }\n  OPTIONAL { ?p sc:birthDate ?dob }\n  OPTIONAL { ?p wc:clubAtTournament ?c . ?c rdfs:label ?club }\n} ORDER BY xsd:integer(?num)"},
      {"family": "Aggregate", "label": "🏟️ Clubs that sent the most players", "view": "table", "cols": {"club": "Club", "players": "Players"}, "tip": "Now that every squad is in the graph, ask which clubs supplied the most World Cup players — the elite European sides dominate (Barcelona, Bayern, Man City), with the host nation's club a surprise high entry.\nGROUP BY over wc:clubAtTournament across all 32/48 full squads.", "q": "PREFIX wc: <https://w3id.org/rete/worldcup#>\nPREFIX rdfs: <http://www.w3.org/2000/01/rdf-schema#>\nSELECT ?club (COUNT(?p) AS ?players) WHERE {\n  ?p wc:clubAtTournament ?c .\n  ?c rdfs:label ?club .\n} GROUP BY ?club ORDER BY DESC(?players) LIMIT 20"},
      {"family": "Summary", "label": "What's in this graph", "view": "table", "cols": {"kind": "Kind", "n": "#"}, "tip": "The shape of the WC2022 graph: 64 Matches, 32 SportsTeams, players, Goals, 8 Stadiums, and the Prediction nodes (538 × 6 dates + Opta + bookmaker) plus CareerStints.\nOne row per rdf:type.", "q": "PREFIX wc: <https://w3id.org/rete/worldcup#>\nPREFIX rdfs: <http://www.w3.org/2000/01/rdf-schema#>\nSELECT ?kind (COUNT(?s) AS ?n) WHERE {\n  ?s a ?c . ?c rdfs:label ?kind .\n  VALUES ?c { wc:Match wc:Goal wc:Stadium wc:Prediction wc:CareerStint }\n} GROUP BY ?kind ORDER BY DESC(?n)"},
      {"family": "Aggregate", "label": "🏆 The Golden Boot (top scorers)", "view": "table", "cols": {"player": "Player", "goals": "Goals"}, "tip": "Who scored most: Kylián Mbappé 8 (the actual Golden Boot), Messi 7, then Álvarez & Giroud 4. Own goals are excluded via FILTER NOT EXISTS.\nNames are normalised so 'Mbappé' and 'Kylian Mbappé' count as one player.", "q": "PREFIX wc: <https://w3id.org/rete/worldcup#>\nPREFIX sc: <http://schema.org/>\nSELECT ?player (COUNT(?g) AS ?goals) WHERE {\n  ?g a wc:Goal ; wc:scoredBy ?p .\n  ?p sc:name ?player .\n  FILTER NOT EXISTS { ?g wc:ownGoal true }\n} GROUP BY ?player ORDER BY DESC(?goals) LIMIT 15"},
      {"family": "Select", "label": "📈 Argentina's title odds, snapshot by snapshot (538)", "view": "table", "cols": {"date": "Forecast date", "prob": "Champion prob."}, "tip": "The predictions payoff. FiveThirtyEight's probability that Argentina would WIN the cup, at each of 6 forecast dates: 0.085 pre-tournament → 0.047 right after the shock loss to Saudi Arabia (Nov 24) → climbing to 0.315 by the quarter-finals.\nSwitch Output → Time for the curve. Change the team IRI (…/team/France, /Brazil, /Morocco) to trace another run.", "q": "PREFIX wc: <https://w3id.org/rete/worldcup#>\nPREFIX prov: <http://www.w3.org/ns/prov#>\nSELECT ?date ?prob WHERE {\n  ?pr wc:aboutTeam <https://w3id.org/rete/worldcup/team/Argentina> ;\n      wc:outcome wc:Champion ;\n      wc:probability ?prob ;\n      wc:forecastDate ?date ;\n      prov:wasAttributedTo <https://w3id.org/rete/worldcup/source/538> .\n} ORDER BY ?date"},
      {"family": "Path", "label": "🎲 Who was the favourite? Champion odds by forecaster", "view": "table", "cols": {"team": "Team", "src": "Forecaster", "prob": "Champion prob."}, "tip": "Different parties, side by side: the pre-tournament (Nov 20) probability each source gave for winning the whole thing. Brazil led every forecaster (book 17%, Opta 15%); Argentina — who actually won — sat 2nd around 13-14%.\nThis is the 'compare who predicted what' query, joined on the shared team.", "q": "PREFIX wc: <https://w3id.org/rete/worldcup#>\nPREFIX sc: <http://schema.org/>\nPREFIX prov: <http://www.w3.org/ns/prov#>\nPREFIX xsd: <http://www.w3.org/2001/XMLSchema#>\nSELECT ?team ?src ?prob WHERE {\n  ?pr wc:outcome wc:Champion ;\n      wc:forecastDate \"2022-11-20\"^^xsd:date ;\n      wc:aboutTeam ?t ; wc:probability ?prob ;\n      prov:wasAttributedTo ?s .\n  ?t sc:name ?team .\n  ?s sc:name ?src .\n} ORDER BY DESC(?prob) LIMIT 18"},
      {"family": "Select", "label": "🥅 The final, goal by goal", "view": "table", "cols": {"scorer": "Scorer", "min": "Minute", "team": "For"}, "tip": "Argentina 3-3 France (Argentina won 4-2 on penalties) — the goals in order: Messi 23'(pen), Di María 36', then Mbappé's hat-trick 80'(pen), 81', 118', with Messi's 108'.\nmatch/064 is the final; swap it for match/063 (third place) or any group match.", "q": "PREFIX wc: <https://w3id.org/rete/worldcup#>\nPREFIX sc: <http://schema.org/>\nPREFIX xsd: <http://www.w3.org/2001/XMLSchema#>\nSELECT ?scorer ?min ?team WHERE {\n  ?g wc:inMatch <https://w3id.org/rete/worldcup/match/064> ;\n     wc:scoredBy ?p ; wc:minute ?min ; wc:forTeam ?tm .\n  ?p sc:name ?scorer .\n  ?tm sc:name ?team .\n} ORDER BY xsd:integer(?min)"},
      {"family": "Path", "label": "⚽ Player evolution: Messi's club career", "view": "table", "cols": {"club": "Club", "from": "From", "to": "To"}, "tip": "The temporal dimension. Every club stint of Lionel Messi with start/end years — Newell's Old Boys → Barcelona (2004-2021) → PSG → Inter Miami — pulled from Wikidata (P54 + date qualifiers) and attached to the WC scorer node.\nSwap the player IRI for …/player/Kylian_Mbappé, /Olivier_Giroud, /Luka_Modrić.", "q": "PREFIX wc: <https://w3id.org/rete/worldcup#>\nPREFIX sc: <http://schema.org/>\nPREFIX rdfs: <http://www.w3.org/2000/01/rdf-schema#>\nSELECT ?club ?from ?to WHERE {\n  ?st a wc:CareerStint ;\n      wc:player <https://w3id.org/rete/worldcup/player/Lionel_Messi> ;\n      wc:club ?c .\n  ?c rdfs:label ?club .\n  OPTIONAL { ?st sc:startDate ?from }\n  OPTIONAL { ?st sc:endDate ?to }\n} ORDER BY ?from"},
      {"family": "Select", "label": "🗺️ The eight Qatar stadiums", "view": "map", "cols": {"name": "Stadium", "cap": "Capacity", "wkt": "Location"}, "tip": "Every venue as a GeoSPARQL point — switch Output → Map to see them clustered around Doha. The Lusail Iconic Stadium (88,966) hosted the final.\nEach match links to its wc:venue, so you can map where any game was played.", "q": "PREFIX wc: <https://w3id.org/rete/worldcup#>\nPREFIX sc: <http://schema.org/>\nPREFIX geo: <http://www.opengis.net/ont/geosparql#>\nSELECT ?name ?cap ?wkt WHERE {\n  ?s a wc:Stadium ; sc:name ?name ; geo:asWKT ?wkt .\n  OPTIONAL { ?s wc:capacity ?cap }\n} ORDER BY DESC(?cap)"},
      {"family": "Select", "label": "🚩 The knockout bracket", "view": "table", "cols": {"stage": "Stage", "match": "Match", "when": "Date"}, "tip": "Every knockout tie from Round of 16 to the Final, with the score in the label and the date. The wc:stage predicate partitions the tournament (group / round_of_16 / quarter_final / semi_final / third_place / final).\nDrop the FILTER to include all 64 matches.", "q": "PREFIX wc: <https://w3id.org/rete/worldcup#>\nPREFIX rdfs: <http://www.w3.org/2000/01/rdf-schema#>\nPREFIX sc: <http://schema.org/>\nPREFIX xsd: <http://www.w3.org/2001/XMLSchema#>\nSELECT ?stage ?match ?when WHERE {\n  ?m a wc:Match ; wc:stage ?stage ; rdfs:label ?match .\n  OPTIONAL { ?m sc:startDate ?when }\n  FILTER(?stage != \"group\")\n} ORDER BY ?when"},
      {"family": "Aggregate", "label": "📊 The title race, snapshot by snapshot", "view": "table", "cols": {"date": "Forecast date", "team": "Team", "prob": "Champion prob."}, "tip": "The whole tournament as a shifting leaderboard: FiveThirtyEight's champion probability for every team above 10%, at each of the 6 forecast dates. Read top to bottom and watch the story unfold — Brazil the runaway favourite (peaking at 33% on Dec 6) until Croatia knocked them out on Dec 9, and Argentina surging from off-the-list after the Saudi shock to 32% and the top.\nLower the FILTER threshold for the long tail; swap the source IRI to compare forecasters.", "q": "PREFIX wc: <https://w3id.org/rete/worldcup#>\nPREFIX sc: <http://schema.org/>\nPREFIX prov: <http://www.w3.org/ns/prov#>\nSELECT ?date ?team ?prob WHERE {\n  ?pr wc:outcome wc:Champion ;\n      prov:wasAttributedTo <https://w3id.org/rete/worldcup/source/538> ;\n      wc:aboutTeam ?t ; wc:probability ?prob ; wc:forecastDate ?date .\n  ?t sc:name ?team .\n  FILTER(?prob > 0.10)\n} ORDER BY ?date DESC(?prob)"}
    ],
    "ecal": [
      {"family": "Aggregate", "label": "🎨 The sections of an art & design library", "view": "table", "cols": {"section": "Section", "items": "# items"}, "tip": "ECAL classifies by discipline: Art visuel (9.4k), Photographie (4.2k), Design industriel (4.1k), Cinéma (3.7k), Design graphique… — the shape of an art school's shelves.\nEach ?section is a physical department, browsable by call number.", "q": "PREFIX schema: <http://schema.org/>\nPREFIX dct: <http://purl.org/dc/terms/>\nPREFIX rdfs: <http://www.w3.org/2000/01/rdf-schema#>\nSELECT ?section (COUNT(?w) AS ?items) WHERE {\n  ?w dct:isPartOf ?c .\n  ?c rdfs:label ?section .\n} GROUP BY ?section ORDER BY DESC(?items) LIMIT 15"},
      {"family": "Select", "label": "🖼️ Book-cover gallery", "view": "table", "cols": {"cover": "Cover", "title": "Title", "author": "Author"}, "tip": "A wall of covers — 93% of ECAL's books have one (Shoichi Aoki's Fresh Fruits, Banksy, Le Corbusier…). ?cover renders inline.\nDrop `a schema:Book` for a section to make a themed gallery.", "q": "PREFIX schema: <http://schema.org/>\nSELECT ?cover ?title ?author WHERE {\n  ?w a schema:Book ;\n     schema:thumbnailUrl ?cover ;\n     schema:name ?title .\n  OPTIONAL { ?w schema:creator ?a . ?a schema:name ?author }\n} LIMIT 60"},
      {"family": "Select", "label": "📸 Browse the Photographie shelf (by call number)", "view": "table", "cols": {"call": "Call no.", "cover": "Cover", "title": "Title"}, "tip": "Walk one department in shelf order. This lists the Photographie section by call number — the physical sequence.\nSwap \"Photographie\" for \"Cinéma\", \"Design graphique\", \"Art visuel / Arts plastiques\"…", "q": "PREFIX schema: <http://schema.org/>\nPREFIX dct: <http://purl.org/dc/terms/>\nPREFIX b: <https://data.ecal.ch/>\nPREFIX rdfs: <http://www.w3.org/2000/01/rdf-schema#>\nSELECT ?call ?cover ?title WHERE {\n  ?c rdfs:label \"Photographie\" .\n  ?w dct:isPartOf ?c ;\n     b:callNumber ?call ;\n     schema:name ?title .\n  OPTIONAL { ?w schema:thumbnailUrl ?cover }\n} ORDER BY ?call LIMIT 60"},
      {"family": "Aggregate", "label": "👥 Most-collected artists, designers & authors", "view": "table", "cols": {"name": "Author / maker", "n": "# works"}, "tip": "What ECAL collects deeply: Sotheby's auction catalogues, Jacques Chessex, the International Typeface Corporation, Le Corbusier, design writers (Fiell), architecture (Jodidio).\nAn aggregate over the shared creator nodes.", "q": "PREFIX schema: <http://schema.org/>\nSELECT ?name (COUNT(?w) AS ?n) WHERE {\n  ?w schema:creator ?a .\n  ?a schema:name ?name .\n} GROUP BY ?name ORDER BY DESC(?n) LIMIT 25"},
      {"family": "Select", "label": "🎬 Films & DVDs in the collection", "view": "table", "cols": {"cover": "Cover", "title": "Title", "director": "Director"}, "tip": "ECAL is also a film school — 3,014 moving-image works sit beside the books (Chabrol, Ruiz…).\n?cover shows the sleeve where there is one.", "q": "PREFIX schema: <http://schema.org/>\nSELECT ?cover ?title ?director WHERE {\n  ?w a schema:VideoObject ;\n     schema:name ?title .\n  OPTIONAL { ?w schema:thumbnailUrl ?cover }\n  OPTIONAL { ?w schema:creator ?a . ?a schema:name ?director }\n} LIMIT 60"},
      {"family": "Select", "label": "🔎 Everything on 'Art contemporain'", "view": "table", "cols": {"cover": "Cover", "title": "Title", "author": "Author"}, "tip": "Subjects are shared nodes, so one click gathers every book about a topic — here Art contemporain (874 works).\nChange the label to \"Photographie : 21e siècle\", \"Écrits d'artiste\", \"Typographie\"…", "q": "PREFIX schema: <http://schema.org/>\nPREFIX rdfs: <http://www.w3.org/2000/01/rdf-schema#>\nSELECT ?cover ?title ?author WHERE {\n  ?c rdfs:label \"Art contemporain\" .\n  ?w schema:about ?c ;\n     schema:name ?title .\n  OPTIONAL { ?w schema:thumbnailUrl ?cover }\n  OPTIONAL { ?w schema:creator ?a . ?a schema:name ?author }\n} LIMIT 50"},
      {"family": "Select", "label": "🎨 An art book, fully dressed — every field at once", "view": "table", "cols": {"cover": "Cover", "title": "Title", "author": "Author", "year": "Year", "call": "Call no.", "availability": "On shelf?"}, "tip": "One row, four kinds of cell: ?cover renders as an image, ?year as a date, the rest as text — a book completely described, newest acquisitions first.\n?availability is the live shelf status and ?call is the shelfmark you would walk to.", "q": "PREFIX schema: <http://schema.org/>\nPREFIX b: <https://data.ecal.ch/>\nSELECT ?cover ?title ?author ?year ?call ?availability WHERE {\n  ?w a schema:Book ;\n     schema:thumbnailUrl ?cover ;\n     schema:name ?title ;\n     schema:startDate ?year ;\n     b:callNumber ?call ;\n     b:availability ?availability .\n  OPTIONAL { ?w schema:creator ?a . ?a schema:name ?author }\n} ORDER BY DESC(?year) LIMIT 40"},
      {"family": "Select", "label": "🎬 Film gallery — with a link to each record", "view": "table", "cols": {"cover": "Cover", "title": "Title", "director": "Director", "year": "Year", "record": "Record ↗"}, "tip": "Five columns, four types — including a LINK: ?record is each film's page in the ECAL catalogue, clickable straight from the table (?cover is the sleeve, ?year a date).\nECAL is also a film school — 3,014 moving-image works sit beside the books (Chabrol, Ruiz…).", "q": "PREFIX schema: <http://schema.org/>\nSELECT ?cover ?title ?director ?year ?record WHERE {\n  ?w a schema:VideoObject ;\n     schema:name ?title ;\n     schema:url ?record .\n  OPTIONAL { ?w schema:thumbnailUrl ?cover }\n  OPTIONAL { ?w schema:creator ?a . ?a schema:name ?director }\n  OPTIONAL { ?w schema:startDate ?year }\n} LIMIT 40"},
      {"family": "Select", "label": "🏛️ Le Corbusier, in chronological order", "view": "table", "cols": {"cover": "Cover", "title": "Title", "year": "Year", "call": "Call no."}, "tip": "One maker's whole shelf: 22 works by Le Corbusier, oldest first — cover, year and call number side by side (image + date + text).\nSwap \"LE CORBUSIER\" for any name from the 'Most-collected artists, designers & authors' query above.", "q": "PREFIX schema: <http://schema.org/>\nPREFIX b: <https://data.ecal.ch/>\nSELECT ?cover ?title ?year ?call WHERE {\n  ?a schema:name \"LE CORBUSIER\" .\n  ?w schema:creator ?a ;\n     schema:name ?title .\n  OPTIONAL { ?w schema:thumbnailUrl ?cover }\n  OPTIONAL { ?w schema:startDate ?year }\n  OPTIONAL { ?w b:callNumber ?call }\n} ORDER BY ?year LIMIT 40"}
    ],
    "gbif-birds": [
      {"family": "Path", "label": "★ Semantic zoom: the taxonomic ladder of a species", "view": "table", "cols": {"rank": "Rank", "name": "Taxon"},
       "tip": "The whole point of this dataset. rdfs:subClassOf+ walks the Barn Swallow (Hirundo rustica) UP its taxonomic ladder: Genus Hirundo → Family Hirundinidae → Order Passeriformes → Class Aves → Phylum Chordata → Kingdom Animalia. This same chain, over all 1,104 species, is what the Schema tab rolls the 50M sightings up into — open it to see the 6-level pyramid (level 3 = Orders: Passeriformes 29.7M, raptors 2.9M …).\nAsk a bird question at the Order or Family level and you never touch a single record. Swap the species key (owl:sameAs gbif.org/species/<key>) for any of the 1,104.",
       "q": "PREFIX rdfs: <http://www.w3.org/2000/01/rdf-schema#>\nPREFIX dwc: <http://rs.tdwg.org/dwc/terms/>\nPREFIX v: <https://w3id.org/rete/gbif/vocab#>\nSELECT ?rank ?name WHERE {\n  <https://w3id.org/rete/gbif/taxon/species/9515886> rdfs:subClassOf+ ?anc .\n  ?anc rdfs:label ?name ; dwc:taxonRank ?rank .\n}"},
      {"family": "Select", "label": "🌍 A bird's common name in every European language", "view": "table", "cols": {"lang": "Lang", "name": "Common name"},
       "tip": "New in this build: every species carries its common name in up to 47 European languages as a language-tagged dwc:vernacularName. This lists them all for the Barn Swallow (Hirundo rustica, gbif.org/species/9515886) — ca 'Oreneta vulgar', fr 'Hirondelle rustique', eu (Basque) 'Enara arrunt', cy (Welsh) 'gwennol', br (Breton) 'Gwennili-siminal', cs 'vlaštovka obecná' …\nSwap the species key (2481912 White Stork, 2480649 Bearded Vulture) to read any of the 1,020 named species in your own language.",
       "q": "PREFIX dwc: <http://rs.tdwg.org/dwc/terms/>\nSELECT ?lang ?name WHERE {\n  <https://w3id.org/rete/gbif/taxon/species/9515886> dwc:vernacularName ?name .\n  BIND(LANG(?name) AS ?lang)\n}\nORDER BY ?lang"},
      {"family": "Select", "label": "✨ RDF-star: where did this name come from?", "view": "table", "cols": {"name": "Name", "src": "Source", "when": "Added"},
       "tip": "The headline of this build — provenance as RDF-star. Each name assertion is itself a QUOTED TRIPLE << species dwc:vernacularName \"…\" >> annotated with dct:source (which checklist it came from) and prov:generatedAtTime (when we added it). Here: every name of the Egyptian Vulture (Neophron percnopterus, IUCN Endangered) with its source — fr 'Petit vautour', ca 'Miloca', es 'Alimoche'.\nThis is a statement ABOUT a statement: the name has no node of its own, so the provenance lives on the quoted triple. Add `prov:wasDerivedFrom ?api` to get the live GBIF API URL each name was pulled from.",
       "q": "PREFIX dwc: <http://rs.tdwg.org/dwc/terms/>\nPREFIX dct: <http://purl.org/dc/terms/>\nPREFIX prov: <http://www.w3.org/ns/prov#>\nSELECT ?name ?src ?when WHERE {\n  << <https://w3id.org/rete/gbif/taxon/species/2480696> dwc:vernacularName ?name >>\n     dct:source ?src ; prov:generatedAtTime ?when .\n}"},
      {"family": "Select", "label": "🔎 Find a bird by its Catalan name", "view": "table", "cols": {"sp": "Species", "n": "Catalan name", "sci": "Scientific"},
       "tip": "Reverse lookup: search the multilingual names instead of the scientific ones. 'oreneta' is Catalan for swallow, so this finds every swallow species recorded in Spain + Switzerland — Oreneta vulgar (Barn Swallow), de ribera (Sand Martin), cuablanca (House Martin), de cingle (Crag Martin) …\nChange \"oreneta\" and the language tag to search another language — LANG(?n)=\"fr\" with \"mésange\" for the tits, \"es\" with \"águila\" for the eagles.",
       "q": "PREFIX dwc: <http://rs.tdwg.org/dwc/terms/>\nPREFIX rdfs: <http://www.w3.org/2000/01/rdf-schema#>\nSELECT ?sp ?n ?sci WHERE {\n  ?sp dwc:vernacularName ?n ; rdfs:label ?sci .\n  FILTER(LANG(?n) = \"ca\" && CONTAINS(LCASE(STR(?n)), \"oreneta\"))\n}"},
      {"family": "Select", "label": "🛡️ Threatened birds: the IUCN Red List species", "view": "table", "cols": {"status": "IUCN", "name": "Species", "sci": "Scientific"},
       "tip": "165 of the 1,104 species carry an IUCN Red List category worse than Least Concern (NT near-threatened → CR critically endangered → EX). This lists them with their English name, most-threatened first: the Egyptian Vulture (EN), Aquatic Warbler (VU), Balearic Shearwater (CR) … Conservation status comes from the IUCN Red List via Wikidata; each v:iucnStatus assertion carries its own RDF-star provenance.\nDrop the FILTER for just CR/EN to see only the gravest cases, or map them with the next example.",
       "q": "PREFIX v: <https://w3id.org/rete/gbif/vocab#>\nPREFIX dwc: <http://rs.tdwg.org/dwc/terms/>\nPREFIX rdfs: <http://www.w3.org/2000/01/rdf-schema#>\nSELECT ?status ?name ?sci WHERE {\n  ?sp v:sensitive true ; v:iucnStatus ?status ; rdfs:label ?sci .\n  OPTIONAL { ?sp dwc:vernacularName ?name . FILTER(LANG(?name) = \"en\") }\n}\nORDER BY DESC(?status)"},
      {"family": "Select", "label": "🗺️ Map the Endangered & Critically Endangered birds", "view": "map", "cols": {"wkt": "Location", "st": "IUCN"},
       "tip": "Conservation crossed with geography: every occurrence of a species the IUCN lists as Endangered (EN) or Critically Endangered (CR), mapped across Spain + Switzerland — Egyptian Vulture, Balearic Shearwater, Aquatic Warbler, Steppe Eagle … Switch Output to Map. Fans out over the threatened species (each cheap and selective via v:iucnStatus), so it is LIMITed.\nSwap the FILTER to (\"VU\") to add the vulnerable species, or to a single status for a cleaner map.",
       "q": "PREFIX v: <https://w3id.org/rete/gbif/vocab#>\nPREFIX geo: <http://www.opengis.net/ont/geosparql#>\nSELECT ?wkt ?st WHERE {\n  ?sp v:iucnStatus ?st . FILTER(?st IN (\"EN\", \"CR\"))\n  ?o a ?sp ; geo:asWKT ?wkt .\n}\nLIMIT 3000"},
      {"family": "Select", "label": "📊 A species' record book: counts, individuals, span", "view": "table", "cols": {"rc": "Records", "est": "Individuals", "fy": "First", "ly": "Last"},
       "tip": "Per-species occurrence statistics, precomputed at build time and stored (with RDF-star provenance) so you read them without scanning the 333M records: v:recordCount (how many sightings), v:estimatedIndividuals (the summed dwc:individualCount — the 'number estimated'), and the first/last year seen. White Stork (Ciconia ciconia): 330,039 records, ~7.08M individuals counted, 1579→2026.\nEach occurrence also carries its own dwc:individualCount where the observer recorded one — swap the species key for any bird.",
       "q": "PREFIX v: <https://w3id.org/rete/gbif/vocab#>\nSELECT ?rc ?est ?fy ?ly WHERE {\n  <https://w3id.org/rete/gbif/taxon/species/2481912> v:recordCount ?rc ; v:firstYear ?fy ; v:lastYear ?ly .\n  OPTIONAL { <https://w3id.org/rete/gbif/taxon/species/2481912> v:estimatedIndividuals ?est }\n}"},
      {"family": "Select", "label": "🗺️ Map a species: Barn Swallow across Iberia & the Alps", "view": "map", "cols": {"wkt": "Location"},
       "tip": "Every occurrence carries a GeoSPARQL POINT. This maps the first 4,000 Barn Swallow (Hirundo rustica) sightings — Switch Output to Map. Selective by species with a LIMIT, so it early-outs and stays cheap over HTTP range (it never scans the 1.2 GB file).\nSwap the species key for 5231190 (House Sparrow), 2490719 (Blackbird), 2492956 (Blackcap) to map another bird.",
       "q": "PREFIX geo: <http://www.opengis.net/ont/geosparql#>\nSELECT ?wkt WHERE {\n  ?o a <https://w3id.org/rete/gbif/taxon/species/9515886> ;\n     geo:asWKT ?wkt .\n} LIMIT 4000"},
      {"family": "Path", "label": "Every swallow species (family Hirundinidae)", "view": "table", "cols": {"name": "Species"},
       "tip": "rdfs:subClassOf+ DOWN from a family node lists all species it contains (species ⊑ genus ⊑ family, so it's transitive) — the swallows & martins recorded in Spain + Switzerland: Barn Swallow, House Martin, Sand Martin, Crag Martin, Red-rumped Swallow …\nSwap the family for /Accipitridae (hawks & eagles), /Anatidae (ducks & geese), /Laridae (gulls & terns) to browse another branch.",
       "q": "PREFIX rdfs: <http://www.w3.org/2000/01/rdf-schema#>\nPREFIX dwc: <http://rs.tdwg.org/dwc/terms/>\nPREFIX v: <https://w3id.org/rete/gbif/vocab#>\nSELECT ?name WHERE {\n  ?sp rdfs:subClassOf+ <https://w3id.org/rete/gbif/taxon/family/Hirundinidae> ;\n      rdfs:label ?name ; dwc:taxonRank \"species\" .\n} ORDER BY ?name"},
      {"family": "Select", "label": "A species' records: House Sparrow, with country & year", "view": "table", "cols": {"occ": "Occurrence", "country": "Country", "year": "Year"},
       "tip": "The raw records for one species (House Sparrow, Passer domesticus): each ?occ IRI round-trips to its page on gbif.org/occurrence/<id>, with the country and year of the sighting. Bounded by species + LIMIT, so it's a cheap lazy read.\nMost records are HUMAN_OBSERVATION — citizen-science checklists (eBird and national schemes) dominate bird data.",
       "q": "PREFIX rdfs: <http://www.w3.org/2000/01/rdf-schema#>\nPREFIX dct: <http://purl.org/dc/terms/>\nPREFIX v: <https://w3id.org/rete/gbif/vocab#>\nPREFIX dwc: <http://rs.tdwg.org/dwc/terms/>\nSELECT ?occ ?country ?year WHERE {\n  ?occ a <https://w3id.org/rete/gbif/taxon/species/5231190> ;\n       dct:spatial ?c ; dwc:year ?year .\n  ?c rdfs:label ?country .\n} LIMIT 100"},
      {"family": "Select", "label": "🗺️ Map the raptors: where are the Accipitridae?", "view": "map", "cols": {"wkt": "Location"},
       "tip": "A taxonomy-driven map: all occurrences whose species is (transitively) in the hawk-and-eagle family Accipitridae — hitting the pyramid's Family level, then joining to points. This crosses the taxonomy INTO the geography. Heavier than a single-species map (it fans out over many species), so it's LIMITed; give it a moment on a cold cache.\nThis is the 'ask at the Family level, get a map' pattern — no need to name every eagle and harrier species.",
       "q": "PREFIX rdfs: <http://www.w3.org/2000/01/rdf-schema#>\nPREFIX geo: <http://www.opengis.net/ont/geosparql#>\nSELECT ?wkt WHERE {\n  ?sp rdfs:subClassOf+ <https://w3id.org/rete/gbif/taxon/family/Accipitridae> .\n  ?o a ?sp ; geo:asWKT ?wkt .\n} LIMIT 3000"},
      {"family": "Select", "label": "One sighting, the full record", "view": "table", "cols": {"p": "Property", "o": "Value"},
       "tip": "Everything the graph knows about a single occurrence — its species type, GeoSPARQL point, country, year, basis-of-record and source dataset. The subject IRI is the live GBIF occurrence page.\nSwap the occurrence IRI for any ?occ you get from the other examples.",
       "q": "SELECT ?p ?o WHERE {\n  <https://www.gbif.org/occurrence/1038154182> ?p ?o .\n}"},
      {"family": "Aggregate", "label": "Taxonomic richness: which genera have the most species?", "view": "table", "cols": {"genus": "Genus", "n": "Species"},
       "tip": "A pure-taxonomy question, answered over the small T-Box (the ~13k subClassOf/label triples) — NOT the 50M records, so it's cheap. Counts how many species each genus contains: Emberiza (buntings) 21, Anas (dabbling ducks) 18, Calidris (sandpipers) 18, Sylvia (typical warblers) 18 … the most speciose bird genera recorded in Spain + Switzerland.\nThis is the ontology talking, not the observations — the pyramid's raw material.",
       "q": "PREFIX rdfs: <http://www.w3.org/2000/01/rdf-schema#>\nPREFIX dwc: <http://rs.tdwg.org/dwc/terms/>\nPREFIX v: <https://w3id.org/rete/gbif/vocab#>\nSELECT ?genus (COUNT(?sp) AS ?n) WHERE {\n  ?sp dwc:taxonRank \"species\" ; rdfs:subClassOf ?g .\n  ?g rdfs:label ?genus ; dwc:taxonRank \"genus\" .\n}\nGROUP BY ?genus ORDER BY DESC(?n) LIMIT 25"},
      {"family": "Select", "label": "🗺️ Map the Bearded Vulture — Alps ↔ Pyrenees", "view": "map", "cols": {"wkt": "Location"},
       "tip": "The Bearded Vulture (Gypaetus barbatus, gbif.org/species/2480649) — one of Europe's rarest raptors, its bone-eating lammergeier — maps beautifully across BOTH countries: a native Pyrenean stronghold in Spain and the famous Alpine reintroduction in Switzerland. Bounded to one species, so it's a cheap lazy map.\nTry 2481912 (White Stork), 4352332 (Greater Flamingo), 2481047 (Peregrine Falcon) for other icons.",
       "q": "PREFIX geo: <http://www.opengis.net/ont/geosparql#>\nSELECT ?wkt WHERE {\n  ?o a <https://w3id.org/rete/gbif/taxon/species/2480649> ;\n     geo:asWKT ?wkt .\n} LIMIT 4000"},
      {"family": "Aggregate", "label": "📈 White Stork sightings by year", "view": "time", "cols": {"year": "Year", "n": "Sightings"},
       "tip": "The White Stork (Ciconia ciconia) recorded per year — switch Output to Time to see the curve. Bounded to one species, and the count climbs steeply with the rise of citizen-science checklists (~52k in 2024). A window onto observer effort as much as bird numbers.\nSwap the species key to chart any bird's recording history; the low current-year bar is just the year-to-date.",
       "q": "PREFIX v: <https://w3id.org/rete/gbif/vocab#>\nPREFIX dwc: <http://rs.tdwg.org/dwc/terms/>\nSELECT ?year (COUNT(?o) AS ?n) WHERE {\n  ?o a <https://w3id.org/rete/gbif/taxon/species/2481912> ; dwc:year ?year .\n}\nGROUP BY ?year ORDER BY ?year"},
      {"family": "Aggregate", "label": "How is a species recorded? (basis of record)", "view": "table", "cols": {"basis": "Basis of record", "n": "Count"},
       "tip": "Where the data comes from, for one species (Blue Tit, Cyanistes caeruleus): HUMAN_OBSERVATION (a birdwatcher's checklist) dwarfs PRESERVED_SPECIMEN (a museum skin), LIVING_SPECIMEN (a ringed/zoo bird) and MACHINE_OBSERVATION (a camera/acoustic sensor). Bird data is overwhelmingly citizen science.\nMuseum-heavy groups look very different — try an insect or a plant species in the full-taxa build if we ship one.",
       "q": "PREFIX dwc: <http://rs.tdwg.org/dwc/terms/>\nSELECT ?basis (COUNT(?o) AS ?n) WHERE {\n  ?o a <https://w3id.org/rete/gbif/taxon/species/2487879> ; dwc:basisOfRecord ?basis .\n}\nGROUP BY ?basis ORDER BY DESC(?n)"},
      {"family": "Select", "label": "🗺️ Map a whole genus: the falcons (Falco)", "view": "map", "cols": {"wkt": "Location"},
       "tip": "Taxonomy → geography at the GENUS level: every falcon (genus Falco — Kestrel, Peregrine, Hobby, Merlin, Eleonora's …) mapped in one query via rdfs:subClassOf+, no need to name each species. Fans out over several species, so it's LIMITed and a touch heavier than a single-species map.\nSwap /genus/Falco for /genus/Aquila (true eagles), /genus/Larus (large gulls), /genus/Anas (dabbling ducks).",
       "q": "PREFIX rdfs: <http://www.w3.org/2000/01/rdf-schema#>\nPREFIX geo: <http://www.opengis.net/ont/geosparql#>\nSELECT ?wkt WHERE {\n  ?sp rdfs:subClassOf+ <https://w3id.org/rete/gbif/taxon/genus/Falco> .\n  ?o a ?sp ; geo:asWKT ?wkt .\n} LIMIT 3000"},
      {"family": "Aggregate", "label": "The bird orders, by species richness", "view": "table", "cols": {"order": "Order", "n": "Species"},
       "tip": "Every bird Order and how many species it holds, over the T-Box only (cheap): the perching birds Passeriformes dominate with 410 species, then Charadriiformes (waders & gulls) 148, Anseriformes (wildfowl) 123 — and Psittaciformes (parrots) 60, a telltale sign of escaped/feral cage-birds now breeding in Spain (Monk & Rose-ringed Parakeets).\nThis is level 3 of the schema pyramid as a query; the Schema tab shows the same rollup weighted by sightings instead of species.",
       "q": "PREFIX rdfs: <http://www.w3.org/2000/01/rdf-schema#>\nPREFIX dwc: <http://rs.tdwg.org/dwc/terms/>\nPREFIX v: <https://w3id.org/rete/gbif/vocab#>\nSELECT ?order (COUNT(DISTINCT ?sp) AS ?n) WHERE {\n  ?sp dwc:taxonRank \"species\" ; rdfs:subClassOf+ ?o .\n  ?o dwc:taxonRank \"order\" ; rdfs:label ?order .\n}\nGROUP BY ?order ORDER BY DESC(?n)"},
      {"family": "Select", "label": "Genera in a family: the ducks & geese (Anatidae)", "view": "table", "cols": {"genus": "Genus"},
       "tip": "One step of the taxonomy: the genera directly inside family Anatidae (wildfowl) — Aix, Anas (dabbling ducks), Anser (grey geese), Aythya (diving ducks), Branta, Mergus, Tadorna … the shape of a family at a glance.\nSwap /family/Anatidae for /family/Muscicapidae (chats & flycatchers), /family/Laridae (gulls), /family/Fringillidae (finches).",
       "q": "PREFIX rdfs: <http://www.w3.org/2000/01/rdf-schema#>\nPREFIX dwc: <http://rs.tdwg.org/dwc/terms/>\nPREFIX v: <https://w3id.org/rete/gbif/vocab#>\nSELECT ?genus WHERE {\n  ?g rdfs:subClassOf <https://w3id.org/rete/gbif/taxon/family/Anatidae> ;\n     dwc:taxonRank \"genus\" ; rdfs:label ?genus .\n}\nORDER BY ?genus"},
      {"family": "Select", "label": "🗺️ Flamingos in Spain (Doñana, Ebro delta, Fuente de Piedra)", "view": "map", "cols": {"wkt": "Location"},
       "tip": "A species map narrowed to ONE country: Greater Flamingo (Phoenicopterus roseus) sightings in Spain only — the great saline wetlands light up (the Guadalquivir marshes at Doñana, the Ebro delta, the Fuente de Piedra lagoon). Bounded by species AND v:inCountry, so it's a cheap, targeted lazy read.\nSwitch the country to /country/CH for the handful of Swiss records, or drop the country line to map both.",
       "q": "PREFIX geo: <http://www.opengis.net/ont/geosparql#>\nPREFIX dct: <http://purl.org/dc/terms/>\nPREFIX v: <https://w3id.org/rete/gbif/vocab#>\nSELECT ?wkt WHERE {\n  ?o a <https://w3id.org/rete/gbif/taxon/species/4352332> ;\n     geo:asWKT ?wkt ; dct:spatial <https://w3id.org/rete/gbif/country/ES> .\n} LIMIT 4000"},
      {"family": "Construct", "label": "🌳 The owl family tree (graph)", "view": "graph",
       "tip": "The taxonomy rendered as a NODE-LINK graph — switch Output to Graph. rdfs:subClassOf+ gathers everything under the owl family Strigidae, then the CONSTRUCT rebuilds the tree: family → genera (Athene, Asio, Bubo, Strix, Otus …) → species. A living picture of a clade, all from the T-Box.\nSwap /family/Strigidae for /order/Anseriformes (a bigger tree), /family/Accipitridae (hawks & eagles), or any taxon node.",
       "q": "PREFIX rdfs: <http://www.w3.org/2000/01/rdf-schema#>\nCONSTRUCT {\n  ?child rdfs:subClassOf ?parent . ?child rdfs:label ?cl . ?parent rdfs:label ?pl\n} WHERE {\n  ?child rdfs:subClassOf+ <https://w3id.org/rete/gbif/taxon/family/Strigidae> ;\n         rdfs:subClassOf ?parent ; rdfs:label ?cl .\n  ?parent rdfs:label ?pl .\n}"}
    ],
    "bcul": [
      {"family": "Select", "label": "🏛️ What's held at the Cantonal Archives?", "view": "table", "cols": {"title": "Title", "loc": "Shelving location", "call": "Call no.", "avail": "Availability"}, "tip": "Every book resolves to WHERE it physically lives. This lists items held at the Archives cantonales vaudoises with shelving location, call number and live availability.\nSwap \"Archives cantonales\" for \"BCUL site Riponne\", \"Musée des beaux-arts\", \"BCUL site Unithèque\", … to see any of the ~dozens of Vaud sites.", "q": "PREFIX schema: <http://schema.org/>\nPREFIX b: <https://data.bcu-lausanne.ch/>\nSELECT ?title ?loc ?call ?avail WHERE {\n  ?lib schema:name \"Archives cantonales\" .\n  ?it b:heldAt ?lib ;\n      schema:name ?title ;\n      b:shelvingLocation ?loc ;\n      b:callNumber ?call ;\n      b:availability ?avail .\n} LIMIT 100"},
      {"family": "Aggregate", "label": "🚪 Sections of the Unithèque — the open-access corridors", "view": "table", "cols": {"section": "Section (shelving location)", "items": "# items"}, "tip": "BCU's main site is shelved by subject in open-access corridors: religion (31k), histoire (25k), art (23k), français, antiquité … plus the closed \"magasins\" stacks.\nEach ?section is a physical department you can then walk (see the shelf-walk query). Change the library name for another site.", "q": "PREFIX schema: <http://schema.org/>\nPREFIX b: <https://data.bcu-lausanne.ch/>\nSELECT ?section (COUNT(?it) AS ?items) WHERE {\n  ?lib schema:name \"BCUL site Unithèque\" .\n  ?it b:heldAt ?lib ;\n      b:shelvingLocation ?section .\n} GROUP BY ?section ORDER BY DESC(?items) LIMIT 25"},
      {"family": "Select", "label": "🚶 Walk a shelf — a run of the closed stacks (numerus currens)", "view": "table", "cols": {"call": "Call no. (shelf order)", "title": "Book", "date": "Year"}, "tip": "BCU's magasins shelve by accession number, so consecutive call numbers sit physically side by side. This reconstructs one shelf-run at the Unithèque — books that literally neighbour each other, whatever their subject.\nChange \"USA 108\" to walk a different run.", "q": "PREFIX schema: <http://schema.org/>\nPREFIX b: <https://data.bcu-lausanne.ch/>\nSELECT ?call ?title ?date WHERE {\n  ?it b:callNumber ?call ;\n      schema:name ?title .\n  FILTER(STRSTARTS(?call, \"USA 108\"))\n  OPTIONAL { ?it schema:startDate ?date }\n} ORDER BY ?call LIMIT 60"},
      {"family": "Select", "label": "🖼️ Manuscript facsimiles — folio, IIIF viewer, origin & language", "view": "table", "cols": {"thumb": "Folio", "title": "Manuscript", "date": "From", "place": "Origin", "lang": "Language", "manifest": "IIIF"}, "tip": "The 15 e-codices manuscripts as a media table: ?thumb shows a folio image and ?manifest opens the page-turning IIIF viewer — with where each was made (Iordanes' Getica, Fulda c. 800) and its language.\nThe .jpg renders as an image and the manifest as a IIIF cell automatically.", "q": "PREFIX schema: <http://schema.org/>\nPREFIX b: <https://data.bcu-lausanne.ch/>\nPREFIX rdfs: <http://www.w3.org/2000/01/rdf-schema#>\nSELECT ?thumb ?title ?date ?place ?lang ?manifest WHERE {\n  ?m a schema:Manuscript ;\n     schema:thumbnailUrl ?thumb ;\n     schema:name ?title ;\n     schema:startDate ?date ;\n     b:iiifManifest ?manifest .\n  OPTIONAL { ?m schema:spatialCoverage ?pl . ?pl schema:name ?place }\n  OPTIONAL { ?m schema:inLanguage ?lg . ?lg rdfs:label ?lang }\n} ORDER BY ?date"},
      {"family": "Select", "label": "📷 Digitized-heritage gallery — cover, author, year, open ↗", "view": "table", "cols": {"thumb": "Cover", "title": "Title", "creator": "Author", "date": "Year", "url": "Open ↗"}, "tip": "A visual gallery of Patrinum's digitized rare books: ?thumb is a real page-scan WebP-mirrored to the graph's own CDN (schema:image) so it always renders, and ?url opens the full object on patrinum.ch (e.g. an 1805 Histoire du major Davel, a 1678 Traitté du jeu royal des échets).\nCovers render inline automatically. ~746 items carry a hosted scan; the rest link out.", "q": "PREFIX schema: <http://schema.org/>\nPREFIX b: <https://data.bcu-lausanne.ch/>\nSELECT ?thumb ?title ?creator ?date ?url WHERE {\n  ?it schema:isPartOf <https://data.bcu-lausanne.ch/source/patrinum> ;\n      schema:image ?thumb ;\n      schema:name ?title ;\n      schema:url ?url ;\n      schema:startDate ?date .\n  OPTIONAL { ?it schema:creator ?a . ?a schema:name ?creator }\n} ORDER BY ?date LIMIT 60"},
      {"family": "Select", "label": "💎 Treasures: the oldest printed books, and where they're kept", "view": "table", "cols": {"title": "Title", "date": "Printed", "lib": "Held at"}, "tip": "The incunabula and early imprints, oldest first — a 1457 Gutenberg leaf, Cicero printed at Subiaco (1465), Livy (1469).\n?lib shows which site (or e-resource) holds each; raise the FILTER year to browse later periods.", "q": "PREFIX schema: <http://schema.org/>\nPREFIX b: <https://data.bcu-lausanne.ch/>\nSELECT ?title ?date ?lib WHERE {\n  ?w a schema:Book ;\n     schema:name ?title ;\n     schema:startDate ?date ;\n     b:heldAt ?h .\n  ?h schema:name ?lib .\n  FILTER(?date > 1450)\n} ORDER BY ?date LIMIT 50"},
      {"family": "Aggregate", "label": "👥 Most prolific authors & bodies in the collection", "view": "table", "cols": {"name": "Author / body", "n": "# works"}, "tip": "The twin links every work to shared author / organisation nodes, so you can rank them: the OECD, IMF and UN dominate the modern catalogue, with the CTLO (medieval Latin) close behind.\nAn aggregate over ~2.8M creator edges.", "q": "PREFIX schema: <http://schema.org/>\nPREFIX b: <https://data.bcu-lausanne.ch/>\nSELECT ?name (COUNT(?w) AS ?n) WHERE {\n  ?w schema:creator ?a .\n  ?a schema:name ?name .\n} GROUP BY ?name ORDER BY DESC(?n) LIMIT 25"},
      {"family": "Aggregate", "label": "📊 The whole Vaud network, by holdings size", "view": "table", "cols": {"library": "Library", "items": "# items"}, "tip": "How many catalogued items each site holds — the location backbone of the twin: BCUL site Unithèque (~2M), Riponne, Internef, then faculty / museum / gymnasium libraries.\nAn aggregate over ~4.9M holdings edges (give it a moment over the network).", "q": "PREFIX schema: <http://schema.org/>\nPREFIX b: <https://data.bcu-lausanne.ch/>\nSELECT ?library (COUNT(?it) AS ?items) WHERE {\n  ?it b:heldAt ?lib .\n  ?lib schema:name ?library .\n} GROUP BY ?library ORDER BY DESC(?items) LIMIT 25"},
      {"family": "Select", "label": "📰 Digitized newspaper & press titles (Scriptorium)", "view": "table", "cols": {"title": "Title"}, "tip": "The 331 digitized press titles on Scriptorium — 24 Heures, the Feuille d'avis de Lausanne (from 1762), the Almanach de Lausanne.\nIssues in the twin span 1738–2022.", "q": "PREFIX schema: <http://schema.org/>\nSELECT DISTINCT ?title WHERE {\n  ?s schema:isPartOf <https://data.bcu-lausanne.ch/source/scriptorium> ;\n     a schema:Periodical ;\n     schema:name ?title .\n} LIMIT 60"},
      {"family": "Select", "label": "🔎 What's next to it on the shelf? (a book's neighbours)", "view": "table", "cols": {"call": "Call no.", "neighbour": "Neighbour on the shelf"}, "tip": "Pick a book, see what physically sits beside it. Anchored on \"Foundations for moral relativism\", this walks its shelf-run in the Unithèque magasin — where accession-order shelving makes neighbours gloriously random (Roman Egypt, the 1917 Revolution, medieval Japan, Cold-War Prague …).\nChange the title to any book; on an open-access subject shelf the neighbours come out same-topic instead.", "q": "PREFIX schema: <http://schema.org/>\nPREFIX b: <https://data.bcu-lausanne.ch/>\nSELECT DISTINCT ?call ?neighbour WHERE {\n  ?anchor schema:name \"Foundations for moral relativism\" ;\n          b:callNumber ?c0 ; b:heldAt ?lib .\n  ?it b:heldAt ?lib ;\n      b:callNumber ?call ;\n      schema:name ?neighbour .\n  FILTER(?call >= ?c0)\n} ORDER BY ?call LIMIT 25"},
      {"family": "Select", "label": "🖼️ Five kinds of cell at once — folio · IIIF · century · year · open ↗", "view": "table", "cols": {"thumb": "Folio", "manifest": "IIIF viewer", "title": "Manuscript", "century": "Dated", "year": "From", "record": "e-codices ↗"}, "tip": "One table, five distinct cell types: ?thumb renders a folio image, ?manifest opens the page-turning IIIF viewer, ?century & ?title are text, ?year is a date, and ?record links out to e-codices. The 15 medieval manuscripts BCU has digitized, oldest first.\nTop row is Jordanes' Getica (Fulda, ca. 830); each ?record opens the manuscript on e-codices.ch.", "q": "PREFIX schema: <http://schema.org/>\nPREFIX b: <https://data.bcu-lausanne.ch/>\nPREFIX dct: <http://purl.org/dc/terms/>\nSELECT ?thumb ?manifest ?title ?century ?year ?record WHERE {\n  ?w b:iiifManifest ?manifest ;\n     schema:thumbnailUrl ?thumb ;\n     schema:name ?title ;\n     schema:url ?record .\n  OPTIONAL { ?w dct:date ?century }\n  OPTIONAL { ?w schema:startDate ?year }\n} ORDER BY ?year"}
    ],
    "fuero_juzgo": [
      {"family": "Summary", "label": "What's in this corpus", "view": "table",
       "tip": "The shape of the Fuero Juzgo dataset, counted by fjo: class: 49 Manuscripts + 3 Fragments (the 46 PhiloBiblon witnesses, the Galician López Ferreiro fragment, 3 BOOST-only mss and the 2 Catalan Libre-jutge fragments), 12 Editions, 11 Studies, 3 Works (the Fuero Juzgo, its Latin source the Liber Iudiciorum, and the Catalan Libre jutge), plus the People, Institutions and Places that link them.\nOne row per class.",
       "q": "PREFIX fjo: <https://w3id.org/fuero-juzgo/ontology#>\nPREFIX rdfs: <http://www.w3.org/2000/01/rdf-schema#>\nSELECT ?kind (COUNT(?s) AS ?n) WHERE {\n  ?s a ?c . ?c rdfs:label ?kind .\n  VALUES ?c { fjo:Manuscript fjo:Fragment fjo:Edition fjo:Study fjo:LegalText fjo:Person fjo:Institution fjo:Place }\n}\nGROUP BY ?kind\nORDER BY DESC(?n)"},
      {"family": "Select", "label": "📜 Manuscripts you can SEE (IIIF facsimiles)", "view": "table", "cols": {"ms": "Manuscript", "shelf": "Shelfmark", "img": "Facsimile", "iiif": "IIIF viewer"},
       "tip": "The 12 digitized witnesses that carry a IIIF manifest (fjo:iiifManifest): both Munich BSB codices (Cod.hisp. 28 & 6), the BnF Espagnol 256 (Gallica), the Bodleian Holkham misc. 46, and all 8 Escorial codices (Real Biblioteca Digital). The Facsimile column (fjo:image) renders a folio inline; click a IIIF cell for the page-turning viewer.\nEvery facsimile is MIRRORED into our own R2 (all 3,223 full-res pages under fuero_juzgo/iiif/) with a self-hosted IIIF manifest, so it renders and downloads without depending on the source servers; fjo:sourceManifest keeps a link to the original. BSB Cod.hisp. 28 is CC Public Domain Mark.",
       "q": "PREFIX fjo: <https://w3id.org/fuero-juzgo/ontology#>\nPREFIX rdfs: <http://www.w3.org/2000/01/rdf-schema#>\nSELECT ?ms ?shelf ?img ?iiif WHERE {\n  ?w fjo:iiifManifest ?iiif ; fjo:image ?img ; fjo:siglum ?shelf ; rdfs:label ?ms .\n}\nORDER BY ?ms"},
      {"family": "Aggregate", "label": "The tradition over time (witnesses per century)", "view": "table",
       "tip": "Every witness carries an integer date range (fjo:notBefore / fjo:notAfter), so fjo:century buckets the 52 testimonies: 2 in the 12th c. (the Catalan fragments), 26 in the 13th (the core of the surviving tradition, copied within a generation of the c. 1260 translation), 11 in the 14th, then a long tail of late copies down to the 18th-c. antiquarian apographs.\nSwitch Output to a bar chart to see the 13th-century peak.",
       "q": "PREFIX fjo: <https://w3id.org/fuero-juzgo/ontology#>\nSELECT ?century (COUNT(?w) AS ?witnesses) WHERE {\n  ?w fjo:witnessOf ?work ; fjo:century ?century .\n}\nGROUP BY ?century\nORDER BY ?century"},
      {"family": "Select", "label": "The oldest witnesses & their dialect", "view": "table", "cols": {"witness": "Witness", "date": "Date", "dialect": "Dialect / scripta", "holder": "Held by"},
       "tip": "The 13th-century-or-earlier witnesses, with the linguistic characterisation editors give them (fjo:languageVariety): the tradition is a dialect map of the medieval north-west - leonés occidental / centro-occidental / oriental, asturiano (the Munich Cod.hisp. 28), and castellano. The Códice Murciano (1288) is castellano copied from a strongly Leonese model.\nSorted earliest-first; 'Held by' is the current holding library.",
       "q": "PREFIX fjo: <https://w3id.org/fuero-juzgo/ontology#>\nPREFIX rdfs: <http://www.w3.org/2000/01/rdf-schema#>\nSELECT ?witness ?date ?dialect ?holder WHERE {\n  ?w fjo:notBefore ?nb ; rdfs:label ?witness ; fjo:dateText ?date ; fjo:heldBy ?h .\n  ?h rdfs:label ?holder .\n  OPTIONAL { ?w fjo:languageVariety ?dialect }\n  FILTER(?nb <= 1300)\n}\nORDER BY ?nb\nLIMIT 30"},
      {"family": "Path", "label": "The manuscript stemma (copies of copies)", "view": "table", "cols": {"copy": "Later copy", "copyDate": "Its date", "exemplar": "Copied from", "exDate": "Exemplar's date"},
       "tip": "fjo:copyOf reconstructs the transmission: the 18th-century antiquarian copies made for the RAE academic edition trace back to their medieval exemplars - BNE 683 (Santiago y Palomares, 1755, corrected by the Jesuit Burriel) is a copy of the Códice Murciano (1288), and BNE 1681 (1762-64) copies the Escorial codices P.II.17 and d.III.18.\nThree copy-of links; the exemplar is always centuries older than the copy.",
       "q": "PREFIX fjo: <https://w3id.org/fuero-juzgo/ontology#>\nPREFIX rdfs: <http://www.w3.org/2000/01/rdf-schema#>\nSELECT ?copy ?copyDate ?exemplar ?exDate WHERE {\n  ?c fjo:copyOf ?e ; rdfs:label ?copy ; fjo:dateText ?copyDate .\n  ?e rdfs:label ?exemplar ; fjo:dateText ?exDate .\n}"},
      {"family": "Path", "label": "Critical editions ↔ the codices they establish", "view": "table", "cols": {"edition": "Edition", "codex": "Establishes the codex", "shelf": "Shelfmark"},
       "tip": "fjo:editsWitness links each modern edition to the exact manuscript it edits: Mencé-Caster (1996) -> Escorial Z.III.6, Orazi (1997) -> Z.III.21, Perona et al. (2002) -> the Códice Murciano, Romero Cambrón (2016) -> the Bodleian Holkham misc. 46, and the RAE academic edition (1815, really 1817) collates three Escorial codices (Z.III.18, M.II.18, d.III.18).\nThe scholarly editions, joined to their base witnesses.",
       "q": "PREFIX fjo: <https://w3id.org/fuero-juzgo/ontology#>\nPREFIX rdfs: <http://www.w3.org/2000/01/rdf-schema#>\nSELECT ?edition ?codex ?shelf WHERE {\n  ?ed fjo:editsWitness ?w ; rdfs:label ?edition .\n  ?w rdfs:label ?codex ; fjo:siglum ?shelf .\n}\nORDER BY ?edition"},
      {"family": "Select", "label": "Search the prologues & colophons", "view": "table", "cols": {"witness": "Witness", "prologue": "Prologue incipit"},
       "tip": "Full-text search (backed by the .rete text index) over the incipits, prologues and scribal colophons transcribed from PhiloBiblon. Here, the witnesses whose prologue names King Sisenando (the Visigothic king in whose reign the IV Council of Toledo met, invoked at the head of the code) - the shared 'Con cuidado del amor de Cristo... de don Sisnando' opening that threads through the tradition.\nChange 'sisnando' to 'godos', 'iudizios' or a scribe's name to trace a formula across the corpus.",
       "q": "PREFIX fjo: <https://w3id.org/fuero-juzgo/ontology#>\nPREFIX rdfs: <http://www.w3.org/2000/01/rdf-schema#>\nSELECT ?witness ?prologue WHERE {\n  ?w fjo:prologueIncipit ?prologue ; rdfs:label ?witness .\n  FILTER(CONTAINS(LCASE(STR(?prologue)), \"sisnando\"))\n}"},
      {"family": "Select", "label": "🔗 Linked out: witnesses also in Biblissima", "view": "table", "cols": {"witness": "Witness", "biblissima": "Biblissima entity", "iiif": "IIIF"},
       "tip": "Two witnesses carry owl:sameAs into Biblissima+ (data.biblissima.fr), the French manuscript-and-IIIF aggregator: the BnF Espagnol 256 (Q44277) and the Munich BSB Cod.hisp. 6 (Q264558). This is the bridge that lets the graph FEDERATE with the playground's `biblissima` dataset - the same manuscript, described from two catalogues.\nEvery digitized witness is also reachable through its IIIF manifest, which Biblissima indexes.",
       "q": "PREFIX fjo: <https://w3id.org/fuero-juzgo/ontology#>\nPREFIX owl: <http://www.w3.org/2002/07/owl#>\nPREFIX rdfs: <http://www.w3.org/2000/01/rdf-schema#>\nSELECT ?witness ?biblissima ?iiif WHERE {\n  ?w owl:sameAs ?biblissima ; fjo:iiifManifest ?iiif ; rdfs:label ?witness .\n  FILTER(STRSTARTS(STR(?biblissima), \"https://data.biblissima.fr/\"))\n}"},
      {"family": "Path", "label": "Cross-source JOIN: the libraries' countries (live Wikidata)", "view": "table", "fed": [{"endpoint": "https://query.wikidata.org/sparql", "label": "Wikidata"}], "cols": {"library": "Holding library", "country": "Country (via Wikidata)"},
       "tip": "ONE query split across TWO sources and joined on the shared ?wd. This file answers 'which library holds a witness' and gives it an owl:sameAs Wikidata id; LIVE Wikidata answers wdt:P17 (the library's country). The playground routes each pattern to the source that can answer it, then bound-joins - so you see the international diaspora of the tradition's custodians (Germany, France, Spain, United Kingdom, United States, Sweden, Denmark, Portugal).\nWikidata is pre-added as a live endpoint (see the Sources strip). Hit 🔍 Plan to see the routing.",
       "q": "PREFIX fjo: <https://w3id.org/fuero-juzgo/ontology#>\nPREFIX owl: <http://www.w3.org/2002/07/owl#>\nPREFIX rdfs: <http://www.w3.org/2000/01/rdf-schema#>\nPREFIX wdt: <http://www.wikidata.org/prop/direct/>\nSELECT DISTINCT ?library ?country WHERE {\n  ?w fjo:heldBy ?lib .\n  ?lib owl:sameAs ?wd ; rdfs:label ?library .\n  ?wd wdt:P17 ?c . ?c rdfs:label ?country .\n  FILTER(LANG(?country) = \"en\")\n}\nORDER BY ?country"},
      {"family": "Path", "label": "🌍 The holding libraries on a live-Wikidata map", "view": "map", "fed": [{"endpoint": "https://query.wikidata.org/sparql", "label": "Wikidata"}],
       "tip": "The same cross-source join, but asking Wikidata for wdt:P625 (coordinate location) instead of the country - and rendered on the map. Each pin is a library that holds a Fuero Juzgo witness, placed by live Wikidata coordinates: Munich, Paris, Oxford, Madrid, El Escorial, Lisbon, Stockholm, Copenhagen, Washington, New York.\nSwitch Output to Table to read the raw WKT; the geography IS the reception history of the code.",
       "q": "PREFIX fjo: <https://w3id.org/fuero-juzgo/ontology#>\nPREFIX owl: <http://www.w3.org/2002/07/owl#>\nPREFIX rdfs: <http://www.w3.org/2000/01/rdf-schema#>\nPREFIX wdt: <http://www.wikidata.org/prop/direct/>\nSELECT DISTINCT ?library ?wkt WHERE {\n  ?w fjo:heldBy ?lib .\n  ?lib owl:sameAs ?wd ; rdfs:label ?library .\n  ?wd wdt:P625 ?wkt .\n}"}
    ],
    "ustc": [
      {"family": "Select", "label": "Digitised editions \u2014 cover + IIIF", "view": "table",
       "tip": "USTC hosts no images itself, but ~40% of these editions link to a digitised copy at an open IIIF library (Bavarian State Library / MDZ, Gallica, ONB Austria, Google Books). A cover thumbnail is resolved deterministically from that link (schema:image, renders inline) plus a link to the full IIIF manifest \u2014 here the Rosicrucian, Kabbalah and Hermetic title-pages. Providers like ProQuest/EEBO are subscription-gated, so they stay as plain out-links.",
       "q": "PREFIX schema: <http://schema.org/>\nPREFIX rdfs: <http://www.w3.org/2000/01/rdf-schema#>\nPREFIX u: <https://www.ustc.ac.uk/ontology#>\nSELECT ?cover ?title ?year ?manifest WHERE {\n  ?e schema:image ?cover ; rdfs:label ?title ; schema:datePublished ?year .\n  OPTIONAL { ?e u:iiifManifest ?manifest }\n}\nLIMIT 40"},
      {"family": "Summary", "label": "What's in this USTC subset", "view": "table",
       "tip": "The shape of the extract: 727 early-modern Editions, 10,779 library Copies (physical holdings), 1,146 Libraries, and ~1,226 Persons (authors + printers). One count per rdf:type. This is only the 727 editions the Embassy/BPH collection cites - a fair-use slice of USTC's ~1M records.",
       "q": "PREFIX rdf: <http://www.w3.org/1999/02/22-rdf-syntax-ns#>\nSELECT ?type (COUNT(?s) AS ?count) WHERE {\n  ?s rdf:type ?type\n}\nGROUP BY ?type\nORDER BY DESC(?count)"},
      {"family": "Aggregate", "label": "Which libraries hold these books", "view": "table",
       "tip": "Counts holdings per library (by institution + city, so same-named 'Universit\u00e4tsbibliothek' entries stay distinct). The great early-modern collections surface: Herzog August Bibliothek (Wolfenb\u00fcttel), the British Library, Bayerische Staatsbibliothek (M\u00fcnchen), Staatsbibliothek zu Berlin.",
       "q": "PREFIX u: <https://www.ustc.ac.uk/ontology#>\nPREFIX rdfs: <http://www.w3.org/2000/01/rdf-schema#>\nSELECT ?library ?city (COUNT(?e) AS ?editions) WHERE {\n  ?e u:heldBy ?l . ?l rdfs:label ?library ; u:city ?city\n}\nGROUP BY ?l ?library ?city\nORDER BY DESC(?editions)\nLIMIT 25"},
      {"family": "Aggregate", "label": "Books by USTC classification", "view": "table",
       "tip": "The subject make-up of the Embassy's cited editions in USTC's own classification scheme - Religious (313), Science, Medical Texts, Philosophy and Morality, and the tell-tale 'Witchcraft and Demonology'. Count of editions per dcterms:subject.",
       "q": "PREFIX dct: <http://purl.org/dc/terms/>\nPREFIX skos: <http://www.w3.org/2004/02/skos/core#>\nSELECT ?classification (COUNT(?e) AS ?editions) WHERE {\n  ?e dct:subject ?c . ?c skos:prefLabel ?classification\n}\nGROUP BY ?classification\nORDER BY DESC(?editions)"},
      {"family": "Select", "label": "Editions with a digital copy", "view": "table",
       "tip": "USTC records where a digitisation is available online - the title plus the deep link (Google Books, ProQuest Early European Books, Cervantes Virtual...). 514 of the 727 editions have at least one. The URL column is clickable.",
       "q": "PREFIX u: <https://www.ustc.ac.uk/ontology#>\nPREFIX rdfs: <http://www.w3.org/2000/01/rdf-schema#>\nSELECT ?title ?provider ?url WHERE {\n  ?e u:digitalCopy ?url ; rdfs:label ?title ; u:digitisationProvider ?provider\n}\nLIMIT 50"},
      {"family": "Aggregate", "label": "The busiest printers", "view": "table",
       "tip": "Editions per printer/press (skipping 's.n.' = sine nomine, printer unknown). This is a who's-who of the early-modern alchemical trade: Lukas Jennis and the heirs of Lazarus Zetzner (both Frankfurt/Strasbourg) printed much of the Rosicrucian and Hermetic corpus.",
       "q": "PREFIX u: <https://www.ustc.ac.uk/ontology#>\nPREFIX rdfs: <http://www.w3.org/2000/01/rdf-schema#>\nSELECT ?printer (COUNT(?e) AS ?editions) WHERE {\n  ?e u:printer ?p . ?p rdfs:label ?printer\n  FILTER(?printer != \"s.n.\")\n}\nGROUP BY ?printer\nORDER BY DESC(?editions)\nLIMIT 25"},
      {"family": "Select", "label": "The oldest editions", "view": "table",
       "tip": "Sorted by year ascending - the earliest imprints in the Embassy's cited set, each with its place of printing and format (2\u00b0 folio, 4\u00b0 quarto, 8\u00b0 octavo...). schema:datePublished is a typed xsd:gYear.",
       "q": "PREFIX u: <https://www.ustc.ac.uk/ontology#>\nPREFIX rdfs: <http://www.w3.org/2000/01/rdf-schema#>\nPREFIX schema: <http://schema.org/>\nSELECT ?year ?title ?place ?format WHERE {\n  ?e a u:Edition ; schema:datePublished ?year ; rdfs:label ?title ;\n     u:place ?place ; u:format ?format\n}\nORDER BY ?year\nLIMIT 30"},
      {"family": "Select", "label": "Federation: Embassy cover \u22c8 USTC record", "view": "table", "fed": ["bph"],
       "tip": "A CROSS-SOURCE join. This pre-loads the bph (Embassy of the Free Mind) graph as a second source (see the Sources strip). Each USTC edition back-links u:embassyBook -> the Embassy book; the query then pulls that book's cover image + AI summary from bph and shows them beside USTC's imprint and place - two catalogues describing the same physical book, joined on the shared identity. Remove the bph chip to query USTC alone.",
       "q": "PREFIX u: <https://www.ustc.ac.uk/ontology#>\nPREFIX rdfs: <http://www.w3.org/2000/01/rdf-schema#>\nPREFIX schema: <http://schema.org/>\nPREFIX efmo: <https://data.embassyofthefreemind.com/ontology#>\nSELECT ?cover ?title ?imprint ?place ?summary WHERE {\n  ?e a u:Edition ; rdfs:label ?title ; u:imprint ?imprint ;\n     u:place ?place ; u:embassyBook ?book .\n  ?book schema:image ?cover ; efmo:summary ?summary\n}\nLIMIT 25"}
    ],
    "ramon_llull": [
      {"family": "Summary", "label": "What's in this collection", "view": "table",
       "tip": "The shape of the BVPB Ramón Llull collection: 142 digitised works, split by dc:type into 'Manuscritos' (hand-written medieval codices) and 'Libros' (early printed editions, 15th-18th c.).\nOne count per Dublin Core type - the manuscripts are the older, rarer half of the Lullian corpus here.",
       "q": "PREFIX dc: <http://purl.org/dc/elements/1.1/>\nSELECT ?type (COUNT(?s) AS ?count) WHERE {\n  ?s dc:type ?type\n}\nGROUP BY ?type\nORDER BY DESC(?count)"},
      {"family": "Aggregate", "label": "Languages of the Lullian corpus", "view": "table",
       "tip": "Works per language (dc:language, ISO 639-2 codes): 'lat' = Latin (the bulk of Llull's scholastic output), 'cat' = Catalan (his own vernacular, which he pioneered for philosophy), 'spa' = Castilian.\nRamón Llull wrote across all three, and the collection reflects it.",
       "q": "PREFIX dc: <http://purl.org/dc/elements/1.1/>\nSELECT ?language (COUNT(?s) AS ?works) WHERE {\n  ?s dc:language ?language\n}\nGROUP BY ?language\nORDER BY DESC(?works)"},
      {"family": "Select", "label": "Read the books: every work + its PDF", "view": "table", "cols": {"title": "Title", "date": "Date", "pdf": "PDF (digitised object)"},
       "tip": "Each work with its date and edm:isShownBy - the whole digitised object as a single PDF served from bvpb.mcu.es. The PDF column is a clickable link (the 142 PDFs are ~13 GB in total).\nSorted oldest-first, so the 14th-15th-century manuscripts (e.g. the Ars demonstrativa, Arbor Scientiae) lead.",
       "q": "PREFIX dc: <http://purl.org/dc/elements/1.1/>\nPREFIX edm: <http://www.europeana.eu/schemas/edm/>\nSELECT ?title ?date ?pdf WHERE {\n  ?s dc:title ?title ; dc:date ?date ; edm:isShownBy ?pdf .\n}\nORDER BY ?date\nLIMIT 50"},
      {"family": "Aggregate", "label": "The printers & publishers", "view": "table",
       "tip": "Editions per publisher/printer (dc:publisher). The early-modern Mallorca presses surface - the house of the Viuda de Guasp and Typis Viduae Frau (Palma de Mallorca) printed much of the 17th-18th-century Lullian revival.\nManuscripts have no publisher, so this counts only the printed editions.",
       "q": "PREFIX dc: <http://purl.org/dc/elements/1.1/>\nSELECT ?publisher (COUNT(?s) AS ?works) WHERE {\n  ?s dc:publisher ?publisher\n}\nGROUP BY ?publisher\nORDER BY DESC(?works)\nLIMIT 25"},
      {"family": "Select", "label": "The manuscripts, with their physical description", "view": "table", "cols": {"title": "Title", "date": "Date", "format": "Physical description"},
       "tip": "Just the hand-written codices (dc:type = 'Manuscritos'): each with its date and dc:format - the codicological description (number of leaves, parchment vs paper, columns, dimensions in cm).\nThese are the medieval witnesses of Llull's text, the scholarly heart of the collection.",
       "q": "PREFIX dc: <http://purl.org/dc/elements/1.1/>\nSELECT ?title ?date ?format WHERE {\n  ?s dc:type ?type ; dc:title ?title ; dc:date ?date ; dc:format ?format .\n  FILTER(STR(?type) = \"Manuscritos\")\n}\nORDER BY ?date\nLIMIT 40"},
      {"family": "Select", "label": "🔎 Full-text search across the OCR'd books", "view": "table", "cols": {"title": "Book", "date": "Date", "manifest": "Open (IIIF)"},
       "tip": "Full-text search over the OCR of all 71 PRINTED editions (Tesseract lat + dictionary long-ſ repair, ~8.7M words held as bvpb:fulltext, backed by the .rete text index). This finds every book whose pages mention 'sapientia' (wisdom) - change the term to search the whole corpus.\nThe Open (IIIF) column opens the page-turning facsimile with the OCR word layer; the OCR is an automatic scaffold, not a critical edition. NOTE a FILTER(CONTAINS) scans the fulltext (~19 MB fetched), so give it a moment; enable Settings → Range cache to make re-runs instant.",
       "q": "PREFIX bvpb: <https://bvpb.mcu.es/ns#>\nPREFIX dc: <http://purl.org/dc/elements/1.1/>\nSELECT ?title ?date ?manifest WHERE {\n  ?s bvpb:fulltext ?txt ; dc:title ?title ; dc:date ?date ; bvpb:iiifManifest ?manifest .\n  FILTER(CONTAINS(LCASE(STR(?txt)), \"sapientia\"))\n}\nLIMIT 25"},
      {"family": "Select", "label": "📖 Page-turning IIIF viewers (printed editions)", "view": "table", "cols": {"title": "Book", "pages": "Pages", "manifest": "Open (IIIF)"},
       "tip": "The 71 printed editions, each with a IIIF v3 manifest (bvpb:iiifManifest) - click a ?manifest cell to open the page-turning viewer (facsimile images served from R2 + the OCR word layer). Sorted by page count; the Operum Tomus volumes run past 1,000 pages.\nManuscripts are absent here - Tesseract can't read the hands, which would need HTR (Transkribus / eScriptorium).",
       "q": "PREFIX bvpb: <https://bvpb.mcu.es/ns#>\nPREFIX dc: <http://purl.org/dc/elements/1.1/>\nSELECT ?title ?pages ?manifest WHERE {\n  ?s bvpb:iiifManifest ?manifest ; dc:title ?title ; bvpb:pageCount ?pages .\n}\nORDER BY DESC(?pages)\nLIMIT 30"}
    ],
    "scrolls": [
      {"family": "Summary", "label": "What's in the collection", "view": "table",
       "tip": "The shape of the graph at a glance: 45 samples (27 Scrolls + 9 Fragments + 9 not-yet-typed), 63 Scan sessions, 66 reconstructed Volumes, 204 traced Segments, 2,336 downloadable DataAssets and 9 ML Models. One count per class.\nThe classes are self-describing (open the Schema view) - the whole Vesuvius Challenge digitisation pipeline as a typed graph.",
       "q": "PREFIX rdfs: <http://www.w3.org/2000/01/rdf-schema#>\nSELECT ?type (COUNT(?s) AS ?n) WHERE {\n  ?s a ?c . ?c rdfs:label ?type\n}\nGROUP BY ?type\nORDER BY DESC(?n)"},
      {"family": "Select", "label": "★ Download the data: every file for Scroll 1", "view": "table", "cols": {"type": "File type", "download": "Download ↓"},
       "tip": "Every downloadable file for Scroll 1 (PHercParis4) - OME-Zarr CT volumes, OBJ surface meshes, tifxyz coordinate maps, surface-volume layer zarrs and ink-detection prediction TIFFs. Each ?download IRI IS the file's public URL on the Vesuvius open-data bucket, so the cell is a working download link.\nSwap the sample IRI for /PHerc0332 (Scroll 3), /PHerc1667 (Scroll 4) or /PHerc0172 (Scroll 5). Remove the LIMIT to list all ~1,000 files.",
       "q": "PREFIX vs: <https://scrollprize.org/vocab#>\nSELECT ?type ?download WHERE {\n  ?download vs:assetOf ?e ; vs:assetType ?type .\n  ?e vs:sample <https://scrollprize.org/id/sample/PHercParis4> .\n}\nORDER BY ?type\nLIMIT 100"},
      {"family": "Select", "label": "The scrolls & fragments, with photographs", "view": "table", "cols": {"sample": "Sample", "type": "Type", "photo": "Photograph"},
       "tip": "The physical samples that carry a photograph, each rendered as a thumbnail (the same Thumbor image service the scrollprize.org data browser uses; click to open full size). PHerc numbers are the Naples inventory numbers of the carbonised papyri excavated from the Villa dei Papiri.\n33 of the 45 samples have a photo; the rest are scan-only.",
       "q": "PREFIX vs: <https://scrollprize.org/vocab#>\nPREFIX rdfs: <http://www.w3.org/2000/01/rdf-schema#>\nSELECT ?sample ?type ?photo WHERE {\n  ?s vs:thumbnail ?photo ; rdfs:label ?sample ; vs:sampleType ?type .\n}\nORDER BY ?sample"},
      {"family": "Select", "label": "🔥 The revealed ink, as images", "view": "table", "cols": {"scroll": "Scroll", "segment": "Segment", "ink": "Revealed ink (preview)"},
       "tip": "The payoff of the whole project: ML ink-detection predictions over the unwrapped papyrus surfaces, as inline image previews (the downsampled .jpg renders in the table; the full-resolution TIFF is a separate asset). Dark strokes are predicted ink - the Greek text of a 2,000-year-old scroll.\nThese are Segments of every scroll that has been read; sort or filter by ?scroll to focus on one. The full predictions are in the next example.",
       "q": "PREFIX vs: <https://scrollprize.org/vocab#>\nPREFIX rdfs: <http://www.w3.org/2000/01/rdf-schema#>\nSELECT ?scroll ?segment ?ink WHERE {\n  ?ink vs:assetType \"ink-detection-downsampled\" ; vs:assetOf ?g .\n  ?g rdfs:label ?segment ; vs:sample ?s . ?s rdfs:label ?scroll .\n}\nLIMIT 40"},
      {"family": "Select", "label": "Ink predictions + which model made them", "view": "table", "cols": {"scroll": "Scroll", "model": "Model", "download": "Prediction TIFF ↓"},
       "tip": "Full provenance: every full-resolution ink-detection TIFF (a working download link), joined via vs:usedModel to the ML model that produced it - the resnet3d-152 'new_canon_autoresearch_recipe', the TimeSformer scroll-5 models, etc. This is the graph paying off: from a downloadable prediction back to the exact model run.\nSee the models themselves (architecture + task) with the Schema view or the Model class.",
       "q": "PREFIX vs: <https://scrollprize.org/vocab#>\nPREFIX rdfs: <http://www.w3.org/2000/01/rdf-schema#>\nSELECT ?scroll ?model ?download WHERE {\n  ?download vs:assetType \"ink-detection\" ; vs:assetOf ?g ; vs:usedModel ?m .\n  ?g vs:sample ?s . ?s rdfs:label ?scroll . ?m rdfs:label ?model .\n}\nLIMIT 40"},
      {"family": "Select", "label": "The 3D CT volumes to download (OME-Zarr)", "view": "table", "cols": {"scroll": "Scroll", "shape": "Voxel shape (z×y×x)", "zarr": "OME-Zarr volume ↓"},
       "tip": "The reconstructed micro-CT volumes - each a cloud-optimised OME-Zarr store (a directory URL you can open with zarr/dask, or in Neuroglancer). ?shape is the voxel grid: these are tens of thousands of voxels on a side, so a single volume is tens to hundreds of GB.\nOne scroll can have several volumes at different resolutions/energies; ?zarr is the download root.",
       "q": "PREFIX vs: <https://scrollprize.org/vocab#>\nPREFIX rdfs: <http://www.w3.org/2000/01/rdf-schema#>\nSELECT ?scroll ?shape ?zarr WHERE {\n  ?zarr vs:assetType \"ome-zarr\" ; vs:assetOf ?v .\n  ?v vs:sample ?s ; vs:shape ?shape . ?s rdfs:label ?scroll .\n}\nORDER BY ?scroll\nLIMIT 40"},
      {"family": "Select", "label": "Scanning sessions: facility, energy, resolution", "view": "table", "cols": {"scan": "Scan session", "facility": "Facility", "keV": "Energy (keV)", "resolution": "Voxel size (µm)"},
       "tip": "The 63 synchrotron micro-CT sessions, finest voxel first. Two beamlines did the scanning: DLS (Diamond Light Source, Oxfordshire) and ESRF (Grenoble). Higher keV penetrates the dense carbonised roll; smaller µm resolves finer papyrus layers.\nThe scan feeds a reconstructed Volume (vs:fromScan) - the physics half of the pipeline.",
       "q": "PREFIX vs: <https://scrollprize.org/vocab#>\nPREFIX rdfs: <http://www.w3.org/2000/01/rdf-schema#>\nSELECT ?scan ?facility ?keV ?resolution WHERE {\n  ?sc a vs:Scan ; rdfs:label ?scan ; vs:facility ?f ;\n     vs:energyKeV ?keV ; vs:pixelSizeUm ?resolution .\n  ?f rdfs:label ?facility .\n}\nORDER BY ?resolution\nLIMIT 40"},
      {"family": "Construct", "label": "Which model read which scroll (graph)", "view": "graph",
       "tip": "A bipartite graph: each Scroll/Fragment joined to every ML Model that produced a prediction on its data (via the DataAsset's vs:usedModel). Shared models tie the scrolls together - the 'new_canon_autoresearch_recipe' ink detector was applied across several.\nSwitch Output to Graph. Nodes are labelled; the hubs are the most-worked scrolls and the general-purpose models.",
       "q": "PREFIX vs: <https://scrollprize.org/vocab#>\nPREFIX rdfs: <http://www.w3.org/2000/01/rdf-schema#>\nCONSTRUCT {\n  ?s vs:usedModel ?m . ?s rdfs:label ?sl . ?m rdfs:label ?ml\n} WHERE {\n  ?a vs:usedModel ?m ; vs:assetOf ?g . ?g vs:sample ?s .\n  ?s rdfs:label ?sl . ?m rdfs:label ?ml\n}\nLIMIT 200"}
    ],
    "peirce": [
      {"family": "Select", "label": "★ Read the manuscripts (45,423 IIIF pages)", "view": "table", "cols": {"title": "Manuscript", "pages": "Pages", "iiif": "Scans (IIIF)"},
       "tip": "423 components are digitized end to end - 45,423 page images from Harvard's IIIF servers. ?iiif is each object's IIIF Presentation v3 manifest: the in-table viewer pages through the actual notebook (‹ › arrows; click a page for the lightbox). Longest first: 'Chapter III. The Simplest Mathematics (Logic III)' runs 822 pages, the 'Minute Logic' 728, 'Pragmatism (Prag)' 698 - Peirce's great unfinished books, readable here.",
       "q": "PREFIX a: <https://hollisarchives.lib.harvard.edu/ontology#>\nPREFIX rdfs: <http://www.w3.org/2000/01/rdf-schema#>\nSELECT ?title ?pages ?iiif WHERE {\n  ?c a:iiifManifest ?iiif ; rdfs:label ?title ; a:pageCount ?pages .\n} ORDER BY DESC(?pages) LIMIT 24"},
      {"family": "Aggregate", "label": "Who wrote to Peirce", "view": "table", "cols": {"who": "Correspondent", "letters": "Letters"},
       "tip": "The correspondence titles ('Adams, Brooks. Two letters to Charles S. Peirce, ...') parse into a DIRECTED network: sender, addressee, letter count. Summing letters TO Peirce: Wendell Phillips Garrison (216 - editor of The Nation, Peirce's main reviewing outlet), Paul Carus (112 - editor of The Monist, who published the late pragmatism essays), his brother James Mills Peirce (87), then J. McKeen Cattell, the Smithsonian, and the publishers.",
       "q": "PREFIX a: <https://hollisarchives.lib.harvard.edu/ontology#>\nPREFIX rdfs: <http://www.w3.org/2000/01/rdf-schema#>\nSELECT ?who (SUM(?k) AS ?letters) WHERE {\n  ?c a:sender ?p ;\n     a:addressee <https://id.lib.harvard.edu/ead/hou02614/person/peirce-charles-sanders> ;\n     a:letterCount ?k .\n  ?p rdfs:label ?who .\n} GROUP BY ?who ORDER BY DESC(?letters) LIMIT 20"},
      {"family": "Summary", "label": "The papers at a glance", "view": "table", "cols": {"series": "Series", "components": "Components"},
       "tip": "dcterms:isPartOf+ walks the whole finding-aid tree transitively, rolling every component up to its top series: I. Manuscripts (1,772 - the notebooks and drafts, arranged by Richard Robin's topical scheme from Mathematics to Biographical) and II. Correspondence (748) dominate. 2,527 components in all, across 166 boxes.",
       "q": "PREFIX a: <https://hollisarchives.lib.harvard.edu/ontology#>\nPREFIX dct: <http://purl.org/dc/terms/>\nPREFIX rdfs: <http://www.w3.org/2000/01/rdf-schema#>\nSELECT ?series (COUNT(?c) AS ?components) WHERE {\n  ?s a a:Series ; rdfs:label ?series .\n  ?c dct:isPartOf+ ?s .\n} GROUP BY ?series ORDER BY DESC(?components)"},
      {"family": "Select", "label": "The Robin arrangement: B. Pragmatism, item by item", "view": "table", "cols": {"ms": "R №", "title": "Manuscript", "date": "Date"},
       "tip": "Peirce scholars cite these manuscripts by their Robin numbers ('R 318') - Richard Robin's 1967 catalogue numbering, carried here as a:msNumber. This walks subseries B. Pragmatism in R order: the 1903 Harvard Lectures on Pragmatism, 'What Pragmatism Is', the Monist essays... ?date is parsed straight from each title's tail ('circa 1903'). Change the subseries IRI (hou02614c00357 = D. Logic, c00881 = E. Metaphysics) to browse another shelf of his mind.",
       "q": "PREFIX a: <https://hollisarchives.lib.harvard.edu/ontology#>\nPREFIX dct: <http://purl.org/dc/terms/>\nPREFIX rdfs: <http://www.w3.org/2000/01/rdf-schema#>\nSELECT ?ms ?title ?date WHERE {\n  ?c dct:isPartOf <https://hollisarchives.lib.harvard.edu/catalog/hou02614c00288> ;\n     a:msNumber ?ms ; rdfs:label ?title .\n  OPTIONAL { ?c a:dateText ?date }\n} ORDER BY ?ms"},
      {"family": "Select", "label": "Search: existential graphs", "view": "table", "cols": {"title": "Component", "date": "Date", "genre": "Genre"},
       "tip": "Full-text search over the titles (a text index backs CONTAINS). 'Existential graphs' - Peirce's diagrammatic logic, which he called his 'chef d'oeuvre' - surfaces the notebooks where he developed it. Try 'map coloring' (his four-color-problem attack), 'photometric' (his Harvard Observatory research), 'Century Dictionary' (his 6,000 definitions), or any correspondent's name.",
       "q": "PREFIX a: <https://hollisarchives.lib.harvard.edu/ontology#>\nPREFIX rdfs: <http://www.w3.org/2000/01/rdf-schema#>\nSELECT ?title ?date ?genre WHERE {\n  ?c rdfs:label ?title .\n  FILTER(CONTAINS(LCASE(STR(?title)), \"existential graphs\"))\n  OPTIONAL { ?c a:dateText ?date }\n  OPTIONAL { ?c a:genre ?genre }\n} LIMIT 40"},
      {"family": "Select", "label": "Dated 1903: the annus mirabilis, with scans", "view": "table", "cols": {"title": "Manuscript", "date": "Date", "iiif": "Scans"},
       "tip": "Integer year ranges (a:startYear/a:endYear) are parsed from every datable title, so the archive filters by time: 1903 - the year of the Harvard Lectures on Pragmatism AND the Lowell Lectures - catches the drafts of both lecture series, joined to their IIIF scans where digitized. Change the year to 1877 (Illustrations of the Logic of Science) or 1867 (On a New List of Categories).",
       "q": "PREFIX a: <https://hollisarchives.lib.harvard.edu/ontology#>\nPREFIX rdfs: <http://www.w3.org/2000/01/rdf-schema#>\nSELECT ?title ?date ?iiif WHERE {\n  ?c a:startYear ?y1 ; a:endYear ?y2 ; a:dateText ?date ; rdfs:label ?title .\n  FILTER(?y1 <= 1903 && ?y2 >= 1903)\n  OPTIONAL { ?c a:iiifManifest ?iiif }\n} ORDER BY ?title LIMIT 40"}
    ],
    "boe": [
      {"family": "Select", "label": "🧠 Every norm with the force of law — reasoning over the taxonomy", "view": "table", "reason": true, "cols": {"title": "Norma con rango de ley"},
       "tip": "The rango classes now form a two-tier ontology: bx:NormaConRangoDeLey groups the force-of-law kinds (Ley, Ley Orgánica, Decreto Legislativo, Decreto-ley…) and bx:NormaReglamentaria the regulatory ones (Real Decreto, Orden, Resolución…), both under eli:LegalResource.\nNO norm is typed with the intermediate class directly, so with 🧠 Reason OFF this returns 0 rows. Turn it ON and OWL 2 QL rewrites `a bx:NormaConRangoDeLey` along rdfs:subClassOf, returning all 4,967 norms with the force of law. Flip Reason off to see the difference.",
       "q": "PREFIX bx: <https://w3id.org/rete/boe#>\nPREFIX eli: <http://data.europa.eu/eli/ontology#>\nSELECT ?title WHERE {\n  ?norm a bx:NormaConRangoDeLey ;\n        eli:title ?title .\n} LIMIT 100"},
      {"family": "Path", "label": "🧠 What has amended this law? — inverse reasoning", "view": "table", "reason": true, "cols": {"title": "Norma modificadora"},
       "tip": "The amendment network stores only the forward edge: X eli:amends Y. The ontology declares bx:amendedBy owl:inverseOf eli:amends, so the reverse edges are NOT stored — they're entailed.\nWith 🧠 Reason ON, asking what amended the Ley 38/1988 de Demarcación y de Planta Judicial (the most-reformed law in force — its judicial map redrawn with every new court) returns all 134 amending norms by reversing the eli:amends edges. Reason OFF → 0 rows. Swap the IRI for any norm.",
       "q": "PREFIX bx: <https://w3id.org/rete/boe#>\nPREFIX eli: <http://data.europa.eu/eli/ontology#>\nSELECT ?title WHERE {\n  <https://www.boe.es/eli/es/l/1988/12/28/38> bx:amendedBy ?amender .\n  ?amender eli:title ?title .\n}"},
      {"family": "Aggregate", "label": "★ The living laws — most-reformed norms in force", "view": "table", "cols": {"title": "Norma", "reformas": "Reformas"},
       "tip": "eli:amends is the 'modifica' relation mined from the BOE's own análisis, so counting the INCOMING amendments ranks the most-reformed laws still in force. Top is the Ley 38/1988 de Demarcación y de Planta Judicial (134 reforms — the judicial map is redrawn every time a court is added), then the great tax codes (IRPF — Ley 35/2006; IVA — Ley 37/1992; Impuestos Especiales), the texto refundido de la Ley General de la Seguridad Social, the Ley 1/2000 de Enjuiciamiento Civil and the Ley Orgánica 6/1985 del Poder Judicial. These are the laws Parliament keeps touching.",
       "q": "PREFIX eli: <http://data.europa.eu/eli/ontology#>\nSELECT ?title (COUNT(?s) AS ?reformas) WHERE {\n  ?s eli:amends ?n .\n  ?n eli:title ?title .\n} GROUP BY ?n ?title ORDER BY DESC(?reformas) LIMIT 25"},
      {"family": "Select", "label": "What the 1978 Constitution repealed", "view": "table", "cols": {"derogada": "Norma derogada"},
       "tip": "The Constitución Española (its ELI work IRI is es/c/1978/12/27) sits at the top of the hierarchy of norms — and here is what it swept away. Its disposición derogatoria repeals the Franco-era Leyes Fundamentales del Reino: the Ley de Sucesión en la Jefatura del Estado (1947), the Ley reguladora del Fuero de los Españoles (1945), the Ley Orgánica del Estado (1967), the Ley de Principios del Movimiento Nacional (1958) and the Ley 1/1977 para la Reforma Política that dissolved the regime. The transition to democracy, as eli:repeals edges.",
       "q": "PREFIX eli: <http://data.europa.eu/eli/ontology#>\nSELECT ?derogada WHERE {\n  <https://www.boe.es/eli/es/c/1978/12/27/(1)> eli:repeals ?n .\n  ?n eli:title ?derogada .\n}"},
      {"family": "Aggregate", "label": "Most challenged before the Constitutional Court", "view": "table", "cols": {"title": "Norma", "recursos": "Recursos / cuestiones"},
       "tip": "bx:challenges gathers the recursos and cuestiones de inconstitucionalidad and conflictos raised against a norm (BOE análisis relation codes 520/530/552/694). The most-contested law in force is the Ley Orgánica 10/1995 del Código Penal (51 challenges), then the Ley 24/1983 de saneamiento de las Haciendas Locales and the Real Decreto-ley 28/2012 on the Seguridad Social pension freeze — the fault lines of Spanish constitutional litigation.",
       "q": "PREFIX eli: <http://data.europa.eu/eli/ontology#>\nPREFIX bx: <https://w3id.org/rete/boe#>\nSELECT ?title (COUNT(?s) AS ?recursos) WHERE {\n  ?s bx:challenges ?n .\n  ?n eli:title ?title .\n} GROUP BY ?n ?title ORDER BY DESC(?recursos) LIMIT 20"},
      {"family": "Select", "label": "Search: laws on artificial intelligence", "view": "table", "cols": {"title": "Norma", "fecha": "Fecha"},
       "tip": "Full-text search over the 41k norm titles (a rete text index backs CONTAINS). 'Inteligencia artificial' surfaces the Real Decreto 729/2023 that created AESIA — the Agencia Española de Supervisión de Inteligencia Artificial, the first such national supervisor in the EU — and Extremadura's Decreto-ley 2/2023. Swap the phrase for 'cambio climático', 'protección de datos', 'memoria democrática', 'vivienda' or any ministry's name.",
       "q": "PREFIX eli: <http://data.europa.eu/eli/ontology#>\nSELECT ?title ?fecha WHERE {\n  ?s eli:title ?title ; eli:date_document ?fecha .\n  FILTER(CONTAINS(LCASE(STR(?title)), \"inteligencia artificial\"))\n} ORDER BY DESC(?fecha) LIMIT 40"},
      {"family": "Construct", "label": "Reforms of the Código Penal, as a network", "view": "graph",
       "tip": "Switch Output to Graph. The node-link neighbourhood of ONE norm — the Código Penal (Ley Orgánica 10/1995) — with every law that has amended it fanning out from the centre (eli:amends), 43 reforms in all: the 2015 overhaul, the LO 4/2023 on delitos contra la libertad sexual ('sólo sí es sí'), the LO 1/2024 de amnistía… Change the central IRI to es/l/2000/01/07/1 (Enjuiciamiento Civil) or es/lo/1985/07/01/6 (Poder Judicial) to map another law's reform history.",
       "q": "PREFIX eli: <http://data.europa.eu/eli/ontology#>\nPREFIX rdfs: <http://www.w3.org/2000/01/rdf-schema#>\nCONSTRUCT {\n  ?cp rdfs:label ?cl .\n  ?law eli:amends ?cp .\n  ?law rdfs:label ?ll .\n} WHERE {\n  VALUES ?cp { <https://www.boe.es/eli/es/lo/1995/11/23/10> }\n  ?cp rdfs:label ?cl .\n  ?law eli:amends ?cp ; rdfs:label ?ll .\n}"},
      {"family": "Aggregate", "label": "Legislative output, year by year", "view": "time", "cols": {"year": "Año", "normas": "Normas"},
       "tip": "Switch Output to Time. The norms in force counted by their year of adoption (eli:date_document) — the shape of the currently-live statute book over time. It climbs through the 2000s and 2010s and spikes in 2020 (795 norms — the COVID-19 emergency legislation). Older law is thinner because superseded norms have dropped out of the consolidated set.",
       "q": "PREFIX eli: <http://data.europa.eu/eli/ontology#>\nSELECT ?year (COUNT(?s) AS ?normas) WHERE {\n  ?s eli:date_document ?d .\n  BIND(SUBSTR(STR(?d), 1, 4) AS ?year)\n} GROUP BY ?year ORDER BY ?year"}
    ],
    "bph": [
      {"family": "Select", "label": "Federation: where are these books held? (via USTC)", "view": "table", "fed": ["ustc"],
       "tip": "A CROSS-SOURCE join from the other side: pre-loads the USTC catalogue as a second source. Each Embassy book links rdfs:seeAlso -> its USTC edition; the query follows that into USTC to list the libraries that hold a physical copy (Herzog August, BSB, British Library...) and the printer. Remove the ustc chip to query the Embassy alone.",
       "q": "PREFIX rdfs: <http://www.w3.org/2000/01/rdf-schema#>\nPREFIX schema: <http://schema.org/>\nPREFIX u: <https://www.ustc.ac.uk/ontology#>\nSELECT ?title ?printer ?library WHERE {\n  ?book a schema:Book ; rdfs:label ?title ; rdfs:seeAlso ?ed .\n  ?ed u:printer ?p . ?p rdfs:label ?printer .\n  ?ed u:heldBy ?l . ?l rdfs:label ?library\n}\nLIMIT 40"},
      {"family": "Summary", "label": "What's in the collection", "view": "table",
       "tip": "The shape of the graph at a glance: 2,307 hermetic/esoteric Books, 764,708 Pages (each a scanned page image), 991 authors (schema:Person), 725 geocoded Places and ~127k subject/category/collection Concepts. One count per rdf:type.",
       "q": "PREFIX rdf: <http://www.w3.org/1999/02/22-rdf-syntax-ns#>\nSELECT ?type (COUNT(?s) AS ?count) WHERE {\n  ?s rdf:type ?type\n}\nGROUP BY ?type\nORDER BY DESC(?count)"},
      {"family": "Select", "label": "Alchemy books (with cover images)", "view": "table",
       "tip": "Books filed under the 'alchemy' subject category, oldest first, each with author, year and its cover thumbnail (the -thumb.jpg renders inline). Swap the category IRI for /kabbalah, /rosicrucianism, /mysticism, /astrology or /medicine to browse another shelf.",
       "q": "PREFIX schema: <http://schema.org/>\nPREFIX dct: <http://purl.org/dc/terms/>\nPREFIX rdfs: <http://www.w3.org/2000/01/rdf-schema#>\nPREFIX skos: <http://www.w3.org/2004/02/skos/core#>\nPREFIX geo: <http://www.opengis.net/ont/geosparql#>\nPREFIX efmo: <https://data.embassyofthefreemind.com/ontology#>\nSELECT ?cover ?title ?author ?year WHERE {\n  ?b dct:subject <https://data.embassyofthefreemind.com/category/alchemy> ;\n     schema:image ?cover ; rdfs:label ?title ;\n     dct:creator ?author ; schema:datePublished ?year\n}\nORDER BY ?year\nLIMIT 40"},
      {"family": "Aggregate", "label": "Books by language", "view": "table",
       "tip": "The library is deeply multilingual: German (929) and Latin (585) dominate, then English, French, Dutch, Italian, Hebrew and more. Count of books per dct:language.",
       "q": "PREFIX schema: <http://schema.org/>\nPREFIX dct: <http://purl.org/dc/terms/>\nPREFIX rdfs: <http://www.w3.org/2000/01/rdf-schema#>\nPREFIX skos: <http://www.w3.org/2004/02/skos/core#>\nPREFIX geo: <http://www.opengis.net/ont/geosparql#>\nPREFIX efmo: <https://data.embassyofthefreemind.com/ontology#>\nSELECT ?language (COUNT(?b) AS ?books) WHERE {\n  ?b a schema:Book ; dct:language ?language\n}\nGROUP BY ?language\nORDER BY DESC(?books)"},
      {"family": "Select", "label": "The oldest books in the library", "view": "table",
       "tip": "Sorted by publication year ascending, so the incunabula and early-16th-century imprints surface first, each with its author and English title. schema:datePublished is a typed xsd:gYear.",
       "q": "PREFIX schema: <http://schema.org/>\nPREFIX dct: <http://purl.org/dc/terms/>\nPREFIX rdfs: <http://www.w3.org/2000/01/rdf-schema#>\nPREFIX skos: <http://www.w3.org/2004/02/skos/core#>\nPREFIX geo: <http://www.opengis.net/ont/geosparql#>\nPREFIX efmo: <https://data.embassyofthefreemind.com/ontology#>\nSELECT ?year ?title ?author WHERE {\n  ?b a schema:Book ; schema:datePublished ?year ;\n     rdfs:label ?title ; dct:creator ?author\n}\nORDER BY ?year\nLIMIT 30"},
      {"family": "Aggregate", "label": "Most-represented authors", "view": "table",
       "tip": "Which authors have the most books in the BPH? Books per schema:author, with the author's name. Beyond 'Anonymous'/'Unknown', the alchemical and Rosicrucian corpus (Paracelsus, Boehme, Fludd, Khunrath...) rises to the top.",
       "q": "PREFIX schema: <http://schema.org/>\nPREFIX dct: <http://purl.org/dc/terms/>\nPREFIX rdfs: <http://www.w3.org/2000/01/rdf-schema#>\nPREFIX skos: <http://www.w3.org/2004/02/skos/core#>\nPREFIX geo: <http://www.opengis.net/ont/geosparql#>\nPREFIX efmo: <https://data.embassyofthefreemind.com/ontology#>\nSELECT ?author (COUNT(?b) AS ?books) WHERE {\n  ?b schema:author ?a . ?a rdfs:label ?author\n}\nGROUP BY ?author\nORDER BY DESC(?books)\nLIMIT 25"},
      {"family": "Geo", "label": "Where the books were printed", "view": "map",
       "tip": "The geocoded publication places of the collection, drawn as points (switch Output to Map). Clusters over the great early-modern printing cities - Frankfurt, Nuremberg, Basel, Amsterdam, Paris, London. NB a handful are mis-geocoded homonyms in the source data (e.g. a 'Cracow' placed in Australia).",
       "q": "PREFIX schema: <http://schema.org/>\nPREFIX dct: <http://purl.org/dc/terms/>\nPREFIX rdfs: <http://www.w3.org/2000/01/rdf-schema#>\nPREFIX skos: <http://www.w3.org/2004/02/skos/core#>\nPREFIX geo: <http://www.opengis.net/ont/geosparql#>\nPREFIX efmo: <https://data.embassyofthefreemind.com/ontology#>\nSELECT ?place ?wkt WHERE {\n  ?p a schema:Place ; rdfs:label ?place ; geo:asWKT ?wkt\n}\nLIMIT 500"},
      {"family": "Select", "label": "Highest-rated books (AI quality score)", "view": "table",
       "tip": "Source Library scores each book with an AI quality_score (0-100). Top books by that score, with title and author - a quick way to surface landmark works like Michael Maier's 'Atalanta Fugiens' (here 'Atalanta Fleeing', 100).",
       "q": "PREFIX schema: <http://schema.org/>\nPREFIX dct: <http://purl.org/dc/terms/>\nPREFIX rdfs: <http://www.w3.org/2000/01/rdf-schema#>\nPREFIX skos: <http://www.w3.org/2004/02/skos/core#>\nPREFIX geo: <http://www.opengis.net/ont/geosparql#>\nPREFIX efmo: <https://data.embassyofthefreemind.com/ontology#>\nSELECT ?title ?author ?score WHERE {\n  ?b a schema:Book ; rdfs:label ?title ;\n     dct:creator ?author ; efmo:qualityScore ?score\n}\nORDER BY DESC(?score)\nLIMIT 20"},
      {"family": "Construct", "label": "A book's related-works network", "view": "graph",
       "tip": "The AI-inferred related-works links (efmo:relatedBook) as a node-link graph - each book joined to the titles it is connected to (citations, influences, same-work editions). A small influence neighbourhood of the collection.",
       "q": "PREFIX schema: <http://schema.org/>\nPREFIX dct: <http://purl.org/dc/terms/>\nPREFIX rdfs: <http://www.w3.org/2000/01/rdf-schema#>\nPREFIX skos: <http://www.w3.org/2004/02/skos/core#>\nPREFIX geo: <http://www.opengis.net/ont/geosparql#>\nPREFIX efmo: <https://data.embassyofthefreemind.com/ontology#>\nCONSTRUCT {\n  ?b rdfs:label ?bl . ?b efmo:relatedBook ?r . ?r rdfs:label ?rl\n} WHERE {\n  ?b efmo:relatedBook ?r ; rdfs:label ?bl . ?r rdfs:label ?rl\n}\nLIMIT 200"}
    ],
    // CauseNet-Full (256M triples / 6.39 GB) is the HEAVY lazy dataset: its
    // dictionary holds 24.4M source sentences + 12.2M concepts. CauseNet models
    // POSITIVE causation only (cn:causes = "X causes Y" mined from text); there is
    // no inhibitory/negative relation, so cn:support (the source count on the
    // reified cn:CausalRelation) is the strong-vs-weak / confidence signal we surface
    // as a 2nd column. The multi-column examples read the reified relation
    // (cn:cause/effect/support), so they cost a bit more lazily than the bare
    // cn:causes edge. cn: = https://causenet.org/ontology#; concepts are
    // https://causenet.org/concept/<label>. (The small embedded `causal` dataset has
    // a protective ex:reduces relation if you want true positive-vs-negative.)
    "causenet": [
      {"family": "Select", "label": "What does smoking cause? (effect + evidence)", "view": "table",
       "tip": "Everything 'smoking' is claimed to cause, each with cn:support = how many source sentences back the claim, so the best-evidenced effects (cancer, heart disease, death) sort to the top. Two columns: effect + support. Reads the reified cn:CausalRelation, so a bit heavier than the bare-edge version (which was ~57 MB lazy). Swap the IRI for /stress, /obesity, /war.",
       "q": "PREFIX cn: <https://causenet.org/ontology#>\nSELECT ?effect ?support WHERE {\n  ?r cn:cause <https://causenet.org/concept/smoking> ;\n     cn:effect ?effect ;\n     cn:support ?support\n}\nORDER BY DESC(?support)\nLIMIT 50"},
      {"family": "Select", "label": "What causes obesity? (cause + evidence)", "view": "table",
       "tip": "The reverse direction with evidence: every concept claimed to cause 'obesity', ranked by source count. Columns: cause + support. Swap the effect IRI for /diabetes, /cancer or /depression to mine a different outcome.",
       "q": "PREFIX cn: <https://causenet.org/ontology#>\nSELECT ?cause ?support WHERE {\n  ?r cn:effect <https://causenet.org/concept/obesity> ;\n     cn:cause ?cause ;\n     cn:support ?support\n}\nORDER BY DESC(?support)\nLIMIT 50"},
      {"family": "Aggregate", "label": "Best-evidenced causes of cancer", "view": "table",
       "tip": "Causes of 'cancer' ranked by how much web evidence backs each claim (cn:support = true source count). Columns: cause + support. Benchmarked lazy cost on the lean file: ~114 MB over HTTP range, ~42 s cold.",
       "q": "PREFIX cn: <https://causenet.org/ontology#>\nSELECT ?cause ?support WHERE {\n  ?r cn:effect <https://causenet.org/concept/cancer> ;\n     cn:cause ?cause ;\n     cn:support ?support\n}\nORDER BY DESC(?support)\nLIMIT 25"},
      {"family": "Path", "label": "Causal chain: smoking -> X -> Y", "view": "table",
       "tip": "A two-step chain over cn:causes: what smoking causes (column 1), and what each of those in turn causes (column 2) - tracing how one cause cascades. For full transitive closure use the Reach tab on cn:causes; this 2-hop join stays bounded.",
       "q": "PREFIX cn: <https://causenet.org/ontology#>\nSELECT ?step1 ?step2 WHERE {\n  <https://causenet.org/concept/smoking> cn:causes ?step1 .\n  ?step1 cn:causes ?step2\n}\nLIMIT 50"},
      {"family": "Select", "label": "Effects + evidence sentence (HEAVY, 3 cols)", "view": "table",
       "tip": "Three columns - effect, support, and a REAL source sentence CauseNet mined the claim from (cn:hasSource -> cn:sentence). The richest view of a causal claim. HEAVIEST example: 'smoking' has thousands of sources, so it fetches 100s of MB over HTTP range; keep the LIMIT low or pick a narrower cause.",
       "q": "PREFIX cn: <https://causenet.org/ontology#>\nSELECT ?effect ?support ?sentence WHERE {\n  ?r cn:cause <https://causenet.org/concept/smoking> ;\n     cn:effect ?effect ;\n     cn:support ?support ;\n     cn:hasSource ?s .\n  ?s cn:sentence ?sentence\n}\nLIMIT 10"},
      {"family": "Construct", "label": "Map smoking's effects (graph)", "view": "graph",
       "tip": "CONSTRUCT a small cause->effect subgraph around 'smoking' for the graph view - the selective bare cn:causes read (~57 MB lazy), rendered as nodes + arrows. Each arrow is a mined causal claim from Wikipedia / ClueWeb12.",
       "q": "PREFIX cn: <https://causenet.org/ontology#>\nCONSTRUCT { <https://causenet.org/concept/smoking> cn:causes ?effect }\nWHERE { <https://causenet.org/concept/smoking> cn:causes ?effect }\nLIMIT 40"}
    ],
    causal: [
      {
        family: "Summary",
        label: "What's in the model",
        view: "table",
        tip: "Predicate totals: ~48 ex:causes edges, the protective ex:reduces relation, plus the rdf:type / label / attribute triples.",
        q: `SELECT ?p (COUNT(*) AS ?n) WHERE { ?s ?p ?o } GROUP BY ?p ORDER BY DESC(?n)`
      },
      {
        family: "Path",
        label: "Everything that leads to a heart attack",
        view: "graph",
        tip: "ex:causes* gathers the whole upstream subgraph feeding a myocardial infarction - proximal disease and distal risk factors, drawn as a causal network.",
        q: `PREFIX ex: <http://ex/>
SELECT ?from ?to WHERE {
  ?from ex:causes ?to .
  ?to ex:causes* ex:MyocardialInfarction
}`
      },
      {
        family: "Path",
        label: "Downstream effects of obesity",
        view: "graph",
        tip: "The forward closure from obesity. The metabolic feedback loop pulls obesity back into its own effects, so you'll see it reappear downstream.",
        q: `PREFIX ex: <http://ex/>
SELECT ?from ?to WHERE {
  ?from ex:causes ?to .
  ex:Obesity ex:causes* ?from
}`
      },
      {
        family: "Path",
        label: "Feedback loops (vicious cycles)",
        view: "table",
        tip: "A factor that is among its own causes sits on a cycle: ex:causes+ ?x = ?x. Two loops light up - the metabolic loop (obesity / inflammation / insulin resistance) and the stress / sleep loop.",
        q: `PREFIX ex: <http://ex/>
SELECT DISTINCT ?factor WHERE {
  ?factor ex:causes+ ?factor
} ORDER BY ?factor`
      },
      {
        family: "Select",
        label: "Confounders (a common cause of two factors)",
        view: "table",
        tip: "A confounder is one factor that causes two others (Z -> X, Z -> Y). Poverty drives smoking, poor diet, inactivity and stress at once; aging drives several vascular conditions.",
        q: `PREFIX ex: <http://ex/>
SELECT ?confounder ?effectA ?effectB WHERE {
  ?confounder ex:causes ?effectA .
  ?confounder ex:causes ?effectB .
  FILTER(STR(?effectA) < STR(?effectB))
} ORDER BY ?confounder`
      },
      {
        family: "Select",
        label: "Colliders (two causes, one effect)",
        view: "table",
        tip: "A collider is a common effect of two causes (X -> C <- Y) - conditioning on it can create spurious associations. Chest pain is reached by both coronary disease and anxiety.",
        q: `PREFIX ex: <http://ex/>
SELECT ?causeA ?causeB ?collider WHERE {
  ?causeA ex:causes ?collider .
  ?causeB ex:causes ?collider .
  FILTER(STR(?causeA) < STR(?causeB))
} ORDER BY ?collider`
      },
      {
        family: "Path",
        label: "How obesity leads to diabetes (mediators)",
        view: "graph",
        tip: "The subgraph that sits on a path from obesity to diabetes - the metabolic steps mediating the effect (and the loop they form).",
        q: `PREFIX ex: <http://ex/>
SELECT ?from ?to WHERE {
  ?from ex:causes ?to .
  ex:Obesity ex:causes* ?from .
  ?to ex:causes* ex:Diabetes
}`
      },
      {
        family: "Aggregate",
        label: "Biggest causal footprint",
        view: "table",
        tip: "Rank factors by how many distinct things they can ultimately influence (ex:causes+). Distal root causes like poverty top the list - small levers, wide reach.",
        q: `PREFIX ex: <http://ex/>
SELECT ?factor (COUNT(DISTINCT ?reached) AS ?reach) WHERE {
  ?factor ex:causes+ ?reached
} GROUP BY ?factor ORDER BY DESC(?reach) LIMIT 15`
      },
      {
        family: "Select",
        label: "What lowers the risk of a heart attack",
        view: "graph",
        tip: "ex:reduces is the protective relation. This finds interventions that lower a factor on a causal path to a heart attack - where treatment meets the disease pathway.",
        q: `PREFIX ex: <http://ex/>
SELECT DISTINCT ?treatment ?target WHERE {
  ?target ex:causes+ ex:MyocardialInfarction .
  ?treatment ex:reduces ?target
} ORDER BY ?treatment`
      },
      {
        family: "Select",
        label: "Exogenous root causes",
        view: "table",
        tip: "Factors that cause something but that nothing in the model causes - the entry points for prevention. FILTER NOT EXISTS asks for no incoming ex:causes edge.",
        q: `PREFIX ex: <http://ex/>
SELECT DISTINCT ?root WHERE {
  ?root ex:causes ?x .
  FILTER NOT EXISTS { ?y ex:causes ?root }
} ORDER BY ?root`
      }
    ],
    "wikidata-100mb": [
      {"family": "Select", "label": "Physicists who are also philosophers", "view": "graph", "tip": "An occupation intersection (wdt:P106 twice). Selective - the lazy reader faults in ~10 MB of the 104 MB file and returns scientist-philosophers like Ilya Prigogine and Marin Mersenne.", "q": "PREFIX wdt: <http://www.wikidata.org/prop/direct/>\nPREFIX wd: <http://www.wikidata.org/entity/>\nPREFIX rdfs: <http://www.w3.org/2000/01/rdf-schema#>\nSELECT ?p ?who WHERE {\n  ?p wdt:P106 wd:Q169470 ;   # physicist\n     wdt:P106 wd:Q4964182 ;  # philosopher\n     rdfs:label ?who .\n  FILTER(LANG(?who) = \"en\")\n} LIMIT 50"},
      {"family": "Select", "label": "People influenced by Plato", "view": "graph", "tip": "Bound-object star: everyone whose 'influenced by' (wdt:P737) points at Plato (wd:Q859). A bound term = a selective lazy read.", "q": "PREFIX wdt: <http://www.wikidata.org/prop/direct/>\nPREFIX wd: <http://www.wikidata.org/entity/>\nPREFIX rdfs: <http://www.w3.org/2000/01/rdf-schema#>\nSELECT ?p ?who WHERE {\n  ?p wdt:P737 wd:Q859 ;   # influenced by Plato\n     rdfs:label ?who .\n  FILTER(LANG(?who) = \"en\")\n} LIMIT 100"},
      {"family": "Path", "label": "Lines of influence from Plato (transitive)", "view": "graph", "tip": "wdt:P737+ walks the 'influenced by' chain transitively from Plato (wd:Q859) - intellectual lineage across generations. LIMIT keeps the lazy walk bounded.", "q": "PREFIX wdt: <http://www.wikidata.org/prop/direct/>\nPREFIX wd: <http://www.wikidata.org/entity/>\nPREFIX rdfs: <http://www.w3.org/2000/01/rdf-schema#>\nSELECT DISTINCT ?p ?who WHERE {\n  ?p wdt:P737+ wd:Q859 .\n  ?p rdfs:label ?who . FILTER(LANG(?who) = \"en\")\n} LIMIT 150"},
      {"family": "Select", "label": "Writers who are also politicians", "view": "table", "tip": "Another occupation intersection (wdt:P106 writer + politician) - novelists and poets who also held office.", "q": "PREFIX wdt: <http://www.wikidata.org/prop/direct/>\nPREFIX wd: <http://www.wikidata.org/entity/>\nPREFIX rdfs: <http://www.w3.org/2000/01/rdf-schema#>\nSELECT ?p ?who WHERE {\n  ?p wdt:P106 wd:Q36180 ;   # writer\n     wdt:P106 wd:Q82955 ;   # politician\n     rdfs:label ?who .\n  FILTER(LANG(?who) = \"en\")\n} LIMIT 50"},
      {"family": "Aggregate", "label": "Most common occupations", "view": "table", "tip": "Counts people per occupation (wdt:P106), with an OPTIONAL rdfs:label join so each row reads as a Q-id plus its English name rather than a bare code. Aggregating over a WHOLE predicate faults in more tiles than a selective query - heavier, but still streamed lazily, not a full download.", "q": "PREFIX wdt: <http://www.wikidata.org/prop/direct/>\nPREFIX rdfs: <http://www.w3.org/2000/01/rdf-schema#>\nSELECT ?occupation ?occLabel (COUNT(?p) AS ?people) WHERE {\n  ?p wdt:P106 ?occupation .\n  OPTIONAL { ?occupation rdfs:label ?occLabel . FILTER(LANG(?occLabel) = \"en\") }\n}\nGROUP BY ?occupation ?occLabel\nORDER BY DESC(?people)\nLIMIT 20"},
      {"family": "Construct", "label": "People with occupation + birth date", "view": "graph", "tip": "CONSTRUCT a small subgraph of (person)->occupation and (person)->birth-date edges; touches only those two predicates' tiles.", "q": "PREFIX wdt: <http://www.wikidata.org/prop/direct/>\nCONSTRUCT { ?p wdt:P106 ?occ ; wdt:P569 ?dob }\nWHERE {\n  ?p wdt:P106 ?occ .\n  OPTIONAL { ?p wdt:P569 ?dob }\n} LIMIT 200"}
    ],
    "wikidata-xxl": [
      {"family": "Select", "label": "★ Entities across all 6 shards", "view": "table", "cols": {"s": "Entity", "label": "Label", "desc": "Description"}, "tip": "This dataset is ONE logical graph split across 6 independent .rete shards (100M triples each). The query FANS ACROSS all 6 (UNION) and the rows merge — watch the banner say 'federated 6 sources'. HEAVY: it queries 6 remote ~900 MB R2 objects, so a fan-out can still take a minute. The architecture, not the speed, is the point: a dataset bigger than any single file, queried as one.", "q": "PREFIX rdfs: <http://www.w3.org/2000/01/rdf-schema#>\nPREFIX schema: <http://schema.org/>\nSELECT ?s ?label ?desc WHERE {\n  ?s rdfs:label ?label ; schema:description ?desc .\n  FILTER(LANG(?label) = \"en\") FILTER(LANG(?desc) = \"en\")\n} LIMIT 40"},
      {"family": "Select", "label": "People and their occupation", "view": "table", "cols": {"s": "Person", "name": "Name", "occ": "Occupation (QID)"}, "tip": "wdt:P106 = occupation. A star query (same subject), so every shard answers for its own people and the 6 partial results merge. ?occ is the occupation entity's QID — turn on the Labels toggle (or federate a label lookup) to read it.", "q": "PREFIX rdfs: <http://www.w3.org/2000/01/rdf-schema#>\nPREFIX wdt: <http://www.wikidata.org/prop/direct/>\nSELECT ?s ?name ?occ WHERE {\n  ?s wdt:P106 ?occ ; rdfs:label ?name .\n  FILTER(LANG(?name) = \"en\")\n} LIMIT 40"},
      {"family": "Select", "label": "Search every shard's labels for a word", "view": "table", "cols": {"s": "Entity", "label": "Label"}, "tip": "A FILTER(CONTAINS) over the English labels — whichever shards hold matching entities contribute rows. Change \"observatory\" to anything. Each shard does its own scan in parallel; the matches are unioned.", "q": "PREFIX rdfs: <http://www.w3.org/2000/01/rdf-schema#>\nSELECT ?s ?label WHERE {\n  ?s rdfs:label ?label .\n  FILTER(LANG(?label) = \"en\") FILTER(CONTAINS(?label, \"observatory\"))\n} LIMIT 30"},
      {"family": "Path", "label": "Instance-of statements (P31)", "view": "table", "cols": {"s": "Entity", "type": "Type (QID)"}, "tip": "Raw wdt:P31 (instance-of) edges drawn from across the 6 shards — 945k+ in the first shard alone. The Labels toggle decodes the class QID (e.g. Q5 = human). The selective sharded read: each shard faults in only the P31 tiles it needs.", "q": "PREFIX wdt: <http://www.wikidata.org/prop/direct/>\nSELECT ?s ?type WHERE {\n  ?s wdt:P31 ?type .\n} LIMIT 50"}
    ],
    "ohm-full": [
      {"family": "Geo", "label": "Map: British Empire across the centuries", "view": "map", "tip": "Switch Output -> Map. 315 boundary snapshots of the British Empire, bound by rdfs:label so it is a fast object-index lookup (a few range reads, not a scan). The overlapping polygons trace its territorial extent over time; hover a shape for its start year.", "q": "PREFIX ex: <http://ex/>\nPREFIX rdfs: <http://www.w3.org/2000/01/rdf-schema#>\nPREFIX geo: <http://www.opengis.net/ont/geosparql#>\nSELECT ?from ?to ?w WHERE {\n  ?x rdfs:label \"British Empire\"@en ;\n     geo:hasGeometry/geo:asWKT ?w ;\n     ex:startYear ?from ; ex:endYear ?to .\n}"},
      {"family": "Aggregate", "label": "Time: British Empire boundary snapshots", "view": "time", "tip": "Switch Output -> Time. When the empire's mapped extent was recorded - a multi-year heatmap of boundary snapshots per start year (from 1707). Bound by label, so it stays selective in lazy mode.", "q": "PREFIX ex: <http://ex/>\nPREFIX rdfs: <http://www.w3.org/2000/01/rdf-schema#>\nSELECT ?from ?x WHERE {\n  ?x rdfs:label \"British Empire\"@en ; ex:startYear ?from\n}"},
      {"family": "Geo", "label": "Map: empires compared", "view": "map", "tip": "Switch Output -> Map. Four historical empires' boundaries on one world map - VALUES binds the labels so each is a selective lookup, never a scan. Hover a polygon for its empire.", "q": "PREFIX ex: <http://ex/>\nPREFIX rdfs: <http://www.w3.org/2000/01/rdf-schema#>\nPREFIX geo: <http://www.opengis.net/ont/geosparql#>\nSELECT ?empire ?w WHERE {\n  VALUES ?empire { \"British Empire\"@en \"Mongol Empire\"@en \"Mughal Empire\"@en \"Empire colonial français\"@en }\n  ?x rdfs:label ?empire ; geo:hasGeometry/geo:asWKT ?w .\n}"},
      {"family": "Geo", "label": "Empires as map thumbnails (table)", "view": "table", "tip": "The SAME four empires, but as a TABLE - the ?w geometry column renders a per-row mini-map, so each empire's boundary draws in its own little square with lat-lon ticks on the borders. (The Map view above overlays them all on one basemap; this shows them side by side.) Auto-detects the geo:wktLiteral, or set a column's type dropdown to Map to force it.", "q": "PREFIX rdfs: <http://www.w3.org/2000/01/rdf-schema#>\nPREFIX geo: <http://www.opengis.net/ont/geosparql#>\nSELECT ?empire ?w WHERE {\n  VALUES ?empire { \"British Empire\"@en \"Mongol Empire\"@en \"Mughal Empire\"@en \"Empire colonial français\"@en }\n  ?x rdfs:label ?empire ; geo:hasGeometry/geo:asWKT ?w .\n}"},
      {"family": "Geo", "label": "Who ruled here? (point-in-polygon + time)", "view": "table", "tip": "A point (Berlin, 13.405 52.52) tested against every feature's polygon and filtered to the year 1900 returns the nested jurisdictions of that place and time: the city, the German Empire (Deutsches Reich) and the Kingdom of Prussia. Note: with no spatial index this scans geometry, so on the 150 MB planet it is slow in lazy mode - the bound-label examples above stay snappy.", "q": "PREFIX ex: <http://ex/>\nPREFIX rdfs: <http://www.w3.org/2000/01/rdf-schema#>\nPREFIX geo: <http://www.opengis.net/ont/geosparql#>\nPREFIX geof: <http://www.opengis.net/def/function/geosparql/>\nSELECT ?label ?s ?e WHERE {\n  ?x rdfs:label ?label ; ex:startYear ?s ; ex:endYear ?e ;\n     geo:hasGeometry/geo:asWKT ?w .\n  FILTER(?s <= 1900 && ?e >= 1900)\n  FILTER(geof:sfContains(?w, \"POINT(13.405 52.52)\"^^geo:wktLiteral))\n}"},
      {"family": "Select", "label": "Find a place by name", "view": "table", "tip": "A bound object on rdfs:label routes to just the tiles holding that name - a few range reads of a 150 MB file. Abdera is an ancient Greek colony on the Thracian coast.", "q": "PREFIX ex: <http://ex/>\nPREFIX rdfs: <http://www.w3.org/2000/01/rdf-schema#>\nSELECT ?x ?s ?e WHERE {\n  ?x rdfs:label \"Abdera\"@en ; ex:startYear ?s ; ex:endYear ?e\n}"},
      {"family": "Select", "label": "All facts about one feature", "view": "table", "tip": "A bound subject is the most selective shape - minimal bytes fetched. This is the OHM node for Abdera; the IRI round-trips to openhistoricalmap.org.", "q": "SELECT ?p ?o WHERE { <https://www.openhistoricalmap.org/node/2095928201> ?p ?o }"},
      {"family": "Aggregate", "label": "How many features predate the Common Era?", "view": "table", "tip": "Counts features whose start year is negative (BCE) - 2,057. An aggregate scans the whole ex:startYear predicate, so it fetches more tiles than the selective examples above.", "q": "PREFIX ex: <http://ex/>\nSELECT (COUNT(*) AS ?n) WHERE { ?x ex:startYear ?s . FILTER(?s < 0) }"},
      {"family": "Select", "label": "The oldest things on the map", "view": "table", "tip": "Order every feature by its start year, oldest first - deep-BCE sites (down to the -10000 clamp): ancient settlements, megaliths and prehistoric landmarks.", "q": "PREFIX ex: <http://ex/>\nPREFIX rdfs: <http://www.w3.org/2000/01/rdf-schema#>\nSELECT ?label ?s ?e WHERE {\n  ?x rdfs:label ?label ; ex:startYear ?s ; ex:endYear ?e\n} ORDER BY ?s LIMIT 25"}
    ],
    "wikidata-zenodo": [
      { family: "Select", label: "Everything about Plato", view: "table",
        tip: "A bound subject is the most selective shape: SPO routing jumps straight to the tiles holding wd:Q859, so the reader fetches only a few HTTP ranges - from the Zenodo DOI, never a scan of the 1.5 GB.",
        q: `SELECT ?p ?o WHERE { <http://www.wikidata.org/entity/Q859> ?p ?o }` },
      { family: "Select", label: "20 typed entities (instance-of)", view: "table",
        tip: "A cheap bound-predicate scan over a gigabyte hosted on Zenodo. Wikidata types entities with wdt:P31 (instance of), not rdf:type; reads a few MB, not the whole file.",
        q: `SELECT ?s ?type WHERE { ?s <http://www.wikidata.org/prop/direct/P31> ?type } LIMIT 20` },
      { family: "Select", label: "Physicists who were also philosophers", view: "table",
        tip: "Occupation intersection: P106 = physicist (Q169470) AND philosopher (Q4964182), with an English label. A selective join, every byte range-fetched from the Zenodo record.",
        q: `PREFIX wdt: <http://www.wikidata.org/prop/direct/>
PREFIX rdfs: <http://www.w3.org/2000/01/rdf-schema#>
SELECT ?p ?who WHERE {
  ?p wdt:P106 <http://www.wikidata.org/entity/Q169470> ;
     wdt:P106 <http://www.wikidata.org/entity/Q4964182> ;
     rdfs:label ?who . FILTER(LANG(?who) = "en")
}` },
      { family: "Aggregate", label: "Most common occupations", view: "table",
        tip: "GROUP BY + COUNT over the P106 predicate. Aggregates touch more tiles than a bound query but still only the occupation slices - a good stress test of HTTP range-reads against Zenodo.",
        q: `PREFIX wdt: <http://www.wikidata.org/prop/direct/>
SELECT ?occ (COUNT(?p) AS ?n) WHERE {
  ?p wdt:P106 ?occ
}
GROUP BY ?occ
ORDER BY DESC(?n)
LIMIT 15` }
    ],
    wikidata: [
      {
        family: "Select",
        label: "All facts about an entity",
        view: "table",
        tip: "A bound subject is the most selective query shape: SPO routing jumps straight to the tiles holding Bemelen (entity Q100001, a small Dutch village), so the reader fetches only a few HTTP ranges of the whole file - never a scan. Its coordinate comes back as a geo:wktLiteral, the datatype reconstructed during the parquet -> rete build.",
        q: `SELECT ?p ?o WHERE { <http://www.wikidata.org/entity/Q100001> ?p ?o }`
      },
      {
        family: "Select",
        label: "Labels of an entity, across languages",
        view: "table",
        tip: "Bound subject + bound predicate (rdfs:label) is the tightest shape of all - it touches only the label tiles for Bemelen (Q100001), a handful of bytes. The second column is each label's language tag (LANG), so you can read the same entity named across dozens of languages instead of a single opaque row.",
        q: `PREFIX rdfs: <http://www.w3.org/2000/01/rdf-schema#>
SELECT ?label (LANG(?label) AS ?language) WHERE {
  <http://www.wikidata.org/entity/Q100001> rdfs:label ?label
} LIMIT 50`
      },
      {
        family: "Select",
        label: "Find a thing by its name",
        view: "table",
        tip: "The inverse of looking up by Q-id - start from a human string. CONTAINS scans rdfs:label for the text you type (here 'bemelen', matching the village Q100001) and returns each entity paired with its name. A plain substring search faults more label tiles than a bound lookup; for big graphs rete can build an optional TEXT_INDEX so this becomes a few range reads instead of a scan.",
        q: `PREFIX rdfs: <http://www.w3.org/2000/01/rdf-schema#>
SELECT ?entity ?name WHERE {
  ?entity rdfs:label ?name .
  FILTER(CONTAINS(LCASE(STR(?name)), "bemelen"))
} LIMIT 25`
      },
      {
        family: "Select",
        label: "Coordinates of a place",
        view: "table",
        tip: "wdt:P625 is Wikidata's 'coordinate location' property. For Bemelen (Q100001) it returns a single geo:wktLiteral point - the WKT datatype recovered during the parquet -> rete conversion, so it round-trips as proper GeoSPARQL rather than an opaque string.",
        q: `PREFIX wdt: <http://www.wikidata.org/prop/direct/>
SELECT ?coord WHERE {
  <http://www.wikidata.org/entity/Q100001> wdt:P625 ?coord
}`
      },
      {
        family: "Path",
        label: "Subclasses of a class (with names)",
        view: "table",
        tip: "A reverse bound-object lookup: every class that declares wdt:P279 (subclass of) -> Q515, the entity for 'city'. The bound object routes to just the POS tiles for that one edge, and the OPTIONAL rdfs:label join adds an English-name column so you read 'town, capital, seaport, ...' instead of bare Q-ids.",
        q: `PREFIX wdt: <http://www.wikidata.org/prop/direct/>
PREFIX rdfs: <http://www.w3.org/2000/01/rdf-schema#>
SELECT ?sub ?name WHERE {
  ?sub wdt:P279 <http://www.wikidata.org/entity/Q515> .
  OPTIONAL { ?sub rdfs:label ?name . FILTER(LANG(?name) = "en") }
} LIMIT 50`
      },
      {
        family: "Select",
        label: "Physicists who are also philosophers (in 1 GB)",
        view: "table",
        tip: "The occupation intersection (people who are both wdt:P106 physicist Q169470 and philosopher Q4964182) on the lazy 1 GB file. Optimised shape: the intersection runs in a subquery with LIMIT, so the name, birth year (wdt:P569) and English description (schema:description) are resolved for only the ~25 people that pass - not the thousands in each occupation. Reading both whole occupation sets to intersect them is still the heavy part; enable Settings -> Range cache so a re-run is near-instant.",
        q: `PREFIX wdt: <http://www.wikidata.org/prop/direct/>
PREFIX wd: <http://www.wikidata.org/entity/>
PREFIX rdfs: <http://www.w3.org/2000/01/rdf-schema#>
PREFIX schema: <http://schema.org/>
SELECT ?p ?who ?year ?description ?image WHERE {
  { SELECT ?p WHERE {
      ?p wdt:P106 wd:Q169470 ;   # physicist
         wdt:P106 wd:Q4964182    # philosopher
    } LIMIT 25 }
  ?p rdfs:label ?who . FILTER(LANG(?who) = "en")
  OPTIONAL { ?p wdt:P569 ?dob . BIND(YEAR(?dob) AS ?year) }
  OPTIONAL { ?p schema:description ?description . FILTER(LANG(?description) = "en") }
  OPTIONAL { ?p wdt:P18 ?image }
}`
      },
      {
        family: "Select",
        label: "Painters who are also writers",
        view: "table",
        tip: "Another occupation intersection - people who are both wdt:P106 painter (Q1028181) and writer (Q36180), polymaths like William Blake. The remote-aware join scans both occupation sets in bulk and intersects them, then resolves the name and birth year for the handful that pass.",
        q: `PREFIX wdt: <http://www.wikidata.org/prop/direct/>
PREFIX wd: <http://www.wikidata.org/entity/>
PREFIX rdfs: <http://www.w3.org/2000/01/rdf-schema#>
PREFIX schema: <http://schema.org/>
SELECT ?p ?who ?year ?description ?image WHERE {
  { SELECT ?p WHERE {
      ?p wdt:P106 wd:Q1028181 ;   # painter
         wdt:P106 wd:Q36180       # writer
    } LIMIT 25 }
  ?p rdfs:label ?who . FILTER(LANG(?who) = "en")
  OPTIONAL { ?p wdt:P569 ?dob . BIND(YEAR(?dob) AS ?year) }
  OPTIONAL { ?p schema:description ?description . FILTER(LANG(?description) = "en") }
  OPTIONAL { ?p wdt:P18 ?image }
}`
      },
      {
        family: "Select",
        label: "Physicists born before 1900",
        view: "table",
        tip: "People by year: seed on a selective occupation (physicists, wdt:P106 Q169470), then keep those whose birth year - YEAR(wdt:P569) - is before 1900. Seeding on the occupation first means only physicists' birth dates are read, never the whole date-of-birth predicate. Oldest first.",
        q: `PREFIX wdt: <http://www.wikidata.org/prop/direct/>
PREFIX wd: <http://www.wikidata.org/entity/>
PREFIX rdfs: <http://www.w3.org/2000/01/rdf-schema#>
PREFIX schema: <http://schema.org/>
SELECT ?p ?who ?year ?description ?image WHERE {
  { SELECT ?p WHERE { ?p wdt:P106 wd:Q169470 } LIMIT 200 }
  ?p rdfs:label ?who . FILTER(LANG(?who) = "en")
  ?p wdt:P569 ?dob . BIND(YEAR(?dob) AS ?year)
  FILTER(?year < 1900)
  OPTIONAL { ?p schema:description ?description . FILTER(LANG(?description) = "en") }
  OPTIONAL { ?p wdt:P18 ?image }
} ORDER BY ?year LIMIT 25`
      },
      {
        family: "Select",
        label: "People born in Paris",
        view: "table",
        tip: "City of birth: a bound object on wdt:P19 (place of birth) -> Q90 (Paris) routes straight to the people born there - a selective lazy read - then resolves each one's name, birth year and occupation.",
        q: `PREFIX wdt: <http://www.wikidata.org/prop/direct/>
PREFIX wd: <http://www.wikidata.org/entity/>
PREFIX rdfs: <http://www.w3.org/2000/01/rdf-schema#>
PREFIX schema: <http://schema.org/>
SELECT ?p ?who ?year ?occupation ?description ?image WHERE {
  { SELECT ?p WHERE { ?p wdt:P19 wd:Q90 } LIMIT 25 }
  ?p rdfs:label ?who . FILTER(LANG(?who) = "en")
  OPTIONAL { ?p wdt:P569 ?dob . BIND(YEAR(?dob) AS ?year) }
  OPTIONAL { ?p wdt:P106 ?occ . ?occ rdfs:label ?occupation . FILTER(LANG(?occupation) = "en") }
  OPTIONAL { ?p schema:description ?description . FILTER(LANG(?description) = "en") }
  OPTIONAL { ?p wdt:P18 ?image }
}`
      },
      {
        family: "Select",
        label: "Painters born in Paris",
        view: "table",
        tip: "Occupation + city of birth: painters (wdt:P106 Q1028181) who were also born in Paris (wdt:P19 Q90). A double-bound intersection the remote-aware join handles by scanning both sets and intersecting, with each painter's birth year.",
        q: `PREFIX wdt: <http://www.wikidata.org/prop/direct/>
PREFIX wd: <http://www.wikidata.org/entity/>
PREFIX rdfs: <http://www.w3.org/2000/01/rdf-schema#>
PREFIX schema: <http://schema.org/>
SELECT ?p ?who ?year ?description ?image WHERE {
  { SELECT ?p WHERE {
      ?p wdt:P106 wd:Q1028181 ;   # painter
         wdt:P19 wd:Q90           # born in Paris
    } LIMIT 25 }
  ?p rdfs:label ?who . FILTER(LANG(?who) = "en")
  OPTIONAL { ?p wdt:P569 ?dob . BIND(YEAR(?dob) AS ?year) }
  OPTIONAL { ?p schema:description ?description . FILTER(LANG(?description) = "en") }
  OPTIONAL { ?p wdt:P18 ?image }
}`
      },
      {
        family: "Select",
        label: "Composers born in the 1700s",
        view: "table",
        tip: "People by era: seed on composers (wdt:P106 Q36834), then keep those born in the 18th century - 1700 <= YEAR(wdt:P569) < 1800 - Bach, Mozart, Haydn and contemporaries. Only composers' birth dates are read, oldest first.",
        q: `PREFIX wdt: <http://www.wikidata.org/prop/direct/>
PREFIX wd: <http://www.wikidata.org/entity/>
PREFIX rdfs: <http://www.w3.org/2000/01/rdf-schema#>
PREFIX schema: <http://schema.org/>
SELECT ?p ?who ?year ?description ?image WHERE {
  { SELECT ?p WHERE { ?p wdt:P106 wd:Q36834 } LIMIT 250 }
  ?p rdfs:label ?who . FILTER(LANG(?who) = "en")
  ?p wdt:P569 ?dob . BIND(YEAR(?dob) AS ?year)
  FILTER(?year >= 1700 && ?year < 1800)
  OPTIONAL { ?p schema:description ?description . FILTER(LANG(?description) = "en") }
  OPTIONAL { ?p wdt:P18 ?image }
} ORDER BY ?year LIMIT 25`
      },
      {
        family: "Select",
        label: "Writers and their country of citizenship",
        view: "table",
        tip: "Seed on a selective occupation (writers, wdt:P106 Q36180), then resolve each one's country of citizenship (wdt:P27) and birth year. Three readable columns from one occupation seed - a few range reads of the 1 GB file.",
        q: `PREFIX wdt: <http://www.wikidata.org/prop/direct/>
PREFIX wd: <http://www.wikidata.org/entity/>
PREFIX rdfs: <http://www.w3.org/2000/01/rdf-schema#>
PREFIX schema: <http://schema.org/>
SELECT ?p ?who ?country ?year ?description ?image WHERE {
  { SELECT ?p WHERE { ?p wdt:P106 wd:Q36180 } LIMIT 25 }
  ?p rdfs:label ?who . FILTER(LANG(?who) = "en")
  OPTIONAL { ?p wdt:P27 ?c . ?c rdfs:label ?country . FILTER(LANG(?country) = "en") }
  OPTIONAL { ?p wdt:P569 ?dob . BIND(YEAR(?dob) AS ?year) }
  OPTIONAL { ?p schema:description ?description . FILTER(LANG(?description) = "en") }
  OPTIONAL { ?p wdt:P18 ?image }
}`
      },
      {
        family: "Construct",
        label: "Build a small graph (export as TTL / JSON-LD)",
        view: "graph",
        tip: "A CONSTRUCT builds a NEW RDF graph - here a card for each scientist-philosopher (label, description, image). Because it produces triples, you can switch Output to TTL or JSON-LD to read and copy the serialization. (TTL / JSON-LD write triples, so they only apply to CONSTRUCT or DESCRIBE - a SELECT returns a table, not a graph.)",
        q: `PREFIX wdt: <http://www.wikidata.org/prop/direct/>
PREFIX wd: <http://www.wikidata.org/entity/>
PREFIX rdfs: <http://www.w3.org/2000/01/rdf-schema#>
PREFIX schema: <http://schema.org/>
CONSTRUCT {
  ?p rdfs:label ?who ; schema:description ?description ; wdt:P18 ?image .
} WHERE {
  { SELECT ?p WHERE {
      ?p wdt:P106 wd:Q169470 ; wdt:P106 wd:Q4964182
    } LIMIT 15 }
  ?p rdfs:label ?who . FILTER(LANG(?who) = "en")
  OPTIONAL { ?p schema:description ?description . FILTER(LANG(?description) = "en") }
  OPTIONAL { ?p wdt:P18 ?image }
}`
      }
    ],
    scholar: [
      {
        family: "Summary",
        label: "One query, all three engines (Whole · Progressive · Community)",
        strategy: "progressive",
        view: "table",
        tip: "The same answer — 6,954 triples — under every Strategy. Switch the dropdown and watch the machinery change: Whole index scans the full SPO index; Progressive answers exactly from the pyramid summary in ~3 small range reads, never touching the triple index; Split by community sums 13 per-community partials. Identical result, three different engines.",
        q: `SELECT (COUNT(*) AS ?triples) WHERE { ?s ?p ?o }`
      },
      {
        family: "Summary",
        label: "Predicate totals",
        strategy: "progressive",
        view: "table",
        tip: "Progressive strategy: exact per-predicate counts straight from the pyramid summary — the triple index is skipped. Also correct under Whole index and Split by community.",
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
        label: "Citation closure (Whole index only)",
        view: "table",
        tip: "Transitive cito:cites+ from one recent paper reaches 73 papers (citations only point backwards in time). A property path needs the Whole index: Progressive can't serve it from the summary, and Split by community refuses it (a pure path has nothing to split).",
        q: `PREFIX cito: <http://purl.org/spar/cito/>
SELECT DISTINCT ?reached WHERE { <http://ex/paper/245> cito:cites+ ?reached }`
      },
      {
        family: "Aggregate",
        label: "Most-cited papers",
        view: "table",
        tip: "Counts incoming cito:cites per paper and ranks them, exposing the preferential-attachment power law: a handful of papers soak up most of the citations while the long tail gets almost none.",
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
        tip: "Groups papers by ex:hasField and counts each, giving a Zipfian size distribution: genomics dominates the corpus while the remaining fields thin out quickly down the tail.",
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
        tip: "CONSTRUCTs a two-hop coauthorship ego network around author/105, the busiest hub: its direct coauthors (the first UNION branch) plus their coauthors (the second), drawn as a graph.",
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
        tip: "Progressive strategy: exact per-predicate counts read straight from the pyramid summary in a few small range reads, so the triple index is never touched. The numbers are identical under the Whole-index strategy.",
        q: `SELECT ?p (COUNT(*) AS ?triples) WHERE { ?s ?p ?o } GROUP BY ?p`
      },
      {
        family: "Select",
        label: "Mangled titles",
        view: "table",
        tip: "REGEX(?title, \"^  \") matches titles that start with stray leading whitespace - the formatting mess the noise knob injected into 20 of them - so it surfaces exactly those dirty records.",
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
        tip: "FILTER NOT EXISTS keeps only ex:Person records that have no ex:orcid triple at all - the 16 authors whose ORCID the noise knob stripped out. A classic data-completeness check.",
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
        tip: "CONSTRUCTs the noise as a graph: genomics papers (ex:hasField genomics) whose citations land in some other field. Real corpora rarely cross fields, so these edges are mostly the noise knob's rewires.",
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
    history: [
      {
        family: "Geo",
        label: "Map: territories of 1914",
        view: "map",
        tip: "Switch Output → Map for the whole picture (try the new Basemap dropdown). Or stay on Table: the ?w geometry column renders a per-row mini-map — each 1914 border drawn in its own square with lat-lon ticks. ?territory is the label.",
        q: `PREFIX geo: <http://www.opengis.net/ont/geosparql#>
PREFIX ex: <http://ex/>
PREFIX rdfs: <http://www.w3.org/2000/01/rdf-schema#>
SELECT ?territory ?w WHERE {
  ?t ex:year 1914 ; rdfs:label ?territory ;
     geo:hasGeometry/geo:asWKT ?w .
}`
      },
      {
        family: "Aggregate",
        label: "Time: territories per year",
        view: "time",
        tip: "Switch Output → Time. ?year drives a multi-year heatmap (323 BCE–1994 CE); a cell's colour is how many territories are recorded that year — hover to list them.",
        q: `PREFIX ex: <http://ex/>
PREFIX rdfs: <http://www.w3.org/2000/01/rdf-schema#>
SELECT ?year ?territory WHERE { ?t ex:year ?year ; rdfs:label ?territory }`
      },
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
    "jonas": [
      {"family": "Summary", "label": "What's in the graph (record types)", "view": "table", "tip": "The shape of the LostMa/Jonas corpus in one scan: 3,738 witnesses, 4,253 text parts, 3,153 manuscripts (documents), 1,152 places, 1,128 texts, plus physical descriptions, repositories, stories, stemmata and a 4,506-term controlled vocabulary. The whole file is ~2 MB, so even this scan is a couple of range reads.", "q": "PREFIX rdf: <http://www.w3.org/1999/02/22-rdf-syntax-ns#>\nPREFIX rdfs: <http://www.w3.org/2000/01/rdf-schema#>\nSELECT ?type ?label (COUNT(?s) AS ?n) WHERE {\n  ?s rdf:type ?type . OPTIONAL { ?type rdfs:label ?label }\n} GROUP BY ?type ?label ORDER BY DESC(?n) LIMIT 20"},
      {"family": "Select", "label": "Witnesses linking back to their live Jonas page", "view": "table", "tip": "A URL-ID column: prop:described_at_URL is each witness's permanent link back to the live Jonas repertory (jonas.irht.cnrs.fr/...). It renders as a clickable link - or set the column's type dropdown to Link. Paired with the witness siglum, so every row is a citable handle on the source record.", "q": "PREFIX p: <https://lostma-erc.github.io/jonas/prop/>\nSELECT ?siglum ?page WHERE {\n  ?w p:preferred_siglum ?siglum ; p:described_at_URL ?page .\n} LIMIT 100"},
      {"family": "Aggregate", "label": "The most-copied medieval texts", "view": "table", "tip": "Counts the surviving manuscript witnesses per text - the medieval 'bestsellers'. The prose Lancelot leads with 122 witnesses, then Tristan (93), Wolfram von Eschenbach's Parzival (88) and Willehalm (80), the Roman de Troie, Merlin and the Estoire del Saint Graal. p:is_manifestation_of links each witness to the text it copies.", "q": "PREFIX p: <https://lostma-erc.github.io/jonas/prop/>\nPREFIX rdfs: <http://www.w3.org/2000/01/rdf-schema#>\nSELECT ?text (COUNT(?w) AS ?witnesses) WHERE {\n  ?w p:is_manifestation_of ?t . ?t rdfs:label ?text .\n} GROUP BY ?t ?text ORDER BY DESC(?witnesses) LIMIT 25"},
      {"family": "Select", "label": "Witnesses of the Lancelot, with their manuscripts", "view": "table", "tip": "Every manuscript copy of the most-witnessed text, the prose Lancelot: each witness's siglum, the manuscript shelfmark it survives in (p:last_observed_in_doc) and its date. A bound text label keeps it selective - the core philological view, one work seen across all the books that carry it.", "q": "PREFIX p: <https://lostma-erc.github.io/jonas/prop/>\nPREFIX rdfs: <http://www.w3.org/2000/01/rdf-schema#>\nSELECT ?siglum ?shelfmark ?date WHERE {\n  ?t rdfs:label \"Lancelot\" .\n  ?w p:is_manifestation_of ?t ; p:preferred_siglum ?siglum .\n  OPTIONAL { ?w p:last_observed_in_doc ?d . ?d p:current_shelfmark ?shelfmark }\n  OPTIONAL { ?w p:date_freetext ?date }\n} LIMIT 100"},
      {"family": "Select", "label": "Texts and the manuscripts that carry them", "view": "table", "tip": "Walks witness -> text (p:is_manifestation_of) and witness -> manuscript (p:last_observed_in_doc) to show which texts survive in which books, with the witness siglum: the Roman van Limborch, the Roelantslied and the rest, each tied to a shelfmark. The witness IS the join - a text attested in a manuscript.", "q": "PREFIX p: <https://lostma-erc.github.io/jonas/prop/>\nPREFIX rdfs: <http://www.w3.org/2000/01/rdf-schema#>\nSELECT ?text ?shelfmark ?siglum WHERE {\n  ?w p:is_manifestation_of ?t ; p:last_observed_in_doc ?d ; p:preferred_siglum ?siglum .\n  ?t rdfs:label ?text . ?d p:current_shelfmark ?shelfmark .\n} LIMIT 100"},
      {"family": "Aggregate", "label": "The corpus by language", "view": "table", "tip": "The LostMa corpus is comparative and multilingual: Old French (fro) 309 texts, Middle High German (gmh) 150, Middle Irish (mga) 92, Middle French (frm) 91, Middle Dutch (dum) 85, Middle English (enm) 81. Each text's language is a controlled-vocabulary term (p:language_term -> skos:prefLabel).", "q": "PREFIX p: <https://lostma-erc.github.io/jonas/prop/>\nPREFIX skos: <http://www.w3.org/2004/02/skos/core#>\nSELECT ?language (COUNT(?t) AS ?texts) WHERE {\n  ?t p:language_term ?l . ?l skos:prefLabel ?language .\n} GROUP BY ?language ORDER BY DESC(?texts)"},
      {"family": "Select", "label": "Where the manuscripts are held", "view": "table", "tip": "Follows each manuscript to its holding place (p:location -> the library/place label): the Beinecke, the Leeds Brotherton library and hundreds more. Places carry GeoNames links (owl:sameAs) and repositories carry VIAF, so the books thread out into the linked-data web - switch the place column to Link to follow them.", "q": "PREFIX p: <https://lostma-erc.github.io/jonas/prop/>\nPREFIX rdfs: <http://www.w3.org/2000/01/rdf-schema#>\nSELECT ?shelfmark ?place WHERE {\n  ?d p:location ?pl ; p:current_shelfmark ?shelfmark . ?pl rdfs:label ?place .\n} LIMIT 100"},
      {"family": "Path", "label": "Texts with a reconstructed stemma", "view": "table", "tip": "The texts whose manuscript tradition has a published stemma codicum (p:in_stemma -> a stemma record with its openStemmata id): Fierabras, Girart de Vienne, Aliscans, Florence de Rome, the Chevalerie Ogier de Danemarche. These genealogies of surviving copies are what LostMa studies to model how texts were lost.", "q": "PREFIX p: <https://lostma-erc.github.io/jonas/prop/>\nPREFIX rdfs: <http://www.w3.org/2000/01/rdf-schema#>\nSELECT ?text ?stemma WHERE {\n  ?t p:in_stemma ?s ; rdfs:label ?text . OPTIONAL { ?s p:openstemmata_id ?stemma }\n} LIMIT 100"}
    ],
    "postscriptum": [
      {"family": "Select", "label": "Who wrote to whom, and when", "view": "table", "tip": "The correspondence network: each letter's sender (ps:sentBy) and recipient (ps:receivedBy) resolved to names, with its date - everyday Portuguese & Spanish letters from 1500 onward (Beatriz Carneira -> Francisco de Figueiredo, 1500-06-27). Sort the date column to walk three centuries of private mail.", "q": "PREFIX ps: <https://teitok.clul.ul.pt/postscriptum/ns#>\nPREFIX rdfs: <http://www.w3.org/2000/01/rdf-schema#>\nSELECT ?from ?to ?date WHERE {\n  ?l ps:sentBy ?s ; ps:receivedBy ?r ; ps:date ?date .\n  ?s rdfs:label ?from . ?r rdfs:label ?to .\n} ORDER BY ?date LIMIT 100"},
      {"family": "Select", "label": "Read a letter online (TEITOK link)", "view": "table", "tip": "A URL-ID column: foaf:page is each letter's permanent link to its edition on the live TEITOK site - click to read the diplomatic + modernised transcription with manuscript images. Paired with the letter's title (year, sender, recipient). Set the column type to Link if it doesn't auto-detect.", "q": "PREFIX ps: <https://teitok.clul.ul.pt/postscriptum/ns#>\nPREFIX foaf: <http://xmlns.com/foaf/0.1/>\nPREFIX dct: <http://purl.org/dc/terms/>\nSELECT ?title ?page WHERE {\n  ?l a ps:Letter ; dct:title ?title ; foaf:page ?page .\n} LIMIT 100"},
      {"family": "Aggregate", "label": "Letters by social register", "view": "table", "tip": "Post Scriptum classifies each letter's relationship (ps:letterType): personal (1,794) dominates, then family (926), friendship (525) and love (270) - the everyday emotional registers of 16th-18th-century correspondence, plus 42 anonymous (denunciations, threats).", "q": "PREFIX ps: <https://teitok.clul.ul.pt/postscriptum/ns#>\nSELECT ?type (COUNT(?l) AS ?letters) WHERE {\n  ?l ps:letterType ?type .\n} GROUP BY ?type ORDER BY DESC(?letters)"},
      {"family": "Select", "label": "Search the letters by content", "view": "table", "tip": "Full-text search over the modernised letter text (a text index backs it). Change the term (here 'amor') to any word to find the letters that use it - the actual language people wrote. Each row is the matching letter's title; add ?text to the SELECT to read the passage.", "q": "PREFIX ps: <https://teitok.clul.ul.pt/postscriptum/ns#>\nPREFIX dct: <http://purl.org/dc/terms/>\nSELECT ?title WHERE {\n  ?l a ps:Letter ; dct:title ?title ; ps:text ?text .\n  FILTER(CONTAINS(LCASE(STR(?text)), \"amor\"))\n} LIMIT 100"},
      {"family": "Aggregate", "label": "The busiest letter-writers", "view": "table", "tip": "Rank the 3,265 correspondents by how many surviving letters they sent (ps:sentBy). The network's hubs - writers whose archives preserved many letters (often from Inquisition or judicial cases, which is why the letters survive at all) - rise to the top.", "q": "PREFIX ps: <https://teitok.clul.ul.pt/postscriptum/ns#>\nPREFIX rdfs: <http://www.w3.org/2000/01/rdf-schema#>\nSELECT ?writer (COUNT(?l) AS ?letters) WHERE {\n  ?l ps:sentBy ?p . ?p rdfs:label ?writer .\n} GROUP BY ?writer ORDER BY DESC(?letters) LIMIT 20"},
      {"family": "Aggregate", "label": "What the letters are about (topics)", "view": "table", "tip": "Post Scriptum tags each letter with keyword terms (ps:keyword) - the recurring themes of everyday early-modern mail. Correspondence itself (448), Family (421), Money (349), Justice (265), Illness (263), Debts (198), Marriage (190): the texture of private life across three centuries. Spanish and Portuguese terms are tagged separately (@es / @pt).", "q": "PREFIX ps: <https://teitok.clul.ul.pt/postscriptum/ns#>\nSELECT ?keyword (COUNT(?l) AS ?letters) WHERE {\n  ?l ps:keyword ?keyword .\n} GROUP BY ?keyword ORDER BY DESC(?letters) LIMIT 25"},
      {"family": "Aggregate", "label": "Letters by language and century", "view": "table", "tip": "Each letter's sub-corpus (ps:corpus) encodes language x century: Spanish 1500-1800 (310 / 684 / 938 / 514) and Portuguese 1500-1800 (252 / 430 / 509 / 12). The 17th-18th centuries dominate - when literacy spread and the judicial / Inquisition archives that preserved these letters were most active.", "q": "PREFIX ps: <https://teitok.clul.ul.pt/postscriptum/ns#>\nSELECT ?corpus (COUNT(?l) AS ?letters) WHERE {\n  ?l ps:corpus ?corpus .\n} GROUP BY ?corpus ORDER BY ?corpus"}
    ],
    "databnf": [
      {"family": "Select", "label": "Authors and their VIAF / ISNI identity", "view": "table", "tip": "data.bnf.fr's authority layer: each author (foaf:Person) carries a name and owl:sameAs alignments to VIAF, ISNI and IdRef - the SAME VIAF ids that MMM, Jonas and Biblissima link to, so this is the federation hub for 'who is this author' across every manuscript dataset (8.8M sameAs links). The leading space in some names is BnF's sort form.", "q": "PREFIX foaf: <http://xmlns.com/foaf/0.1/>\nPREFIX owl: <http://www.w3.org/2002/07/owl#>\nSELECT ?name ?authority WHERE {\n  ?p a foaf:Person ; foaf:name ?name ; owl:sameAs ?authority .\n  FILTER(CONTAINS(STR(?authority), \"viaf\"))\n} LIMIT 100"},
      {"family": "Select", "label": "Authors joined to the works they created", "view": "table", "tip": "The single-file payoff: a cross-perimeter JOIN the 10 shards could NOT do (federation is union + routing, not term-level joins). Works (dcterms:creator -> an author ARK) join persons (foaf:name) in ONE graph, so 'this author wrote these works' resolves in a single query - no '+ Add source'.", "q": "PREFIX dct: <http://purl.org/dc/terms/>\nPREFIX foaf: <http://xmlns.com/foaf/0.1/>\nSELECT ?author ?title WHERE {\n  ?w dct:creator ?p ; dct:title ?title .\n  ?p foaf:name ?author .\n} LIMIT 100"},
      {"family": "Select", "label": "RAMEAU subject headings", "view": "table", "tip": "RAMEAU is the French national subject thesaurus - the controlled vocabulary the BnF indexes everything under. Each heading is a skos:Concept with a prefLabel (and broader/narrower hierarchy in the full graph).", "q": "PREFIX skos: <http://www.w3.org/2004/02/skos/core#>\nSELECT ?label WHERE {\n  ?c a skos:Concept ; skos:prefLabel ?label .\n} LIMIT 100"},
      {"family": "Summary", "label": "What's in the whole BnF (predicate totals)", "view": "table", "tip": "Predicate totals over the COMPLETE data.bnf.fr - 673.5M unique triples in ONE file (was 10 shards): rdf:type, dcterms:subject (RAMEAU), owl:sameAs (VIAF/ISNI/IdRef), the FRBNF id, titles, contributions, editions. The types pyramid + planner query_stats answer this kind of whole-graph summary index-free.", "q": "SELECT ?p (COUNT(*) AS ?n) WHERE { ?s ?p ?o } GROUP BY ?p ORDER BY DESC(?n) LIMIT 25"}
    ],
    "bne": [
      {"family": "Select", "label": "Authorities and their external identities", "view": "table", "tip": "The BNE authority layer: each authority carries a name (the BNE ontology's P5001) and owl:sameAs links out to VIAF, ISNI, GND, LoC, IdRef, DBpedia and data.bnf.fr - the same authority hubs the French BnF uses, which is why bne + databnf federate. Here, authorities with a VIAF alignment.", "q": "PREFIX owl: <http://www.w3.org/2002/07/owl#>\nPREFIX bne: <https://datos.bne.es/def/>\nSELECT ?name ?viaf WHERE {\n  ?p bne:P5001 ?name ; owl:sameAs ?viaf .\n  FILTER(CONTAINS(STR(?viaf), \"viaf.org\"))\n} LIMIT 100"},
      {"family": "Aggregate", "label": "What's in the BNE by class", "view": "table", "tip": "Group every resource by its rdf:type - the BNE's own ontology classes (datos.bne.es/def/C* codes: persons, corporate bodies, works/expressions, and the bibliographic manifestations that are the bulk). The types pyramid answers this whole-graph aggregate index-free.", "q": "SELECT ?class (COUNT(?s) AS ?n) WHERE {\n  ?s a ?class .\n} GROUP BY ?class ORDER BY DESC(?n) LIMIT 20"},
      {"family": "Select", "label": "BNE authorities that bridge to data.bnf.fr", "view": "table", "tip": "The Spanish-to-French bridge: BNE authorities whose owl:sameAs points at a data.bnf.fr id (Alfaro -> data.bnf.fr/10000803, etc.). Switch on the databnf dataset with '+ Add source' and these ids resolve in the French graph too.", "q": "PREFIX owl: <http://www.w3.org/2002/07/owl#>\nPREFIX bne: <https://datos.bne.es/def/>\nSELECT ?name ?bnf WHERE {\n  ?p bne:P5001 ?name ; owl:sameAs ?bnf .\n  FILTER(STRSTARTS(STR(?bnf), \"http://data.bnf.fr/\"))\n} LIMIT 100"},
      {"family": "Select", "label": "Federation: authority links across BNE + BnF", "view": "table", "fed": ["databnf"], "tip": "Federation is a kind of SPARQL. This pre-loads databnf (the whole data.bnf.fr) as a second source - see the Sources strip. The query asks for owl:sameAs->VIAF links, which BOTH graphs answer, so the result is the UNION of the Spanish (bne) and French (databnf) authority alignments to VIAF - the shared hub identifying the same person across both national libraries. (Cross-source term JOINS are a future feature; this is union + predicate routing.) Remove the databnf chip to query the BNE alone.", "q": "PREFIX owl: <http://www.w3.org/2002/07/owl#>\nSELECT ?person ?viaf WHERE {\n  ?person owl:sameAs ?viaf .\n  FILTER(CONTAINS(STR(?viaf), \"viaf.org\"))\n} LIMIT 100"},
      {"family": "Select", "label": "A profile from the RDA layer (Cervantes)", "view": "table", "tip": "datos.bne.es describes persons with RDA Elements (www.rdaregistry.info/Elements/a/P501xx): their professions (P50104), birthplace (P50119) and the period they worked in (P50101). Here, everything the RDA layer records for Cervantes - dramatist / novelist / poet / soldier, born in Alcala de Henares, of the Siglo de Oro. Change the name in P5001 to any BNE authority (a bound subject = a selective, fast read).", "q": "PREFIX bne: <https://datos.bne.es/def/>\nPREFIX rda: <http://www.rdaregistry.info/Elements/a/>\nSELECT ?profession ?birthplace ?period WHERE {\n  ?s bne:P5001 \"Cervantes Saavedra, Miguel de\" ;\n     rda:P50104 ?profession ; rda:P50119 ?birthplace ; rda:P50101 ?period .\n} LIMIT 50"},
      {"family": "Select", "label": "An author's links out (Wikipedia, VIAF, DBpedia)", "view": "table", "tip": "A URL-ID column: the BNE links each authority to the wider web - rdfs:seeAlso to Wikipedia, owl:sameAs to VIAF, DBpedia, GND, the Library of Congress, IdRef and data.bnf.fr. Here, all of Cervantes's outbound links (they render as clickable cells - set the column type to Link if needed). This is the federation hub: the same ids bne, databnf and Biblissima share to mean the same person.", "q": "PREFIX bne: <https://datos.bne.es/def/>\nPREFIX owl: <http://www.w3.org/2002/07/owl#>\nPREFIX rdfs: <http://www.w3.org/2000/01/rdf-schema#>\nSELECT ?link WHERE {\n  ?s bne:P5001 \"Cervantes Saavedra, Miguel de\" .\n  { ?s owl:sameAs ?link } UNION { ?s rdfs:seeAlso ?link }\n} LIMIT 50"}
    ],
    "biblissima": [
      {"family": "Select", "label": "Manuscripts you can SEE (IIIF thumbnails)", "view": "table", "tip": "The ?iiif column holds a IIIF manifest (prop/direct/P196) - the digitized manuscript at its holding library (Cambridge cudl, e-codices, the Vatican's digi.vatlib.it...). The playground fetches each manifest and renders it as a PAGED viewer right in the table: use the ‹ › buttons or the page box to leaf through the manuscript, and CLICK the image to open a lightbox - an enlarged page with prev/next paging, the manifest link and its metadata (each page faults from the IIIF Image API on demand). ?manuscript is the Biblissima entity, ?label its shelfmark.", "q": "PREFIX bd: <https://data.biblissima.fr/prop/direct/>\nPREFIX rdfs: <http://www.w3.org/2000/01/rdf-schema#>\nSELECT ?manuscript ?label ?iiif WHERE {\n  ?manuscript bd:P196 ?iiif ; rdfs:label ?label .\n  FILTER(LANG(?label) = \"en\")\n} LIMIT 24"},
      {"family": "Select", "label": "See manuscripts held in two cities", "view": "table", "tip": "A richer pull: digitized manuscripts (a IIIF manifest, P196) grouped by their holding collection (P194, whose label is 'City. Library'), filtered to TWO cities - here Paris and Geneve (STRSTARTS on the city prefix). Each ?iiif is a paged viewer (‹ › to leaf through; click to open the lightbox with the enlarged page + manifest metadata). NOTE: Biblissima records the current holding city, not a production->migration path, and these manuscript records carry no production date (P72/P73 'earliest/latest date' sit on other entity types) - for true manuscript MIGRATION between cities over time, see the MMM dataset (which, however, has no images). Swap the two cities for any others (Roma, Oxford, Cambridge...).", "q": "PREFIX bd: <https://data.biblissima.fr/prop/direct/>\nPREFIX rdfs: <http://www.w3.org/2000/01/rdf-schema#>\nSELECT ?city ?shelfmark ?iiif WHERE {\n  ?ms bd:P196 ?iiif ; bd:P194 ?c ; bd:P195 ?shelfmark .\n  ?c rdfs:label ?city . FILTER(LANG(?city) = \"fr\")\n  FILTER(STRSTARTS(?city, \"Paris\") || STRSTARTS(?city, \"Gen\"))\n} ORDER BY ?city LIMIT 24"},
      {"family": "Summary", "label": "What's in the graph (predicate totals)", "view": "table", "tip": "The shape of the whole Biblissima+ Wikibase - now ONE 254M-triple file (was 3 shards): rdf:type, wikibase:rank, multilingual labels (rdfs:label / skos:prefLabel / schema:name) and the prop/direct/P* truthy statements. Whole-graph totals; the P2-keyed types pyramid can answer this kind of summary index-free.", "q": "SELECT ?p (COUNT(*) AS ?n) WHERE { ?s ?p ?o } GROUP BY ?p ORDER BY DESC(?n) LIMIT 25"},
      {"family": "Select", "label": "Entities by their French label", "view": "table", "tip": "Biblissima describes manuscripts, works, persons, places and editions - all as Wikibase entities (data.biblissima.fr/entity/Q*) with multilingual labels. Here, every entity carrying a French rdfs:label (persons, saints, instruments, institutions...).", "q": "PREFIX rdfs: <http://www.w3.org/2000/01/rdf-schema#>\nSELECT ?entity ?label WHERE {\n  ?entity rdfs:label ?label . FILTER(LANG(?label) = \"fr\")\n} LIMIT 100"},
      {"family": "Aggregate", "label": "Entities grouped by type (instance of)", "view": "table", "tip": "Group entities by their type (prop/direct/P2 = 'instance of' in the Biblissima Wikibase), resolved to a French label - the composition of the whole graph (manuscripts, works, persons, places, editions...). Full totals now that it is a single file, not a shard.", "q": "PREFIX bd: <https://data.biblissima.fr/prop/direct/>\nPREFIX rdfs: <http://www.w3.org/2000/01/rdf-schema#>\nSELECT ?typeLabel (COUNT(?s) AS ?n) WHERE {\n  ?s bd:P2 ?type . ?type rdfs:label ?typeLabel . FILTER(LANG(?typeLabel) = \"fr\")\n} GROUP BY ?typeLabel ORDER BY DESC(?n) LIMIT 20"},
      {"family": "Select", "label": "Authority names across languages", "view": "table", "tip": "Biblissima reconciles persons, works and places into multilingual authorities. Each entity's French rdfs:label paired with a skos:altLabel variant in another script or language (Arabic, Turkish, Italian, English transliterations of the same name) - the cross-lingual name graph that lets a manuscript catalogued in one tradition match a record in another.", "q": "PREFIX rdfs: <http://www.w3.org/2000/01/rdf-schema#>\nPREFIX skos: <http://www.w3.org/2004/02/skos/core#>\nSELECT ?label ?altLabel WHERE {\n  ?e rdfs:label ?label ; skos:altLabel ?altLabel .\n  FILTER(LANG(?label) = \"fr\")\n} LIMIT 100"}
    ],
    "albala": [
      {"family": "Aggregate", "label": "Records per archive", "view": "table", "tip": "The catalogue holds two archives of the Institución Colombina: the Archivo General del Arzobispado de Sevilla (AGAS, the Seville diocesan archive) with 41,930 records and the Archivo Catedral de Sevilla (ACS, the cathedral chapter) with 27,970. arcas:inArchive links each record to its holding archive, arcas:ArchivalRecord is the record type.", "q": "PREFIX a: <https://albala.icolombina.es/arcas/ns#>\nPREFIX rdfs: <http://www.w3.org/2000/01/rdf-schema#>\nSELECT ?archive (COUNT(?r) AS ?records) WHERE {\n  ?r a a:ArchivalRecord ; a:inArchive ?ar . ?ar rdfs:label ?archive .\n} GROUP BY ?archive ORDER BY DESC(?records)"},
      {"family": "Select", "label": "The earliest records (15th-16th century)", "view": "table", "tip": "Filters dates that end in a year 1400-1599 (dates come as '01/01/1424' or just '1424'). The Cathedral's oldest holdings surface: liturgical codices from 1424, chapter acts, and the censos / capellanías / patronatos (rents and chantry endowments) of the early 1500s. Switch the date column sort to read them chronologically.", "q": "PREFIX a: <https://albala.icolombina.es/arcas/ns#>\nPREFIX dct: <http://purl.org/dc/terms/>\nPREFIX rdfs: <http://www.w3.org/2000/01/rdf-schema#>\nSELECT ?title ?date ?archive ?signatura WHERE {\n  ?r a a:ArchivalRecord ; dct:title ?title ; dct:date ?date ;\n     a:inArchive ?ar ; a:signatura ?signatura . ?ar rdfs:label ?archive .\n  FILTER(REGEX(?date, \"1[45][0-9][0-9]$\"))\n} ORDER BY ?date LIMIT 100"},
      {"family": "Select", "label": "Marriage-licence files of the Archdiocese", "view": "table", "tip": "Free-text over the record titles (a full-text index backs the search). The Archdiocese's 'expedientes matrimoniales' - marriage-dispensation case files that name the couple and their Andalusian town (e.g. Jerez de la Frontera, Puerto Real) - are the bulk of AGAS and a major genealogical source for western Andalusia.", "q": "PREFIX a: <https://albala.icolombina.es/arcas/ns#>\nPREFIX dct: <http://purl.org/dc/terms/>\nSELECT ?title ?date ?signatura WHERE {\n  ?r a a:ArchivalRecord ; dct:title ?title ; dct:date ?date ; a:signatura ?signatura .\n  FILTER(CONTAINS(LCASE(STR(?title)), \"matrimon\"))\n} LIMIT 100"},
      {"family": "Path", "label": "A file and the series it belongs to", "view": "table", "tip": "dcterms:isPartOf threads every record to its parent in the ISAD description tree, so file -> series -> fonds reconstructs. Here each record paired with the title of the unit that contains it (e.g. a single marriage expediente under 'Expedientes matrimoniales ordinarios de Pueblos. Letra A.').", "q": "PREFIX a: <https://albala.icolombina.es/arcas/ns#>\nPREFIX dct: <http://purl.org/dc/terms/>\nSELECT ?title ?parentTitle WHERE {\n  ?r a a:ArchivalRecord ; dct:title ?title ; dct:isPartOf ?p .\n  ?p dct:title ?parentTitle .\n} LIMIT 100"},
      {"family": "Aggregate", "label": "Records by archival level", "view": "table", "tip": "ISAD describes archives as a hierarchy of levels - fonds, series, file (expediente), item. arcas:level records each unit's level; most of this catalogue is described at the 'unidad documental compuesta' (compound documentary unit) level, with the upper fonds/series levels far fewer.", "q": "PREFIX a: <https://albala.icolombina.es/arcas/ns#>\nSELECT ?level (COUNT(?r) AS ?records) WHERE {\n  ?r a a:ArchivalRecord ; a:level ?level .\n} GROUP BY ?level ORDER BY DESC(?records)"},
      {"family": "Select", "label": "Capellanías & patronatos (chantry endowments)", "view": "table", "tip": "Text search for 'capellan' over the titles: the capellanías and patronatos - endowed chantries and lay patronages - are a distinctive Cathedral-chapter holding, with their censos, tributos and property deeds. Each row gives the title, its date and the box signatura (arcas:signatura, e.g. 'Caja / 5853 - 2').", "q": "PREFIX a: <https://albala.icolombina.es/arcas/ns#>\nPREFIX dct: <http://purl.org/dc/terms/>\nSELECT ?title ?date ?signatura WHERE {\n  ?r a a:ArchivalRecord ; dct:title ?title ; a:signatura ?signatura .\n  OPTIONAL { ?r dct:date ?date }\n  FILTER(CONTAINS(LCASE(STR(?title)), \"capellan\"))\n} LIMIT 100"}
    ],
    "memoria": [
      {"family": "Aggregate", "label": "Victims by province of birth", "view": "table", "tip": "Counts named victims / repressed persons by province of birth (mc:bornInProvince -> a shared mc:Province node). Barcelona leads with 22,555, then Tarragona, Lleida and Girona - this corpus is Catalonia-heavy (the reparació-jurídica registry), with Múrcia, Almería and València next.", "q": "PREFIX mc: <https://w3id.org/rete/memoria/ns#>\nPREFIX rdfs: <http://www.w3.org/2000/01/rdf-schema#>\nSELECT ?province (COUNT(?v) AS ?victims) WHERE {\n  ?v mc:bornInProvince ?p . ?p rdfs:label ?province .\n} GROUP BY ?province ORDER BY DESC(?victims) LIMIT 15"},
      {"family": "Aggregate", "label": "How people died (Basque registry)", "view": "table", "tip": "The Euskadi víctimas registry records a manner of death (mc:cause). Combat deaths lead - 7,322 gudaris/milicianos killed in action and 5,953 on the rebel front - then deaths in captivity, and the executions: extrajudicial killings by rebels (1,143) and by Republicans (957), and 996 shot after court-martial (Consejo de Guerra).", "q": "PREFIX mc: <https://w3id.org/rete/memoria/ns#>\nSELECT ?cause (COUNT(?v) AS ?n) WHERE {\n  ?v mc:cause ?cause .\n} GROUP BY ?cause ORDER BY DESC(?n)"},
      {"family": "Geo", "label": "Map the mass graves (Catalonia)", "view": "map", "tip": "Switch Output -> Map (try the new Basemap dropdown to put them on satellite/streets). The 1,027 Catalan fosas carry WGS84 geometry (geo:asWKT) - each point is a documented Civil War mass grave. Stay on Table and the ?w column renders a per-row mini-map (each grave a dot on a small world map). Other regions' graves (Andalucía, Castilla y León, València) are in the graph too but without coordinates in their open data.", "q": "PREFIX mc: <https://w3id.org/rete/memoria/ns#>\nPREFIX geo: <http://www.opengis.net/ont/geosparql#>\nPREFIX rdfs: <http://www.w3.org/2000/01/rdf-schema#>\nSELECT ?grave ?w WHERE {\n  ?g a mc:MassGrave ; rdfs:label ?grave ; geo:asWKT ?w .\n} LIMIT 1027"},
      {"family": "Select", "label": "Largest mass graves by victim count", "view": "table", "tip": "Graves ordered by mc:victimCount: the largest are in Andalucía - Huelva and Órgiva (Granada) at ~5,000, Málaga ~4,000, Sevilla ~3,600, Granada ~2,992 - reflecting the mass extrajudicial killings of 1936 in the south. Counts are estimates from each regional registry.", "q": "PREFIX mc: <https://w3id.org/rete/memoria/ns#>\nPREFIX rdfs: <http://www.w3.org/2000/01/rdf-schema#>\nSELECT ?grave ?victims WHERE {\n  ?g a mc:MassGrave ; rdfs:label ?grave ; mc:victimCount ?victims .\n} ORDER BY DESC(?victims) LIMIT 25"},
      {"family": "Select", "label": "Find a person by surname", "view": "table", "tip": "A full-text index over names backs this. Change the literal (here a CONTAINS on 'GARCIA') to any surname to pull matching victims with their birthplace - the personal layer of the registries. Names are stored as 'SURNAME1 SURNAME2, Given'.", "q": "PREFIX mc: <https://w3id.org/rete/memoria/ns#>\nPREFIX rdfs: <http://www.w3.org/2000/01/rdf-schema#>\nSELECT ?name ?birth WHERE {\n  ?v a mc:Victim ; rdfs:label ?name .\n  OPTIONAL { ?v mc:bornInMunicipality ?birth }\n  FILTER(CONTAINS(UCASE(STR(?name)), \"GARCIA\"))\n} LIMIT 100"},
      {"family": "Aggregate", "label": "Provinces with both victims and graves", "view": "table", "tip": "The province nodes join the two layers. Two sub-aggregates - victims born per province and mass graves per province - joined on the shared mc:Province, so you see where the documented dead and the documented graves coincide. One query instead of two because mc:Province is the join key.", "q": "PREFIX mc: <https://w3id.org/rete/memoria/ns#>\nPREFIX rdfs: <http://www.w3.org/2000/01/rdf-schema#>\nSELECT ?province ?victims ?graves WHERE {\n  { SELECT ?p (COUNT(?v) AS ?victims) WHERE { ?v mc:bornInProvince ?p } GROUP BY ?p }\n  { SELECT ?p (COUNT(?g) AS ?graves) WHERE { ?g mc:province ?p } GROUP BY ?p }\n  ?p rdfs:label ?province .\n} ORDER BY DESC(?graves) LIMIT 20"}
    ],
    "chemotion": [{"family": "Summary", "label": "What's in the graph (predicate totals)", "view": "table", "tip": "The shape of the Chemotion KG in one scan: obo:BFO_0000178 (has-part) dominates at ~960k edges, then rdf:type, the NFDICore dataset/study properties and the ChEBI/CHMO annotation predicates. Summary-answerable - try the Progressive strategy to read it from the pyramid alone.", "q": "SELECT ?p (COUNT(*) AS ?triples) WHERE { ?s ?p ?o } GROUP BY ?p ORDER BY DESC(?triples) LIMIT 25"},{"family": "Aggregate", "label": "How many of each thing", "view": "table", "tip": "Instance counts per rdf:type: 20.7k datasets, 20.6k studies (obo:BFO_0000015 'process'), 4.9k substances (CHEBI_59999), 3.7k molecules (CHEBI_23367), 250 creators - plus the owl:Class / owl:Axiom rows from the merged CHMO + RXNO ontologies.", "q": "PREFIX rdfs: <http://www.w3.org/2000/01/rdf-schema#>\nSELECT ?type ?label (COUNT(?s) AS ?n) WHERE {\n  ?s a ?type . OPTIONAL { ?type rdfs:label ?label }\n} GROUP BY ?type ?label ORDER BY DESC(?n) LIMIT 20"},{"family": "Select", "label": "Molecules with their structure", "view": "table", "tip": "The 3,746 molecular entities (obo:CHEBI_23367) carry real cheminformatics - name, molecular formula and SMILES (InChI / InChIKey too). A bound class keeps this a selective tile read.", "q": "PREFIX rdfs: <http://www.w3.org/2000/01/rdf-schema#>\nPREFIX obo: <http://purl.obolibrary.org/obo/>\nPREFIX chebi: <http://purl.obolibrary.org/obo/chebi/>\nSELECT ?name ?formula ?smiles WHERE {\n  ?m a obo:CHEBI_23367 ; rdfs:label ?name ; chebi:formula ?formula ; chebi:smiles ?smiles\n} LIMIT 200"},{"family": "Aggregate", "label": "Most common molecular formulas", "view": "table", "tip": "Group the molecules by molecular formula - the recurring C/H/N/O skeletons in this lab's compound collection (isomers share a formula).", "q": "PREFIX chebi: <http://purl.obolibrary.org/obo/chebi/>\nSELECT ?formula (COUNT(?m) AS ?molecules) WHERE {\n  ?m chebi:formula ?formula\n} GROUP BY ?formula ORDER BY DESC(?molecules) LIMIT 20"},{"family": "Select", "label": "CHMO analytical methods", "view": "table", "tip": "The Chemical Methods Ontology the ELN annotates analyses with: ~3,000 method classes (colorimetry, amperometry, NMR / MS / IR / Raman spectroscopy, chromatography ...) merged in as owl:Class with rdfs:label.", "q": "PREFIX rdfs: <http://www.w3.org/2000/01/rdf-schema#>\nSELECT ?method ?name WHERE {\n  ?method a <http://www.w3.org/2002/07/owl#Class> ; rdfs:label ?name .\n  FILTER(STRSTARTS(STR(?method), \"http://purl.obolibrary.org/obo/CHMO\"))\n} ORDER BY ?name LIMIT 200"},{"family": "Path", "label": "Every subtype of spectroscopy", "view": "table", "tip": "A transitive rdfs:subClassOf+ walk down from CHMO_0000228 (spectroscopy) - acoustic emission, alpha-particle, electronic spectroscopy and dozens more. This deep method DAG is what powers the Schema pyramid and Reach.", "q": "PREFIX rdfs: <http://www.w3.org/2000/01/rdf-schema#>\nSELECT ?name WHERE {\n  ?sub rdfs:subClassOf+ <http://purl.obolibrary.org/obo/CHMO_0000228> ; rdfs:label ?name\n} ORDER BY ?name LIMIT 200"},{"family": "Construct", "label": "A molecule's chemistry card", "view": "graph", "tip": "Switch Output -> Graph. CONSTRUCT a small star around molecules - each linked to its formula, SMILES and InChIKey - drawable as node-link cards.", "q": "PREFIX rdfs: <http://www.w3.org/2000/01/rdf-schema#>\nPREFIX obo: <http://purl.obolibrary.org/obo/>\nPREFIX chebi: <http://purl.obolibrary.org/obo/chebi/>\nCONSTRUCT { ?m rdfs:label ?name ; chebi:formula ?f ; chebi:inchikey ?k }\nWHERE {\n  ?m a obo:CHEBI_23367 ; rdfs:label ?name ; chebi:formula ?f ; chebi:inchikey ?k\n} LIMIT 15"},{"family": "Select", "label": "Federation: resolve terms across chemotion + ChEBI", "view": "table", "fed": ["chebi-full"], "tip": "Federation is a kind of SPARQL. This pre-loads chebi-full (the entire ChEBI ontology, 8.83M triples) as a second source - see the Sources strip. The same query resolves five terms across BOTH files: chemotion answers the CHMO method + BFO process labels, chebi-full answers the CHEBI chemical labels - no single file has all five. Run it and the result merges both, with a per-source byte/row breakdown. Both are remote-lazy, so each answers over HTTP range; remove the chip to query chemotion alone.", "q": "PREFIX rdfs: <http://www.w3.org/2000/01/rdf-schema#>\nPREFIX obo: <http://purl.obolibrary.org/obo/>\nSELECT ?term ?label WHERE {\n  VALUES ?term { obo:CHEBI_15377 obo:CHEBI_27732 obo:CHMO_0000228 obo:BFO_0000015 obo:CHEBI_23367 }\n  ?term rdfs:label ?label\n} ORDER BY ?term"}],
    "chebi-full": [
      {"family": "Summary", "label": "What's in the ontology (predicate totals)", "view": "table", "tip": "The shape of all of ChEBI in one scan: rdf:type leads (1.2M, mostly the owl:Axiom cross-reference reification), then oboInOwl xrefs/synonyms, rdfs:subClassOf (380k classification edges), rdfs:label (205k), and the ChemROF structural predicates (formula, charge, mass, SMILES, InChI). Summary-answerable - try the Progressive strategy to read it from the pyramid alone.", "q": "SELECT ?p (COUNT(*) AS ?n) WHERE { ?s ?p ?o } GROUP BY ?p ORDER BY DESC(?n) LIMIT 25"},
      {"family": "Select", "label": "Caffeine's full chemistry card", "view": "table", "tip": "A bound molecule (obo:CHEBI_27732 = caffeine) returns its name plus the ChemROF structural data ChEBI carries: molecular formula, average mass, SMILES and InChIKey. A bound subject keeps this a selective tile read.", "q": "PREFIX rdfs: <http://www.w3.org/2000/01/rdf-schema#>\nPREFIX chemrof: <https://w3id.org/chemrof/>\nPREFIX obo: <http://purl.obolibrary.org/obo/>\nSELECT ?label ?formula ?mass ?smiles ?inchikey WHERE {\n  obo:CHEBI_27732 rdfs:label ?label ;\n    chemrof:generalized_empirical_formula ?formula ;\n    chemrof:mass ?mass ;\n    chemrof:smiles_string ?smiles ;\n    chemrof:inchi_key_string ?inchikey .\n}"},
      {"family": "Path", "label": "How caffeine is classified (ancestors)", "view": "table", "tip": "A transitive rdfs:subClassOf+ walk UP from caffeine, joined to labels: trimethylxanthine -> purine alkaloid -> ... -> molecular entity. The named ancestors are ChEBI's full classification of one compound (the blank-node restriction parents are filtered out by the label join).", "q": "PREFIX rdfs: <http://www.w3.org/2000/01/rdf-schema#>\nPREFIX obo: <http://purl.obolibrary.org/obo/>\nSELECT ?ancestor ?label WHERE {\n  obo:CHEBI_27732 rdfs:subClassOf+ ?ancestor .\n  ?ancestor rdfs:label ?label\n} LIMIT 100"},
      {"family": "Path", "label": "Every subtype of alkaloid", "view": "table", "tip": "A transitive rdfs:subClassOf+ walk DOWN from obo:CHEBI_22315 (alkaloid, 461 direct subclasses): the whole alkaloid sub-tree by name. This deep subClassOf DAG is what powers the Schema pyramid and Reach.", "q": "PREFIX rdfs: <http://www.w3.org/2000/01/rdf-schema#>\nPREFIX obo: <http://purl.obolibrary.org/obo/>\nSELECT ?sub ?name WHERE {\n  ?sub rdfs:subClassOf+ obo:CHEBI_22315 ; rdfs:label ?name\n} ORDER BY ?name LIMIT 200"},
      {"family": "Aggregate", "label": "Most common molecular formulas", "view": "table", "tip": "Group all ChEBI entities by ChemROF molecular formula - the recurring C/H/N/O skeletons across the whole ontology (isomers share a formula). A whole-predicate scan over ~195k formula values, so it reads more than a bound query.", "q": "PREFIX chemrof: <https://w3id.org/chemrof/>\nSELECT ?formula (COUNT(?m) AS ?molecules) WHERE {\n  ?m chemrof:generalized_empirical_formula ?formula\n} GROUP BY ?formula ORDER BY DESC(?molecules) LIMIT 20"},
      {"family": "Select", "label": "An amino acid's definition + synonyms", "view": "table", "tip": "ChEBI's human-readable layer: the textual definition (obo:IAO_0000115) and the exact synonyms of obo:CHEBI_33709 (amino acid). Swap the IRI for any class to read its definition.", "q": "PREFIX obo: <http://purl.obolibrary.org/obo/>\nPREFIX oio: <http://www.geneontology.org/formats/oboInOwl#>\nSELECT ?def ?synonym WHERE {\n  obo:CHEBI_33709 obo:IAO_0000115 ?def .\n  OPTIONAL { obo:CHEBI_33709 oio:hasExactSynonym ?synonym }\n}"},
      {"family": "Construct", "label": "A molecule's chemistry card (graph)", "view": "graph", "tip": "Switch Output -> Graph. CONSTRUCT a small star around caffeine - linked to its name, formula, SMILES and InChIKey - drawable as a node-link card.", "q": "PREFIX rdfs: <http://www.w3.org/2000/01/rdf-schema#>\nPREFIX chemrof: <https://w3id.org/chemrof/>\nPREFIX obo: <http://purl.obolibrary.org/obo/>\nCONSTRUCT {\n  obo:CHEBI_27732 rdfs:label ?l ; chemrof:generalized_empirical_formula ?f ; chemrof:smiles_string ?s ; chemrof:inchi_key_string ?k\n} WHERE {\n  obo:CHEBI_27732 rdfs:label ?l ; chemrof:generalized_empirical_formula ?f ; chemrof:smiles_string ?s ; chemrof:inchi_key_string ?k\n}"},
      {"family": "Select", "label": "Federation: resolve 5 terms across 2 ontologies", "view": "table", "fed": ["chemotion"], "tip": "Federation is a kind of SPARQL. This example pre-loads chemotion as a second source (see the Sources strip): no single file resolves all five labels - chebi-full answers the CHEBI_* terms, chemotion answers CHMO/BFO. Run it and the result merges both, with a per-source breakdown. Remove the chip to query chebi-full alone.", "q": "PREFIX rdfs: <http://www.w3.org/2000/01/rdf-schema#>\nPREFIX obo: <http://purl.obolibrary.org/obo/>\nSELECT ?term ?label WHERE {\n  VALUES ?term { obo:CHEBI_15377 obo:CHEBI_27732 obo:CHEBI_23367 obo:CHMO_0000228 obo:BFO_0000015 }\n  ?term rdfs:label ?label\n} ORDER BY ?term"}
    ],
    "linked-jazz": [{"family": "Select","label": "Musicians — name, photo & DBpedia page","view": "table","tip": "A table with two RICH column types. ?photo is an image URL (linkedjazz.org/.../<name>.png) that auto-renders as a thumbnail picture; ?person is a DBpedia IRI that renders as a clickable identity link. Every Linked Jazz musician carries a foaf:name and usually a dbo:thumbnail. If a column doesn't auto-detect, use the little type dropdown on its header to force Image / Link.","q": "PREFIX foaf: <http://xmlns.com/foaf/0.1/>\nPREFIX dbo: <http://dbpedia.org/ontology/>\nSELECT ?person ?name ?photo WHERE {\n  ?person foaf:name ?name ; dbo:thumbnail ?photo .\n} LIMIT 60"},{"family": "Summary","label": "Relationship-type totals","view": "table","tip": "How the jazz community is wired: knowsOf dominates (3,649 edges), then the typed ties influencedBy / collaborated_with / mentorOf. foaf:name and dbo:thumbnail are the per-person metadata.","q": "SELECT ?p (COUNT(*) AS ?triples) WHERE { ?s ?p ?o } GROUP BY ?p ORDER BY DESC(?triples)"},{"family": "Select","label": "Everything Mary Lou Williams said about people","view": "table","tip": "A bound subject (one of the 54 interviewed musicians) returns her whole ego: 216 facts, including 151 knowsOf links plus mentorOf, friendOf and playedTogether ties pulled from her oral-history transcript.","q": "PREFIX foaf: <http://xmlns.com/foaf/0.1/>\nSELECT ?rel ?name WHERE {\n  <http://dbpedia.org/resource/Mary_Lou_Williams> ?rel ?other .\n  ?other foaf:name ?name .\n} ORDER BY ?rel LIMIT 100"},{"family": "Aggregate","label": "Most talked-about musicians","view": "table","tip": "In-degree across every social predicate, joined to names. Count Basie tops the network at 165 mentions, then Louis Armstrong (117) and Duke Ellington (72) - the gravitational centres of jazz memory.","q": "PREFIX foaf: <http://xmlns.com/foaf/0.1/>\nSELECT ?name (COUNT(?s) AS ?mentions) WHERE {\n  ?s ?rel ?person .\n  FILTER(?rel != foaf:name && ?rel != <http://dbpedia.org/ontology/thumbnail>)\n  ?person foaf:name ?name .\n} GROUP BY ?name ORDER BY DESC(?mentions) LIMIT 15"},{"family": "Path","label": "Who Count Basie reaches by word of mouth","view": "table","tip": "A transitive rel:knowsOf+ closure from a hub that is both a major subject and the #1 object. Because 40 of the 54 ego-musicians cross-reference each other, the chain hops Basie -> his circle -> their circles across the network. The result is the reachable set of people, listed by name.","q": "PREFIX rel: <http://purl.org/vocab/relationship/>\nPREFIX foaf: <http://xmlns.com/foaf/0.1/>\nSELECT DISTINCT ?name WHERE {\n  <http://dbpedia.org/resource/Count_Basie> rel:knowsOf+ ?reached .\n  ?reached foaf:name ?name .\n} ORDER BY ?name LIMIT 100"},{"family": "Aggregate","label": "Most-cited influences","view": "table","tip": "Restricting to rel:influencedBy reveals the acknowledged masters: Louis Armstrong (21), Count Basie (17) and Duke Ellington (11) are named most often as the people who shaped others.","q": "PREFIX foaf: <http://xmlns.com/foaf/0.1/>\nPREFIX rel: <http://purl.org/vocab/relationship/>\nSELECT ?name (COUNT(?s) AS ?cited) WHERE {\n  ?s rel:influencedBy ?infl .\n  ?infl foaf:name ?name .\n} GROUP BY ?name ORDER BY DESC(?cited) LIMIT 12"},{"family": "Construct","label": "Mary Lou Williams collaboration ego-network","view": "graph","tip": "Builds a drawable subgraph of who she actually made music with - mo:collaborated_with + lj:playedTogether edges (14 of them) - then re-labels each node so the renderer shows real names instead of IRIs.","q": "PREFIX foaf: <http://xmlns.com/foaf/0.1/>\nPREFIX mo: <http://purl.org/ontology/mo/>\nPREFIX lj: <http://linkedjazz.org/ontology/>\nCONSTRUCT {\n  <http://dbpedia.org/resource/Mary_Lou_Williams> ?p ?other .\n  ?other foaf:name ?name .\n} WHERE {\n  <http://dbpedia.org/resource/Mary_Lou_Williams> ?p ?other .\n  FILTER(?p = mo:collaborated_with || ?p = lj:playedTogether)\n  ?other foaf:name ?name .\n}"}],
    "getty-ulan": [{"family": "Select","label": "The pupils of Rembrandt","view": "graph","tip": "Bound-subject star: everyone Rembrandt (ulan:500011051) is recorded as teacher of, with their bios. The selective shape lazy access wants.","q": "PREFIX gvp: <http://vocab.getty.edu/ontology#>\nPREFIX skos: <http://www.w3.org/2004/02/skos/core#>\nPREFIX schema: <http://schema.org/>\nPREFIX ulan: <http://vocab.getty.edu/ulan/>\nSELECT ?pupil ?name ?bio WHERE {\n  ulan:500011051 gvp:teacherOf ?pupil .\n  OPTIONAL { ?pupil skos:prefLabel ?name }\n  OPTIONAL { ?pupil schema:description ?bio }\n}"},{"family": "Path","label": "Artistic descendants of Rembrandt (transitive)","view": "graph","tip": "Follows gvp:teacherOf+ down the master->pupil lineage from one bound seed - returns ~369 descendants.","q": "PREFIX gvp: <http://vocab.getty.edu/ontology#>\nPREFIX skos: <http://www.w3.org/2004/02/skos/core#>\nPREFIX ulan: <http://vocab.getty.edu/ulan/>\nSELECT DISTINCT ?descendant ?name WHERE {\n  ulan:500011051 gvp:teacherOf+ ?descendant .\n  OPTIONAL { ?descendant skos:prefLabel ?name }\n} LIMIT 500"},{"family": "Path","label": "Two-generation teaching chains","view": "graph","tip": "master -> pupil -> grand-pupil triples: how technique passed down two academic generations.","q": "PREFIX gvp: <http://vocab.getty.edu/ontology#>\nPREFIX skos: <http://www.w3.org/2004/02/skos/core#>\nSELECT ?masterName ?pupilName ?grandPupilName WHERE {\n  ?master gvp:teacherOf ?pupil . ?pupil gvp:teacherOf ?grandPupil .\n  ?master skos:prefLabel ?masterName . ?pupil skos:prefLabel ?pupilName . ?grandPupil skos:prefLabel ?grandPupilName .\n} LIMIT 100"},{"family": "Aggregate","label": "Most prolific teachers","view": "table","tip": "Ranks masters by number of recorded pupils - surfaces the great academic studios (top result taught 163 students).","q": "PREFIX gvp: <http://vocab.getty.edu/ontology#>\nPREFIX skos: <http://www.w3.org/2004/02/skos/core#>\nPREFIX schema: <http://schema.org/>\nSELECT ?teacher ?name ?bio (COUNT(?pupil) AS ?pupils) WHERE {\n  ?teacher gvp:teacherOf ?pupil .\n  OPTIONAL { ?teacher skos:prefLabel ?name } OPTIONAL { ?teacher schema:description ?bio }\n} GROUP BY ?teacher ?name ?bio ORDER BY DESC(?pupils) LIMIT 25"},{"family": "Aggregate","label": "Teaching ties by nationality","view": "table","tip": "Counts master->pupil edges grouped by the teacher's nationality - which national schools dominate the lineage.","q": "PREFIX gvp: <http://vocab.getty.edu/ontology#>\nSELECT ?nationality (COUNT(*) AS ?teachingLinks) WHERE {\n  ?teacher gvp:teacherOf ?pupil ; gvp:nationality ?nationality .\n} GROUP BY ?nationality ORDER BY DESC(?teachingLinks) LIMIT 25"}],
    "causalgraph": [
      {"family": "Select", "label": "The causal model (cause → effect, confidence, lag)", "view": "table", "tip": "Every CausalEdge reifies a cause→effect link carrying hasConfidence (0-1: how sure the mechanism is) and hasTimeLag (seconds until the effect shows). The injection-moulding model, ranked by confidence: operator setpoint → injection pressure (0.98, +0s), pressure → mould fill (0.92, +2s), cooling time → warpage (0.81, +30s)...", "q": "PREFIX cg: <http://iwu.fraunhofer.de/causalgraph#>\nPREFIX rdfs: <http://www.w3.org/2000/01/rdf-schema#>\nSELECT ?cause ?effect ?confidence ?timeLag WHERE {\n  ?e cg:hasCause ?c ; cg:hasEffect ?f ; cg:hasConfidence ?confidence ; cg:hasTimeLag ?timeLag .\n  ?c rdfs:label ?cause . ?f rdfs:label ?effect .\n} ORDER BY DESC(?confidence)"},
      {"family": "Path", "label": "What causes a defect?", "view": "table", "tip": "The direct causes of the short-shot defect: insufficient injection pressure (0.80) and incomplete mould fill (0.65). Edges are REIFIED, so you walk node <- edge(hasEffect) and edge(hasCause) -> node. Trace further back (pressure <- setpoint) for the whole chain.", "q": "PREFIX cg: <http://iwu.fraunhofer.de/causalgraph#>\nPREFIX rdfs: <http://www.w3.org/2000/01/rdf-schema#>\nSELECT ?cause ?confidence WHERE {\n  ?effect rdfs:label \"Short-shot defect\" .\n  ?e cg:hasEffect ?effect ; cg:hasCause ?c ; cg:hasConfidence ?confidence .\n  ?c rdfs:label ?cause .\n} ORDER BY DESC(?confidence)"},
      {"family": "Path", "label": "Downstream effects of injection pressure", "view": "table", "tip": "Everything injection pressure directly causes - mould fill (0.92, +2s), the short-shot defect (0.80, +2s) and part warpage (0.60, +25s). The time-lags reveal how long after a pressure change each effect appears.", "q": "PREFIX cg: <http://iwu.fraunhofer.de/causalgraph#>\nPREFIX rdfs: <http://www.w3.org/2000/01/rdf-schema#>\nSELECT ?effect ?confidence ?timeLag WHERE {\n  ?cause rdfs:label \"Injection pressure\" .\n  ?e cg:hasCause ?cause ; cg:hasEffect ?f ; cg:hasConfidence ?confidence ; cg:hasTimeLag ?timeLag .\n  ?f rdfs:label ?effect .\n} ORDER BY DESC(?confidence)"},
      {"family": "Select", "label": "Human knowledge vs algorithm-discovered", "view": "table", "tip": "causalgraph records WHERE each edge came from (hasCreator). Some links were ASSERTED by the process engineer (press physics - domain knowledge); others were DISCOVERED by a PCMCI causal-discovery run on the data. Provenance is first-class - you can trust, filter or audit the model by who claimed each edge.", "q": "PREFIX cg: <http://iwu.fraunhofer.de/causalgraph#>\nPREFIX rdfs: <http://www.w3.org/2000/01/rdf-schema#>\nSELECT ?link ?assertedBy WHERE {\n  ?e cg:hasCreator ?cr ; rdfs:label ?link .\n  ?cr rdfs:label ?assertedBy .\n} ORDER BY ?assertedBy"},
      {"family": "Summary", "label": "The ontology: classes & definitions", "view": "table", "tip": "The causalgraph ontology itself (the TBox, shipped in the same file). A CausalNode is an Event, State or Variable (split Human-input vs Machine); a CausalEdge connects two nodes with a direction + confidence; a Creator is the human or machine that asserted an individual. Definitions are the OWL rdfs:comment, verbatim.", "q": "PREFIX rdfs: <http://www.w3.org/2000/01/rdf-schema#>\nPREFIX owl: <http://www.w3.org/2002/07/owl#>\nSELECT ?class ?definition WHERE {\n  ?c a owl:Class ; rdfs:label ?class .\n  OPTIONAL { ?c rdfs:comment ?definition }\n} ORDER BY ?class"},
      {"family": "Path", "label": "The class hierarchy", "view": "table", "tip": "The taxonomy via rdfs:subClassOf: Machine_Variable -> Variable -> CausalNode; LearningAlgorithm_Creator -> Machine_Creator -> Creator. The 'origin' axis (Human-input vs Machine) and the 'kind' axis (Event / State / Variable) together classify every causal node.", "q": "PREFIX rdfs: <http://www.w3.org/2000/01/rdf-schema#>\nSELECT ?subclass ?superclass WHERE {\n  ?a rdfs:subClassOf ?b . ?a rdfs:label ?subclass . ?b rdfs:label ?superclass .\n} ORDER BY ?superclass ?subclass"}
    ],
    "ontoneurolog": [
      {"family": "Summary", "label": "The shape of the ontology (classes per module)", "view": "table", "tip": "OntoNeuroLOG is 35 OWL modules; this counts the owl:Class in each. The instruments-declaration module dominates (440 - every item of clinical scales like the BDI-II), then MR physics (mr-protocol 120, mr-sequence 46), the dataset taxonomies (dataset-processing 86, dataset 73) and the DOLCE 'particular' upper layer. A one-query map of where the modelling effort went.", "q": "PREFIX owl: <http://www.w3.org/2002/07/owl#>\nSELECT ?module (COUNT(?c) AS ?classes) WHERE {\n  ?c a owl:Class .\n  FILTER(STRSTARTS(STR(?c), \"http://www.irisa.fr\"))\n  BIND(REPLACE(STR(?c), \"^.*/([a-z-]+)-owl-lite.*$\", \"$1\") AS ?module)\n} GROUP BY ?module ORDER BY DESC(?classes)"},
      {"family": "Select", "label": "MR physics, defined", "view": "table", "tip": "The heart of the ontology: the 111 classes of the mr-sequence and mr-protocol modules with their scholarly rdfs:comment definitions. An MR-protocol 'specifies the actions to perform an MR dataset acquisition'; flip-angle, field-of-view, coil and 1H-nucleus are each defined with citations to the DICOM standard, Hornak (2006) and Boyle et al. Read it like a referenced glossary of MRI.", "q": "PREFIX rdfs: <http://www.w3.org/2000/01/rdf-schema#>\nPREFIX owl: <http://www.w3.org/2002/07/owl#>\nSELECT ?concept ?definition WHERE {\n  ?concept a owl:Class ; rdfs:comment ?definition .\n  FILTER(CONTAINS(STR(?concept), \"mr-sequence\") || CONTAINS(STR(?concept), \"mr-protocol\"))\n} ORDER BY ?concept"},
      {"family": "Summary", "label": "How the concepts connect (the grammar)", "view": "table", "tip": "The ontology's grammar: every owl:ObjectProperty with its rdfs:domain and rdfs:range - what may link to what. This is the DOLCE / participant-role backbone - an agentive is-agent-of-at an action, a functional-entity has-for-function a function, an action generates an action - the relations that turn a class list into a model.", "q": "PREFIX rdfs: <http://www.w3.org/2000/01/rdf-schema#>\nPREFIX owl: <http://www.w3.org/2002/07/owl#>\nSELECT ?property ?domain ?range WHERE {\n  ?property a owl:ObjectProperty ; rdfs:domain ?domain ; rdfs:range ?range .\n} ORDER BY ?property"},
      {"family": "Construct", "label": "The image-processing taxonomy (graph)", "view": "graph", "tip": "CONSTRUCT the rdfs:subClassOf tree of the dataset-processing module and render it as a node-and-arrow graph. These are neuroimaging pipeline operations - wavelet-transformation and unstructured-mesh-generation are kinds of dataset-transformation / mesh-generation; voxel-displacement-map-calculation is a calibration-model-estimation. The taxonomy a NeuroLOG processing service is classified against.", "q": "PREFIX rdfs: <http://www.w3.org/2000/01/rdf-schema#>\nCONSTRUCT { ?c rdfs:subClassOf ?parent }\nWHERE {\n  ?c rdfs:subClassOf ?parent .\n  FILTER(CONTAINS(STR(?c), \"dataset-processing\") && CONTAINS(STR(?parent), \"dataset-processing\"))\n} LIMIT 60"},
      {"family": "Select", "label": "Clinical assessment instruments (BDI-II)", "view": "table", "tip": "Beyond imaging, OntoNeuroLOG models the clinical scales used in neuroscience studies. Here every item of the BDI-II (Beck Depression Inventory II) - agitation, changes-in-appetite, concentration-difficulty... - each a labelled owl:Class in the instruments-declaration module, the kind of psychometric variable a study records alongside the MRI.", "q": "PREFIX rdfs: <http://www.w3.org/2000/01/rdf-schema#>\nPREFIX owl: <http://www.w3.org/2002/07/owl#>\nSELECT ?item ?label WHERE {\n  ?item a owl:Class ; rdfs:label ?label .\n  FILTER(CONTAINS(STR(?item), \"BDI-II\"))\n} ORDER BY ?label"}
    ],
    "nidm": [
      {"family": "Summary", "label": "The cohort: subjects by diagnosis", "view": "table", "tip": "The real ds000030 cohort (UCLA Consortium for Neuropsychiatric Phenomics), grouped by ncit:diagnosis: 130 healthy CONTROL, 50 SCHZ (schizophrenia), 49 BIPOLAR, 43 ADHD - 272 subjects in all. Each is a nidm:AcquisitionObject, the entity a scan produced.", "q": "PREFIX ncit: <http://ncitt.ncit.nih.gov/>\nSELECT ?diagnosis (COUNT(?s) AS ?subjects) WHERE {\n  ?s ncit:diagnosis ?diagnosis .\n} GROUP BY ?diagnosis ORDER BY DESC(?subjects)"},
      {"family": "Aggregate", "label": "Mean age per diagnosis group", "view": "table", "tip": "Age (ncit:age, 21-50 across the cohort) averaged within each diagnostic group. A one-line demographic profile of ds000030 - the kind of summary you would compute before a group analysis; the controls skew slightly younger than the clinical groups.", "q": "PREFIX ncit: <http://ncitt.ncit.nih.gov/>\nSELECT ?diagnosis (COUNT(?s) AS ?n) (AVG(?age) AS ?meanAge) WHERE {\n  ?s ncit:diagnosis ?diagnosis ; ncit:age ?age .\n} GROUP BY ?diagnosis ORDER BY DESC(?n)"},
      {"family": "Select", "label": "PROV: how each scan was produced", "view": "table", "tip": "NIDM's core is W3C PROV. Every acquisition records its provenance: prov:wasGeneratedBy the activity that produced it and prov:wasAttributedTo the agent responsible. This is the whole point of NIDM - the data carries where it came from, machine-readably.", "q": "PREFIX nidm: <http://purl.org/nidash/nidm#>\nPREFIX prov: <http://www.w3.org/ns/prov#>\nSELECT ?subject ?activity ?agent WHERE {\n  ?subject a nidm:AcquisitionObject ;\n           prov:wasGeneratedBy ?activity ;\n           prov:wasAttributedTo ?agent .\n} LIMIT 25"},
      {"family": "Summary", "label": "OntoNeuroLOG instruments, reused by NIDM", "view": "table", "tip": "NIDM-Experiment doesn't reinvent psychometrics - it REUSES OntoNeuroLOG's assessment-instrument taxonomy (at v3.0). Here those instrument concepts (assessment-instrument, questionnaire, behavioural-instrument, scale-item, numerical-score...) with their skos:definition, shipped inside this file. The bridge to the ontoneurolog dataset (see the next, federated, example).", "q": "PREFIX owl: <http://www.w3.org/2002/07/owl#>\nPREFIX skos: <http://www.w3.org/2004/02/skos/core#>\nSELECT ?instrument ?definition WHERE {\n  ?c a owl:Class ; skos:definition ?definition .\n  FILTER(STRSTARTS(STR(?c), \"http://neurolog.unice.fr/ontoneurolog/v3.0/instrument.owl#\"))\n  BIND(STRAFTER(STR(?c), \"#\") AS ?instrument)\n} ORDER BY ?instrument"},
      {"family": "Path", "label": "★ Cross-source: NIDM instruments × OntoNeuroLOG definitions", "view": "table", "fed": ["ontoneurolog"], "tip": "The real federation: ONE query split across TWO datasets and JOINED. NIDM adopted OntoNeuroLOG's instrument ontology at v3.0 (neurolog.unice.fr) while the ontoneurolog dataset holds it at v2.2 (irisa.fr); this file ships owl:sameAs links bridging the two. The playground routes ?v3 owl:sameAs ?v2 + ?v3 skos:definition to NIDM and ?v2 rdfs:comment to ontoneurolog (the only source with rdfs:comment), meeting on the shared ?v2 - so each concept (the ?v2 IRI names it, e.g. .../questionnaire) shows BOTH definitions, v2.2 and v3.0, both CDISC-sourced. 29 concepts defined on both sides; ontoneurolog is pre-added as a second source (see the Sources strip).", "q": "PREFIX owl: <http://www.w3.org/2002/07/owl#>\nPREFIX rdfs: <http://www.w3.org/2000/01/rdf-schema#>\nPREFIX skos: <http://www.w3.org/2004/02/skos/core#>\nSELECT ?v2 ?ontoneurologDef ?nidmDef WHERE {\n  ?v3 owl:sameAs ?v2 .\n  ?v3 skos:definition ?nidmDef .\n  ?v2 rdfs:comment ?ontoneurologDef .\n}"}
    ],
    "mira-wikidata": [
      {"family": "Select", "label": "The mappings", "view": "table", "tip": "The whole linkset: every MIrA entity reconciled to Wikidata as skos:exactMatch - 13 manual mappings (people + texts): Eriugena -> Q184500, Isidore -> Q166876, the Pauline Epistles -> Q265283... This is the shareable bridge; ?wikidata renders as a clickable link.", "q": "PREFIX skos: <http://www.w3.org/2004/02/skos/core#>\nSELECT ?subject ?wikidata WHERE {\n  ?subject skos:exactMatch ?wikidata .\n} ORDER BY ?subject"},
      {"family": "Select", "label": "A mapping with its SSSOM provenance", "view": "table", "tip": "A mapping is a CLAIM, not a fact - so SSSOM records who/how/how-sure. Each link is reified as an owl:Axiom carrying sssom:mapping_justification (semapv:ManualMappingCuration = a human curated it), sssom:confidence (1.0) and the subject_label. Here, the provenance of the Eriugena -> Wikidata mapping.", "q": "PREFIX owl: <http://www.w3.org/2002/07/owl#>\nSELECT ?property ?value WHERE {\n  ?ax owl:annotatedSource <https://mira.ie/entity/person/eriugena> ; ?property ?value .\n}"},
      {"family": "Summary", "label": "The mapping set's metadata", "view": "table", "tip": "The linkset describes ITSELF as a void:Linkset / sssom:MappingSet: license, creator, date, and void:linkPredicate (skos:exactMatch). This travelling metadata is what makes a mapping set citable + reusable - the whole point of SSSOM.", "q": "SELECT ?property ?value WHERE {\n  <https://mira.ie/mappings/mira-wikidata> ?property ?value .\n}"}
    ],
    "mira": [
      {"family": "Select", "label": "Manuscripts you can SEE (IIIF)", "view": "table", "tip": "189 of MIrA's manuscripts are digitised. The ?iiif column holds each one's IIIF manifest (wdt:P6108) from its holding library - the British Library, Trinity College Dublin, e-codices, the IRHT... The playground renders it as an in-table image viewer: page the folios with the ‹ › arrows, and CLICK a folio for the lightbox (enlarged page, prev/next, the manifest link and its metadata). ?manuscript is the shelfmark.", "q": "PREFIX wdt: <http://www.wikidata.org/prop/direct/>\nPREFIX rdfs: <http://www.w3.org/2000/01/rdf-schema#>\nSELECT ?manuscript ?iiif WHERE {\n  ?m wdt:P6108 ?iiif ; rdfs:label ?manuscript .\n} LIMIT 30"},
      {"family": "Aggregate", "label": "Where early Irish books were made", "view": "table", "tip": "wdt:P1071 'location of creation', resolved to its Wikidata label. Ireland leads (100), then Bobbio - the monastery Columbanus founded in Italy, a hub of Irish learning abroad - then France, Italy, Germany and Salzburg: the trail of the Irish 'peregrini' who carried their book-culture across early-medieval Europe.", "q": "PREFIX wdt: <http://www.wikidata.org/prop/direct/>\nPREFIX rdfs: <http://www.w3.org/2000/01/rdf-schema#>\nSELECT ?place (COUNT(?m) AS ?manuscripts) WHERE {\n  ?m wdt:P1071 ?p . ?p rdfs:label ?place .\n} GROUP BY ?place ORDER BY DESC(?manuscripts)"},
      {"family": "Select", "label": "The scholars, linked to Wikidata", "view": "table", "tip": "MIrA reconciles its named people to Wikidata with owl:sameAs - John Scottus Eriugena, Sedulius Scottus, Isidore, Dúngal... ?wikidata is a clickable link. Because of this alignment the dataset FEDERATES: add a Wikidata source with '+ Add source' and the ids join across both graphs.", "q": "PREFIX owl: <http://www.w3.org/2002/07/owl#>\nPREFIX rdfs: <http://www.w3.org/2000/01/rdf-schema#>\nSELECT ?person ?wikidata WHERE {\n  ?p owl:sameAs ?wikidata ; rdfs:label ?person .\n} ORDER BY ?person"},
      {"family": "Path", "label": "Cross-source JOIN: MIrA people × live Wikidata", "view": "table", "fed": [{"endpoint": "https://query.wikidata.org/sparql", "label": "Wikidata"}], "tip": "The real thing: ONE query split across TWO sources and JOINED. MIrA answers the first two patterns (owl:sameAs + the person's name); LIVE Wikidata answers ?wd wdt:P106 ?occ (their occupations); they meet on the shared ?wd. The playground routes each pattern to the source that can answer it - by predicate AND variable provenance, so the SECOND rdfs:label correctly goes to Wikidata (where ?occ lives), not MIrA - then bound-joins. This is NOT union (run-everywhere-and-merge); it's a genuine cross-source join. Wikidata is pre-added as a live SPARQL endpoint (see the Sources strip). Result: Eriugena -> theologian/translator, Isidore -> polymath/musicologist, Sedulius Scottus -> poet/grammarian.", "q": "PREFIX owl: <http://www.w3.org/2002/07/owl#>\nPREFIX rdfs: <http://www.w3.org/2000/01/rdf-schema#>\nPREFIX wdt: <http://www.wikidata.org/prop/direct/>\nSELECT ?person ?occupation WHERE {\n  ?p owl:sameAs ?wd ; rdfs:label ?person .\n  ?wd wdt:P106 ?occ . ?occ rdfs:label ?occupation .\n  FILTER(LANG(?occupation) = \"en\")\n} LIMIT 40"},
      {"family": "Path", "label": "★ SPARQL SERVICE: scholars' lifespans from live Wikidata", "view": "table", "cols": {"person": "Scholar", "born": "Born", "died": "Died"}, "tip": "The W3C way to federate, now supported by the ENGINE itself: SERVICE <endpoint> { … } inside the query ships that block to live Wikidata and joins the answers back on the shared ?wd. MIrA's owl:sameAs supplies the Wikidata ids; the VALUES pins three early-medieval scholars — Isidore of Seville (Q166876), John Scottus Eriugena (Q184500) and Sedulius Scottus (Q2609522) — because the block is sent AS WRITTEN, so keep it selective (don't ask Wikidata for every birth date it knows). OPTIONAL inside the block works too: the dates of death come back where Wikidata has them. Compare with the 'Cross-source JOIN' example above — same goal, but there the PLAYGROUND splits one pattern set across sources; here the split is written into the query, per the spec.", "q": "PREFIX owl: <http://www.w3.org/2002/07/owl#>\nPREFIX rdfs: <http://www.w3.org/2000/01/rdf-schema#>\nPREFIX wdt: <http://www.wikidata.org/prop/direct/>\nSELECT ?person ?born ?died WHERE {\n  ?p owl:sameAs ?wd ; rdfs:label ?person .\n  SERVICE <https://query.wikidata.org/sparql> {\n    VALUES ?wd { <http://www.wikidata.org/entity/Q166876> <http://www.wikidata.org/entity/Q184500> <http://www.wikidata.org/entity/Q2609522> }\n    ?wd wdt:P569 ?born .\n    OPTIONAL { ?wd wdt:P570 ?died }\n  }\n}"},
      {"family": "Path", "label": "Same join, but via a shareable mapping linkset", "view": "table", "fed": ["mira-wikidata", {"endpoint": "https://query.wikidata.org/sparql", "label": "Wikidata"}], "tip": "The SAME person -> occupation join, but the LINKS now live in their OWN file: the mira-wikidata SSSOM linkset (skos:exactMatch + provenance), federated in. THREE sources - the linkset answers ?p skos:exactMatch ?wd, MIrA the name, live Wikidata the occupation - routed by predicate AND variable provenance, then bound-joined. This is how mappings are SHARED in the community (SSSOM): a small, citable, queryable artifact decoupled from both datasets, rather than baked into one. Hit 🔍 Plan to see the 3-way routing before it runs.", "q": "PREFIX skos: <http://www.w3.org/2004/02/skos/core#>\nPREFIX rdfs: <http://www.w3.org/2000/01/rdf-schema#>\nPREFIX wdt: <http://www.wikidata.org/prop/direct/>\nSELECT ?person ?occupation WHERE {\n  ?p skos:exactMatch ?wd .\n  ?p rdfs:label ?person .\n  ?wd wdt:P106 ?occ . ?occ rdfs:label ?occupation .\n  FILTER(LANG(?occupation) = \"en\")\n} LIMIT 40"},
      {"family": "Aggregate", "label": "Datable manuscripts over time", "view": "time", "tip": "Switch Output -> Time. wdt:P571 'inception' is an xsd:gYear; each manuscript carries an earliest+latest year, so the heatmap shows the surviving corpus thickening through the 6th-10th centuries - the height of the Irish scribal achievement.", "q": "PREFIX wdt: <http://www.wikidata.org/prop/direct/>\nPREFIX rdfs: <http://www.w3.org/2000/01/rdf-schema#>\nSELECT ?manuscript ?date WHERE {\n  ?m wdt:P571 ?date ; rdfs:label ?manuscript .\n}"},
      {"family": "Select", "label": "The largest manuscripts", "view": "table", "tip": "Page dimensions in cm: wdt:P2048 height x wdt:P2049 width. The grand display gospel-books at the top vs the tiny 'pocket gospels' Irish monks carried on their travels - the physical range of early Irish book production.", "q": "PREFIX wdt: <http://www.wikidata.org/prop/direct/>\nPREFIX rdfs: <http://www.w3.org/2000/01/rdf-schema#>\nSELECT ?manuscript ?height ?width WHERE {\n  ?m wdt:P2048 ?height ; wdt:P2049 ?width ; rdfs:label ?manuscript .\n} ORDER BY DESC(?height) LIMIT 25"},
      {"family": "Select", "label": "Search the contents", "view": "table", "tip": "Full-text search (a text index backs it) over what each manuscript actually contains. Here, the 32 books carrying gospels; change 'gospel' to 'psalm', 'Priscian', 'computus', 'Isidore'... to trace where a given text survives. ?script is the hand (e.g. 'Irish minuscule').", "q": "PREFIX mira: <https://mira.ie/prop/>\nPREFIX rdfs: <http://www.w3.org/2000/01/rdf-schema#>\nSELECT ?manuscript ?script ?contents WHERE {\n  ?m mira:contents ?contents ; rdfs:label ?manuscript .\n  OPTIONAL { ?m mira:script ?script }\n  FILTER(CONTAINS(LCASE(STR(?contents)), \"gospel\"))\n}"},
      {"family": "Summary", "label": "What the catalogue describes", "view": "table", "tip": "Instance-of (P31) breakdown, resolved to Wikidata labels: ~300 manuscripts, the libraries that hold them, the named people (with Wikidata links) and the literary works they carry - the whole shape of MIrA in one scan.", "q": "PREFIX wdt: <http://www.wikidata.org/prop/direct/>\nPREFIX rdfs: <http://www.w3.org/2000/01/rdf-schema#>\nSELECT ?class (COUNT(?s) AS ?n) WHERE {\n  ?s wdt:P31 ?t . OPTIONAL { ?t rdfs:label ?class }\n} GROUP BY ?class ORDER BY DESC(?n)"},
      {"family": "Aggregate", "label": "Why each book is in the corpus", "view": "table", "tip": "MIrA's inclusion criteria (mira:category, straight from the project's 'About' page), each with its manuscript count - the published figures: Script: Irish (148), Origin: Ireland (106), Vernacular Old-Irish content (84), Named Irish scribe (41), Exemplar: Irish, Text of Irish origin, and the tentative 'Insular (Irish?)' outline categories. The reason a book joins the corpus, as data.", "q": "PREFIX mira: <https://mira.ie/prop/>\nPREFIX rdfs: <http://www.w3.org/2000/01/rdf-schema#>\nSELECT ?criterion (COUNT(?m) AS ?manuscripts) WHERE {\n  ?m mira:category ?c . ?c rdfs:label ?criterion .\n} GROUP BY ?criterion ORDER BY DESC(?manuscripts)"},
      {"family": "Select", "label": "The Old Irish glossed books (with images)", "view": "table", "tip": "Manuscripts carrying vernacular Old-Irish content (mira:category = 'Vernacular content') - the famous glossed books (Würzburg, Milan, St Gall...) where Irish scholars wrote their own language between the Latin lines. Joined to the IIIF image where digitised: click a folio to read the glosses.", "q": "PREFIX mira: <https://mira.ie/prop/>\nPREFIX wdt: <http://www.wikidata.org/prop/direct/>\nPREFIX rdfs: <http://www.w3.org/2000/01/rdf-schema#>\nSELECT ?manuscript ?iiif WHERE {\n  ?m mira:category <https://mira.ie/entity/category/vern> ; rdfs:label ?manuscript .\n  OPTIONAL { ?m wdt:P6108 ?iiif }\n}"}
    ],
    "bioexplora": [
      {"family": "Summary", "label": "The collection at a glance", "view": "table", "cols": {"collection": "Collection", "specimens": "Specimens"}, "tip": "The museum's six collections by specimen count: arthropods (MCNB-Art, 82,900) and molluscs (MCNB-Malac, 50,083) dominate, then paleontology (MGB), vertebrates (MCNB-Cord), the tissue bank and general zoology. 207,163 Darwin Core specimens in all.", "q": "PREFIX dwc: <http://rs.tdwg.org/dwc/terms/>\nSELECT ?collection (COUNT(*) AS ?specimens) WHERE {\n  ?s dwc:collectionCode ?collection .\n} GROUP BY ?collection ORDER BY DESC(?specimens)"},
      {"family": "Select", "label": "Specimens you can SEE (photos)", "view": "table", "cols": {"species": "Species", "collector": "Collector", "photo": "Photo"}, "tip": "~9,000 specimens are photographed. The ?photo column renders INLINE — molluscs, crustaceans, insects, fossils. Photos are mirrored to the bucket as fast WebP (p:preview, ~10-40 KB; the museum's source link is p:image), and ?collector comes from the shared Agent node. CC BY 4.0, credit the Museu de Ciencies Naturals de Barcelona.", "q": "PREFIX dwc: <http://rs.tdwg.org/dwc/terms/>\nPREFIX p: <https://bioexplora.cat/prop/>\nPREFIX rdfs: <http://www.w3.org/2000/01/rdf-schema#>\nSELECT ?species ?collector ?photo WHERE {\n  ?s dwc:scientificName ?species ; p:preview ?photo .\n  OPTIONAL { ?s p:collectedBy ?c . ?c rdfs:label ?collector }\n} LIMIT 40"},
      {"family": "Path", "label": "See it, hear it, place it — photo + sound + map", "view": "table", "cols": {"species": "Species", "photo": "Photo", "sound": "Call", "location": "Where found"}, "tip": "One row, four media, joined THROUGH THE GRAPH: a photographed+georeferenced specimen and a Xeno-canto recording that hang off the SAME species Taxon node (?spec p:taxon ?t, ?rec p:taxon ?t) — no string matching. A bird like the Great Crested Grebe shows its specimen photo (WebP), its call (inline player) and where it was collected (click the map cell for a full zoomable map). Stay in Table view to see all four.", "q": "PREFIX p: <https://bioexplora.cat/prop/>\nPREFIX rdfs: <http://www.w3.org/2000/01/rdf-schema#>\nPREFIX geo: <http://www.opengis.net/ont/geosparql#>\nSELECT ?species ?photo ?sound ?location WHERE {\n  ?t p:rank \"species\" ; rdfs:label ?species .\n  ?spec p:taxon ?t ; p:preview ?photo ; geo:asWKT ?location .\n  ?rec p:taxon ?t ; p:audio ?sound .\n} LIMIT 24"},
      {"family": "Path", "label": "The taxonomic tree (family › genus › species)", "view": "table", "cols": {"family": "Family", "genus": "Genus", "species": "Species"}, "tip": "The taxonomy is now a real GRAPH, not flat strings: every name is a Taxon node linked to its parent rank via p:parentTaxon. This walks species → its genus → its family, one row per species placed in the tree.", "q": "PREFIX p: <https://bioexplora.cat/prop/>\nPREFIX rdfs: <http://www.w3.org/2000/01/rdf-schema#>\nSELECT ?family ?genus ?species WHERE {\n  ?sp p:rank \"species\" ; rdfs:label ?species ; p:parentTaxon ?g .\n  ?g rdfs:label ?genus ; p:parentTaxon ?f .\n  ?f p:rank \"family\" ; rdfs:label ?family .\n} ORDER BY ?family ?genus LIMIT 40"},
      {"family": "Path", "label": "Walk a whole family down to its species", "view": "table", "cols": {"genus": "Genus", "species": "Species"}, "tip": "A TRANSITIVE traversal: p:parentTaxon+ collects every species under a family in one hop, however many ranks deep. Change 'Cerambycidae' (longhorn beetles) to Carabidae, Helicidae (snails), Muridae (rodents)… to walk any branch.", "q": "PREFIX p: <https://bioexplora.cat/prop/>\nPREFIX rdfs: <http://www.w3.org/2000/01/rdf-schema#>\nSELECT ?genus ?species WHERE {\n  ?fam p:rank \"family\" ; rdfs:label \"Cerambycidae\" .\n  ?sp p:parentTaxon+ ?fam ; p:rank \"species\" ; rdfs:label ?species ; p:parentTaxon ?g .\n  ?g rdfs:label ?genus .\n} ORDER BY ?genus LIMIT 40"},
      {"family": "Aggregate", "label": "Who collected the most", "view": "table", "cols": {"collector": "Collector", "specimens": "Specimens"}, "tip": "Collectors are shared Agent nodes (dwc:recordedBy) you can count and traverse. José Fernández de Villalta leads with ~32,000 records, ahead of the malacologists Rius Dalmau and Gasull. 3,876 named collectors in all.", "q": "PREFIX p: <https://bioexplora.cat/prop/>\nPREFIX rdfs: <http://www.w3.org/2000/01/rdf-schema#>\nSELECT ?collector (COUNT(?s) AS ?specimens) WHERE {\n  ?s p:collectedBy ?c . ?c rdfs:label ?collector .\n} GROUP BY ?collector ORDER BY DESC(?specimens) LIMIT 20"},
      {"family": "Path", "label": "A naturalist's collection (species + family + place + photo)", "view": "table", "cols": {"species": "Species", "family": "Family", "place": "Country", "photo": "Photo"}, "tip": "Everything one collector brought in, connected through the graph: the Agent node → each specimen → up the taxonomy to its family → out to the country it was found → its photo. Change the name to anyone from 'Who collected the most'.", "q": "PREFIX p: <https://bioexplora.cat/prop/>\nPREFIX dwc: <http://rs.tdwg.org/dwc/terms/>\nPREFIX rdfs: <http://www.w3.org/2000/01/rdf-schema#>\nSELECT ?species ?family ?place ?photo WHERE {\n  ?c rdfs:label \"Gasull i Martínez, Lluís\" .\n  ?s p:collectedBy ?c ; dwc:scientificName ?species .\n  OPTIONAL { ?s p:taxon ?sp . ?sp p:parentTaxon+ ?f . ?f p:rank \"family\" ; rdfs:label ?family }\n  OPTIONAL { ?s p:foundIn ?pl . ?pl rdfs:label ?place }\n  OPTIONAL { ?s p:preview ?photo }\n} LIMIT 30"},
      {"family": "Aggregate", "label": "Map of the collection", "view": "map", "cols": {"species": "Species", "point": "Location"}, "tip": "43,826 specimens are georeferenced (GeoSPARQL points). Switch Output -> Map to see where the holdings were collected across the world — Catalonia and Spain dominate, with material from Morocco, Mexico, France and beyond. Shows up to 3,000 points.", "q": "PREFIX dwc: <http://rs.tdwg.org/dwc/terms/>\nPREFIX geo: <http://www.opengis.net/ont/geosparql#>\nSELECT ?species ?point WHERE {\n  ?s dwc:scientificName ?species ; geo:asWKT ?point .\n} LIMIT 3000"},
      {"family": "Path", "label": "★ FEDERATED: specimen → its country's boundary polygon", "view": "map", "fed": ["geoadmin"], "cols": {"species": "Species", "country": "Country", "boundary": "Country boundary"}, "tip": "A CROSS-SOURCE join, the whole point of federation: bioexplora has the specimens (dwc:countryCode), geoadmin (auto-added as a source) has the country polygons. The query routes each block of patterns to its own dataset and joins on the ISO code — so every specimen pulls in the boundary polygon of the country it was collected in. Output → Map plots them as areas; Table view shows each boundary as a clickable mini-map. This is how you 'get the geospatial column' from another graph.", "q": "PREFIX dwc: <http://rs.tdwg.org/dwc/terms/>\nPREFIX g: <https://w3id.org/rete/geoadmin/prop/>\nPREFIX schema: <http://schema.org/>\nPREFIX geo: <http://www.opengis.net/ont/geosparql#>\nSELECT ?species ?country ?boundary WHERE {\n  ?s dwc:scientificName ?species ; dwc:countryCode ?code .\n  ?a g:iso ?code ; schema:name ?country ; geo:asWKT ?boundary .\n} LIMIT 20"},
      {"family": "Select", "label": "Type specimens (the holotypes)", "view": "table", "cols": {"species": "Species", "type": "Type status", "cat": "Catalogue №"}, "tip": "672 name-bearing TYPE specimens — the single most scientifically important objects in any natural-history collection, because each one anchors a species name. ?type is the kind (Holotype, Paratype, Syntype...), ?cat the catalogue number.", "q": "PREFIX dwc: <http://rs.tdwg.org/dwc/terms/>\nSELECT ?species ?type ?cat WHERE {\n  ?s dwc:typeStatus ?type ; dwc:scientificName ?species ; dwc:catalogNumber ?cat .\n} LIMIT 40"},
      {"family": "Select", "label": "3D skull scans you can rotate (Atles osteologic)", "view": "table", "cols": {"model": "Specimen", "mesh": "3D model"}, "tip": "Skull and bone 3D scans from the museum's Atles osteologic. The ?mesh column is a Draco-compressed .glb streamed from the bucket and rendered INLINE — click 🧊 3D, then drag to turn the actual cranium of a hyena, fox or bird. 92 models stream inline so far (mirrored + compressed from Sketchfab account laboratorinatura); the rest link out via p:sketchfab. CC BY.", "q": "PREFIX class: <https://bioexplora.cat/class/>\nPREFIX p: <https://bioexplora.cat/prop/>\nPREFIX rdfs: <http://www.w3.org/2000/01/rdf-schema#>\nSELECT ?model ?mesh WHERE {\n  ?m a class:Model3D ; rdfs:label ?model ; p:mesh ?mesh .\n} LIMIT 40"},
      {"family": "Select", "label": "Nature sounds (the fonoteca)", "view": "table", "cols": {"species": "Species", "common": "Common name", "country": "Country", "recordist": "Recordist", "audio": "Recording"}, "tip": "173 nature sound recordings that PLAY inline (?audio is a player) — owls, warblers, frogs and more across 100 species, each with its full Xeno-canto metadata (common name, genus, country, recordist, behaviour, sex/life stage…) and wired into the same Taxon / Collector / Place graph as the specimens. NOTE: CC BY-NC-ND by the recordists via Xeno-canto, NOT attributable to the MCNB.", "q": "PREFIX class: <https://bioexplora.cat/class/>\nPREFIX dwc: <http://rs.tdwg.org/dwc/terms/>\nPREFIX p: <https://bioexplora.cat/prop/>\nSELECT ?species ?common ?country ?recordist ?audio WHERE {\n  ?r a class:Recording ; dwc:scientificName ?species ; p:audio ?audio .\n  OPTIONAL { ?r dwc:vernacularName ?common }\n  OPTIONAL { ?r dwc:country ?country }\n  OPTIONAL { ?r dwc:recordedBy ?recordist }\n} LIMIT 40"},
      {"family": "Path", "label": "Longhorn beetles, and where they were found", "view": "table", "cols": {"species": "Species", "cat": "Catalogue №", "locality": "Found at"}, "tip": "A taxonomic + geographic slice: every Cerambycidae (longhorn beetle) specimen with its species, catalogue number and collection locality. Change 'Cerambycidae' to any family — Helicidae (snails), Muridae (rodents), Formicidae (ants)...", "q": "PREFIX dwc: <http://rs.tdwg.org/dwc/terms/>\nSELECT ?species ?cat ?locality WHERE {\n  ?s dwc:family \"Cerambycidae\" ; dwc:scientificName ?species ; dwc:catalogNumber ?cat .\n  OPTIONAL { ?s dwc:locality ?locality }\n} LIMIT 40"}
    ],
    "geoadmin": [
      {"family": "Aggregate", "label": "Countries of the world", "view": "map", "cols": {"c": "Area", "country": "Country", "boundary": "Boundary"}, "tip": "All 213 country polygons (geoBoundaries ADM0, OSM-derived). Output → Map draws the world; in Table view each ?boundary cell is a mini-map (coarse ~1 km). Click one to open the full map — it fetches THAT country's fine-detail (~110 m) boundary on demand (multi-LOD: only the polygon you inspect is downloaded in detail).", "q": "PREFIX g: <https://w3id.org/rete/geoadmin/prop/>\nPREFIX schema: <http://schema.org/>\nPREFIX geo: <http://www.opengis.net/ont/geosparql#>\nPREFIX class: <https://w3id.org/rete/geoadmin/class/>\nSELECT ?c ?country ?boundary WHERE {\n  ?c a class:Country ; schema:name ?country ; geo:asWKT ?boundary .\n} LIMIT 200"},
      {"family": "Path", "label": "★ SPARQL SERVICE: capitals from live Wikidata", "view": "table", "cols": {"country": "Country", "capital": "Capital (Wikidata)"}, "tip": "SPARQL 1.1 federated query, ENGINE-level: the SERVICE block is shipped verbatim to query.wikidata.org and its solutions JOIN the local rows on the shared ?code — geoadmin's ISO 3166-1 code (g:iso, e.g. \"ES\" for Spain) meets Wikidata's P297 value. No VALUES pinning needed: the block is naturally bounded (~250 countries carry an ISO code). The local file answers the country names; live Wikidata the capitals (P36 + English label): Spain → Madrid, Kenya → Nairobi… Unlike '+ Add source' federation (the playground splitting one pattern set across sources), SERVICE is written INTO the query, per the W3C spec — both now work.", "q": "PREFIX g: <https://w3id.org/rete/geoadmin/prop/>\nPREFIX schema: <http://schema.org/>\nPREFIX wdt: <http://www.wikidata.org/prop/direct/>\nPREFIX rdfs: <http://www.w3.org/2000/01/rdf-schema#>\nSELECT ?country ?capital WHERE {\n  ?c g:iso ?code ; schema:name ?country .\n  SERVICE <https://query.wikidata.org/sparql> {\n    ?wd wdt:P297 ?code ; wdt:P36 ?cap .\n    ?cap rdfs:label ?capital . FILTER(LANG(?capital) = \"en\")\n  }\n} ORDER BY ?country LIMIT 30"},
      {"family": "Geo", "label": "★ Vector tiles (PMTiles next to rete)", "view": "tiles", "cols": {"s": "Area", "region": "Region", "boundary": "Boundary"}, "tip": "Output → Tiles renders a 113 MB PMTiles vector basemap (tippecanoe, true per-zoom LOD) of EVERY admin area — pan/zoom and ADM1→ADM2 detail streams in, only the visible tiles fetched. The features your SPARQL returns light up in green ON the basemap: here Spain's autonomous communities. The tiles draw the geometry; rete answers the query right next to them (option B of the geo-LOD work).", "q": "PREFIX g: <https://w3id.org/rete/geoadmin/prop/>\nPREFIX schema: <http://schema.org/>\nPREFIX geo: <http://www.opengis.net/ont/geosparql#>\nPREFIX class: <https://w3id.org/rete/geoadmin/class/>\nSELECT ?s ?region ?boundary WHERE {\n  ?s a class:Region ; g:country \"ES\" ; schema:name ?region ; geo:asWKT ?boundary .\n}"},
      {"family": "Select", "label": "One country (multi-LOD on zoom)", "view": "map", "cols": {"c": "Area", "country": "Country", "boundary": "Boundary"}, "tip": "A single country by ISO code (change \"ES\" → FR, MA, IT, KE, PH…). The cell shows the coarse ~1 km outline; click it to open the full map and the modal sharpens to the fine ~110 m boundary — fetched on demand for just this one feature (the remote-lazy LOD payoff: zoom in → fetch only what you inspect).", "q": "PREFIX g: <https://w3id.org/rete/geoadmin/prop/>\nPREFIX schema: <http://schema.org/>\nPREFIX geo: <http://www.opengis.net/ont/geosparql#>\nSELECT ?c ?country ?boundary WHERE {\n  ?c g:iso \"ES\" ; schema:name ?country ; geo:asWKT ?boundary .\n}"},
      {"family": "Aggregate", "label": "Regions / states (admin-1)", "view": "map", "cols": {"s": "Area", "region": "Region", "boundary": "Boundary"}, "tip": "3,133 first-level admin areas (geoBoundaries ADM1). Coarse outlines in the cells; click a region's map cell and the modal fetches its fine ~110 m boundary on demand — same multi-LOD model as countries.", "q": "PREFIX g: <https://w3id.org/rete/geoadmin/prop/>\nPREFIX schema: <http://schema.org/>\nPREFIX geo: <http://www.opengis.net/ont/geosparql#>\nPREFIX class: <https://w3id.org/rete/geoadmin/class/>\nSELECT ?s ?region ?boundary WHERE {\n  ?s a class:Region ; schema:name ?region ; geo:asWKT ?boundary .\n} LIMIT 400"},
      {"family": "Select", "label": "Districts of a country (admin-2)", "view": "map", "cols": {"district": "District", "boundary": "Boundary"}, "tip": "48,362 second-level admin areas (geoBoundaries ADM2). Here every district of Spain (ES) — change the code to FR, MA, IT… Detailed OSM boundaries; click a cell for the full zoomable map.", "q": "PREFIX g: <https://w3id.org/rete/geoadmin/prop/>\nPREFIX schema: <http://schema.org/>\nPREFIX geo: <http://www.opengis.net/ont/geosparql#>\nPREFIX class: <https://w3id.org/rete/geoadmin/class/>\nSELECT ?district ?boundary WHERE {\n  ?d a class:District ; g:country \"ES\" ; schema:name ?district ; geo:asWKT ?boundary .\n} LIMIT 400"},
      {"family": "Aggregate", "label": "Populated places (points)", "view": "map", "cols": {"place": "Place", "point": "Location"}, "tip": "1,251 cities and towns as POINTS — points and polygons share one geometry model.", "q": "PREFIX g: <https://w3id.org/rete/geoadmin/prop/>\nPREFIX schema: <http://schema.org/>\nPREFIX geo: <http://www.opengis.net/ont/geosparql#>\nPREFIX class: <https://w3id.org/rete/geoadmin/class/>\nSELECT ?place ?point WHERE {\n  ?p a class:Place ; schema:name ?place ; geo:asWKT ?point .\n} LIMIT 1251"}
    ],
    "geoadmin-tiles": [
      {"family": "Geo", "label": "★ Graph + vector tiles in ONE .rete (option C)", "view": "tiles", "cols": {"s": "Area", "region": "Region", "boundary": "Boundary"}, "tip": "This dataset is ONE .rete file that holds BOTH the RDF graph AND a PMTiles vector basemap (a kind-7 section). The SPARQL below runs on the graph; Output → Tiles range-reads the vector tiles from the SAME file (the browser parses the 1 KB header for the section's byte offset, then faults tiles from there). The features the query returns light up in green — here Spain's regions. One immutable, content-hashed, range-queryable file = graph + map tiles.", "q": "PREFIX g: <https://w3id.org/rete/geoadmin/prop/>\nPREFIX schema: <http://schema.org/>\nPREFIX geo: <http://www.opengis.net/ont/geosparql#>\nPREFIX class: <https://w3id.org/rete/geoadmin/class/>\nSELECT ?s ?region ?boundary WHERE {\n  ?s a class:Region ; g:country \"ES\" ; schema:name ?region ; geo:asWKT ?boundary .\n}"},
      {"family": "Aggregate", "label": "The graph still works (countries)", "view": "table", "cols": {"country": "Country", "iso": "ISO", "boundary": "Boundary"}, "tip": "Proof the embedded tiles don't disturb the graph: a normal SPARQL query over the same one file. The .rete reader skips the unknown tile section, so quads, dictionary and pyramid open exactly as in plain geoadmin. Switch Output → Tiles to see the map come from the same bytes.", "q": "PREFIX g: <https://w3id.org/rete/geoadmin/prop/>\nPREFIX schema: <http://schema.org/>\nPREFIX geo: <http://www.opengis.net/ont/geosparql#>\nPREFIX class: <https://w3id.org/rete/geoadmin/class/>\nSELECT ?country ?iso ?boundary WHERE {\n  ?c a class:Country ; schema:name ?country ; g:iso ?iso ; geo:asWKT ?boundary .\n} LIMIT 100"}
    ],
    "smithsonian3d": [
      {"family": "Select", "label": "3D models you can rotate", "view": "table", "tip": "Every row is an interactive 3D model. The ?model column is a Draco-compressed .glb streamed straight from the Smithsonian's S3 bucket — click 🧊 3D to open the inline viewer, then drag to rotate and scroll to zoom. All CC0 / public domain. 2,199 models in the collection; this shows the first 60.", "q": "PREFIX p: <https://3d.si.edu/prop/>\nPREFIX rdfs: <http://www.w3.org/2000/01/rdf-schema#>\nSELECT ?object ?model WHERE {\n  ?o p:mesh ?model ; rdfs:label ?object .\n} LIMIT 60"},
      {"family": "Select", "label": "Spinning turntable previews", "view": "table", "tip": "A lightweight alternative to the live 3D viewer: ?spin is a tiny pre-rendered turntable clip (a Blender Cycles render, ~20 KB webm) that loops automatically right in the cell — no WebGL needed. Shown beside the full ?model mesh so you can compare. About 200 of the 2,199 models have a pre-rendered spin.", "q": "PREFIX p: <https://3d.si.edu/prop/>\nPREFIX rdfs: <http://www.w3.org/2000/01/rdf-schema#>\nSELECT ?object ?spin ?model WHERE {\n  ?o p:spinVideo ?spin ; p:mesh ?model ; rdfs:label ?object .\n} LIMIT 48"},
      {"family": "Aggregate", "label": "Models by museum", "view": "table", "tip": "Which Smithsonian museum each model comes from. The National Museum of Natural History dominates (its mass-digitised skull and specimen collections), with Air and Space, American History, the National Portrait Gallery, Cooper Hewitt, African American History and the Freer|Sackler also represented.", "q": "PREFIX p: <https://3d.si.edu/prop/>\nPREFIX rdfs: <http://www.w3.org/2000/01/rdf-schema#>\nSELECT ?museum (COUNT(?o) AS ?models) WHERE {\n  ?o p:unit ?u . ?u rdfs:label ?museum .\n} GROUP BY ?museum ORDER BY DESC(?models)"},
      {"family": "Select", "label": "Search the collection", "view": "table", "tip": "Full-text search over the model titles (a text index backs it), each result a rotatable 3D model. Here the crania; change 'cranium' to 'whale', 'mammoth', 'mask', 'Apollo', 'Lincoln', or a genus like 'Pongo' to pull a different slice of the 2,199 models.", "q": "PREFIX p: <https://3d.si.edu/prop/>\nPREFIX rdfs: <http://www.w3.org/2000/01/rdf-schema#>\nSELECT ?object ?model WHERE {\n  ?o p:mesh ?model ; rdfs:label ?object .\n  FILTER(CONTAINS(LCASE(STR(?object)), \"cranium\"))\n} LIMIT 40"},
      {"family": "Select", "label": "Apollo & Air and Space", "view": "table", "tip": "The National Air and Space Museum's 3D scans — including the Apollo Command Module Columbia, modelled both inside and out. Open the 🧊 3D viewer to fly around the actual spacecraft that carried Armstrong, Aldrin and Collins to the Moon and back.", "q": "PREFIX p: <https://3d.si.edu/prop/>\nPREFIX rdfs: <http://www.w3.org/2000/01/rdf-schema#>\nSELECT ?object ?model WHERE {\n  ?o p:unit <https://3d.si.edu/unit/nasm> ; p:mesh ?model ; rdfs:label ?object .\n}"},
      {"family": "Select", "label": "Natural History specimens (with catalogue numbers)", "view": "table", "tip": "Vertebrate-zoology specimens from the National Museum of Natural History, each with its USNM catalogue number and a rotatable 3D scan — a digitised reference collection of primate and animal crania, mandibles and bones used by researchers worldwide.", "q": "PREFIX p: <https://3d.si.edu/prop/>\nPREFIX rdfs: <http://www.w3.org/2000/01/rdf-schema#>\nSELECT ?object ?catalogNumber ?model WHERE {\n  ?o p:unit <https://3d.si.edu/unit/nmnhvz> ; rdfs:label ?object ; p:catalogNumber ?catalogNumber ; p:mesh ?model .\n} LIMIT 40"},
      {"family": "Select", "label": "Open the full Smithsonian record", "view": "table", "tip": "Each 3D model links back to its full catalogue record on si.edu (?record) — the museum metadata, provenance and rights — shown beside the inline 3D mesh. The bridge from the scan to everything else the Smithsonian knows about the object.", "q": "PREFIX p: <https://3d.si.edu/prop/>\nPREFIX rdfs: <http://www.w3.org/2000/01/rdf-schema#>\nSELECT ?object ?record ?model WHERE {\n  ?o rdfs:label ?object ; p:record ?record ; p:mesh ?model .\n} LIMIT 30"}
    ],
    "lineara": [
      {"family": "Summary", "label": "The shape of the corpus", "view": "table", "tip": "The seven kinds of thing in the graph: 1,721 Inscriptions, 1,314 distinct word-sequences, 375 Signs, 102 Scribes, 52 Sites, plus Support and Period. Because Linear A is undeciphered, 'Word' and 'Sign' mean recurring graphic sequences, not known vocabulary.", "q": "PREFIX rdfs: <http://www.w3.org/2000/01/rdf-schema#>\nSELECT ?type (COUNT(?s) AS ?count) WHERE {\n  ?s a ?type .\n} GROUP BY ?type ORDER BY DESC(?count)"},
      {"family": "Aggregate", "label": "The most common signs", "view": "table", "tip": "How often each sign is attested across the whole corpus. KA, KU, SI and A lead — the workhorse syllabograms of the Minoan script. Entries like *301 are Douros/GORILA catalogue numbers for signs that have no agreed phonetic value (the script is undeciphered).", "q": "PREFIX la: <https://lineara.xyz/prop/>\nPREFIX rdfs: <http://www.w3.org/2000/01/rdf-schema#>\nSELECT ?sign (COUNT(?i) AS ?attestations) WHERE {\n  ?i la:sign ?s . ?s rdfs:label ?sign .\n} GROUP BY ?sign ORDER BY DESC(?attestations) LIMIT 25"},
      {"family": "Aggregate", "label": "Where the tablets were found", "view": "table", "tip": "Findspots, by document count. Haghia Triada — the great Minoan villa archive in south Crete — dominates with 1,110 inscriptions, followed by the Khania, Phaistos, Knossos and Zakros palaces. Most surviving Linear A is administrative clay from a handful of Cretan centres.", "q": "PREFIX la: <https://lineara.xyz/prop/>\nPREFIX rdfs: <http://www.w3.org/2000/01/rdf-schema#>\nSELECT ?site (COUNT(?i) AS ?documents) WHERE {\n  ?i la:site ?s . ?s rdfs:label ?site .\n} GROUP BY ?site ORDER BY DESC(?documents)"},
      {"family": "Select", "label": "Cross-referenced to the 3D & photo archives", "view": "table", "tip": "The corpus links out to two scholarly digital archives. prop:model3d → the ERC INSCRIBE project's interactive 3D scans (67 artifacts, University of Bologna; rotate and measure the actual clay — e.g. HT 29). prop:paito → the PAITO Project (Sapienza, Prof. Alessandro Greco): per-artifact pages for the Haghia Triada sealings (HT Wa …) and the Phaistos catalogue (tablets / clay sealings / vases — individual pages forthcoming). Both columns are clickable; HT Wa 1561 has both. The scans/photos are © their projects — acknowledge them; the graph stores links, not media.", "q": "PREFIX la: <https://lineara.xyz/prop/>\nPREFIX rdfs: <http://www.w3.org/2000/01/rdf-schema#>\nSELECT ?inscription ?model3d ?paito WHERE {\n  ?i rdfs:label ?inscription .\n  OPTIONAL { ?i la:model3d ?model3d }\n  OPTIONAL { ?i la:paito ?paito }\n  FILTER(BOUND(?model3d) || BOUND(?paito))\n} ORDER BY ?inscription"},
      {"family": "Aggregate", "label": "The sequences that recur across the corpus", "view": "table", "tip": "The word-sequences attested in the most documents — the recurring administrative vocabulary of Minoan accounting. KU heads it, and the famous KU-RO ('total', see the next example) and KI-RO ('deficit') are in the list. Fraction signs (¹⁄₂ …) appear because the accounts record measured quantities.", "q": "PREFIX la: <https://lineara.xyz/prop/>\nPREFIX rdfs: <http://www.w3.org/2000/01/rdf-schema#>\nSELECT ?word (COUNT(DISTINCT ?i) AS ?documents) WHERE {\n  ?i la:word ?w . ?w rdfs:label ?word .\n} GROUP BY ?word ORDER BY DESC(?documents) LIMIT 25"},
      {"family": "Select", "label": "KU-RO — the Minoan word for 'total'", "view": "table", "tip": "Full-text search (a text index backs it) over the Latin transliteration. KU-RO is the one Linear A word almost everyone agrees on: it means 'total', and it closes an account the way a sum line does — here 34 tablets end with it. Its companion KI-RO means 'deficit / owed'. Change 'KU-RO' to 'KI-RO' or 'SA-RA₂' to follow a different term.", "q": "PREFIX la: <https://lineara.xyz/prop/>\nPREFIX rdfs: <http://www.w3.org/2000/01/rdf-schema#>\nSELECT ?inscription ?transliteration WHERE {\n  ?i la:transliteration ?transliteration ; rdfs:label ?inscription .\n  FILTER(CONTAINS(?transliteration, \"KU-RO\"))\n}"},
      {"family": "Path", "label": "Signs that travel with KU", "view": "table", "tip": "Sign co-occurrence: which signs share a document with KU, and in how many. Since Linear A cannot be read, this distributional method — which signs cluster together — is one of the main tools for probing the script's structure. PA, RO, DA, NA and TA are KU's commonest companions.", "q": "PREFIX la: <https://lineara.xyz/prop/>\nPREFIX rdfs: <http://www.w3.org/2000/01/rdf-schema#>\nSELECT ?sign (COUNT(DISTINCT ?i) AS ?sharedDocs) WHERE {\n  ?i la:sign ?ku . ?ku rdfs:label \"KU\" .\n  ?i la:sign ?o . ?o rdfs:label ?sign .\n  FILTER(?o != ?ku)\n} GROUP BY ?sign ORDER BY DESC(?sharedDocs) LIMIT 20"},
      {"family": "Construct", "label": "Documents sharing the term SA-RA₂", "view": "graph", "tip": "Switch Output → Graph. A star of the 20 documents that share the sequence SA-RA₂ — one of the most-discussed Linear A terms (a transaction heading or commodity, found across several sites). Linking documents by a shared sequence is how scholars hunt for genres, scribal habits and the hand of single administrators.", "q": "PREFIX la: <https://lineara.xyz/prop/>\nPREFIX rdfs: <http://www.w3.org/2000/01/rdf-schema#>\nCONSTRUCT {\n  ?i la:word ?w . ?i rdfs:label ?inscription . ?w rdfs:label ?word .\n} WHERE {\n  ?w rdfs:label \"SA-RA₂\" . ?i la:word ?w ; rdfs:label ?inscription .\n  ?w rdfs:label ?word .\n}"}
    ],
    "nomisma": [{"family": "Summary","label": "Shape of the corpus","view": "table","tip": "Predicate totals over the whole graph: ~7.2k coin types each carrying a label, dates, material, denomination, and (mostly) a mint and authority. 9 predicates.","q": "SELECT ?p (COUNT(*) AS ?triples) WHERE { ?s ?p ?o } GROUP BY ?p ORDER BY DESC(?triples)"},{"family": "Select","label": "Silver tetradrachms of Alexander the Great","view": "table","tip": "Bound-authority + bound-material + bound-denomination star, joined to mint labels: the famous AR tetradrachms of Alexander III, by mint (Abydus, Aegae, Amphipolis...).","q": "PREFIX nmo: <http://nomisma.org/ontology#>\nPREFIX nm: <http://nomisma.org/id/>\nPREFIX rdfs: <http://www.w3.org/2000/01/rdf-schema#>\nSELECT ?type ?label ?mintName WHERE {\n  ?type a nmo:TypeSeriesItem ; rdfs:label ?label ;\n        nmo:hasAuthority nm:alexander_iii ; nmo:hasMaterial nm:ar ; nmo:hasDenomination nm:tetradrachm ; nmo:hasMint ?mint .\n  ?mint rdfs:label ?mintName .\n} ORDER BY ?mintName LIMIT 50"},{"family": "Aggregate","label": "Most prolific mints","view": "table","tip": "GROUP BY mint: the Macedonian capital Pella (1501 types) and Amphipolis (1266) dominate, then the eastern conquests Babylon and Sardis.","q": "PREFIX nmo: <http://nomisma.org/ontology#>\nPREFIX rdfs: <http://www.w3.org/2000/01/rdf-schema#>\nSELECT ?mintName (COUNT(?type) AS ?types) WHERE {\n  ?type a nmo:TypeSeriesItem ; nmo:hasMint ?mint .\n  ?mint rdfs:label ?mintName .\n} GROUP BY ?mintName ORDER BY DESC(?types) LIMIT 15"},{"family": "Aggregate","label": "Coin types per issuing authority","view": "table","tip": "The cast of the Macedonian story by output: Alexander III (1972), Philip III Arrhidaeus (1025), Philip II (480), then the Diadochi Cassander, Lysimachus and Ptolemy I.","q": "PREFIX nmo: <http://nomisma.org/ontology#>\nPREFIX rdfs: <http://www.w3.org/2000/01/rdf-schema#>\nSELECT ?authName (COUNT(?type) AS ?types) WHERE {\n  ?type a nmo:TypeSeriesItem ; nmo:hasAuthority ?auth .\n  ?auth rdfs:label ?authName .\n} GROUP BY ?authName ORDER BY DESC(?types)"},{"family": "Path","label": "Mints used by 3+ successive rulers","view": "table","tip": "Two-hop join through a shared mint reveals political continuity: Amphipolis was struck by FOUR rulers (Philip II -> Alexander III -> Philip III -> Cassander). HAVING + GROUP_CONCAT name them.","q": "PREFIX nmo: <http://nomisma.org/ontology#>\nPREFIX rdfs: <http://www.w3.org/2000/01/rdf-schema#>\nSELECT ?mintName (COUNT(DISTINCT ?auth) AS ?rulers) (GROUP_CONCAT(DISTINCT ?authName; SEPARATOR=\", \") AS ?who) WHERE {\n  ?type a nmo:TypeSeriesItem ; nmo:hasMint ?mint ; nmo:hasAuthority ?auth .\n  ?mint rdfs:label ?mintName . ?auth rdfs:label ?authName .\n} GROUP BY ?mintName HAVING(COUNT(DISTINCT ?auth) >= 3) ORDER BY DESC(?rulers)"},{"family": "Construct","label": "Who else struck at Cassander's mints","view": "graph","tip": "Builds a compact succession star: every authority that minted at a city Cassander also used. Centres on Amphipolis, linking Philip II, Alexander III, Philip III and Cassander - the whole Macedonian dynastic line in one mint.","q": "PREFIX nmo: <http://nomisma.org/ontology#>\nPREFIX nm: <http://nomisma.org/id/>\nCONSTRUCT { ?auth nmo:hasMint ?mint . } WHERE {\n  ?tc a nmo:TypeSeriesItem ; nmo:hasAuthority nm:cassander ; nmo:hasMint ?mint .\n  ?t a nmo:TypeSeriesItem ; nmo:hasMint ?mint ; nmo:hasAuthority ?auth .\n}"}],
    "mimotext": [{"family": "Summary","label": "What is in this graph?","view": "table","tip": "Counts the main entity kinds (literary works, people, themes, spatial concepts) so you see the shape of the literary network at a glance.","q": "PREFIX wdt: <http://data.mimotext.uni-trier.de/prop/direct/>\nPREFIX wd:  <http://data.mimotext.uni-trier.de/entity/>\nSELECT ?kind (COUNT(DISTINCT ?x) AS ?n) WHERE {\n  VALUES (?class ?kind) { (wd:Q2 \"literary work\") (wd:Q10 \"person\") (wd:Q20 \"thematic concept\") (wd:Q26 \"spatial concept\") }\n  ?x wdt:P2 ?class .\n} GROUP BY ?kind ORDER BY DESC(?n)"},{"family": "Aggregate","label": "Most common themes across the novels","view": "table","tip": "Ranks thematic concepts (P36 'about') by how many distinct novels treat them - the dominant motifs of the Enlightenment novel (sentiment, sentimentalism, travel, love).","q": "PREFIX wdt: <http://data.mimotext.uni-trier.de/prop/direct/>\nPREFIX rdfs:<http://www.w3.org/2000/01/rdf-schema#>\nSELECT ?theme (COUNT(DISTINCT ?work) AS ?novels) WHERE {\n  ?work wdt:P36 ?t . ?t rdfs:label ?theme . FILTER(LANG(?theme)=\"en\")\n} GROUP BY ?theme ORDER BY DESC(?novels) LIMIT 15"},{"family": "Select","label": "Baculard d'Arnaud's novels, by year, with genre","view": "table","tip": "Lists the works of the most prolific author in this subset - Baculard d'Arnaud (41 novels) - with publication year (from xsd:dateTime P9) and genre. Author names are stored 'SURNAME, Given', so the match uses CONTAINS on the family name.","q": "PREFIX wdt: <http://data.mimotext.uni-trier.de/prop/direct/>\nPREFIX rdfs:<http://www.w3.org/2000/01/rdf-schema#>\nSELECT ?work ?year ?genre WHERE {\n  ?author rdfs:label ?aName . FILTER(LANG(?aName)=\"en\" && CONTAINS(?aName,\"Baculard\"))\n  ?w wdt:P5 ?author ; rdfs:label ?work . FILTER(LANG(?work)=\"en\")\n  OPTIONAL { ?w wdt:P9 ?d . BIND(YEAR(?d) AS ?year) }\n  OPTIONAL { ?w wdt:P12 ?g . ?g rdfs:label ?genre FILTER(LANG(?genre)=\"en\") }\n} ORDER BY ?year"},{"family": "Aggregate","label": "Novels that share the most themes","view": "table","tip": "Finds the pair of novels with the largest overlap of shared thematic concepts - a thematic 'bridge' between two books.","q": "PREFIX wdt: <http://data.mimotext.uni-trier.de/prop/direct/>\nPREFIX rdfs:<http://www.w3.org/2000/01/rdf-schema#>\nSELECT ?novelA ?novelB (COUNT(?t) AS ?sharedThemes) WHERE {\n  ?a wdt:P36 ?t . ?b wdt:P36 ?t . FILTER(STR(?a) < STR(?b))\n  ?a rdfs:label ?novelA FILTER(LANG(?novelA)=\"en\")\n  ?b rdfs:label ?novelB FILTER(LANG(?novelB)=\"en\")\n} GROUP BY ?novelA ?novelB ORDER BY DESC(?sharedThemes) LIMIT 10"},{"family": "Select","label": "Stylometrically closest novels","view": "table","tip": "The 520 computed P49 stylometric-similarity edges link novels close in writing style (a Burrows-Delta neighbourhood). This lists labelled novel - neighbour pairs. In this subset the similarity is a direct work-to-work edge; the distance value itself is not carried.","q": "PREFIX wdt: <http://data.mimotext.uni-trier.de/prop/direct/>\nPREFIX rdfs:<http://www.w3.org/2000/01/rdf-schema#>\nSELECT ?novel ?stylisticNeighbour WHERE {\n  ?a wdt:P49 ?b .\n  ?a rdfs:label ?novel FILTER(LANG(?novel)=\"en\")\n  ?b rdfs:label ?stylisticNeighbour FILTER(LANG(?stylisticNeighbour)=\"en\")\n} LIMIT 20"},{"family": "Construct","label": "Author ego-network: Baculard d'Arnaud -> novels -> genre","view": "graph","tip": "Builds a small subgraph of one author (Baculard d'Arnaud), their novels, and each novel's genre - rendered as a node-link diagram.","q": "PREFIX wdt: <http://data.mimotext.uni-trier.de/prop/direct/>\nPREFIX rdfs:<http://www.w3.org/2000/01/rdf-schema#>\nCONSTRUCT {\n  ?work wdt:P5 ?author . ?work wdt:P12 ?genre .\n  ?author rdfs:label ?aL . ?work rdfs:label ?wL . ?genre rdfs:label ?gL .\n} WHERE {\n  ?author rdfs:label ?aL . FILTER(LANG(?aL)=\"en\" && CONTAINS(?aL,\"Baculard\"))\n  ?work wdt:P5 ?author ; rdfs:label ?wL . FILTER(LANG(?wL)=\"en\")\n  OPTIONAL { ?work wdt:P12 ?genre . ?genre rdfs:label ?gL FILTER(LANG(?gL)=\"en\") }\n}"}],
    "mmm": [
      {"family": "Select", "label": "MS Gg.1.1 - the trilingual compendium's full record", "view": "table", "tip": "Every fact MMM holds about Cambridge, University Library, MS Gg.1.1 - the 14th-century trilingual compendium of texts (SDBM entry 212926). A bound subject is the most selective shape: SPO routing fetches only this manuscript's tiles, a few HTTP ranges of the 141 MB file. You'll see the work it carries, its owners, shelfmark, material (parchment), folio/line counts and the Cambridge catalogue that documents it.", "q": "SELECT ?p ?o WHERE {\n  <http://ldf.fi/mmm/manifestation_singleton/sdbm_orphan_212926> ?p ?o\n}"},
      {"family": "Select", "label": "MS Gg.1.1 - the trilingual texts inside (Latin, French, English)", "view": "table", "tip": "What the 'trilingual compendium' actually carries: the work 'A large collection of poetry' and the THREE languages SDBM records for its expression via crm:P72_has_language - Latin, French and English. That mix (Latin + Anglo-Norman French + Middle English) is exactly what makes Cambridge UL MS Gg.1.1 famous. All bound to one manuscript, so it stays a selective lazy read.", "q": "PREFIX crm: <http://erlangen-crm.org/current/>\nPREFIX skos: <http://www.w3.org/2004/02/skos/core#>\nSELECT ?text ?language WHERE {\n  <http://ldf.fi/mmm/manifestation_singleton/sdbm_orphan_212926> crm:P128_carries ?expr .\n  ?expr skos:prefLabel ?text ; crm:P72_has_language/skos:prefLabel ?language .\n}"},
      {"family": "Path", "label": "MS Gg.1.1 - its journey across the world", "view": "table", "tip": "The manuscript's migration - the very thing MMM maps. Made in 14th-century England (production 1300-1401), MS Gg.1.1's recorded owners span 'Cambridge, University' (England) and 'Washington' (United States), and its last-known location is back at Cambridge with WGS84 coordinates: a medieval book that crossed the Atlantic and seven centuries. UNION gathers the ownership stops and the final location into one result.", "q": "PREFIX crm: <http://erlangen-crm.org/current/>\nPREFIX mmm: <http://ldf.fi/schema/mmm/>\nPREFIX skos: <http://www.w3.org/2004/02/skos/core#>\nPREFIX wgs: <http://www.w3.org/2003/01/geo/wgs84_pos#>\nSELECT ?stage ?place ?lat ?long WHERE {\n  BIND(<http://ldf.fi/mmm/manifestation_singleton/sdbm_orphan_212926> AS ?ms)\n  { ?ms crm:P51_has_former_or_current_owner ?owner . ?owner skos:prefLabel ?place . BIND(\"owned by\" AS ?stage) }\n  UNION\n  { ?ms mmm:last_known_location ?loc . ?loc skos:prefLabel ?place .\n    OPTIONAL { ?loc wgs:lat ?lat ; wgs:long ?long } BIND(\"last known at\" AS ?stage) }\n}"},
      {"family": "Select", "label": "MS Gg.1.1 - work, owners and shelfmark", "view": "table", "tip": "The compendium's text ('A large collection of poetry', SDBM's terse title), its shelfmark 'Gg. 1. 1.' and its two recorded owners (Cambridge, University; Washington), resolved through skos:prefLabel. One row per owner. Everything is bound to one manuscript, so it stays a selective lazy read.", "q": "PREFIX crm: <http://erlangen-crm.org/current/>\nPREFIX mmm: <http://ldf.fi/schema/mmm/>\nPREFIX skos: <http://www.w3.org/2004/02/skos/core#>\nSELECT ?work ?ownerName ?shelfmark WHERE {\n  <http://ldf.fi/mmm/manifestation_singleton/sdbm_orphan_212926>\n    mmm:manuscript_work/skos:prefLabel ?work ;\n    mmm:catalog_or_lot_number ?shelfmark ;\n    crm:P51_has_former_or_current_owner/skos:prefLabel ?ownerName .\n}"},
      {"family": "Construct", "label": "MS Gg.1.1 - provenance ego-network", "view": "graph", "tip": "Switch Output -> Graph. A small star around MS Gg.1.1: the work it carries, its owners, its material (parchment) and its shelfmark. Each OPTIONAL lets an edge appear only if present, drawing one manuscript's provenance as a node-link card.", "q": "PREFIX crm: <http://erlangen-crm.org/current/>\nPREFIX mmm: <http://ldf.fi/schema/mmm/>\nPREFIX skos: <http://www.w3.org/2004/02/skos/core#>\nCONSTRUCT {\n  ?ms mmm:manuscript_work ?work ; crm:P51_has_former_or_current_owner ?owner ;\n      crm:P45_consists_of ?material ; mmm:catalog_or_lot_number ?shelf .\n} WHERE {\n  BIND(<http://ldf.fi/mmm/manifestation_singleton/sdbm_orphan_212926> AS ?ms)\n  OPTIONAL { ?ms mmm:manuscript_work/skos:prefLabel ?work }\n  OPTIONAL { ?ms crm:P51_has_former_or_current_owner/skos:prefLabel ?owner }\n  OPTIONAL { ?ms crm:P45_consists_of ?material }\n  OPTIONAL { ?ms mmm:catalog_or_lot_number ?shelf }\n}"},
      {"family": "Path", "label": "Other trilingual manuscripts (Latin + French + English), and where they are now", "view": "table", "tip": "Gg.1.1's exact language profile, generalised across the corpus: manuscripts whose text is recorded in ALL THREE of Latin, French and English. SDBM's language IRIs are shared, so this is a three-way intersection on crm:P72_has_language - 160 manuscripts qualify - paired with their last-known location (often London or Oxford). A real multi-join: carries -> expression -> three languages, then where each ended up.", "q": "PREFIX crm: <http://erlangen-crm.org/current/>\nPREFIX mmm: <http://ldf.fi/schema/mmm/>\nPREFIX skos: <http://www.w3.org/2004/02/skos/core#>\nSELECT DISTINCT ?msLabel ?place WHERE {\n  ?ms crm:P128_carries ?e .\n  ?e crm:P72_has_language <http://ldf.fi/mmm/language/sdbm_1> , <http://ldf.fi/mmm/language/sdbm_3> , <http://ldf.fi/mmm/language/sdbm_4> .\n  ?ms skos:prefLabel ?msLabel .\n  OPTIONAL { ?ms mmm:last_known_location/skos:prefLabel ?place }\n  FILTER(?ms != <http://ldf.fi/mmm/manifestation_singleton/sdbm_orphan_212926>)\n} LIMIT 100"},
      {"family": "Path", "label": "The rare company MS Gg.1.1 keeps - the 3 manuscripts owned by 'Washington'", "view": "table", "tip": "Gg.1.1 has two recorded owners: the University of Cambridge (1,190 manuscripts) and an obscure 'Washington' (only 3). Follow the rarer one backwards (reverse crm:P51) to surface the unlikely trio it sits among - a book of English statutes (Statuta Angliae, Edward III & Richard II), a Bible, and Gg.1.1's own 'large collection of poetry'. A bound owner keeps this a selective lazy read.", "q": "PREFIX crm: <http://erlangen-crm.org/current/>\nPREFIX mmm: <http://ldf.fi/schema/mmm/>\nPREFIX skos: <http://www.w3.org/2004/02/skos/core#>\nSELECT ?msLabel (SAMPLE(?text) AS ?work) WHERE {\n  ?ms crm:P51_has_former_or_current_owner <http://ldf.fi/mmm/actor/sdbm_38068> ; skos:prefLabel ?msLabel .\n  OPTIONAL { ?ms mmm:manuscript_work/skos:prefLabel ?text }\n} GROUP BY ?ms ?msLabel"},
      {"family": "Select", "label": "MS Gg.1.1's owner, the University of Cambridge, across the linked-data web", "view": "table", "tip": "Follow Gg.1.1's owner outward. The University of Cambridge is reconciled (owl:sameAs) to SIX authority files - Wikidata (Q35794), VIAF, the BnF, the US Library of Congress, the German GND and the French IdRef. Each ?authority is a real web IRI, so the table links straight out to it. This is how MMM threads its actors into the wider linked-data web.", "q": "PREFIX crm: <http://erlangen-crm.org/current/>\nPREFIX skos: <http://www.w3.org/2004/02/skos/core#>\nPREFIX owl: <http://www.w3.org/2002/07/owl#>\nSELECT ?owner ?authority WHERE {\n  <http://ldf.fi/mmm/manifestation_singleton/sdbm_orphan_212926> crm:P51_has_former_or_current_owner ?o .\n  ?o skos:prefLabel ?owner ; owl:sameAs ?authority .\n}"},
      {"family": "Aggregate", "label": "How typical are Gg.1.1's three languages? (Latin vs French vs English)", "view": "table", "tip": "Begin at Gg.1.1's own three languages, then fan out across the WHOLE graph to count how many manuscripts carry each: Latin ~121,600 (the universal language of medieval learning), French ~12,500, and English just ~5,900 - the vernaculars, English the rarest of the three. The heaviest query here: it walks every manuscript in those languages, so run it to watch the engine work, not for a quick answer.", "q": "PREFIX crm: <http://erlangen-crm.org/current/>\nPREFIX skos: <http://www.w3.org/2004/02/skos/core#>\nSELECT ?language (COUNT(DISTINCT ?ms) AS ?manuscripts) WHERE {\n  <http://ldf.fi/mmm/manifestation_singleton/sdbm_orphan_212926> crm:P128_carries ?gg .\n  ?gg crm:P72_has_language ?lang . ?lang skos:prefLabel ?language .\n  ?other crm:P72_has_language ?lang . ?ms crm:P128_carries ?other .\n} GROUP BY ?language ORDER BY DESC(?manuscripts)"},
      {"family": "Select", "label": "Cambridge University Library - the Gg manuscripts", "view": "table", "tip": "Manuscripts documented in 'A Catalogue of the Manuscripts preserved in the University Library, Cambridge' (collection sdbm_33357), with their shelfmarks - the Gg.1.x series that MS Gg.1.1 sits in (Gg.1.1, 1.2, 1.3, 1.5, 1.6, 1.7 ...). A bound object on crm:P70i_is_documented_in routes to just that catalogue's tiles, so it stays a selective lazy read.", "q": "PREFIX crm: <http://erlangen-crm.org/current/>\nPREFIX mmm: <http://ldf.fi/schema/mmm/>\nSELECT ?ms ?shelfmark WHERE {\n  ?ms crm:P70i_is_documented_in <http://ldf.fi/mmm/collection/sdbm_33357> ;\n      mmm:catalog_or_lot_number ?shelfmark .\n} LIMIT 100"},
      {"family": "Path", "label": "Manuscripts produced in England", "view": "table", "tip": "Bind the place 'England' by name, then walk the CIDOC-CRM production event backwards (P7_took_place_at <- E12_Production -> P108_has_produced) to the manuscripts made there. Many now live elsewhere - several here are in the BnF in Paris - which is exactly the migration MMM traces. England produced ~14,650 of the graph's manuscripts.", "q": "PREFIX crm: <http://erlangen-crm.org/current/>\nPREFIX skos: <http://www.w3.org/2004/02/skos/core#>\nSELECT ?msLabel WHERE {\n  ?place skos:prefLabel \"England\" .\n  ?prod crm:P7_took_place_at ?place ; crm:P108_has_produced ?ms .\n  ?ms skos:prefLabel ?msLabel .\n} LIMIT 100"},
      {"family": "Aggregate", "label": "Where were manuscripts produced?", "view": "table", "tip": "Counts manuscripts by production place through the CIDOC-CRM production event (E12_Production -> P7_took_place_at): Italy ~16k, England ~15k, France ~11k, then cities like Paris, Florence and Venice. An aggregate over the whole production relation scans many tiles - heavier than the bound examples above, but still streamed lazily, never a full download.", "q": "PREFIX crm: <http://erlangen-crm.org/current/>\nPREFIX skos: <http://www.w3.org/2004/02/skos/core#>\nSELECT ?placeName (COUNT(?ms) AS ?manuscripts) WHERE {\n  ?prod a crm:E12_Production ; crm:P108_has_produced ?ms ; crm:P7_took_place_at ?place .\n  ?place skos:prefLabel ?placeName .\n} GROUP BY ?placeName ORDER BY DESC(?manuscripts) LIMIT 25"},
      {"family": "Summary", "label": "Predicate totals (the shape of the graph)", "view": "table", "tip": "Which CIDOC-CRM / FRBRoo predicates carry the provenance across all 23.3M triples - ownership (P51), production (P108/P7), documentation (P70i), the works carried. This scans the whole graph, so it's the heaviest example: switch the strategy to Progressive to read it from the embedded pyramid instead of a full scan.", "q": "SELECT ?p (COUNT(*) AS ?triples) WHERE { ?s ?p ?o } GROUP BY ?p ORDER BY DESC(?triples) LIMIT 30"}
    ],
    "openalex-astrocytes": [{"family": "Summary", "label": "What's in the graph", "view": "table", "tip": "Predicate totals: ex:author and ex:topic edges dominate, with 4,113 cito:cites citation links and the dct:title / ex:citationCount paper metadata.", "q": "SELECT ?p (COUNT(*) AS ?n) WHERE { ?s ?p ?o } GROUP BY ?p ORDER BY DESC(?n)"}, {"family": "Aggregate", "label": "Most-cited astrocyte papers", "view": "table", "tip": "ex:citationCount is the global OpenAlex citation count; Liddelow 2017 'Neurotoxic reactive astrocytes' tops the field.", "q": "PREFIX dct: <http://purl.org/dc/terms/>\nPREFIX ex: <http://ex/>\nSELECT ?title ?c WHERE { ?w a ex:Work ; dct:title ?title ; ex:citationCount ?c } ORDER BY DESC(?c) LIMIT 15"}, {"family": "Aggregate", "label": "Most prolific astrocyte authors", "view": "table", "tip": "Count papers per author across the citation core - the researchers who define the field.", "q": "PREFIX ex: <http://ex/>\nPREFIX foaf: <http://xmlns.com/foaf/0.1/>\nSELECT ?name (COUNT(?w) AS ?papers) WHERE { ?w ex:author ?a . ?a foaf:name ?name } GROUP BY ?name ORDER BY DESC(?papers) LIMIT 15"}, {"family": "Aggregate", "label": "Leading institutions", "view": "table", "tip": "Join author -> ex:affiliation -> institution label; which labs/universities produce the most astrocyte research.", "q": "PREFIX ex: <http://ex/>\nPREFIX rdfs: <http://www.w3.org/2000/01/rdf-schema#>\nSELECT ?inst (COUNT(DISTINCT ?w) AS ?papers) WHERE { ?w ex:author ?a . ?a ex:affiliation ?i . ?i rdfs:label ?inst } GROUP BY ?inst ORDER BY DESC(?papers) LIMIT 15"}, {"family": "Aggregate", "label": "Adjacent sub-topics", "view": "table", "tip": "The OpenAlex concepts co-tagged with these papers - the neighbouring fields astrocyte research bridges into.", "q": "PREFIX ex: <http://ex/>\nPREFIX rdfs: <http://www.w3.org/2000/01/rdf-schema#>\nSELECT ?topic (COUNT(?w) AS ?papers) WHERE { ?w ex:topic ?t . ?t rdfs:label ?topic } GROUP BY ?topic ORDER BY DESC(?papers) LIMIT 20"}, {"family": "Path", "label": "Who cites the field's landmark paper", "view": "table", "tip": "Reverse cito:cites against Liddelow 2017 (W2572710398): every paper in the core that cites the most-cited astrocyte study, newest first.", "q": "PREFIX cito: <http://purl.org/spar/cito/>\nPREFIX dct: <http://purl.org/dc/terms/>\nPREFIX ex: <http://ex/>\nSELECT ?title ?year WHERE { ?citing cito:cites <https://openalex.org/W2572710398> ; dct:title ?title ; ex:year ?year } ORDER BY DESC(?year) LIMIT 50"}, {"family": "Construct", "label": "Citation network of the top papers", "view": "graph", "tip": "Draws cito:cites among the most-cited works (citationCount > 1500) - the backbone of the astrocyte literature.", "q": "PREFIX cito: <http://purl.org/spar/cito/>\nPREFIX ex: <http://ex/>\nCONSTRUCT { ?a cito:cites ?b } WHERE { ?a cito:cites ?b . ?a ex:citationCount ?ca . FILTER(?ca > 1500) . ?b ex:citationCount ?cb . FILTER(?cb > 1500) }"}],
    "antarctic-expeditions": [{"family": "Summary", "label": "Shape of the expedition graph", "view": "table", "tip": "Predicate totals: ex:participant dominates (~76 crew edges), then ex:vessel / ex:leader, plus rdfs:label and ex:startYear/ex:endYear. Shows the 6-expedition / 5-ship / ~76-person skeleton at a glance.", "q": "SELECT ?p (COUNT(*) AS ?triples) WHERE { ?s ?p ?o } GROUP BY ?p ORDER BY DESC(?triples)"}, {"family": "Select", "label": "The crew of the Endurance", "view": "graph", "tip": "Bound-subject star on Shackleton's Endurance (Q1162294): every ex:participant with their name. Includes Mrs. Chippy, the ship's cat, a genuine P710 participant.", "q": "PREFIX ex: <http://ex/>\nPREFIX wd: <http://www.wikidata.org/entity/>\nPREFIX rdfs: <http://www.w3.org/2000/01/rdf-schema#>\nSELECT ?person ?name WHERE {\n  wd:Q1162294 ex:participant ?person .\n  ?person rdfs:label ?name .\n} ORDER BY ?name"}, {"family": "Path", "label": "Crew who served on more than one expedition", "view": "table", "tip": "Self-join through a shared ex:participant: people linked to two different expeditions are the network bridges (men who sailed with both Scott and Shackleton). STR(?e1)<STR(?e2) dedups the pair.", "q": "PREFIX ex: <http://ex/>\nPREFIX rdfs: <http://www.w3.org/2000/01/rdf-schema#>\nSELECT ?name ?exp1 ?exp2 WHERE {\n  ?e1 ex:participant ?p ; rdfs:label ?exp1 .\n  ?e2 ex:participant ?p ; rdfs:label ?exp2 .\n  FILTER(STR(?e1) < STR(?e2))\n  ?p rdfs:label ?name .\n} ORDER BY ?name"}, {"family": "Aggregate", "label": "Largest crews", "view": "table", "tip": "GROUP BY expedition counting ex:participant edges, joined to label and years — ranks the voyages by recorded crew size.", "q": "PREFIX ex: <http://ex/>\nPREFIX rdfs: <http://www.w3.org/2000/01/rdf-schema#>\nSELECT ?expedition ?startYear (COUNT(?p) AS ?crew) WHERE {\n  ?e ex:participant ?p ; rdfs:label ?expedition ; ex:startYear ?startYear .\n} GROUP BY ?expedition ?startYear ORDER BY DESC(?crew)"}, {"family": "Construct", "label": "Expedition -> leader + ship + crew ego-network", "view": "graph", "tip": "Builds one drawable star around an expedition (Terra Nova, Q973919): its leader, its vessel, its crew, each re-labelled so the renderer shows real names instead of QIDs.", "q": "PREFIX ex: <http://ex/>\nPREFIX wd: <http://www.wikidata.org/entity/>\nPREFIX rdfs: <http://www.w3.org/2000/01/rdf-schema#>\nCONSTRUCT {\n  wd:Q973919 ?p ?o . ?o rdfs:label ?name .\n} WHERE {\n  wd:Q973919 ?p ?o .\n  FILTER(?p = ex:leader || ?p = ex:vessel || ?p = ex:participant)\n  ?o rdfs:label ?name .\n}"}, {"family": "Aggregate", "label": "Time: expeditions by start year", "view": "time", "tip": "Switch Output -> Time. The six Heroic-Age expeditions plotted by ex:startYear (1897-1917); hover a cell for the expedition names.", "q": "PREFIX ex: <http://ex/>\nPREFIX rdfs: <http://www.w3.org/2000/01/rdf-schema#>\nSELECT ?year ?expedition WHERE { ?e ex:startYear ?year ; rdfs:label ?expedition }"}],
    "factgrid-illuminati": [{"family": "Summary", "label": "What's in the graph", "view": "table", "tip": "Predicate totals: P91 (member of), P2 (instance of) and the other FactGrid properties on each member.", "q": "SELECT ?p (COUNT(*) AS ?n) WHERE { ?s ?p ?o } GROUP BY ?p ORDER BY DESC(?n) LIMIT 20"}, {"family": "Select", "label": "Members of the Illuminati", "view": "table", "tip": "Everyone linked by P91 (member of) to the Order of the Illuminati (Q10677).", "q": "PREFIX wdt: <https://database.factgrid.de/prop/direct/>\nPREFIX rdfs: <http://www.w3.org/2000/01/rdf-schema#>\nSELECT ?name WHERE { ?m wdt:P91 <https://database.factgrid.de/entity/Q10677> ; rdfs:label ?name } ORDER BY ?name LIMIT 200"}, {"family": "Aggregate", "label": "Which properties describe members", "view": "table", "tip": "Group every fact about the members by predicate and show the FactGrid property label - the schema in plain language.", "q": "PREFIX wdt: <https://database.factgrid.de/prop/direct/>\nPREFIX rdfs: <http://www.w3.org/2000/01/rdf-schema#>\nSELECT ?plabel (COUNT(*) AS ?n) WHERE { ?m wdt:P91 <https://database.factgrid.de/entity/Q10677> . ?m ?p ?o . OPTIONAL { ?p rdfs:label ?plabel } } GROUP BY ?plabel ORDER BY DESC(?n)"}],
    "theographic-graph": [{"family": "Summary", "label": "What's in the graph", "view": "table", "tip": "Predicate totals: tg:sibling/child/father kinship, foaf:gender, places with wgs84 coordinates, events.", "q": "SELECT ?p (COUNT(*) AS ?n) WHERE { ?s ?p ?o } GROUP BY ?p ORDER BY DESC(?n) LIMIT 20"}, {"family": "Path", "label": "Descendants of Abraham", "view": "table", "tip": "Transitive tg:child+ from Abraham - the genealogical tree the text traces from the patriarch.", "q": "PREFIX tg: <http://theographic/ontology#>\nPREFIX rdfs: <http://www.w3.org/2000/01/rdf-schema#>\nSELECT DISTINCT ?name WHERE { <http://ex/person/abraham_58> tg:child+ ?d . ?d rdfs:label ?name } LIMIT 200"}, {"family": "Aggregate", "label": "Who had the most children", "view": "table", "tip": "Count tg:child edges per person - the prolific patriarchs and kings.", "q": "PREFIX tg: <http://theographic/ontology#>\nPREFIX rdfs: <http://www.w3.org/2000/01/rdf-schema#>\nSELECT ?name (COUNT(?c) AS ?children) WHERE { ?p tg:child ?c ; rdfs:label ?name } GROUP BY ?name ORDER BY DESC(?children) LIMIT 15"}, {"family": "Construct", "label": "Abraham's children", "view": "graph", "tip": "Child edges one hop from Abraham, relabelled to names for the graph view.", "q": "PREFIX tg: <http://theographic/ontology#>\nPREFIX rdfs: <http://www.w3.org/2000/01/rdf-schema#>\nCONSTRUCT { <http://ex/person/abraham_58> tg:child ?c . ?c rdfs:label ?n } WHERE { <http://ex/person/abraham_58> tg:child ?c . ?c rdfs:label ?n }"}],
    "monarch": [{"family": "Summary", "label": "What's in the graph", "view": "table", "tip": "Predicate totals across the biolink associations: has_phenotype, interacts_with, in_taxon, plus labels.", "q": "SELECT ?p (COUNT(*) AS ?n) WHERE { ?s ?p ?o } GROUP BY ?p ORDER BY DESC(?n) LIMIT 20"}, {"family": "Select", "label": "Phenotypes in the graph", "view": "table", "tip": "Everything typed as a biolink PhenotypicFeature, with its label.", "q": "PREFIX bl: <https://w3id.org/biolink/vocab/>\nPREFIX rdfs: <http://www.w3.org/2000/01/rdf-schema#>\nSELECT ?name WHERE { ?ph a bl:PhenotypicFeature ; rdfs:label ?name } ORDER BY ?name LIMIT 100"}, {"family": "Aggregate", "label": "Most-connected genes", "view": "table", "tip": "Rank genes by their biolink:interacts_with degree - the hubs of the interaction network.", "q": "PREFIX bl: <https://w3id.org/biolink/vocab/>\nPREFIX rdfs: <http://www.w3.org/2000/01/rdf-schema#>\nSELECT ?name (COUNT(?o) AS ?deg) WHERE { ?g bl:interacts_with ?o . ?g rdfs:label ?name } GROUP BY ?name ORDER BY DESC(?deg) LIMIT 15"}],
    "opencitations": [{"family": "Summary", "label": "What's in the graph", "view": "table", "tip": "Predicate totals: cito:cites citation edges and the Dublin Core / FOAF bibliographic metadata.", "q": "SELECT ?p (COUNT(*) AS ?n) WHERE { ?s ?p ?o } GROUP BY ?p ORDER BY DESC(?n) LIMIT 20"}, {"family": "Aggregate", "label": "Most-cited works", "view": "table", "tip": "In-degree over cito:cites joined to dct:title - the references everything points back to.", "q": "PREFIX cito: <http://purl.org/spar/cito/>\nPREFIX dct: <http://purl.org/dc/terms/>\nSELECT ?title (COUNT(?citing) AS ?cites) WHERE { ?citing cito:cites ?w . ?w dct:title ?title } GROUP BY ?title ORDER BY DESC(?cites) LIMIT 15"}, {"family": "Aggregate", "label": "Publications per year", "view": "table", "tip": "Group dct:date (xsd:gYear) to see the time profile of the neighbourhood.", "q": "PREFIX dct: <http://purl.org/dc/terms/>\nSELECT ?year (COUNT(?w) AS ?n) WHERE { ?w dct:date ?year } GROUP BY ?year ORDER BY ?year"}, {"family": "Path", "label": "Citation closure of a seed paper", "view": "table", "tip": "Transitive cito:cites+ from one JAMA article - everything it reaches by following references.", "q": "PREFIX cito: <http://purl.org/spar/cito/>\nSELECT DISTINCT ?w WHERE { <https://doi.org/10.1001/jama.2014.16543> cito:cites+ ?w } LIMIT 100"}],
    "orkg": [{"family": "Summary", "label": "What's in the graph", "view": "table", "tip": "Predicate totals across the ORKG model: papers, contributions, hasAuthors and the Pxx contribution properties.", "q": "SELECT ?p (COUNT(*) AS ?n) WHERE { ?s ?p ?o } GROUP BY ?p ORDER BY DESC(?n) LIMIT 20"}, {"family": "Select", "label": "Papers", "view": "table", "tip": "Everything typed as an ORKG Paper, with its title.", "q": "PREFIX rdfs: <http://www.w3.org/2000/01/rdf-schema#>\nSELECT ?title WHERE { ?p a <https://orkg.org/class/Paper> ; rdfs:label ?title } ORDER BY ?title LIMIT 100"}, {"family": "Aggregate", "label": "Node types in the graph", "view": "table", "tip": "Count subjects per rdf:type - papers vs contributions vs lists vs problems.", "q": "SELECT ?type (COUNT(?s) AS ?n) WHERE { ?s a ?type } GROUP BY ?type ORDER BY DESC(?n) LIMIT 20"}],
  },
  shacl: {
    "causenet": [
      {
        label: "Every causal relation has a cause, an effect and a support count",
        tip: "Data-integrity over the reified relations: every cn:CausalRelation must declare exactly one cn:cause and one cn:effect and at least one cn:support. Remote SHACL validates lazily over the shape's targets (range reads) - a broad target class fetches many targets, so give it a moment.",
        shape: `@prefix sh: <http://www.w3.org/ns/shacl#> .
@prefix cn: <https://causenet.org/ontology#> .

[] a sh:NodeShape ;
  sh:targetClass cn:CausalRelation ;
  sh:property [ sh:path cn:cause ;   sh:minCount 1 ; sh:maxCount 1 ] ;
  sh:property [ sh:path cn:effect ;  sh:minCount 1 ; sh:maxCount 1 ] ;
  sh:property [ sh:path cn:support ; sh:minCount 1 ;
    sh:datatype <http://www.w3.org/2001/XMLSchema#integer> ] .`
      },
      {
        label: "Every concept is named",
        tip: "Every cn:Concept node should carry an rdfs:label (its surface form). Validated lazily over the concept targets via HTTP range.",
        shape: `@prefix sh: <http://www.w3.org/ns/shacl#> .
@prefix cn: <https://causenet.org/ontology#> .
@prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> .

[] a sh:NodeShape ;
  sh:targetClass cn:Concept ;
  sh:property [ sh:path rdfs:label ; sh:minCount 1 ;
    sh:message "Every concept should carry a label." ] .`
      },
      {
        label: "Every ClueWeb12 source keeps its sentence and pattern",
        tip: "Provenance completeness: every ClueWeb12 source record must keep the sentence it was extracted from and the dependency path-pattern that matched it. Validated lazily over the ClueWeb12 source targets.",
        shape: `@prefix sh: <http://www.w3.org/ns/shacl#> .
@prefix cn: <https://causenet.org/ontology#> .

[] a sh:NodeShape ;
  sh:targetClass cn:ClueWeb12SentenceSource ;
  sh:property [ sh:path cn:sentence ; sh:minCount 1 ] ;
  sh:property [ sh:path cn:pattern ;  sh:minCount 1 ] .`
      }
    ],
    chemotion: [
      {
        label: "Molecules carry a structure",
        tip: "Data quality over the merged graph: every molecule should declare a SMILES and an InChIKey structure. SHACL materializes the data graph, so run it after Cache remote (it needs the dataset in memory).",
        shape: `@prefix sh: <http://www.w3.org/ns/shacl#> .
@prefix obo: <http://purl.obolibrary.org/obo/> .
@prefix chebi: <http://purl.obolibrary.org/obo/chebi/> .

[] a sh:NodeShape ;
  sh:targetClass obo:CHEBI_23367 ;
  sh:property [ sh:path chebi:smiles ;   sh:minCount 1 ] ;
  sh:property [ sh:path chebi:inchikey ; sh:minCount 1 ] .`
      },
      {
        label: "Substances are named",
        tip: "Run after Cache remote. A second class: every substance (a chemical mixture) should carry a name.",
        shape: `@prefix sh: <http://www.w3.org/ns/shacl#> .
@prefix obo: <http://purl.obolibrary.org/obo/> .
@prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> .

[] a sh:NodeShape ;
  sh:targetClass obo:CHEBI_59999 ;
  sh:property [ sh:path rdfs:label ; sh:minCount 1 ;
    sh:message "Every substance should carry a label." ] .`
      },
      {
        label: "Molecules carry a formula",
        tip: "Run after Cache remote. Every molecule should declare a molecular formula.",
        shape: `@prefix sh: <http://www.w3.org/ns/shacl#> .
@prefix obo: <http://purl.obolibrary.org/obo/> .
@prefix chebi: <http://purl.obolibrary.org/obo/chebi/> .

[] a sh:NodeShape ;
  sh:targetClass obo:CHEBI_23367 ;
  sh:property [ sh:path chebi:formula ; sh:minCount 1 ;
    sh:message "Molecule has no formula." ] .`
      },
      {
        label: "Molecules carry an InChI",
        tip: "Run after Cache remote. Every molecule should declare an InChI structure string.",
        shape: `@prefix sh: <http://www.w3.org/ns/shacl#> .
@prefix obo: <http://purl.obolibrary.org/obo/> .
@prefix chebi: <http://purl.obolibrary.org/obo/chebi/> .

[] a sh:NodeShape ;
  sh:targetClass obo:CHEBI_23367 ;
  sh:property [ sh:path chebi:inchi ; sh:minCount 1 ;
    sh:message "Molecule has no InChI." ] .`
      }
    ],
    "chebi-full": [
      {
        label: "Molecules with a SMILES carry a formula + InChIKey",
        tip: "Data-completeness over the whole ontology: every ChEBI entity that declares a SMILES should also declare a molecular formula and an InChIKey. SHACL materializes the data graph, so run it after Cache remote (it needs the dataset in memory).",
        shape: `@prefix sh: <http://www.w3.org/ns/shacl#> .
@prefix chemrof: <https://w3id.org/chemrof/> .

[] a sh:NodeShape ;
  sh:targetSubjectsOf chemrof:smiles_string ;
  sh:property [ sh:path chemrof:generalized_empirical_formula ; sh:minCount 1 ] ;
  sh:property [ sh:path chemrof:inchi_key_string ;             sh:minCount 1 ] .`
      },
      {
        label: "Every classified entity is named",
        tip: "Run after Cache remote. Every subject of rdfs:subClassOf (a class in the DAG) should carry an rdfs:label.",
        shape: `@prefix sh: <http://www.w3.org/ns/shacl#> .
@prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> .

[] a sh:NodeShape ;
  sh:targetSubjectsOf rdfs:subClassOf ;
  sh:property [ sh:path rdfs:label ; sh:minCount 1 ;
    sh:message "Every classified entity should carry a label." ] .`
      },
      {
        label: "SMILES-bearing molecules carry an InChI",
        tip: "Run after Cache remote. Every entity with a SMILES should also declare an InChI string.",
        shape: `@prefix sh: <http://www.w3.org/ns/shacl#> .
@prefix chemrof: <https://w3id.org/chemrof/> .

[] a sh:NodeShape ;
  sh:targetSubjectsOf chemrof:smiles_string ;
  sh:property [ sh:path chemrof:inchi_string ; sh:minCount 1 ;
    sh:message "Molecule with a SMILES has no InChI." ] .`
      },
      {
        label: "Classes carry a definition",
        tip: "Run after Cache remote. Every class in the subclass hierarchy should carry a textual definition. ChEBI is incomplete, so expect violations.",
        shape: `@prefix sh: <http://www.w3.org/ns/shacl#> .
@prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> .
@prefix obo: <http://purl.obolibrary.org/obo/> .

[] a sh:NodeShape ;
  sh:targetSubjectsOf rdfs:subClassOf ;
  sh:property [ sh:path obo:IAO_0000115 ; sh:minCount 1 ;
    sh:message "Class has no definition." ] .`
      }
    ],
    causal: [
      {
        label: "Causal links join factors",
        tip: "Structural integrity: every subject and object of ex:causes must be a typed Factor (the subclass closure counts RiskFactor, Disease, Outcome ... as Factors). The model conforms.",
        shape: `@prefix sh: <http://www.w3.org/ns/shacl#> .
@prefix ex: <http://ex/> .

ex:CauseIsFactor a sh:NodeShape ;
  sh:targetSubjectsOf ex:causes ;
  sh:class ex:Factor .

ex:EffectIsFactor a sh:NodeShape ;
  sh:targetObjectsOf ex:causes ;
  sh:class ex:Factor .`
      },
      {
        label: "Risk factors are well described",
        tip: "Three planted data defects: a missing ex:modifiable flag (Poverty), a prevalence outside [0,1] (Stress = 1.4), and an off-list evidence value (Air pollution = ex:rumored).",
        shape: `@prefix sh: <http://www.w3.org/ns/shacl#> .
@prefix ex: <http://ex/> .
@prefix xsd: <http://www.w3.org/2001/XMLSchema#> .

ex:RiskFactorShape a sh:NodeShape ;
  sh:targetClass ex:RiskFactor ;
  sh:property [
    sh:path ex:modifiable ; sh:minCount 1 ; sh:datatype xsd:boolean ;
    sh:message "Risk factor must state whether it is modifiable."
  ] ;
  sh:property [
    sh:path ex:prevalence ; sh:datatype xsd:decimal ;
    sh:minInclusive 0 ; sh:maxInclusive 1 ;
    sh:message "Prevalence must be a probability in [0,1]."
  ] ;
  sh:property [
    sh:path ex:evidence ; sh:minCount 1 ;
    sh:in ( ex:established ex:probable ex:hypothesized ) ;
    sh:message "Evidence must be established, probable or hypothesized."
  ] .`
      },
      {
        label: "A node is not both healthy and diseased",
        tip: "The same disjoint-class clash the Coherence tab proves from the schema - here SHACL catches it on the individual: patient :p is typed as both states.",
        shape: `@prefix sh: <http://www.w3.org/ns/shacl#> .
@prefix ex: <http://ex/> .

ex:NotBothStatesShape a sh:NodeShape ;
  sh:targetClass ex:DiseaseState ;
  sh:not [ sh:class ex:HealthyState ] ;
  sh:message "A node cannot be both a DiseaseState and a HealthyState." .`
      },
      {
        label: "Every outcome has a cause",
        tip: "An inverse-path check: every ex:Outcome must be reachable by at least one ex:causes edge. The model conforms - no orphan outcomes.",
        shape: `@prefix sh: <http://www.w3.org/ns/shacl#> .
@prefix ex: <http://ex/> .

ex:OutcomeHasCauseShape a sh:NodeShape ;
  sh:targetClass ex:Outcome ;
  sh:property [
    sh:path [ sh:inversePath ex:causes ] ;
    sh:minCount 1 ;
    sh:message "Every outcome must be reachable by at least one ex:causes edge."
  ] .`
      }
    ],
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
      },
      {
        label: "Authors carry an ORCID + h-index",
        tip: "Every ex:Person should declare an ORCID and an h-index. The clean graph conforms.",
        shape: `@prefix ex: <http://ex/> .
@prefix sh: <http://www.w3.org/ns/shacl#> .

ex:AuthorIdShape a sh:NodeShape ;
  sh:targetClass ex:Person ;
  sh:property [ sh:path ex:orcid ; sh:minCount 1 ; sh:message "Author has no ORCID." ] ;
  sh:property [ sh:path ex:hIndex ; sh:minCount 1 ; sh:message "Author has no h-index." ] .`
      },
      {
        label: "Journals carry an ISSN",
        tip: "Every ex:Journal should declare an ISSN. The clean graph conforms.",
        shape: `@prefix ex: <http://ex/> .
@prefix sh: <http://www.w3.org/ns/shacl#> .

ex:JournalIssnShape a sh:NodeShape ;
  sh:targetClass ex:Journal ;
  sh:property [ sh:path ex:issn ; sh:minCount 1 ; sh:message "Journal has no ISSN." ] .`
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
      },
      {
        label: "Papers are titled and placed",
        tip: "Every ex:Paper should carry exactly one title and a venue.",
        shape: `@prefix ex: <http://ex/> .
@prefix dct: <http://purl.org/dc/terms/> .
@prefix sh: <http://www.w3.org/ns/shacl#> .

ex:NoisyPaperShape a sh:NodeShape ;
  sh:targetClass ex:Paper ;
  sh:property [ sh:path dct:title ; sh:minCount 1 ; sh:maxCount 1 ] ;
  sh:property [ sh:path ex:publishedIn ; sh:minCount 1 ; sh:message "Paper has no venue." ] .`
      },
      {
        label: "Conferences are named",
        tip: "Every ex:Conference should carry an ex:name.",
        shape: `@prefix ex: <http://ex/> .
@prefix sh: <http://www.w3.org/ns/shacl#> .

ex:ConferenceShape a sh:NodeShape ;
  sh:targetClass ex:Conference ;
  sh:property [ sh:path ex:name ; sh:minCount 1 ; sh:message "Conference has no name." ] .`
      }
    ],
    "antarctic-expeditions": [
      {
        label: "Expeditions are fully described",
        tip: "Structural completeness: every expedition should name a leader (a Person), a vessel (a Ship) and start/end years. SHACL flags 2 expeditions with no recorded leader.",
        shape: `@prefix sh: <http://www.w3.org/ns/shacl#> .
@prefix ex: <http://ex/> .

ex:ExpeditionShape a sh:NodeShape ;
  sh:targetClass ex:Expedition ;
  sh:property [ sh:path ex:leader ; sh:class ex:Person ; sh:minCount 1 ;
    sh:message "Every expedition should record a leader." ] ;
  sh:property [ sh:path ex:vessel ; sh:class ex:Ship ] ;
  sh:property [ sh:path ex:startYear ; sh:minCount 1 ] ;
  sh:property [ sh:path ex:endYear ; sh:minCount 1 ] .`
      },
      {
        label: "Every person is named",
        tip: "A simpler structural check: every ex:Person carries an rdfs:label. The roster conforms.",
        shape: `@prefix sh: <http://www.w3.org/ns/shacl#> .
@prefix ex: <http://ex/> .
@prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> .

ex:PersonNamedShape a sh:NodeShape ;
  sh:targetClass ex:Person ;
  sh:property [ sh:path rdfs:label ; sh:minCount 1 ;
    sh:message "Every expedition member should be named." ] .`
      },
      {
        label: "Ships are named",
        tip: "Every ex:Ship should carry an rdfs:label.",
        shape: `@prefix sh: <http://www.w3.org/ns/shacl#> .
@prefix ex: <http://ex/> .
@prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> .

ex:ShipShape a sh:NodeShape ;
  sh:targetClass ex:Ship ;
  sh:property [ sh:path rdfs:label ; sh:minCount 1 ; sh:message "Ship has no name." ] .`
      },
      {
        label: "Participants are people",
        tip: "Every value of ex:participant must be a typed ex:Person.",
        shape: `@prefix sh: <http://www.w3.org/ns/shacl#> .
@prefix ex: <http://ex/> .

ex:ParticipantShape a sh:NodeShape ;
  sh:targetObjectsOf ex:participant ;
  sh:class ex:Person .`
      }
    ],
    "factgrid-illuminati": [
      {
        label: "Every member is named",
        tip: "Data quality on an untyped Wikibase graph: every classified member — anything typed as an instance of a class — should carry a readable label. The graph conforms; the labels were resolved at build time.",
        shape: `@prefix sh: <http://www.w3.org/ns/shacl#> .
@prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> .
@prefix fgp: <https://database.factgrid.de/prop/direct/> .

[] a sh:NodeShape ;
  sh:targetSubjectsOf fgp:P2 ;
  sh:property [ sh:path rdfs:label ; sh:minCount 1 ;
    sh:message "Every classified member should carry a human-readable label." ] .`
      },
      {
        label: "Members are named",
        tip: "A second target: every entity with a \"Member of\" link should carry a label, so the network reads in plain language.",
        shape: `@prefix sh: <http://www.w3.org/ns/shacl#> .
@prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> .
@prefix fgp: <https://database.factgrid.de/prop/direct/> .

[] a sh:NodeShape ;
  sh:targetSubjectsOf fgp:P91 ;
  sh:property [ sh:path rdfs:label ; sh:minCount 1 ;
    sh:message "An entity in a P91 relation should carry a label." ] .`
      },
      {
        label: "Entities with a recorded gender are named",
        tip: "Every entity that records a gender should carry a label.",
        shape: `@prefix sh: <http://www.w3.org/ns/shacl#> .
@prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> .
@prefix fgp: <https://database.factgrid.de/prop/direct/> .

[] a sh:NodeShape ;
  sh:targetSubjectsOf fgp:P154 ;
  sh:property [ sh:path rdfs:label ; sh:minCount 1 ;
    sh:message "A P154 subject should carry a label." ] .`
      },
      {
        label: "People with a career statement are named",
        tip: "Every entity that records a career statement (the densest relation) should carry a label.",
        shape: `@prefix sh: <http://www.w3.org/ns/shacl#> .
@prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> .
@prefix fgp: <https://database.factgrid.de/prop/direct/> .

[] a sh:NodeShape ;
  sh:targetSubjectsOf fgp:P165 ;
  sh:property [ sh:path rdfs:label ; sh:minCount 1 ;
    sh:message "A P165 subject should carry a label." ] .`
      }
    ],
    history: [
      {
        label: "Territories are named, dated and geolocated",
        tip: "Every historical territory should carry a label, a year and a GeoSPARQL geometry. The 7-era basemap conforms - all 2,059 territories are complete.",
        shape: `@prefix sh: <http://www.w3.org/ns/shacl#> .
@prefix ex: <http://ex/> .
@prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> .
@prefix geo: <http://www.opengis.net/ont/geosparql#> .

ex:TerritoryShape a sh:NodeShape ;
  sh:targetClass ex:Territory ;
  sh:property [ sh:path rdfs:label ; sh:minCount 1 ] ;
  sh:property [ sh:path ex:year ; sh:minCount 1 ] ;
  sh:property [ sh:path geo:hasGeometry ; sh:minCount 1 ;
    sh:message "Every historical territory must carry a geometry." ] .`
      },
      {
        label: "Territories record their political context",
        tip: "Every territory should record what it is part of (ex:partOf) and whom it is subject to (ex:subjectTo) — a few snapshots leave one or the other blank.",
        shape: `@prefix sh: <http://www.w3.org/ns/shacl#> .
@prefix ex: <http://ex/> .

ex:TerritoryContextShape a sh:NodeShape ;
  sh:targetClass ex:Territory ;
  sh:property [ sh:path ex:partOf ; sh:minCount 1 ;
    sh:message "Territory should record what it is part of." ] ;
  sh:property [ sh:path ex:subjectTo ; sh:minCount 1 ;
    sh:message "Territory should record whom it is subject to." ] .`
      },
      {
        label: "Territories carry one geometry",
        tip: "Every ex:Territory should carry exactly one GeoSPARQL geometry.",
        shape: `@prefix sh: <http://www.w3.org/ns/shacl#> .
@prefix ex: <http://ex/> .
@prefix geo: <http://www.opengis.net/ont/geosparql#> .

ex:TerritoryGeomShape a sh:NodeShape ;
  sh:targetClass ex:Territory ;
  sh:property [ sh:path geo:hasGeometry ; sh:minCount 1 ; sh:maxCount 1 ;
    sh:message "Territory should carry exactly one geometry." ] .`
      },
      {
        label: "Geometries are WKT literals",
        tip: "Every geometry node (a subject of geo:hasGeometry's value) should carry a geo:asWKT serialization.",
        shape: `@prefix sh: <http://www.w3.org/ns/shacl#> .
@prefix geo: <http://www.opengis.net/ont/geosparql#> .

[] a sh:NodeShape ;
  sh:targetSubjectsOf geo:asWKT ;
  sh:property [ sh:path geo:asWKT ; sh:minCount 1 ] .`
      }
    ],
    "linked-jazz": [
      {
        label: "Every musician has a name",
        tip: "On this untyped social graph, every musician who 'knows of' another (a subject of rel:knowsOf) should carry a foaf:name. Conforms - the network is fully named.",
        shape: `@prefix sh: <http://www.w3.org/ns/shacl#> .
@prefix foaf: <http://xmlns.com/foaf/0.1/> .
@prefix rel: <http://purl.org/vocab/relationship/> .

[] a sh:NodeShape ;
  sh:targetSubjectsOf rel:knowsOf ;
  sh:property [ sh:path foaf:name ; sh:minCount 1 ;
    sh:message "Every musician in the social graph should carry a name." ] .`
      },
      {
        label: "Influenced musicians are named",
        tip: "A second relation: every subject of relationship:influencedBy should carry a foaf:name.",
        shape: `@prefix sh: <http://www.w3.org/ns/shacl#> .
@prefix foaf: <http://xmlns.com/foaf/0.1/> .
@prefix rel: <http://purl.org/vocab/relationship/> .

[] a sh:NodeShape ;
  sh:targetSubjectsOf rel:influencedBy ;
  sh:property [ sh:path foaf:name ; sh:minCount 1 ;
    sh:message "An influenced musician should carry a name." ] .`
      },
      {
        label: "Mentored musicians are named",
        tip: "Every subject of relationship:mentorOf should carry a foaf:name.",
        shape: `@prefix sh: <http://www.w3.org/ns/shacl#> .
@prefix foaf: <http://xmlns.com/foaf/0.1/> .
@prefix rel: <http://purl.org/vocab/relationship/> .

[] a sh:NodeShape ;
  sh:targetSubjectsOf rel:mentorOf ;
  sh:property [ sh:path foaf:name ; sh:minCount 1 ; sh:message "A mentor should be named." ] .`
      },
      {
        label: "Band collaborators are named",
        tip: "Every subject of linkedjazz:playedTogether should carry a foaf:name.",
        shape: `@prefix sh: <http://www.w3.org/ns/shacl#> .
@prefix foaf: <http://xmlns.com/foaf/0.1/> .
@prefix lj: <http://linkedjazz.org/ontology/> .

[] a sh:NodeShape ;
  sh:targetSubjectsOf lj:playedTogether ;
  sh:property [ sh:path foaf:name ; sh:minCount 1 ; sh:message "A collaborator should be named." ] .`
      }
    ],
    mimotext: [
      {
        label: "Novels in the style network are complete",
        tip: "MiMoText's distinctive layer is 520 stylometric-similarity edges. Every novel in that style network should carry a label and name its author. SHACL flags 48 nodes that don't.",
        shape: `@prefix sh: <http://www.w3.org/ns/shacl#> .
@prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> .
@prefix mt: <http://data.mimotext.uni-trier.de/prop/direct/> .

[] a sh:NodeShape ;
  sh:targetSubjectsOf mt:P49 ;
  sh:property [ sh:path rdfs:label ; sh:minCount 1 ;
    sh:message "A novel in the stylometric-similarity network should carry a label." ] ;
  sh:property [ sh:path mt:P5 ; sh:minCount 1 ;
    sh:message "A novel in the stylometric-similarity network should name its author (P5)." ] .`
      },
      {
        label: "Novels with an author carry a date",
        tip: "Every work that names an author should also carry a publication date — a few are undated.",
        shape: `@prefix sh: <http://www.w3.org/ns/shacl#> .
@prefix mt: <http://data.mimotext.uni-trier.de/prop/direct/> .

[] a sh:NodeShape ;
  sh:targetSubjectsOf mt:P5 ;
  sh:property [ sh:path mt:P9 ; sh:minCount 1 ;
    sh:message "A novel that names an author should carry a publication date." ] .`
      },
      {
        label: "Themed works are labelled",
        tip: "Every work that has a theme should carry a label.",
        shape: `@prefix sh: <http://www.w3.org/ns/shacl#> .
@prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> .
@prefix mt: <http://data.mimotext.uni-trier.de/prop/direct/> .

[] a sh:NodeShape ;
  sh:targetSubjectsOf mt:P36 ;
  sh:property [ sh:path rdfs:label ; sh:minCount 1 ;
    sh:message "A themed work should carry a label." ] .`
      },
      {
        label: "Works with a genre are labelled",
        tip: "Every work that records a genre should carry a label.",
        shape: `@prefix sh: <http://www.w3.org/ns/shacl#> .
@prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> .
@prefix mt: <http://data.mimotext.uni-trier.de/prop/direct/> .

[] a sh:NodeShape ;
  sh:targetSubjectsOf mt:P12 ;
  sh:property [ sh:path rdfs:label ; sh:minCount 1 ;
    sh:message "A work with a genre should carry a label." ] .`
      }
    ],
    mmm: [
      {
        label: "Manuscripts record their provenance",
        tip: "Provenance completeness over CIDOC-CRM: every manuscript (F4_Manifestation_Singleton) should carry a label, a former-or-current owner (a person) and a production date. On the full 23.3M-triple graph this targets every manuscript, so it is heavy - use the lazy remote path (it fetches only the shapes' targets) or run it on a bound subset; real provenance is incomplete, so expect many violations.",
        shape: `@prefix sh: <http://www.w3.org/ns/shacl#> .
@prefix skos: <http://www.w3.org/2004/02/skos/core#> .
@prefix crm: <http://erlangen-crm.org/current/> .
@prefix frbr: <http://erlangen-crm.org/efrbroo/> .
@prefix mmm: <http://ldf.fi/schema/mmm/> .

mmm:ManuscriptShape a sh:NodeShape ;
  sh:targetClass frbr:F4_Manifestation_Singleton ;
  sh:property [ sh:path skos:prefLabel ; sh:minCount 1 ] ;
  sh:property [ sh:path crm:P51_has_former_or_current_owner ; sh:class crm:E21_Person ; sh:minCount 1 ;
    sh:message "Every manuscript should record a former or current owner." ] ;
  sh:property [ sh:path mmm:produced_when ; sh:minCount 1 ;
    sh:message "Every manuscript should record when it was produced." ] .`
      },
      {
        label: "Every person is named",
        tip: "Every CIDOC-CRM E21_Person should carry a skos:prefLabel. The actor list conforms.",
        shape: `@prefix sh: <http://www.w3.org/ns/shacl#> .
@prefix skos: <http://www.w3.org/2004/02/skos/core#> .
@prefix crm: <http://erlangen-crm.org/current/> .

crm:PersonNamedShape a sh:NodeShape ;
  sh:targetClass crm:E21_Person ;
  sh:property [ sh:path skos:prefLabel ; sh:minCount 1 ;
    sh:message "Every person should carry a preferred label." ] .`
      },
      {
        label: "Manuscripts name a work",
        tip: "Every manuscript (F4_Manifestation_Singleton) should record the work it manifests.",
        shape: `@prefix sh: <http://www.w3.org/ns/shacl#> .
@prefix frbr: <http://erlangen-crm.org/efrbroo/> .
@prefix mmm: <http://ldf.fi/schema/mmm/> .

mmm:ManuscriptWorkShape a sh:NodeShape ;
  sh:targetClass frbr:F4_Manifestation_Singleton ;
  sh:property [ sh:path mmm:manuscript_work ; sh:minCount 1 ;
    sh:message "Manuscript records no work." ] .`
      },
      {
        label: "Places are geolocated",
        tip: "Every CIDOC-CRM E53_Place should carry WGS84 lat/long coordinates.",
        shape: `@prefix sh: <http://www.w3.org/ns/shacl#> .
@prefix crm: <http://erlangen-crm.org/current/> .
@prefix wgs: <http://www.w3.org/2003/01/geo/wgs84_pos#> .

crm:PlaceGeoShape a sh:NodeShape ;
  sh:targetClass crm:E53_Place ;
  sh:property [ sh:path wgs:lat ; sh:minCount 1 ] ;
  sh:property [ sh:path wgs:long ; sh:minCount 1 ;
    sh:message "Place has no coordinates." ] .`
      }
    ],
    monarch: [
      {
        label: "Diseases carry phenotypes",
        tip: "Biomedical completeness over the Biolink model: every Disease should be labelled and linked to at least one phenotypic feature (biolink:has_phenotype). Six diseases carry no phenotype.",
        shape: `@prefix sh: <http://www.w3.org/ns/shacl#> .
@prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> .
@prefix bl: <https://w3id.org/biolink/vocab/> .

bl:DiseaseShape a sh:NodeShape ;
  sh:targetClass bl:Disease ;
  sh:property [ sh:path rdfs:label ; sh:minCount 1 ] ;
  sh:property [ sh:path bl:has_phenotype ; sh:class bl:PhenotypicFeature ; sh:minCount 1 ;
    sh:message "Every disease should link to at least one phenotypic feature." ] .`
      },
      {
        label: "Genes are named and placed in a taxon",
        tip: "Every biolink:Gene should carry a label and an in_taxon link (which species it belongs to).",
        shape: `@prefix sh: <http://www.w3.org/ns/shacl#> .
@prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> .
@prefix bl: <https://w3id.org/biolink/vocab/> .

bl:GeneShape a sh:NodeShape ;
  sh:targetClass bl:Gene ;
  sh:property [ sh:path rdfs:label ; sh:minCount 1 ] ;
  sh:property [ sh:path bl:in_taxon ; sh:minCount 1 ;
    sh:message "Every gene should be placed in a taxon." ] .`
      },
      {
        label: "Phenotypes are named",
        tip: "Every biolink:PhenotypicFeature should carry an rdfs:label.",
        shape: `@prefix sh: <http://www.w3.org/ns/shacl#> .
@prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> .
@prefix bl: <https://w3id.org/biolink/vocab/> .

bl:PhenotypeShape a sh:NodeShape ;
  sh:targetClass bl:PhenotypicFeature ;
  sh:property [ sh:path rdfs:label ; sh:minCount 1 ; sh:message "Phenotype has no label." ] .`
      },
      {
        label: "Genes carry cross-reference synonyms",
        tip: "Every biolink:Gene should carry at least one skos:altLabel (symbol/synonym).",
        shape: `@prefix sh: <http://www.w3.org/ns/shacl#> .
@prefix skos: <http://www.w3.org/2004/02/skos/core#> .
@prefix bl: <https://w3id.org/biolink/vocab/> .

bl:GeneAltShape a sh:NodeShape ;
  sh:targetClass bl:Gene ;
  sh:property [ sh:path skos:altLabel ; sh:minCount 1 ;
    sh:message "Gene has no synonym." ] .`
      }
    ],
    nomisma: [
      {
        label: "Coin types record material + dates",
        tip: "Numismatic data quality: every coin type (nm:TypeSeriesItem) should record a denomination, a material and start/end dates. SHACL finds exactly one coin type missing its material.",
        shape: `@prefix sh: <http://www.w3.org/ns/shacl#> .
@prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> .
@prefix nm: <http://nomisma.org/ontology#> .

nm:CoinTypeShape a sh:NodeShape ;
  sh:targetClass nm:TypeSeriesItem ;
  sh:property [ sh:path rdfs:label ; sh:minCount 1 ] ;
  sh:property [ sh:path nm:hasDenomination ; sh:class nm:Denomination ; sh:minCount 1 ] ;
  sh:property [ sh:path nm:hasMaterial ; sh:class nm:Material ; sh:minCount 1 ;
    sh:message "Every coin type should record its material." ] ;
  sh:property [ sh:path nm:hasStartDate ; sh:minCount 1 ] ;
  sh:property [ sh:path nm:hasEndDate ; sh:minCount 1 ] .`
      },
      {
        label: "Mints are named",
        tip: "Every nm:Mint should carry an rdfs:label — the place names behind the coins.",
        shape: `@prefix sh: <http://www.w3.org/ns/shacl#> .
@prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> .
@prefix nm: <http://nomisma.org/ontology#> .

nm:MintShape a sh:NodeShape ;
  sh:targetClass nm:Mint ;
  sh:property [ sh:path rdfs:label ; sh:minCount 1 ;
    sh:message "Every mint should be named." ] .`
      },
      {
        label: "Denominations are named",
        tip: "Every nm:Denomination should carry an rdfs:label.",
        shape: `@prefix sh: <http://www.w3.org/ns/shacl#> .
@prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> .
@prefix nm: <http://nomisma.org/ontology#> .

nm:DenominationShape a sh:NodeShape ;
  sh:targetClass nm:Denomination ;
  sh:property [ sh:path rdfs:label ; sh:minCount 1 ; sh:message "Denomination has no name." ] .`
      },
      {
        label: "Coins record a minting authority",
        tip: "Every nm:TypeSeriesItem should record an issuing authority — coins of uncertain authority are flagged.",
        shape: `@prefix sh: <http://www.w3.org/ns/shacl#> .
@prefix nm: <http://nomisma.org/ontology#> .

nm:CoinAuthorityShape a sh:NodeShape ;
  sh:targetClass nm:TypeSeriesItem ;
  sh:property [ sh:path nm:hasAuthority ; sh:class nm:Authority ; sh:minCount 1 ;
    sh:message "Coin type records no authority." ] .`
      }
    ],
    "openalex-astrocytes": [
      {
        label: "Papers are well-formed",
        tip: "Bibliographic integrity: every Work needs exactly one title, a year, at least one author (a Person) and at least one topic (a Concept). One paper in the set has no author - SHACL catches it.",
        shape: `@prefix sh: <http://www.w3.org/ns/shacl#> .
@prefix ex: <http://ex/> .
@prefix dct: <http://purl.org/dc/terms/> .

ex:WorkShape a sh:NodeShape ;
  sh:targetClass ex:Work ;
  sh:property [ sh:path dct:title ; sh:minCount 1 ; sh:maxCount 1 ] ;
  sh:property [ sh:path ex:year ; sh:minCount 1 ] ;
  sh:property [ sh:path ex:author ; sh:class ex:Person ; sh:minCount 1 ;
    sh:message "Every paper should list at least one author." ] ;
  sh:property [ sh:path ex:topic ; sh:class ex:Concept ; sh:minCount 1 ] .`
      },
      {
        label: "Authors are named",
        tip: "Every ex:Person (author) should carry a foaf:name. The author list conforms.",
        shape: `@prefix sh: <http://www.w3.org/ns/shacl#> .
@prefix ex: <http://ex/> .
@prefix foaf: <http://xmlns.com/foaf/0.1/> .

ex:AuthorNamedShape a sh:NodeShape ;
  sh:targetClass ex:Person ;
  sh:property [ sh:path foaf:name ; sh:minCount 1 ;
    sh:message "Every author should carry a name." ] .`
      },
      {
        label: "Concepts are named",
        tip: "Every ex:Concept (research topic) should carry an rdfs:label.",
        shape: `@prefix sh: <http://www.w3.org/ns/shacl#> .
@prefix ex: <http://ex/> .
@prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> .

ex:ConceptShape a sh:NodeShape ;
  sh:targetClass ex:Concept ;
  sh:property [ sh:path rdfs:label ; sh:minCount 1 ; sh:message "Concept has no label." ] .`
      },
      {
        label: "Institutions are named",
        tip: "Every ex:Institution should carry an rdfs:label.",
        shape: `@prefix sh: <http://www.w3.org/ns/shacl#> .
@prefix ex: <http://ex/> .
@prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> .

ex:InstitutionShape a sh:NodeShape ;
  sh:targetClass ex:Institution ;
  sh:property [ sh:path rdfs:label ; sh:minCount 1 ; sh:message "Institution has no label." ] .`
      }
    ],
    opencitations: [
      {
        label: "Articles carry bibliographic metadata",
        tip: "FRBR/Dublin-Core completeness: every fabio:JournalArticle should carry a title, a date, a creator and a container (dct:isPartOf). The citation neighbourhood conforms.",
        shape: `@prefix sh: <http://www.w3.org/ns/shacl#> .
@prefix fabio: <http://purl.org/spar/fabio/> .
@prefix dct: <http://purl.org/dc/terms/> .

fabio:ArticleShape a sh:NodeShape ;
  sh:targetClass fabio:JournalArticle ;
  sh:property [ sh:path dct:title ; sh:minCount 1 ] ;
  sh:property [ sh:path dct:date ; sh:minCount 1 ] ;
  sh:property [ sh:path dct:creator ; sh:minCount 1 ;
    sh:message "Every journal article should record at least one creator." ] ;
  sh:property [ sh:path dct:isPartOf ; sh:minCount 1 ] .`
      },
      {
        label: "Book chapters carry metadata",
        tip: "A second FRBR class: every fabio:BookChapter should carry a title and a date.",
        shape: `@prefix sh: <http://www.w3.org/ns/shacl#> .
@prefix fabio: <http://purl.org/spar/fabio/> .
@prefix dct: <http://purl.org/dc/terms/> .

fabio:ChapterShape a sh:NodeShape ;
  sh:targetClass fabio:BookChapter ;
  sh:property [ sh:path dct:title ; sh:minCount 1 ] ;
  sh:property [ sh:path dct:date ; sh:minCount 1 ;
    sh:message "Every book chapter should carry a date." ] .`
      },
      {
        label: "Reference entries have a creator",
        tip: "Every fabio:ReferenceEntry should record at least one dct:creator.",
        shape: `@prefix sh: <http://www.w3.org/ns/shacl#> .
@prefix fabio: <http://purl.org/spar/fabio/> .
@prefix dct: <http://purl.org/dc/terms/> .

fabio:RefEntryShape a sh:NodeShape ;
  sh:targetClass fabio:ReferenceEntry ;
  sh:property [ sh:path dct:creator ; sh:minCount 1 ;
    sh:message "Reference entry has no creator." ] .`
      },
      {
        label: "Creators are named people",
        tip: "Every value of dct:creator should carry a foaf:name.",
        shape: `@prefix sh: <http://www.w3.org/ns/shacl#> .
@prefix dct: <http://purl.org/dc/terms/> .
@prefix foaf: <http://xmlns.com/foaf/0.1/> .

[] a sh:NodeShape ;
  sh:targetObjectsOf dct:creator ;
  sh:property [ sh:path foaf:name ; sh:minCount 1 ; sh:message "Creator has no name." ] .`
      }
    ],
    orkg: [
      {
        label: "Papers list their authors",
        tip: "Scholarly metadata: every orkg:Paper should be labelled and list its authors (orkgp:hasAuthors). Four papers have no authors recorded - SHACL flags them.",
        shape: `@prefix sh: <http://www.w3.org/ns/shacl#> .
@prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> .
@prefix orkgc: <https://orkg.org/class/> .
@prefix orkgp: <https://orkg.org/property/> .

orkgc:PaperShape a sh:NodeShape ;
  sh:targetClass orkgc:Paper ;
  sh:property [ sh:path rdfs:label ; sh:minCount 1 ] ;
  sh:property [ sh:path orkgp:hasAuthors ; sh:minCount 1 ;
    sh:message "Every paper should list its authors." ] .`
      },
      {
        label: "Authors are named",
        tip: "Every orkg:Author should carry an rdfs:label — the names behind the contributions.",
        shape: `@prefix sh: <http://www.w3.org/ns/shacl#> .
@prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> .
@prefix orkgc: <https://orkg.org/class/> .

orkgc:AuthorShape a sh:NodeShape ;
  sh:targetClass orkgc:Author ;
  sh:property [ sh:path rdfs:label ; sh:minCount 1 ;
    sh:message "Every author should be named." ] .`
      },
      {
        label: "Contributions are labelled",
        tip: "Every orkg:Contribution should carry an rdfs:label.",
        shape: `@prefix sh: <http://www.w3.org/ns/shacl#> .
@prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> .
@prefix orkgc: <https://orkg.org/class/> .

orkgc:ContributionShape a sh:NodeShape ;
  sh:targetClass orkgc:Contribution ;
  sh:property [ sh:path rdfs:label ; sh:minCount 1 ; sh:message "Contribution has no label." ] .`
      },
      {
        label: "Venues are labelled",
        tip: "Every orkg:Venue should carry an rdfs:label.",
        shape: `@prefix sh: <http://www.w3.org/ns/shacl#> .
@prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> .
@prefix orkgc: <https://orkg.org/class/> .

orkgc:VenueShape a sh:NodeShape ;
  sh:targetClass orkgc:Venue ;
  sh:property [ sh:path rdfs:label ; sh:minCount 1 ; sh:message "Venue has no label." ] .`
      }
    ],
    "theographic-graph": [
      {
        label: "Places are geolocated",
        tip: "Geospatial completeness: every Biblical Place should carry a label and lat/long coordinates. SHACL flags 49 places that aren't geolocated.",
        shape: `@prefix sh: <http://www.w3.org/ns/shacl#> .
@prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> .
@prefix theo: <http://theographic/ontology#> .
@prefix geo: <http://www.w3.org/2003/01/geo/wgs84_pos#> .

theo:PlaceShape a sh:NodeShape ;
  sh:targetClass theo:Place ;
  sh:property [ sh:path rdfs:label ; sh:minCount 1 ] ;
  sh:property [ sh:path geo:lat ; sh:minCount 1 ;
    sh:message "Every Biblical place should carry a latitude." ] ;
  sh:property [ sh:path geo:long ; sh:minCount 1 ;
    sh:message "Every Biblical place should carry a longitude." ] .`
      },
      {
        label: "Events are dated and located",
        tip: "Every theo:Event should carry a year and an occurredIn place — a few events are undated.",
        shape: `@prefix sh: <http://www.w3.org/ns/shacl#> .
@prefix theo: <http://theographic/ontology#> .

theo:EventShape a sh:NodeShape ;
  sh:targetClass theo:Event ;
  sh:property [ sh:path theo:year ; sh:minCount 1 ;
    sh:message "Every event should carry a year." ] ;
  sh:property [ sh:path theo:occurredIn ; sh:minCount 1 ;
    sh:message "Every event should record where it occurred." ] .`
      },
      {
        label: "People carry a gender",
        tip: "Every foaf:Person should carry a foaf:gender. The graph conforms.",
        shape: `@prefix sh: <http://www.w3.org/ns/shacl#> .
@prefix foaf: <http://xmlns.com/foaf/0.1/> .

foaf:PersonGenderShape a sh:NodeShape ;
  sh:targetClass foaf:Person ;
  sh:property [ sh:path foaf:gender ; sh:minCount 1 ; sh:message "Person has no gender." ] .`
      },
      {
        label: "People-groups are named",
        tip: "Every theo:PeopleGroup should carry an rdfs:label.",
        shape: `@prefix sh: <http://www.w3.org/ns/shacl#> .
@prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> .
@prefix theo: <http://theographic/ontology#> .

theo:PeopleGroupShape a sh:NodeShape ;
  sh:targetClass theo:PeopleGroup ;
  sh:property [ sh:path rdfs:label ; sh:minCount 1 ; sh:message "People-group has no label." ] .`
      }
    ],
    "getty-ulan": [
      {
        label: "Masters carry a name + biography",
        tip: "Run after Cache remote (SHACL materializes the graph in memory). Lineage completeness: every master who taught a pupil (gvp:teacherOf) should carry a name (skos:prefLabel) and a one-line biography (schema:description). The lineage conforms - every teacher is documented.",
        shape: `@prefix sh: <http://www.w3.org/ns/shacl#> .
@prefix skos: <http://www.w3.org/2004/02/skos/core#> .
@prefix gvp: <http://vocab.getty.edu/ontology#> .
@prefix schema: <http://schema.org/> .

[] a sh:NodeShape ;
  sh:targetSubjectsOf gvp:teacherOf ;
  sh:property [ sh:path skos:prefLabel ; sh:minCount 1 ] ;
  sh:property [ sh:path schema:description ; sh:minCount 1 ;
    sh:message "Every master (teacherOf) should carry a one-line biography." ] .`
      },
      {
        label: "Pupils are named",
        tip: "Run after Cache remote. The other end of the lineage: every pupil (object of gvp:teacherOf) should carry a skos:prefLabel.",
        shape: `@prefix sh: <http://www.w3.org/ns/shacl#> .
@prefix skos: <http://www.w3.org/2004/02/skos/core#> .
@prefix gvp: <http://vocab.getty.edu/ontology#> .

[] a sh:NodeShape ;
  sh:targetObjectsOf gvp:teacherOf ;
  sh:property [ sh:path skos:prefLabel ; sh:minCount 1 ;
    sh:message "Every pupil should carry a name." ] .`
      },
      {
        label: "Influencers are named",
        tip: "Run after Cache remote. Every artist who influenced another (gvp:influenced) should carry a skos:prefLabel.",
        shape: `@prefix sh: <http://www.w3.org/ns/shacl#> .
@prefix skos: <http://www.w3.org/2004/02/skos/core#> .
@prefix gvp: <http://vocab.getty.edu/ontology#> .

[] a sh:NodeShape ;
  sh:targetSubjectsOf gvp:influenced ;
  sh:property [ sh:path skos:prefLabel ; sh:minCount 1 ;
    sh:message "An influencer should be named." ] .`
      },
      {
        label: "Masters carry a nationality",
        tip: "Run after Cache remote. Every master who taught a pupil (gvp:teacherOf) should record a nationality.",
        shape: `@prefix sh: <http://www.w3.org/ns/shacl#> .
@prefix gvp: <http://vocab.getty.edu/ontology#> .

[] a sh:NodeShape ;
  sh:targetSubjectsOf gvp:teacherOf ;
  sh:property [ sh:path gvp:nationality ; sh:minCount 1 ;
    sh:message "A master records no nationality." ] .`
      }
    ],
    "wikidata-100mb": [
      {
        label: "People with a job have a birth date",
        tip: "Run after Cache remote (SHACL materializes ~104 MB in memory). Every person with an occupation should carry a label and a date of birth. Real Wikidata is incomplete, so expect violations — people missing a birth date.",
        shape: `@prefix sh: <http://www.w3.org/ns/shacl#> .
@prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> .
@prefix wdt: <http://www.wikidata.org/prop/direct/> .

[] a sh:NodeShape ;
  sh:targetSubjectsOf wdt:P106 ;
  sh:property [ sh:path rdfs:label ; sh:minCount 1 ] ;
  sh:property [ sh:path wdt:P569 ; sh:minCount 1 ;
    sh:message "A person with an occupation (P106) should record a date of birth (P569)." ] .`
      },
      {
        label: "People with a job have a citizenship",
        tip: "Run after Cache remote. Every person with an occupation should record a country of citizenship. Real Wikidata is incomplete, so expect violations.",
        shape: `@prefix sh: <http://www.w3.org/ns/shacl#> .
@prefix wdt: <http://www.wikidata.org/prop/direct/> .

[] a sh:NodeShape ;
  sh:targetSubjectsOf wdt:P106 ;
  sh:property [ sh:path wdt:P27 ; sh:minCount 1 ;
    sh:message "A person with an occupation should record a citizenship (P27)." ] .`
      },
      {
        label: "People with a job have a place of birth",
        tip: "Run after Cache remote. Every person with an occupation should record a place of birth. Real Wikidata is incomplete, so expect violations.",
        shape: `@prefix sh: <http://www.w3.org/ns/shacl#> .
@prefix wdt: <http://www.wikidata.org/prop/direct/> .

[] a sh:NodeShape ;
  sh:targetSubjectsOf wdt:P106 ;
  sh:property [ sh:path wdt:P19 ; sh:minCount 1 ;
    sh:message "Person records no place of birth (P19)." ] .`
      },
      {
        label: "People with a birthplace are labelled",
        tip: "Run after Cache remote. Every person who records a place of birth should also carry a label.",
        shape: `@prefix sh: <http://www.w3.org/ns/shacl#> .
@prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> .
@prefix wdt: <http://www.wikidata.org/prop/direct/> .

[] a sh:NodeShape ;
  sh:targetSubjectsOf wdt:P19 ;
  sh:property [ sh:path rdfs:label ; sh:minCount 1 ;
    sh:message "Person with a birthplace has no label." ] .`
      }
    ],
    "ohm-full": [
      {
        label: "Map features are named + dated",
        tip: "Run after Cache remote (SHACL materializes ~150 MB in memory). Every geolocated feature (a subject of geo:hasGeometry) should carry a name (rdfs:label) and a start year (ex:startYear). Flags map features that are undated or unnamed.",
        shape: `@prefix sh: <http://www.w3.org/ns/shacl#> .
@prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> .
@prefix ex: <http://ex/> .
@prefix geo: <http://www.opengis.net/ont/geosparql#> .

[] a sh:NodeShape ;
  sh:targetSubjectsOf geo:hasGeometry ;
  sh:property [ sh:path rdfs:label ; sh:minCount 1 ;
    sh:message "Every geolocated historical feature should carry a name." ] ;
  sh:property [ sh:path ex:startYear ; sh:minCount 1 ;
    sh:message "Every geolocated historical feature should carry a start year." ] .`
      },
      {
        label: "Features carry an end year",
        tip: "Run after Cache remote. Every geolocated feature should also carry an ex:endYear (2100 = still present); some are missing it.",
        shape: `@prefix sh: <http://www.w3.org/ns/shacl#> .
@prefix ex: <http://ex/> .
@prefix geo: <http://www.opengis.net/ont/geosparql#> .

[] a sh:NodeShape ;
  sh:targetSubjectsOf geo:hasGeometry ;
  sh:property [ sh:path ex:endYear ; sh:minCount 1 ;
    sh:message "Every geolocated historical feature should carry an end year." ] .`
      },
      {
        label: "Geometries are WKT literals",
        tip: "Run after Cache remote. Every feature geometry should carry a geo:asWKT serialization.",
        shape: `@prefix sh: <http://www.w3.org/ns/shacl#> .
@prefix geo: <http://www.opengis.net/ont/geosparql#> .

[] a sh:NodeShape ;
  sh:targetSubjectsOf geo:asWKT ;
  sh:property [ sh:path geo:asWKT ; sh:minCount 1 ] .`
      },
      {
        label: "Dated features carry both years",
        tip: "Run after Cache remote. Every feature with a start year should also carry an end year (2100 = still present).",
        shape: `@prefix sh: <http://www.w3.org/ns/shacl#> .
@prefix ex: <http://ex/> .

[] a sh:NodeShape ;
  sh:targetSubjectsOf ex:startYear ;
  sh:property [ sh:path ex:endYear ; sh:minCount 1 ;
    sh:message "A dated feature has no end year." ] .`
      }
    ],
    wikidata: [
      {
        label: "People with a job have a birth date",
        tip: "Note: SHACL validates the whole graph in memory, which isn't viable at 1 GB in the browser — run these on the wikidata-100MB twin (same shapes, same vocab) or Cache remote. Every person with an occupation should record a date of birth.",
        shape: `@prefix sh: <http://www.w3.org/ns/shacl#> .
@prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> .
@prefix wdt: <http://www.wikidata.org/prop/direct/> .

[] a sh:NodeShape ;
  sh:targetSubjectsOf wdt:P106 ;
  sh:property [ sh:path rdfs:label ; sh:minCount 1 ] ;
  sh:property [ sh:path wdt:P569 ; sh:minCount 1 ;
    sh:message "Person with an occupation has no date of birth (P569)." ] .`
      },
      {
        label: "People with a job have a citizenship",
        tip: "Best run on the wikidata-100MB twin (1 GB can't be materialized for SHACL). Every person with an occupation should record a country of citizenship.",
        shape: `@prefix sh: <http://www.w3.org/ns/shacl#> .
@prefix wdt: <http://www.wikidata.org/prop/direct/> .

[] a sh:NodeShape ;
  sh:targetSubjectsOf wdt:P106 ;
  sh:property [ sh:path wdt:P27 ; sh:minCount 1 ;
    sh:message "Person with an occupation has no citizenship (P27)." ] .`
      },
      {
        label: "People with a job have a place of birth",
        tip: "Best run on the wikidata-100MB twin. Every person with an occupation should record a place of birth.",
        shape: `@prefix sh: <http://www.w3.org/ns/shacl#> .
@prefix wdt: <http://www.wikidata.org/prop/direct/> .

[] a sh:NodeShape ;
  sh:targetSubjectsOf wdt:P106 ;
  sh:property [ sh:path wdt:P19 ; sh:minCount 1 ;
    sh:message "Person with an occupation has no place of birth (P19)." ] .`
      },
      {
        label: "People with a birthplace are labelled",
        tip: "Best run on the wikidata-100MB twin. Every person who records a place of birth should also carry a label.",
        shape: `@prefix sh: <http://www.w3.org/ns/shacl#> .
@prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> .
@prefix wdt: <http://www.wikidata.org/prop/direct/> .

[] a sh:NodeShape ;
  sh:targetSubjectsOf wdt:P19 ;
  sh:property [ sh:path rdfs:label ; sh:minCount 1 ;
    sh:message "Person with a birthplace has no label." ] .`
      }
    ]
  },
  reach: {
    "causenet": {
      pred: "<https://causenet.org/ontology#causes>",
      seeds: "<https://causenet.org/concept/smoking>",
      examples: [
        { label: "Everything smoking can lead to (causal closure)", pred: "<https://causenet.org/ontology#causes>", seeds: "<https://causenet.org/concept/smoking>", reverse: false },
        { label: "Everything obesity can lead to", pred: "<https://causenet.org/ontology#causes>", seeds: "<https://causenet.org/concept/obesity>", reverse: false },
        { label: "What leads to cancer (root causes, reverse)", pred: "<https://causenet.org/ontology#causes>", seeds: "<https://causenet.org/concept/cancer>", reverse: true }
      ]
    },
    chemotion: {
      pred: "<http://www.w3.org/2000/01/rdf-schema#subClassOf>",
      seeds: "<http://purl.obolibrary.org/obo/CHMO_0000228>",
      examples: [
        { label: "All subtypes of spectroscopy", pred: "<http://www.w3.org/2000/01/rdf-schema#subClassOf>", seeds: "<http://purl.obolibrary.org/obo/CHMO_0000228>", reverse: true },
        { label: "All subtypes of electrochemical analysis", pred: "<http://www.w3.org/2000/01/rdf-schema#subClassOf>", seeds: "<http://purl.obolibrary.org/obo/CHMO_0000003>", reverse: true },
        { label: "What spectroscopy is a kind of (ancestors)", pred: "<http://www.w3.org/2000/01/rdf-schema#subClassOf>", seeds: "<http://purl.obolibrary.org/obo/CHMO_0000228>", reverse: false }
      ]
    },
    "chebi-full": {
      pred: "<http://www.w3.org/2000/01/rdf-schema#subClassOf>",
      seeds: "<http://purl.obolibrary.org/obo/CHEBI_22315>",
      examples: [
        { label: "All subtypes of alkaloid", pred: "<http://www.w3.org/2000/01/rdf-schema#subClassOf>", seeds: "<http://purl.obolibrary.org/obo/CHEBI_22315>", reverse: true },
        { label: "All subtypes of amino acid", pred: "<http://www.w3.org/2000/01/rdf-schema#subClassOf>", seeds: "<http://purl.obolibrary.org/obo/CHEBI_33709>", reverse: true },
        { label: "How caffeine is classified (ancestors)", pred: "<http://www.w3.org/2000/01/rdf-schema#subClassOf>", seeds: "<http://purl.obolibrary.org/obo/CHEBI_27732>", reverse: false }
      ]
    },
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
    causal: {
      pred: "<http://ex/causes>",
      seeds: "<http://ex/Poverty>",
      examples: [
        { label: "What Poverty leads to (causal cascade)", pred: "<http://ex/causes>", seeds: "<http://ex/Poverty>", reverse: false },
        { label: "Everything that causes Atherosclerosis", pred: "<http://ex/causes>", seeds: "<http://ex/Atherosclerosis>", reverse: true }
      ]
    },
    "antarctic-expeditions": {
      pred: "<http://ex/participant>",
      seeds: "<http://www.wikidata.org/entity/Q815478>",
      examples: [
        { label: "Members of an expedition", pred: "<http://ex/participant>", seeds: "<http://www.wikidata.org/entity/Q815478>", reverse: false },
        { label: "Which expeditions a person joined", pred: "<http://ex/participant>", seeds: "<http://www.wikidata.org/entity/Q102527>", reverse: true }
      ]
    },
    "factgrid-illuminati": {
      pred: "<https://database.factgrid.de/prop/direct/P91>",
      seeds: "<https://database.factgrid.de/entity/Q10677>",
      examples: [
        { label: "Members of the Order of the Illuminati", pred: "<https://database.factgrid.de/prop/direct/P91>", seeds: "<https://database.factgrid.de/entity/Q10677>", reverse: true },
        { label: "Which organizations a member belongs to", pred: "<https://database.factgrid.de/prop/direct/P91>", seeds: "<https://database.factgrid.de/entity/Q409>", reverse: false }
      ]
    },
    "linked-jazz": {
      pred: "<http://purl.org/vocab/relationship/knowsOf>",
      seeds: "<http://dbpedia.org/resource/Louie_Bellson>",
      examples: [
        { label: "Who Louie Bellson knows (transitively)", pred: "<http://purl.org/vocab/relationship/knowsOf>", seeds: "<http://dbpedia.org/resource/Louie_Bellson>", reverse: false },
        { label: "Who knows of Count Basie", pred: "<http://purl.org/vocab/relationship/knowsOf>", seeds: "<http://dbpedia.org/resource/Count_Basie>", reverse: true }
      ]
    },
    mimotext: {
      pred: "<http://data.mimotext.uni-trier.de/prop/direct/P49>",
      seeds: "<http://data.mimotext.uni-trier.de/entity/Q1011>",
      examples: [
        { label: "Stylometrically similar novels (a reading neighbourhood)", pred: "<http://data.mimotext.uni-trier.de/prop/direct/P49>", seeds: "<http://data.mimotext.uni-trier.de/entity/Q1011>", reverse: false },
        { label: "Novels written in a similar style to a given one", pred: "<http://data.mimotext.uni-trier.de/prop/direct/P49>", seeds: "<http://data.mimotext.uni-trier.de/entity/Q1022>", reverse: true }
      ]
    },
    mmm: {
      pred: "<http://erlangen-crm.org/current/P51_has_former_or_current_owner>",
      seeds: "<http://ldf.fi/mmm/manifestation_singleton/sdbm_orphan_212926>",
      examples: [
        { label: "Owners of MS Gg.1.1", pred: "<http://erlangen-crm.org/current/P51_has_former_or_current_owner>", seeds: "<http://ldf.fi/mmm/manifestation_singleton/sdbm_orphan_212926>", reverse: false },
        { label: "Manuscripts once owned by a person", pred: "<http://erlangen-crm.org/current/P51_has_former_or_current_owner>", seeds: "<http://ldf.fi/mmm/actor/bibale_301>", reverse: true }
      ]
    },
    monarch: {
      pred: "<https://w3id.org/biolink/vocab/interacts_with>",
      seeds: "<https://identifiers.org/hgnc/4851>",
      examples: [
        { label: "Genes that interact with HTT (huntingtin)", pred: "<https://w3id.org/biolink/vocab/interacts_with>", seeds: "<https://identifiers.org/hgnc/4851>", reverse: false },
        { label: "The huntingtin (HTT) interaction neighbourhood", pred: "<https://w3id.org/biolink/vocab/interacts_with>", seeds: "<https://identifiers.org/hgnc/4851>", reverse: true }
      ]
    },
    nomisma: {
      pred: "<http://nomisma.org/ontology#hasMint>",
      seeds: "<http://nomisma.org/id/pella_macedon>",
      examples: [
        { label: "All coin types struck at Pella", pred: "<http://nomisma.org/ontology#hasMint>", seeds: "<http://nomisma.org/id/pella_macedon>", reverse: true },
        { label: "Mint of a coin type", pred: "<http://nomisma.org/ontology#hasMint>", seeds: "<http://numismatics.org/pella/id/lerider.philip_ii.1.100>", reverse: false }
      ]
    },
    "openalex-astrocytes": {
      pred: "<http://purl.org/spar/cito/cites>",
      seeds: "<https://openalex.org/W2777525962>",
      examples: [
        { label: "Citation closure of a paper", pred: "<http://purl.org/spar/cito/cites>", seeds: "<https://openalex.org/W2777525962>", reverse: false },
        { label: "Who cites a landmark paper", pred: "<http://purl.org/spar/cito/cites>", seeds: "<https://openalex.org/W2124940469>", reverse: true }
      ]
    },
    opencitations: {
      pred: "<http://purl.org/spar/cito/cites>",
      seeds: "<https://doi.org/10.1126/science.1225829>",
      examples: [
        { label: "Citation chain from a paper", pred: "<http://purl.org/spar/cito/cites>", seeds: "<https://doi.org/10.1126/science.1225829>", reverse: false },
        { label: "Who cites this paper", pred: "<http://purl.org/spar/cito/cites>", seeds: "<https://doi.org/10.1126/science.1225829>", reverse: true }
      ]
    },
    orkg: {
      pred: "<https://orkg.org/property/hasAuthors>",
      seeds: "<https://orkg.org/resource/R1348693>",
      examples: [
        { label: "Papers by an author", pred: "<https://orkg.org/property/hasAuthors>", seeds: "<https://orkg.org/resource/R1348693>", reverse: true },
        { label: "Authors of a contribution", pred: "<https://orkg.org/property/hasAuthors>", seeds: "<https://orkg.org/resource/R108344>", reverse: false }
      ]
    },
    "theographic-graph": {
      pred: "<http://theographic/ontology#sibling>",
      seeds: "<http://ex/person/absalom_59>",
      examples: [
        { label: "Sibling network of a Biblical figure", pred: "<http://theographic/ontology#sibling>", seeds: "<http://ex/person/absalom_59>", reverse: false },
        { label: "Ancestors via the father line", pred: "<http://theographic/ontology#father>", seeds: "<http://ex/person/absalom_59>", reverse: false }
      ]
    },
    "getty-ulan": {
      pred: "<http://vocab.getty.edu/ontology#teacherOf>",
      seeds: "<http://vocab.getty.edu/ulan/500004460>",
      examples: [
        { label: "Artistic lineage of a master (all descendants)", pred: "<http://vocab.getty.edu/ontology#teacherOf>", seeds: "<http://vocab.getty.edu/ulan/500004460>", reverse: false },
        { label: "Who taught this artist (all teachers up the line)", pred: "<http://vocab.getty.edu/ontology#teacherOf>", seeds: "<http://vocab.getty.edu/ulan/500004460>", reverse: true }
      ]
    },
    "wikidata-100mb": {
      pred: "<http://www.wikidata.org/prop/direct/P737>",
      seeds: "<http://www.wikidata.org/entity/Q9312>",
      examples: [
        { label: "Everyone influenced by this thinker", pred: "<http://www.wikidata.org/prop/direct/P737>", seeds: "<http://www.wikidata.org/entity/Q9312>", reverse: true },
        { label: "The influence chain behind a person", pred: "<http://www.wikidata.org/prop/direct/P737>", seeds: "<http://www.wikidata.org/entity/Q213736>", reverse: false }
      ]
    },
    wikidata: {
      pred: "<http://www.wikidata.org/prop/direct/P737>",
      seeds: "<http://www.wikidata.org/entity/Q9061>",
      examples: [
        { label: "Everyone influenced by this thinker (lazy, 1 GB)", pred: "<http://www.wikidata.org/prop/direct/P737>", seeds: "<http://www.wikidata.org/entity/Q9061>", reverse: true },
        { label: "The influence chain behind a person", pred: "<http://www.wikidata.org/prop/direct/P737>", seeds: "<http://www.wikidata.org/entity/Q213736>", reverse: false }
      ]
    }
  },
  provenance: {
    "causenet": {
      predicate: "<https://causenet.org/ontology#sentence>",
      examples: [
        {
          label: "Every evidence sentence",
          tip: "Predicate-bound: the whole cn:sentence relation is one contiguous run in the POS permutation - each row shows the tile and byte range it was read from. This is the 24.4M-sentence provenance layer.",
          predicate: "<https://causenet.org/ontology#sentence>"
        },
        {
          label: "Every causal edge",
          tip: "Predicate-bound cn:causes: the 11.6M direct cause->effect edges as one POS run, each with the tile/byte range it came from.",
          predicate: "<https://causenet.org/ontology#causes>"
        },
        {
          label: "All ClueWeb12 web sources",
          tip: "Object-bound rdf:type pattern routed to OSP: every source record typed cn:ClueWeb12SentenceSource, with the tile/byte range each was read from.",
          predicate: "<http://www.w3.org/1999/02/22-rdf-syntax-ns#type>",
          object: "<https://causenet.org/ontology#ClueWeb12SentenceSource>"
        },
        {
          label: "Every support count",
          tip: "Predicate-bound cn:support: each relation's evidence count as one POS run - the numbers behind the 'strongest claims' ranking.",
          predicate: "<https://causenet.org/ontology#support>"
        }
      ]
    },
    chemotion: {
      predicate: "<http://purl.obolibrary.org/obo/chebi/smiles>",
      examples: [
        {
          label: "Every SMILES string",
          tip: "Predicate-bound pattern: the whole chebi:smiles relation is one contiguous run in the POS permutation - each row shows its tile and byte range.",
          predicate: "<http://purl.obolibrary.org/obo/chebi/smiles>"
        },
        {
          label: "All molecular entities",
          tip: "Object-bound rdf:type pattern: routed to OSP - all 3,746 molecules, each with the tile/byte range it was read from.",
          predicate: "<http://www.w3.org/1999/02/22-rdf-syntax-ns#type>",
          object: "<http://purl.obolibrary.org/obo/CHEBI_23367>"
        },
        {
          label: "The whole class hierarchy",
          tip: "Predicate-bound rdfs:subClassOf: the merged CHMO/RXNO/ChEBI subClassOf DAG as one POS run - the edges that build the Schema pyramid.",
          predicate: "<http://www.w3.org/2000/01/rdf-schema#subClassOf>"
        }
      ]
    },
    "chebi-full": {
      predicate: "<https://w3id.org/chemrof/smiles_string>",
      examples: [
        {
          label: "Every SMILES string",
          tip: "Predicate-bound: the whole chemrof:smiles_string relation is one contiguous run in the POS permutation - each row shows its tile and byte range.",
          predicate: "<https://w3id.org/chemrof/smiles_string>"
        },
        {
          label: "The whole subClassOf DAG",
          tip: "Predicate-bound rdfs:subClassOf: ChEBI's ~380k classification edges as one POS run - the edges that build the Schema pyramid.",
          predicate: "<http://www.w3.org/2000/01/rdf-schema#subClassOf>"
        },
        {
          label: "All molecular formulae",
          tip: "Predicate-bound: every chemrof:generalized_empirical_formula value, with the tile/byte range each was read from.",
          predicate: "<https://w3id.org/chemrof/generalized_empirical_formula>"
        }
      ]
    },
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
    causal: {
      predicate: "<http://ex/causes>",
      examples: [
        { label: "Every causal edge", tip: "Predicate-bound: the whole ex:causes relation as one POS run.", predicate: "<http://ex/causes>" },
        { label: "Everything about Poverty", tip: "Subject-bound: SPO routing to one node's facts.", subject: "<http://ex/Poverty>" },
        { label: "All risk factors", tip: "Object-bound rdf:type: routed to OSP.", predicate: "<http://www.w3.org/1999/02/22-rdf-syntax-ns#type>", object: "<http://ex/RiskFactor>" }
      ]
    },
    "antarctic-expeditions": {
      predicate: "<http://ex/participant>",
      examples: [
        { label: "All participation edges", tip: "Predicate-bound: POS permutation.", predicate: "<http://ex/participant>" },
        { label: "Everything about an expedition", tip: "Subject-bound: SPO routing.", subject: "<http://www.wikidata.org/entity/Q815478>" },
        { label: "All expeditions", tip: "Object-bound rdf:type: OSP.", predicate: "<http://www.w3.org/1999/02/22-rdf-syntax-ns#type>", object: "<http://ex/Expedition>" }
      ]
    },
    "factgrid-illuminati": {
      predicate: "<http://www.w3.org/2000/01/rdf-schema#label>",
      examples: [
        { label: "Every resolved label", tip: "Predicate-bound: the dominant rdfs:label relation as one POS run.", predicate: "<http://www.w3.org/2000/01/rdf-schema#label>" },
        { label: "Everything about the Order of the Illuminati", tip: "Subject-bound: SPO routing to one entity's facts.", subject: "<https://database.factgrid.de/entity/Q10677>" }
      ]
    },
    history: {
      predicate: "<http://www.opengis.net/ont/geosparql#asWKT>",
      examples: [
        { label: "Every territory geometry", tip: "Predicate-bound: all geo:asWKT literals as one POS run.", predicate: "<http://www.opengis.net/ont/geosparql#asWKT>" },
        { label: "All territories", tip: "Object-bound rdf:type: OSP.", predicate: "<http://www.w3.org/1999/02/22-rdf-syntax-ns#type>", object: "<http://ex/Territory>" }
      ]
    },
    "linked-jazz": {
      predicate: "<http://xmlns.com/foaf/0.1/name>",
      examples: [
        { label: "Every musician name", tip: "Predicate-bound: all foaf:name literals as one POS run.", predicate: "<http://xmlns.com/foaf/0.1/name>" },
        { label: "Everything about Louie Bellson", tip: "Subject-bound: SPO routing to one musician's edges.", subject: "<http://dbpedia.org/resource/Louie_Bellson>" }
      ]
    },
    mimotext: {
      predicate: "<http://data.mimotext.uni-trier.de/prop/direct/P49>",
      examples: [
        { label: "Every stylometric-similarity edge", tip: "Predicate-bound: the whole stylometric-similarity network as one POS run.", predicate: "<http://data.mimotext.uni-trier.de/prop/direct/P49>" },
        { label: "Everything about one novel", tip: "Subject-bound: SPO routing to a single work's facts.", subject: "<http://data.mimotext.uni-trier.de/entity/Q1011>" }
      ]
    },
    mmm: {
      predicate: "<http://erlangen-crm.org/current/P51_has_former_or_current_owner>",
      examples: [
        { label: "Every ownership edge", tip: "Predicate-bound: the whole former/current-owner relation as one POS run.", predicate: "<http://erlangen-crm.org/current/P51_has_former_or_current_owner>" },
        { label: "Everything about MS Gg.1.1", tip: "Subject-bound (the trilingual compendium): SPO routing fetches only this manuscript's tiles.", subject: "<http://ldf.fi/mmm/manifestation_singleton/sdbm_orphan_212926>" },
        { label: "All people (E21_Person)", tip: "Object-bound rdf:type: OSP.", predicate: "<http://www.w3.org/1999/02/22-rdf-syntax-ns#type>", object: "<http://erlangen-crm.org/current/E21_Person>" }
      ]
    },
    monarch: {
      predicate: "<https://w3id.org/biolink/vocab/has_phenotype>",
      examples: [
        { label: "Every disease-phenotype edge", tip: "Predicate-bound: biolink:has_phenotype as one POS run.", predicate: "<https://w3id.org/biolink/vocab/has_phenotype>" },
        { label: "Everything about a gene", tip: "Subject-bound: SPO routing.", subject: "<https://identifiers.org/hgnc/4851>" },
        { label: "All genes", tip: "Object-bound rdf:type: OSP.", predicate: "<http://www.w3.org/1999/02/22-rdf-syntax-ns#type>", object: "<https://w3id.org/biolink/vocab/Gene>" }
      ]
    },
    nomisma: {
      predicate: "<http://nomisma.org/ontology#hasMint>",
      examples: [
        { label: "Every coin-to-mint edge", tip: "Predicate-bound: nm:hasMint as one POS run.", predicate: "<http://nomisma.org/ontology#hasMint>" },
        { label: "Everything about a coin type", tip: "Subject-bound: SPO routing.", subject: "<http://numismatics.org/pella/id/lerider.philip_ii.1.100>" },
        { label: "All coin types", tip: "Object-bound rdf:type: OSP.", predicate: "<http://www.w3.org/1999/02/22-rdf-syntax-ns#type>", object: "<http://nomisma.org/ontology#TypeSeriesItem>" }
      ]
    },
    "openalex-astrocytes": {
      predicate: "<http://purl.org/spar/cito/cites>",
      examples: [
        { label: "Every citation edge", tip: "Predicate-bound: cito:cites as one POS run.", predicate: "<http://purl.org/spar/cito/cites>" },
        { label: "Everything about a paper", tip: "Subject-bound: SPO routing.", subject: "<https://openalex.org/W2777525962>" },
        { label: "All works", tip: "Object-bound rdf:type: OSP.", predicate: "<http://www.w3.org/1999/02/22-rdf-syntax-ns#type>", object: "<http://ex/Work>" }
      ]
    },
    opencitations: {
      predicate: "<http://purl.org/spar/cito/cites>",
      examples: [
        { label: "Every citation edge", tip: "Predicate-bound: cito:cites as one POS run.", predicate: "<http://purl.org/spar/cito/cites>" },
        { label: "Everything about an article", tip: "Subject-bound: SPO routing.", subject: "<https://doi.org/10.1126/science.1225829>" },
        { label: "All journal articles", tip: "Object-bound rdf:type: OSP.", predicate: "<http://www.w3.org/1999/02/22-rdf-syntax-ns#type>", object: "<http://purl.org/spar/fabio/JournalArticle>" }
      ]
    },
    orkg: {
      predicate: "<https://orkg.org/property/hasAuthors>",
      examples: [
        { label: "Every paper-to-authors edge", tip: "Predicate-bound: orkgp:hasAuthors as one POS run.", predicate: "<https://orkg.org/property/hasAuthors>" },
        { label: "All papers", tip: "Object-bound rdf:type: OSP.", predicate: "<http://www.w3.org/1999/02/22-rdf-syntax-ns#type>", object: "<https://orkg.org/class/Paper>" }
      ]
    },
    "theographic-graph": {
      predicate: "<http://theographic/ontology#sibling>",
      examples: [
        { label: "Every sibling edge", tip: "Predicate-bound: theo:sibling as one POS run.", predicate: "<http://theographic/ontology#sibling>" },
        { label: "Everything about a person", tip: "Subject-bound: SPO routing.", subject: "<http://ex/person/absalom_59>" },
        { label: "All people (foaf:Person)", tip: "Object-bound rdf:type: OSP.", predicate: "<http://www.w3.org/1999/02/22-rdf-syntax-ns#type>", object: "<http://xmlns.com/foaf/0.1/Person>" }
      ]
    },
    "getty-ulan": {
      predicate: "<http://vocab.getty.edu/ontology#teacherOf>",
      examples: [
        { label: "Every teacher-pupil edge", tip: "Predicate-bound: the whole gvp:teacherOf lineage as one POS run, each row with its tile/byte range.", predicate: "<http://vocab.getty.edu/ontology#teacherOf>" },
        { label: "Everything about a master", tip: "Subject-bound: SPO routes to one artist's facts (name, dates, pupils).", subject: "<http://vocab.getty.edu/ulan/500004460>" }
      ]
    },
    "wikidata-100mb": {
      predicate: "<http://www.wikidata.org/prop/direct/P106>",
      examples: [
        { label: "Every occupation edge", tip: "Predicate-bound: the whole occupation relation as one POS run over the lazy file — each row shows the tile/byte range fetched.", predicate: "<http://www.wikidata.org/prop/direct/P106>" },
        { label: "All humans", tip: "Object-bound: routed to OSP — every \"instance of: human\" assertion.", predicate: "<http://www.wikidata.org/prop/direct/P31>", object: "<http://www.wikidata.org/entity/Q5>" }
      ]
    },
    "ohm-full": {
      predicate: "<http://ex/startYear>",
      examples: [
        { label: "Every feature start year", tip: "Predicate-bound: all ex:startYear values as one POS run over the lazy 150 MB file — each row with its tile/byte range.", predicate: "<http://ex/startYear>" },
        { label: "Every feature name", tip: "Predicate-bound: all rdfs:label literals; compare the byte ranges with the start-year run.", predicate: "<http://www.w3.org/2000/01/rdf-schema#label>" }
      ]
    },
    wikidata: {
      predicate: "<http://www.wikidata.org/prop/direct/P106>",
      examples: [
        { label: "Every occupation edge (lazy, 1 GB)", tip: "Predicate-bound over the 1 GB file: the whole occupation relation as one POS run — each row shows the tile/byte range fetched. Selective, so it stays cheap even at 1 GB.", predicate: "<http://www.wikidata.org/prop/direct/P106>" },
        { label: "All humans", tip: "Object-bound: routed to OSP — every \"instance of: human\" assertion over the lazy 1 GB graph.", predicate: "<http://www.wikidata.org/prop/direct/P31>", object: "<http://www.wikidata.org/entity/Q5>" }
      ]
    }
  },
  // Predefined IRI -> human-label hints, per dataset, for the editor's "Labels"
  // decode toggle: instant previews for the opaque-identifier datasets with no
  // graph access. Readable-IRI datasets (ex:Obesity, foaf:name, …) lean on the
  // built-in vocab + a live lookup instead, so they need no entries here.
  labelHints: {
    "wikidata": RETE_WD_LABELS,
    "wikidata-100mb": RETE_WD_LABELS,
    "getty-ulan": {
      "http://vocab.getty.edu/ulan/500011051": "Rembrandt",
      "http://vocab.getty.edu/ontology#teacherOf": "teacher of",
      "http://vocab.getty.edu/ontology#nationality": "nationality"
    },
    "factgrid-illuminati": {
      "https://database.factgrid.de/entity/Q10677": "Order of the Illuminati",
      "https://database.factgrid.de/prop/direct/P91": "member of",
      "https://database.factgrid.de/prop/direct/P2": "instance of"
    },
    "antarctic-expeditions": {
      "http://www.wikidata.org/entity/Q1162294": "Endurance (ship)",
      "http://www.wikidata.org/entity/Q973919": "Terra Nova Expedition",
      "http://ex/participant": "participant",
      "http://ex/vessel": "vessel",
      "http://ex/leader": "leader",
      "http://ex/startYear": "start year",
      "http://ex/endYear": "end year"
    },
    "mimotext": {
      "http://data.mimotext.uni-trier.de/prop/direct/P49": "stylometric similarity",
      "http://data.mimotext.uni-trier.de/prop/direct/P36": "about (theme)",
      "http://data.mimotext.uni-trier.de/prop/direct/P5": "author",
      "http://data.mimotext.uni-trier.de/prop/direct/P12": "genre",
      "http://data.mimotext.uni-trier.de/prop/direct/P9": "publication date",
      "http://data.mimotext.uni-trier.de/prop/direct/P2": "instance of",
      "http://data.mimotext.uni-trier.de/entity/Q2": "literary work",
      "http://data.mimotext.uni-trier.de/entity/Q10": "person",
      "http://data.mimotext.uni-trier.de/entity/Q20": "thematic concept",
      "http://data.mimotext.uni-trier.de/entity/Q26": "spatial concept"
    }
  }
};
