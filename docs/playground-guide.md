# The playground

**[▶ Launch the playground](playground.html)** — one static HTML page that runs
the full rete engine in WebAssembly. No server, no account, no install: it
works offline, even opened from `file://`. Everything below happens in your
browser.

It is the fastest way to understand what rete *is*: pick one of the **40+
published datasets** — library catalogues, citation networks, ontologies,
historical maps, 3D museum collections — and query it live. The small ones are
embedded in the page; the big ones (up to a **117 M-triple library graph** and
a **1 GB Wikidata slice**) stay on their URL and are **range-read lazily**: a
query fetches only the byte ranges it touches, and a counter shows exactly how
few bytes crossed the wire.

## Pick a dataset

The dataset picker groups the catalog by theme (heritage, science, reference,
demos). Each entry shows its size, license, and a description; picking one
loads its **example queries** as one-click chips, each with a short tip
explaining what the query shows. A 🔗 on every chip copies a **deep link** —
`playground.html#dataset=<key>&ex=<n>` — that reopens the playground on that
dataset with that example loaded, ready to share.

A dataset can also be **sharded**: one logical graph served as several files
(a "⛓ N shards" chip appears). Every query fans out across the shards and the
rows merge — you just query it as one dataset.

Not in the catalog? **+ Add source → Connect (lazy)** opens any `.rete` URL
whose host serves ranges + CORS (see [Hosting your .rete](hosting.md)), and the
**Build** mode (below) turns your own RDF into a queryable file without leaving
the page.

## Write a query

The SPARQL editor comes with syntax highlighting and context autocomplete. When
you don't know the graph's vocabulary yet:

- **🔎 Find a term** browses the dataset's classes and predicates (instant,
  from the self-describing card) and searches **entities by label** — lazily
  over HTTP ranges on a remote graph. Click a result to drop its IRI at the
  cursor; each predicate offers a **values ›** drill listing the distinct
  objects it takes.
- **Labels chips** decode the IRIs in your query to human labels inline, so
  `wd:Q937` reads as *Albert Einstein* while you type.
- **✨ SPARQL AI** drafts a query from a plain-language request using a small
  language model that runs **locally on your GPU** (WebGPU) — nothing is sent
  to any API. It is grounded in the dataset's example queries, so its drafts
  use the right vocabulary.

Results stream into the table with media-aware cells: images become
thumbnails, IIIF manifests page through scans, `.glb` models rotate inline,
WKT geometries plot on mini-maps — see
[Media & SQL companions](media-companions.md) for the full matrix.

## Output views

The **Output** menu renders one result several ways. Each view expects a
particular query shape — write your `SELECT`/`CONSTRUCT` so the columns it
needs are present.

| Output | Needs | Query shape |
| --- | --- | --- |
| **Table** | anything | any `SELECT`, `ASK`, or `CONSTRUCT` |
| **Cards** | anything | the same rows as Table, but **one card per row** with the fields stacked — media renders full-width, so it reads well on a phone; on a wide screen the cards flow as a **masonry**. The **⚙ Fields** button picks how each field renders (the same types as a table column's header dropdown). |
| **Graph** | edges to draw | a `CONSTRUCT { ?a ?p ?b } …`, or a `SELECT` with **≥ 2 variables** — 2 vars are read as `v1 → v2` ("related") edges; **3 vars** are read as (subject, predicate, object). A 1-variable `SELECT` has nothing to connect. |
| **Map** | a WKT geometry column | a `SELECT` that binds a variable to a `geo:wktLiteral` (e.g. `?w` via `geo:hasGeometry/geo:asWKT ?w`). `POINT` / `LINESTRING` / `POLYGON` / `MULTI*` all plot; the first **non-geometry** column becomes each feature's hover label. |
| **Time** | a year / date column | a `SELECT` that binds a variable to `xsd:gYear`, `xsd:date` / `xsd:dateTime`, or a plain **year integer** (e.g. `ex:year ?y`). The other selected column(s) become the items listed in each cell's tooltip. |
| **TTL / JSON-LD** | triples | a `CONSTRUCT` query — these serialise the constructed graph. A `SELECT` has no triples to serialise. |

Rules of thumb:

- **Map, Time and Graph render the bindings of a `SELECT`**, so put the geometry /
  year / edge columns in your `SELECT` list. (`CONSTRUCT` also feeds Graph / TTL /
  JSON-LD directly.)
- **Map** and **Time** are available on **every** query (no per-dataset gating):
  each detects its column in the actual result and renders, or shows a short note
  if it's absent — so geometry or dates a query surfaces unexpectedly (e.g. from a
  federated join) still plot.
- Run these views under the **Whole index** (or **Split by community**) strategy.
  **Progressive** answers only from the pyramid summary (counts and community
  structure), so it has no per-row geometry or dates to plot.
- **Time** buckets years automatically to fit the span (per-year for short ranges,
  up to per-1000-year for very long ones); negative years are read as **BCE**. A
  cell's colour encodes the **number of items** in that bucket — hover for the list.
- The map is an **offline equirectangular plot** of the WKT coordinates (no tiles /
  network), auto-fit to the bounding box of the returned geometries. A dataset
  that ships a PMTiles basemap adds a true tiled **Output → Tiles** view.

```sparql
# MAP — labelled territories as polygons (history dataset)
PREFIX geo:  <http://www.opengis.net/ont/geosparql#>
PREFIX ex:   <http://ex/>
PREFIX rdfs: <http://www.w3.org/2000/01/rdf-schema#>
SELECT ?territory ?w WHERE {
  ?t ex:year 1914 ; rdfs:label ?territory ;
     geo:hasGeometry/geo:asWKT ?w .
}

# TIME — how many territories exist per year, as a multi-year heatmap
PREFIX ex:   <http://ex/>
PREFIX rdfs: <http://www.w3.org/2000/01/rdf-schema#>
SELECT ?year ?territory WHERE { ?t ex:year ?year ; rdfs:label ?territory }

# GRAPH — a 3-variable SELECT read as (subject, predicate, object)
PREFIX cito: <http://purl.org/spar/cito/>
SELECT ?a ?p ?b WHERE { ?a ?p ?b . FILTER(?p = cito:cites) } LIMIT 50

# TTL / JSON-LD — a CONSTRUCT produces serialisable triples
PREFIX cito: <http://purl.org/spar/cito/>
CONSTRUCT { ?a cito:cites ?b } WHERE { ?a cito:cites ?b } LIMIT 50
```

## Beyond SPARQL: the other modes

The mode strip turns the same open file around several ways:

- **Schema** — the dataset's effective schema (classes and how they relate),
  read index-free from the schema pyramid; the entry point for
  [semantic zoom](semantic-zoom.md).
- **Explore** — browse per-class entity tables, and (for datasets with
  companions) a **SQL** sub-tab that runs DuckDB-WASM / SQLite-WASM over the
  same triples as Parquet — the columnar engines and the graph engine, side by
  side, both lazy over HTTP.
- **Semantic** — natural-language search over the dataset's **meaning**: a
  sentence-embedding model (running locally, multilingual) embeds your phrase
  and ranks entities by similarity — no keywords needed. **↗ Query these in
  SPARQL** turns the hits into a `VALUES` block; **✨ Answer with AI** drafts a
  grounded answer from them. Available on datasets that ship an embedding
  index.
- **SHACL** — validate the graph against shapes in the editor; on a remote
  dataset only each shape's targets are range-read.
- **Reach** — transitive reachability from seed entities along one predicate.
- **Provenance** — "why is this triple in the result?": the physical
  dictionary/index/tile ranges behind a match.
- **Coherence** — the tiered OWL coherence checks, from an index-free schema
  scan to full materialization (see [Reasoning](reasoning.md)).
- **Build** — paste or upload RDF (N-Triples / N-Quads / Turtle), build a real
  `.rete` **in the browser**, query it immediately, save it to the browser for
  next time, or download the file. The full publish path (ontology, card,
  examples) is in [Media & SQL companions](media-companions.md) and
  [Hosting your .rete](hosting.md).

## Federation: query several sources as one

**+ Add source** adds a second (third, …) `.rete` — from the catalog or any
URL — and the query runs across all of them: union merge with predicate
routing, and for flat BGPs a real **cross-source join** (a pattern's rows from
one file joined against another's on the shared variables). See
[Federated queries](federation.md#in-the-playground).

A source can also be a **SPARQL endpoint**. With the 🔌 **live** checkbox the
endpoint becomes the query target and **SPARQL Update is enabled** — pointed at
a running `rete serve`, the playground becomes the *editing UI* over a live
graph: `INSERT DATA`, watch the next SELECT reflect it, download the snapshot.
(Deep-linkable as `#endpoint=<url>`.)

## Watching the bytes (and the caches)

For a remote dataset the result line reports what the query physically did —
`N range requests · M KB fetched · file is X MB` — and **⊞ requests** opens the
actual byte-range log. Re-running a query reports *"served from cache, 0 new
bytes"*: reads are cached per session, and **Settings** adds opt-in extras:

- **Persist fetched ranges across reloads** — mirrors fetched blocks into
  IndexedDB (per-file usage bars + Clear), so tomorrow's session starts warm.
- **Parallel range reads** — a cross-origin-isolated worker pool fetches
  ranges concurrently (reloads once to enable isolation; disables the
  CDN-loaded DuckDB/SQLite backends while on).
- **Concurrent reads (Asyncify)** — an alternative engine build that overlaps
  remote reads without isolation.

## Under the hood

The page is generated by `scripts/build_playground.py` from
`web/playground.template.html` + `web/playground-src/`: the WASM engine, its
JS glue, and the embedded datasets are inlined into one self-contained file.
The JS API it drives is documented in [WASM & JavaScript API](browser.md); how
datasets get registered is `web/playground-src/catalog.js` (see the repo's
`skills/rete-publish` for the full recipe).
