# `data/` — rete dataset catalog (internal)

Everything under `data/` is **gitignored and regenerable** — the *source recipes*
(`scripts/`) are tracked, the built artifacts are not. This file is the exception
(`!/data/README.md` in `.gitignore`) and documents what each dataset is, its
technical shape, how it's built and served, and the kind of questions it answers.

Two consumers:

| Consumer | Page | How datasets reach it |
|---|---|---|
| **Historical Atlas** (`docs/atlas-app.html`) | a SPARQL+GIS map with a timeline | overlays fetched **remote-lazy** from the HF bucket, queried in-browser by the WASM engine, filtered to the scrub year |
| **Playground** (`docs/playground.html`) | a SPARQL playground | datasets **embedded** (base64 in the page) or **remote-lazy** from the bucket |

## Pipeline

```
source (SPARQL CONSTRUCT | REST/WFS | dump | generator)
   └─ scripts/fetch_*.sh / *_to_nt.py / *_gen.py  ──▶  data/**/<key>.{nt,ttl}   (atlas GeoSPARQL shape or a plain graph)
        └─ rete build <file> -o <key>.rete         (run in Docker: rust:1.92-bookworm, target/release/rete is a Linux ELF)
             ├─ atlas overlay  ──▶  hf buckets cp … hf://buckets/katospiegel/knowledge-graphs/atlas/themes/<key>.rete
             │                       served at https://katospiegel-rete.hf.space/data/atlas/themes/<key>.rete?token=<demo>
             └─ playground      ──▶  embed: copy to web/<key>.rete + register in scripts/build_playground.py + catalog.js
                                      lazy:  hf buckets cp … /playground/<key>.rete  (>~2 MB)
```

- **Upload is `hf buckets cp`** to the HF *bucket* (`katospiegel/knowledge-graphs`), **not** `hf upload` to the Space repo — the Space serves the bucket mounted at `/data`. The Space requires the demo token (`?token=sfdbgf1094by21hd128ru39802`; 401 without).
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

## Verification

- Atlas: `dev/geo/verify_atlas.mjs` (+ `run_verify_atlas.sh`) — boots the page, asserts the
  overlay count, loads a themed + an OHM layer from the bucket, checks for console errors.
- Playground: `dev/geo/verify_playground.mjs` (+ `run_verify_playground.sh`) — asserts the
  catalog dataset count and that every embedded dataset loads and answers a query.
- Both run headless in the `mcr.microsoft.com/playwright` Docker image. **When adding a
  dataset, bump the count assertion and add the key to the run loop.**
