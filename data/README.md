# `data/` — rete dataset catalog (internal)

Everything under `data/` is **gitignored and regenerable** — the *source recipes*
(`scripts/`) are tracked, the built artifacts are not. This file is the exception
(`!/data/README.md` in `.gitignore`) and documents what each dataset is, its
technical shape, how it's built and served, and the kind of questions it answers.

Two consumers:

| Consumer | Page | How datasets reach it |
|---|---|---|
| **Historical Atlas** (`docs/atlas-app.html`) | a SPARQL+GIS map with a timeline | overlays fetched **remote-lazy** from R2, queried in-browser by the WASM engine, filtered to the scrub year |
| **Playground** (`docs/playground.html`) | a SPARQL playground | datasets **embedded** (base64 in the page) or **remote-lazy** from R2 |

## Pipeline

```
source (SPARQL CONSTRUCT | REST/WFS | dump | generator)
   └─ scripts/fetch_*.sh / *_to_nt.py / *_gen.py  ──▶  data/**/<key>.{nt,ttl}   (atlas GeoSPARQL shape or a plain graph)
        └─ rete build <file> -o <key>.rete         (run in Docker: rust:1.92-bookworm, target/release/rete is a Linux ELF)
             ├─ atlas overlay  ──▶  skills/rete-publish/scripts/upload_bucket.sh <file> <key>/<key>.rete
             │                       served at https://data.graphplaza.com/<key>/<key>.rete
             └─ playground      ──▶  embed: copy to web/<key>.rete + register in scripts/build_playground.py + catalog.js
                                      lazy:  upload_bucket.sh … <key>/<key>.rete  (>~2 MB)
```

- **Upload uses the tracked R2 uploader** in `skills/rete-publish/scripts/` and the
  gitignored `.env`. Public reads use direct, token-free R2 URLs.
- **Build is Dockerised** (`MSYS_NO_PATHCONV=1 docker run --rm -v D:/pro/rete:/work -w /work rust:1.92-bookworm bash -c 'target/release/rete build …'`).

### Atlas GeoSPARQL shape

Every atlas overlay is one of two temporal models. `geo = http://www.opengis.net/ont/geosparql#`, `ex = http://ex/`.

```turtle
# INSTANT — a point-in-time event
<x> a ex:TYPE ; rdfs:label "…"@en ; ex:year 1914 ;
    geo:hasGeometry <x/geom> .  <x/geom> geo:asWKT "Point(lon lat)"^^geo:wktLiteral .

# INTERVAL — something with a lifespan ([startYear,endYear]; 2100 = "still present")
<x> a ex:TYPE ; rdfs:label "…"@en ; ex:startYear 1270 ; ex:endYear 2100 ;
    geo:hasGeometry <x/geom> .  <x/geom> geo:asWKT "POLYGON((…))"^^geo:wktLiteral .
```

The atlas chooses the query per layer (`INTERVAL_THEMES` set in `web/atlas.template.html`):
instant layers render in a window around the playhead; interval layers render whenever
`startYear ≤ scrub ≤ endYear`. WKT may be `Point` (drawn as a dot), `Polygon`/`MultiPolygon`
(filled+stroked) or `LineString` (stroked).

---

## Atlas overlays — 84 layers

Served from `…/data/atlas/themes/<key>.rete`. Counts are approximate (cap = `LIMIT 6000` per fetch).

### Borders & base layers
| key | what | shape | source / license | script |
|---|---|---|---|---|
| `history` (embedded) | world territorial borders, 18 era snapshots 2000 BCE–2010 CE | instant polygons | aourednik/historical-basemaps, GPL-3.0 | `scripts/geo_to_rete.py basemaps` |
| `battles`,`sites`,`states` | battles / archaeological sites / historical-state capitals | instant | Wikidata CC0 | `scripts/fetch_wikidata_events.sh` |
| `pleiades` | 35k ancient places, `[startYear,endYear]` | interval | Pleiades, CC-BY | `scripts/geo_to_rete.py pleiades` |

### Wikidata themes — 67 (`scripts/fetch_wikidata_themes.sh`, CC0)
Two emit functions: `emit` (INSTANT events, `ex:year` from P585) and `emiti` (INTERVAL
structures/institutions/polities, `ex:startYear` from P571 inception, `ex:endYear` from
`COALESCE(P576 dissolved, P3999 closure, P582 end, 2100)`).
- **Events (instant):** military-operations, sieges, earthquakes, disasters, floods, meteorite-falls, volcanic-eruptions, nuclear-explosions, assassinations, treaties, epidemics, shipwrecks, massacres, explosions, aviation-accidents, rail-accidents, tsunamis, wildfires, coups, revolutions, terrorist-attacks, expeditions, landslides.
- **Structures/institutions/polities (interval):** castles, fortifications, lighthouses, cathedrals, monasteries, abbeys, palaces, forts, universities, world-heritage-sites, pyramids, polities, bridges, dams, museums, mines, stadiums, libraries, prisons, mosques, synagogues, temples, towers, theatres, observatories, railway-stations, amphitheatres, aqueducts, canals, windmills, botanical-gardens, megaliths, churches, hospitals, cemeteries, gardens, city-gates, power-stations, factories, breweries, shipyards, airports, monuments, memorials.
- `THEMES="castles polities" scripts/fetch_wikidata_themes.sh` fetches a subset.

### Other providers
| key(s) | what | shape | source / license | script |
|---|---|---|---|---|
| `dbpedia-conflicts`, `dbpedia-power` | conflicts (instant) / power plants (interval) | mixed | DBpedia, CC-BY-SA | `scripts/fetch_dbpedia_themes.sh` |
| `ohm` | OpenHistoricalMap features, dated by `start_date`/`end_date`; **real geometry** | interval (Point/Line/Polygon) | OSM/OHM, CC0 | `scripts/fetch_ohm.sh` + `ohm_overpass_to_nt.py` |
| `nomisma` | ~10k ancient coin types at their mints | interval | Nomisma.org, CC-BY | `scripts/fetch_atlas_extra.sh` |
| `factgrid` | ~4.7k geolocated dated places & events | interval | FactGrid, CC0 | `scripts/fetch_atlas_extra.sh` |
| `getty-tgn` | ~4.5k historical places (name-usage spans) | interval | Getty TGN, ODC-By | `scripts/fetch_atlas_extra.sh` (POST-from-file; endpoint flaky) |
| `theographic-bible` | 355 biblical events located + dated | instant | Theographic, CC-BY-SA | `scripts/fetch_dumps_extra.sh` + `theographic_to_nt.py` |
| `samian-ware` | ~3k Roman terra-sigillata potters at production centres | interval | RGZM Samian LOD, DPPL | `scripts/fetch_dumps_extra.sh` + `samian_to_nt.py` |

### Antarctica — 5 layers (`scripts/fetch_antarctica.sh`)
| key | what | n | shape | source / license |
|---|---|---|---|---|
| `antarctic-claims` | 7 territorial-claim longitude sectors + Peter I Island + Marie Byrd gap | 9 | interval Polygon/MultiPolygon | synthetic facts (`antarctic_claims.py`) |
| `antarctic-stations` | research stations, founded→present | 73 | interval Point | Wikidata CC0 |
| `antarctic-deaths` | people who died in Antarctica (Scott's party 1912…) | 23 | instant Point | Wikidata CC0 |
| `antarctic-sites` | Historic Sites & Monuments — huts, crosses, graves (Amundsen's *Polheim*, Mawson's Cape Denison) | 16 | interval Point | Wikidata CC0 |
| `antarctic-places` | SCAR Composite Gazetteer — named features, static `[1820,2100]` basemap | 20,159 | interval Point | SCAR CGA, CC-BY (`scar_to_nt.py`, WFS CSV) |

### Example questions (GeoSPARQL, atlas)

```sparql
# Who ruled Paris in 1914? (point-in-polygon containment + temporal filter)
PREFIX geo: <http://www.opengis.net/ont/geosparql#>
PREFIX geof: <http://www.opengis.net/def/function/geosparql/>
PREFIX ex: <http://ex/>  PREFIX rdfs: <http://www.w3.org/2000/01/rdf-schema#>
SELECT ?territory WHERE {
  ?t ex:year 1914 ; rdfs:label ?territory ; geo:hasGeometry/geo:asWKT ?w .
  FILTER(geof:sfContains(?w, "POINT(2.35 48.85)"^^geo:wktLiteral)) }

# Antarctic research stations active in 1960 (interval overlap)
PREFIX ex: <http://ex/>  PREFIX rdfs: <http://www.w3.org/2000/01/rdf-schema#>
SELECT ?label WHERE { ?s a ex:Station ; rdfs:label ?label ; ex:startYear ?a ; ex:endYear ?b .
  FILTER(?a <= 1960 && ?b >= 1960) }

# Ancient coin types minted before 300 BCE (deep-BCE interval)
PREFIX ex: <http://ex/>  PREFIX rdfs: <http://www.w3.org/2000/01/rdf-schema#>
SELECT ?label ?start WHERE { ?c a ex:CoinType ; rdfs:label ?label ; ex:startYear ?start .
  FILTER(?start < -300) } ORDER BY ?start
```

The atlas itself runs these per-layer (`SELECT ?label ?year ?wkt …` for instant,
`SELECT ?label ?s ?e ?wkt …` for interval) and draws the result; the timeline scrub
supplies the year.

---

## Playground datasets — 19

Registered in `web/playground-src/catalog.js` (full example queries live there, per
dataset, under `examples`). Embedded unless noted. The first 7 are the originals.

| key | what | n (triples) | source / license | mode |
|---|---|---|---|---|
| `scholar`, `scholar-noisy` | synthetic scholarly world (+25% noise variant) | ~5k | synthetic (`synth_graph.py`) | embed |
| `typed`, `deps` | tiny typed / dependency graphs | <1k | synthetic | embed |
| `citations` | OpenCitations AlphaFold sample + synthetic metadata | ~539k | OpenCitations CC0 | embed |
| `history` | world borders (GeoSPARQL, 7 eras) | — | GPL-3.0 | embed |
| `wikidata` | real Wikidata 1 GB truthy slice | 120M | Wikidata CC0 | remote-lazy |
| `linked-jazz` | jazz musician social network | 9.5k | Linked Jazz, CC BY-SA | embed |
| `nomisma` | coinage of Alexander the Great (PELLA) | 53k | Nomisma, CC-BY | embed |
| `mimotext` | French Enlightenment novels + stylometry | 25k | MiMoText, CC0 | embed |
| `mmm` | medieval manuscript provenance (CIDOC-CRM) | 48k | MMM, CC BY-NC | embed |
| `openalex-astrocytes` | 500 top-cited astrocyte papers as a citation core | 24k | OpenAlex, CC0 | embed |
| `antarctic-expeditions` | Heroic-Age expeditions ↔ crew ↔ ships | 275 | Wikidata, CC0 | embed |
| `factgrid-illuminati` | Order of the Illuminati prosopography | 35k | FactGrid, CC0 | embed |
| `theographic-graph` | biblical genealogy/narrative graph | 32k | Theographic, CC BY-SA | embed |
| `monarch` | disease/gene/phenotype neighbourhood (biolink) | 8k | Monarch, CC-BY | embed |
| `opencitations` | a citation neighborhood (cito:cites + DC/FOAF) | 8k | OpenCitations, CC0 | embed |
| `orkg` | research papers → contributions | 37k | ORKG, CC-BY | embed |
| `getty-ulan` | artist teacher→pupil lineage | 376k | Getty ULAN, ODC-By | remote-lazy |

Reproduce: `scripts/fetch_playground_kgs.sh` (linked-jazz/nomisma/mimotext/mmm/getty-ulan),
`scripts/fetch_openalex_astrocytes.py`, and the `antarctic-expeditions` CONSTRUCT.
factgrid-illuminati/theographic-graph/monarch/opencitations/orkg were fetched via the
`ingest-deferred-and-academic` research workflow (recipes there).

### Example questions (playground)

```sparql
# Linked Jazz — who Count Basie reaches by word of mouth (transitive social ties)
PREFIX rel: <http://purl.org/vocab/relationship/>  PREFIX foaf: <http://xmlns.com/foaf/0.1/>
SELECT DISTINCT ?name WHERE { <http://dbpedia.org/resource/Count_Basie> rel:knowsOf+ ?r . ?r foaf:name ?name }

# OpenAlex astrocytes — the most-cited papers in the field
PREFIX dct: <http://purl.org/dc/terms/>  PREFIX ex: <http://ex/>
SELECT ?title ?c WHERE { ?w a ex:Work ; dct:title ?title ; ex:citationCount ?c } ORDER BY DESC(?c) LIMIT 15

# Theographic — descendants of Abraham (transitive genealogy)
PREFIX tg: <http://theographic/ontology#>  PREFIX rdfs: <http://www.w3.org/2000/01/rdf-schema#>
SELECT DISTINCT ?name WHERE { <http://ex/person/abraham_58> tg:child+ ?d . ?d rdfs:label ?name }

# Antarctic expeditions — who sailed on each ship
PREFIX ex: <http://ex/>  PREFIX rdfs: <http://www.w3.org/2000/01/rdf-schema#>
SELECT ?ship ?person WHERE { ?e ex:vessel ?v ; ex:participant ?p . ?v rdfs:label ?ship . ?p rdfs:label ?person }
```

Each dataset ships 3–7 such examples in `catalog.js` (families: Summary, Select, Path,
Aggregate, Construct), surfaced in the playground's example picker.

---

## Full-graph dataset — Mapping Manuscript Migrations (MMM)

The **complete** MMM knowledge graph — not the tiny `mmm` playground sample above
(which CONSTRUCTs a 4-place slice). MMM unified three manuscript-provenance databases
into one CIDOC-CRM + FRBRoo graph: [SDBM](https://sdbm.library.upenn.edu/) (Schoenberg
Database of Manuscripts, U. Penn), [Bibale](http://bibale.irht.cnrs.fr/) (IRHT-CNRS),
and [Medieval Manuscripts in Oxford Libraries](https://medieval.bodleian.ox.ac.uk/)
(Bodleian). Source: Zenodo [DOI 10.5281/zenodo.4019643](https://doi.org/10.5281/zenodo.4019643)
(v2.1.0, 2020-09-08), **CC BY-NC 4.0**. One command builds everything:
`scripts/fetch_mmm_full.sh` (steps: `fetch` | `build` | `export` | `tables`).

### Pipeline & artifacts (all under `data/mmm/`, gitignored & regenerable)
```
Zenodo mmm_data_v2.1.0.zip (66.5 MB, md5 2f97635f…)
  └─ unzip → mmm_{sdbm,bibale,bodley,places}.ttl + mmm-schema.ttl       (~1.3 GB; sdbm.ttl alone is 1.19 GB)
       └─ rete build … --no-pyramid --card   →  mmm-full.rete           (77 MB · 23,349,356 triples · 5.15M terms)
            ├─ rete export                    →  mmm-full.nt             (3.1 GB · lossless N-Triples)
            │    └─ scripts/mmm_to_tables.py  →  tables/*.parquet        (43 per-rdf:type tables + _untyped + _manifest · 164 MB)
            │                                    mmm-tables.duckdb       (44 entity tables, one per class)
            └─ (RDF/XML cidoc-crm.rdf / frbroo.rdf ontologies are TBox-only — not ingested)
```
- Build runs the Linux `rete` ELF in Docker (`rust:1.92-bookworm`): ~10 GB RAM, ~80 s.
- Export writes to container-local tmpfs then copies out once — Rust's line-buffered
  stdout over the Windows bind mount is otherwise ~30× slower (23.3M flushes).

### Graph shape — top classes (`rete sparql mmm-full.rete "SELECT ?class (COUNT(?s)…"`, 2.1 s)
| class | n | what |
|---|--:|---|
| `mmms:ManuscriptActivity` | 668,393 | provenance activity events |
| `frbroo:F1_Work` · `F2_Expression` · `F28_Expression_Creation` | 435,045 · 451,824 · 451,041 | the texts |
| `ecrm:E12_Production` | 225,520 | manuscript production events |
| **`frbroo:F4_Manifestation_Singleton`** | **221,721** | **the manuscripts** — SDBM 195,842 · Bibale 16,351 · Bodley 13,347 |
| `ecrm:E97_Monetary_Amount` | 54,264 | sale prices |
| `ecrm:E10_Transfer_of_Custody` | 30,603 | ownership transfers |
| `ecrm:E21_Person` · `E74_Group` | 27,152 · 6,069 | actors (cross-linked by `owl:sameAs` to VIAF/BnF and across sources) |
| `ecrm:E78_Collection` | 5,953 | collections |
| `ecrm:E53_Place` | 5,050 | geocoded places (`wgs84:lat`/`long`) |

166,450 manuscripts carry ≥1 former/current owner (`ecrm:P51`). Namespaces: `ecrm =
http://erlangen-crm.org/current/`, `frbroo = http://erlangen-crm.org/efrbroo/`,
`mmms = http://ldf.fi/schema/mmm/`.

### Related tables — `scripts/mmm_to_tables.py` (lossless)
The MMM-shaped sibling of `rdf_to_entity_tables.py`: reads the graph's N-Triples export
and emits **one Parquet table per `rdf:type`** — the class's top-24 properties as named
`LIST` columns (manuscripts get `source`, `P51_has_former_or_current_owner`,
`manuscript_work`, `manuscript_author`, `folios`/`height`/`width`, shelfmarks, …), all
`rdf:type`s in a `types` list, *every other* predicate in an `extra` MAP, and a
`skos:prefLabel` `label` column. Objects are stored as N-Triples tokens, so the set is
**lossless**: explode every column across all tables and you recover exactly the
23,349,356 triples (`--verify` confirms it; `_manifest.parquet` maps column→predicate).
`_untyped.parquet` holds the 11,371 no-`rdf:type` subjects (mostly schema TBox). The
DuckDB file (`--duckdb`) materializes one native table per class; `--sqlite` is also
available (LIST/MAP columns as JSON text).

### Example questions
```sparql
# (graph) Where were manuscripts produced? — production → place → coordinates
PREFIX ecrm: <http://erlangen-crm.org/current/>  PREFIX skos: <http://www.w3.org/2004/02/skos/core#>
PREFIX wgs: <http://www.w3.org/2003/01/geo/wgs84_pos#>
SELECT ?placeL ?lat ?long (COUNT(?ms) AS ?n) WHERE {
  ?prod a ecrm:E12_Production ; ecrm:P108_has_produced ?ms ; ecrm:P7_took_place_at ?pl .
  ?pl skos:prefLabel ?placeL ; wgs:lat ?lat ; wgs:long ?long .
} GROUP BY ?placeL ?lat ?long ORDER BY DESC(?n) LIMIT 12
# → Italy 16247, England 14654, France 11392, Germany 4791, Paris 2946, Florence, Venice…
```
```sql
-- (tables) Manuscripts and their former/current owners, from mmm-tables.duckdb
SELECT m.label AS manuscript, o.label AS owner
FROM F4_Manifestation_Singleton m, UNNEST(m.P51_has_former_or_current_owner) AS t(owner_iri)
JOIN E21_Person o ON o.entity = t.owner_iri
WHERE m.label IS NOT NULL AND o.label IS NOT NULL LIMIT 20;
-- → SDBM_MS_1731 ← "Phillipps, Thomas, Sir, 1792-1872"; … ← "Ashmole, Elias, 1617-1692"; …
```

> **Publishing (not done here — outward-facing).** The 77 MB `.rete` is remote-lazy tier.
> To serve it in the playground/atlas, upload it to R2
> (`skills/rete-publish/scripts/upload_bucket.sh data/mmm/mmm-full.rete mmm-full/mmm-full.rete`)
> and register it in `web/playground-src/catalog.js` as a remote-lazy dataset.

---

## Verification

- Atlas: `dev/geo/verify_atlas.mjs` (+ `run_verify_atlas.sh`) — boots the page, asserts the
  overlay count, loads a themed + an OHM layer from the bucket, checks for console errors.
- Playground: `dev/geo/verify_playground.mjs` (+ `run_verify_playground.sh`) — asserts the
  catalog dataset count and that every embedded dataset loads and answers a query.
- Both run headless in the `mcr.microsoft.com/playwright` Docker image. **When adding a
  dataset, bump the count assertion and add the key to the run loop.**

---

# Raw acquisitions — downloaded, `.rete` not yet built

Datasets acquired into `data/<name>/raw/` with committed `scripts/`, a
per-dataset `README.md`, `SHA256SUMS.txt` and a `PROFILE.txt`. These are
**reproducible from git alone** — `data/` is gitignored, so the scripts *are* the
dataset as far as the repo is concerned. Each folder's own README carries the
full provenance, licence, schema and gotchas; this is the index.

| dataset | size | headline | licence |
|---|---:|---|---|
| [`ikea/`](ikea/README.md) | ~52 GB | 157,105 products × 5 locales, **27,765 real `.glb` 3D models**, 4 research datasets | mixed; 3D Assembly CC BY-NC-SA |
| [`lego/`](lego/README.md) | ~23 GB | Rebrickable 1.87M rows, 133,461 images, **~29,500 LDraw 3D parts** | LDraw **CC BY 4.0**; Rebrickable **unverified** |
| [`openfoodfacts/`](openfoodfacts/README.md) | 20.1 GB | **4,639,959 products**, 29,173 SKOS concepts, **GS1 Web Vocabulary** (2,532-term official barcode ontology) | **ODbL** (share-alike) |
| [`openlibrary/`](openlibrary/README.md) | 17.5 GB | **56,442,419 editions**, VIAF/Wikidata/ISNI author links | **CC0** |
| [`geospatial-geneve/`](geospatial-geneve/README.md) | ~11 GB | **239,177-tree** cantonal inventory as GeoJSON, 584 SITG layers, 50 LiDAR tiles | opendata.swiss **Open use** |
| [`music/`](music/README.md) | 12 GB | MusicBrainz CC0 export + **176,581 Lakh MIDI** (71% with drum tracks) + scores + 444 h drumming | mixed; MAESTRO/DCML are **NC** |

## The pattern that makes these federatable

Every one of them turned out to hang off a **global join key**, which is what
makes the collection a graph rather than a pile:

| dataset | join key |
|---|---|
| ikea | `globalId` — same article, different item number per market |
| lego | `elements.design_id` — bridge to the LEGO Group's own part numbering |
| openfoodfacts | **GTIN / barcode** — and GS1 publishes the matching ontology |
| openlibrary | **ISBN**, plus VIAF / Wikidata / ISNI on authors |
| music | MBID, and MSD ids bridging metadata ↔ actual notes |
| scholar hub | DOI / ORCID / ROR (see the published datasets above) |

## Discipline coverage — what this catalogue is and isn't

Strong: **scholarly infrastructure** (Crossref, ORCID, OpenAIRE, DataCite, DBLP,
OpenCitations, ROR, CORDIS), **bibliographic** (databnf, bne, BCUL, USTC,
Biblissima, Open Library), **life sciences** (188 OBO ontologies, ChEBI, HPO,
Uberon, GBIF, five neuro/connectome sets), **cultural heritage** (a dozen
Spanish/Catalan/Swiss/Irish archives, manuscripts, IIIF), **software** (deps.dev,
GH Archive 1.64B, HuggingFace), **geo/3D**.

Known gaps, roughly in order of value:

1. ~~Music~~ — **filled** by `music/`.
2. **Language & linguistics** — no WordNet, DBnary (Wiktionary as RDF),
   Glottolog or Universal Dependencies. Odd, given the collection is about text.
3. **Art & museums** — only Lombardi + Smithsonian 3D + WikiArt. Missing the
   **Getty vocabularies** (AAT/ULAN/TGN), which are native LOD and *the* art
   ontology; also Europeana and the Met's CC0 release.
4. **Classics / ancient world** — Pleiades, Perseus, Nomisma (numismatics LOD),
   Papyri.info.
5. **Astronomy** — nothing. SIMBAD, Gaia.
6. **Mathematics** — nothing. OEIS is small and very graph-shaped.
7. **Film / audiovisual** — subtitles only.
8. **Economics & corporate** — GLEIF (legal-entity ownership graph), Eurostat /
   World Bank SDMX, PatentsView.
9. **Law beyond Spain** — EUR-Lex / CELLAR has a real SPARQL endpoint.
10. **Historical audio** — see below.

### The shape gap, not just the topic gap

Nearly everything here is a **catalogue**: records *about* things. What is scarce
is **notation** — data where the content itself is the structure. `music/` is the
first real entry (scores, MIDI, harmonic analyses); lexicons and mathematical
sequences would be the next. That is a materially different thing to model, and a
better stress test for the format than another catalogue.

### Historical audio — surveyed 2026-07-27, not yet downloaded

Verified item counts via the Internet Archive `advancedsearch` + `scrape` APIs
(the scrape endpoint supports cursor pagination for bulk metadata):

| IA collection | items | what |
|---|---:|---|
| `audio_music` | 520,539 | all music audio |
| `78rpm` | **310,709** | digitised 78 rpm discs |
| `etree` | **301,997** | Live Music Archive — taper-sanctioned concert recordings |
| `georgeblood` | **186,858** | the **Great 78 Project** digitisations |
| `oldtimeradio` | 8,853 | old-time radio broadcasts |

Also worth having: **UCSB Cylinder Audio Archive** (wax cylinders, site live) and
the **LoC National Jukebox** (its JSON API returned 403 to a plain client and
needs proper headers).

**Discogs** monthly dumps (CC0, the authority on physical pressings/labels) could
not be reached: `discogs-data-dumps.s3.us-west-2.amazonaws.com` returns
`AccessDenied` for both bucket listing and direct object URLs from here. Needs
investigation before being promised.

Note the split: IA **metadata** is bulk-harvestable and largely open; the
**audio** is per-item licensed (US-public-domain 78s, artist-sanctioned etree
recordings), so treat rights per collection, not per archive.
