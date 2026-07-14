# plaza — an imaging gallery for self-describing `.rete` datasets (side experiment)

> **Experimental, static, read-only.** A plain HTML/JS site that takes a *list*
> of `.rete` files and turns each into a gallery tile: a generated fingerprint
> image, the dataset's own card, download links for the files (the `.rete` plus
> any Parquet/DuckDB/SQLite companions), and a live autocomplete + SPARQL panel.
> It only **reads** files (over HTTP range or from
> disk) — the format, the CLI and the WASM build are untouched.

## The idea

A `.rete` file is already **self-describing**: its 128-byte header points at every
section, and an optional **[Dataset Card](../../docs/dataset-cards.md)** — compact
JSON in the metadata section — records who made it, the license, the counts, the
predicates/classes actually used, detected signals (geo, temporal, …) and an
auto-generated starter-query library. `rete card-url` already reads all of that
over HTTP in *two small range requests*, never touching the index.

The plaza is the browser-native version of that: a gallery where every dataset
introduces itself.

- **Read the card live.** For each file in the manifest the page fetches bytes
  `0–127` (the header), reads `metadata_offset`/`metadata_len`, then fetches just
  the card bytes and `JSON.parse`s them. Two range requests, no WASM, no download.
  (`js/rete-card.js` — a faithful port of `crates/rete-core/src/header.rs`.)
- **A generated fingerprint per dataset.** A deterministic node-link
  *constellation* seeded by the file's blake3 content hash and shaped by its
  profile — node count ∝ log(triples), palette ← vocabulary mix, plus motifs for
  geo / temporal / incoherent schemas. Same file → same image, every time.
  (`js/procgen.js`, pure SVG.)
- **Browse, search, filter.** A search bar matches name, tags, vocabularies and
  license; tag chips narrow the grid. (`index.html` + `js/catalog.js`.)
- **Explore for real.** The detail page opens the file in the in-browser engine
  (the same WASM the playground uses) and gives you an **autocomplete** search box
  (the label prefix-index) and a **SPARQL** box pre-loaded with the card's own
  starter queries. Bundled files load fully; remote files are queried lazily over
  HTTP range. (`dataset.html` + `js/dataset.js` + `js/plaza-worker.js`.)
- **Companion tables.** When a dataset has Parquet / DuckDB / SQLite mirrors, the
  manifest links them so you can pull the same graph into DuckDB or pandas.

## Two layers of metadata (answering "is the card enough?")

Mostly the card *is* enough — and it travels inside the file, so it can never go
stale. The manifest (`plaza.json`) carries only what a card structurally can't:

| Lives in the **card** (inside the `.rete`) | Lives in the **manifest** (`plaza.json`) |
|---|---|
| title, description, license, source, created | which files exist + their `kind` (bundled / remote-lazy) |
| triple/term counts, predicates, classes, vocabularies | visual identity: `icon`, `tags`, a curated `blurb` |
| signals (geo, temporal, numeric, links) | companion table downloads (Parquet / DuckDB / SQLite) |
| coherence verdict, starter-query library | outbound links; `typePredicate` for non-`rdf:type` graphs |

So the manifest stays tiny and the heavy, derived facts ride inside each file.

## Run it

The site references the WASM build at `../../web/pkg-nomodules` and the bundled `.rete`
files at `../../web/*.rete`, so **serve from the repo root** (not the experiment
folder):

```sh
python -m http.server 8000          # from D:\pro\rete
# then open:
#   http://localhost:8000/experiments/plaza/
```

Remote datasets (Wikidata, ChEBI, Chemotion, …) are read straight from
Cloudflare R2 over HTTP range — the same CORS-enabled origin the playground
uses — so the live cards for the `--card` builds (chemotion, chebi-full) render
without anything local.

> Live explore runs the engine in a **Web Worker** (it loads the `pkg-nomodules`
> WASM build via `importScripts`, exactly as the playground does — remote range
> reads need synchronous XHR, which is worker-only). The card, images, search,
> filtering and companion links work without it.

## The manifest (`plaza.json`)

```jsonc
{
  "datasets": [{
    "key": "chebi-full",
    "title": "chebi-full — the complete ChEBI ontology (remote, lazy)",
    "rete": "https://…/chebi-full.rete?token=…",  // URL or local path
    "kind": "remote-lazy",                          // or "bundled"
    "icon": "⚗️",
    "tags": ["chemistry", "ontology", "federation"],
    "blurb": "short curated line shown on the tile",
    "typePredicate": "<http://…/P31>",             // optional; non-rdf:type graphs
    "companions": [
      { "kind": "parquet", "label": "per-class tables", "url": "…/chebi-full-tables/", "verified": false },
      { "kind": "duckdb",  "label": "chebi-full.duckdb", "url": "…/chebi-full.duckdb", "verified": false }
    ],
    "links": [{ "label": "ChEBI (EMBL-EBI)", "url": "https://www.ebi.ac.uk/chebi/" }]
  }]
}
```

`companions` entries with `"verified": false` follow the bucket naming convention
(`scripts/rdf_to_entity_tables.py` output) and are shown with a *"(by
convention)"* hint until confirmed.

## Status & roadmap (it's a sketch)

- **Works now:** live card reading (header + range); **ink-on-paper schema
  images** drawn with p5.js (classes + class_links, Perlin jitter, paper
  texture, serif depth labels) and cached to PNG; an **interactive d3
  ontology/schema graph** on the detail page (hover/click a class for its
  relations); grid search/tag filter; full card render; a **Files dropdown +
  copy-link** under the thumbnail; live autocomplete + SPARQL on both bundled and
  remote-lazy datasets; a **light/dark theme** (persisted, re-skins the images);
  http→https link upgrading + new-tab; and **derived facet chips** (geo /
  temporal / multilingual / vocab / license) that are shown and filterable; and a
  **"Connected to"** summary of the external databases / identifier providers each
  dataset links to (Wikidata, DBpedia, Getty, ChEBI/OBO, Nomisma, …), detected
  from its IRIs plus curated `connections` in the manifest; and an **"Explore
  tables"** panel that queries the Parquet companions in-browser with DuckDB-Wasm
  (lazy-loaded, HTTP-range — lists the per-class tables from `_manifest.parquet`,
  click one to query it).
- **Vendored** (so it stays offline-capable, no CDN): `vendor/p5.min.js`
  (LGPL-2.1), `vendor/d3.min.js` (ISC), and `vendor/elk.bundled.js` (EPL-2.0, the
  layered/orthogonal layout engine for the UML schema diagram). DuckDB-Wasm (the
  table explorer) is the one CDN-loaded, on-demand, dependency.
- **Cards present:** of the seeded datasets only the `rete build --card` builds
  (chemotion, chebi-full) carry an embedded card today; the rest fall back to a
  **header-only** card (counts + content hash) plus manifest fields. Rebuilding
  the bundled graphs with `--card` lights up their full profile + query library.
  `scripts/extract_cards.py` reports which files lack a card and freezes a static
  snapshot for fully-offline hosting.
- **Next:** a WASM `card`/`card_url` export so even a snapshot is unnecessary;
  consuming the snapshot for zero-request hosting; verifying companion URLs;
  a "federate two datasets" view (chebi-full + chemotion already share IRIs);
  pulling the rest of the playground catalog in automatically.

## Files

| file | what |
|------|------|
| `index.html`, `js/catalog.js` | the gallery grid: load manifest, read cards, render tiles, search/filter |
| `dataset.html`, `js/dataset.js` | one dataset: full card, companions, live explore |
| `ontology.html`, `js/ontology.js` | one ontology's page: metadata + terms + used-in datasets, and the real schema UML when a `.rete` provides it (manifest `provides`) |
| `js/rete-card.js` | dependency-free header + embedded-card reader (HTTP range or bytes) |
| `js/procgen.js` | spec builder + schema-graph layout (`imageInfoFromCard`, `buildSchemaGraph`) shared by the renderers |
| `js/procgen-p5.js` | p5.js ink-on-paper renderer → cached PNG data URL (paper texture, jitter, serif depth labels) |
| `js/schema-uml.js` | UML class-diagram of the ontology (boxes + properties), ELK orthogonal edge routing, hover/click details |
| `js/schema-d3.js` | (legacy) force-directed ontology graph — superseded by `schema-uml.js` |
| `js/vocabs.js` | name the ontologies/vocabularies a dataset is built with (incl. OBO sub-ontologies from class IRIs) |
| `js/facets.js` | derive informative chips (geo / temporal / multilingual / vocab / license) |
| `js/providers.js` | detect external DBs/ID providers a dataset links to (from its IRIs + curated `connections`) |
| `js/tables-duckdb.js` | in-browser DuckDB-Wasm explorer for the Parquet companion tables (lazy CDN load, HTTP-range) |
| `js/theme.js` | light/dark toggle, persisted in localStorage |
| `vendor/` | vendored `p5.min.js` + `d3.min.js` (no CDN) |
| `js/plaza-worker.js` | WASM worker: `Graph` (bundled) / `RemoteGraph` (lazy) for query + autocomplete |
| `plaza.json` | the dataset list + visual identity + companion links |
| `scripts/extract_cards.py` | optional: snapshot cards / report which files lack one |
| `styles.css` | the gallery theme |
