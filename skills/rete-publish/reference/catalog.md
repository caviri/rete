# Playground catalog reference (`web/playground-src/catalog.js`)

Everything is keyed by the dataset **key** (kebab/lowercase, e.g. `foo`,
`geoadmin-tiles`). Add an entry in each relevant section. Top-level config you'll
reference: `remoteBase` (bucket data URL), `remoteToken` (the read token), the
`families` list (example families).

## 1. `datasets` — the catalog entry

```js
// remote-lazy (served from the bucket, range-queried):
{"key": "foo", "kind": "remote-lazy",
 "url": "https://<space>/data/playground/foo.rete?token=<read-token>",
 "label": "foo.rete - one-line what-it-is (remote, lazy)",
 "description": "A full paragraph: what the graph is, its classes/edges, scale, license, how it was built, and what makes it interesting to query. This shows in the dataset browser."},
```
For an **embedded** dataset omit `kind`/`url` — it's discovered from the inlined
bytes; the URL is derived as `remoteBase/playground/<key>.rete`.

## 2. `datasetMeta` — the metadata table

```js
"foo": { triples: "1.2 M", size: "23 MB", license: "CC0-1.0",
         source: "https://example.org",
         provenance: "How it was built: source → converter (scripts/foo_to_nt.py) → rete build --pyramid-algo types --card. Note any simplification/sharding/gotchas." },
```
`triples` may be a number or a human string; `size` is the `.rete` size.

## 3. `datasetExtra` — icon + tags

```js
"foo": { icon: "📚", tags: ["domain", "GeoSPARQL", "federation", "CC0"] },
```
The icon shows in the dataset list and example chips; tags are searchable.

## 4. `examples` — 2–5 example queries

```js
"foo": [
  {"family": "Select", "label": "Short human label", "view": "table",
   "cols": {"s": "Subject", "label": "Name"},          // optional custom column headers
   "tip": "≥2 lines. Say what this shows AND name the human label for any ID in the query (e.g. \"Q5 = human\"). Shown inline under the query chips, not just on hover.",
   "q": "PREFIX ex: <https://foo/>\nSELECT ?s ?label WHERE {\n  ?s a ex:Thing ; rdfs:label ?label .\n} LIMIT 50"},
  // view ∈ table | graph | map | tiles | time. family ∈ the CATALOG.families list.
  // map → needs a geo:asWKT column; tiles → needs a CATALOG.pmtiles[key]; graph → 3-col or CONSTRUCT.
]
```
Tips rules (learned): **≥2 lines**, shown inline (the always-visible strip under the
quick chips), and **name the human label for any opaque ID**. Optional `fed: ["other-key"]`
on an example one-click-adds a federation partner.

SHACL shapes (optional) go in a parallel `shacl` map:
```js
shacl: { "foo": [ {"label": "Each Thing has a label", "tip": "...", "shape": "<TTL shapes>"} ] }
```

## 5. `pmtiles` — a vector-tile basemap for the Tiles view (geo datasets)

```js
// (B) a separate .pmtiles next to the .rete:
"foo": { url: "https://<space>/data/playground/foo.pmtiles?token=<tok>",
         label: "foo basemap", size: "113 MB",
         layers: { countries: "shapeName", regions: "shapeName" } },   // layer → name property
// (C) tiles EMBEDDED inside the .rete (one file = graph + tiles): url is the .rete,
//     add embedded:true so the Tiles view parses the header for the tile section offset.
"foo-tiles": { url: "https://<space>/data/playground/foo-tiles.rete?token=<tok>",
               embedded: true, label: "embedded in foo-tiles.rete", size: "113 MB section",
               layers: { countries: "shapeName" } },
```
Build the PMTiles with tippecanoe (`scripts/geoadmin_pmtiles.sh`); embed into a .rete
with `scripts/embed_tiles.py`.

## 6. `companions` — DuckDB / Parquet / SQLite Explore backends (optional)

```js
companions: { "foo": { duckdb: "foo.duckdb", parquet: "foo-tables/", sqlite: "foo.sqlite" } },
```
Paths are bucket-relative (`playground/<...>`). Only datasets with an entry show the
backend switch in the Explore tab.

---

## After editing

`python scripts/build_playground.py` rebuilds `docs/playground.html` (inlines
catalog.js + app.js + styles + WASM + embedded datasets). Verify in a browser /
the Playwright harness — CodeMirror and the map renderers don't run in jsdom.
