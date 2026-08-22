# Playground catalog reference (`web/playground-src/catalog.js`)

Everything is keyed by a lowercase, kebab-case dataset key such as `foo` or
`geoadmin-tiles`. Top-level configuration uses `remoteBase` and
`companionBase` for the public R2 origin. `remoteToken` is empty.

## 1. `datasets`

```js
// Remote-lazy: served directly from R2 and range-queried.
{"key": "foo", "kind": "remote-lazy",
 "url": "https://data.graphplaza.com/foo/foo.rete",
 "label": "foo.rete - one-line description (remote, lazy)",
 "description": "Explain the graph, classes and edges, scale, license, build recipe, and why it is interesting."},
```

For embedded data, omit `kind` and `url`; it is discovered from the inlined
bytes. Its remote mirror derives as `remoteBase/<key>/<key>.rete`.

**`textIndex`** — set `"textIndex": true` if, and only if, the published file was
built with `--text-index` (a TEXT_INDEX section, kind 6, in its header). It is a
declaration, not a switch: nothing reads it at query time, but two checks hold it
to the truth, so a wrong value is a red gate rather than a quiet lie.

- `tests/gate/checks/check_text_index_claims.mjs` (offline, every gate run) —
  the flag and the prose must agree. If `textIndex: true`, at least one of
  `label` / `description` / `datasetMeta.provenance` / `datasetExtra.tags` must
  say so ("TEXT_INDEX on for full-text search over its literals."); if the flag
  is absent, none of them may claim one.
- `scripts/check_dataset_catalog.py` (network, weekly) — the flag vs the section
  directory the bucket actually serves.

A full-text index is opt-in at build time and `FILTER(CONTAINS(…))` answers with
or without one — by word lookup or by full scan — so an undeclared index is
invisible in both directions until something compares the header to the catalog.

To read the truth off any file before you write the flag, ask the file:

```sh
rete card-url https://data.graphplaza.com/<key>/<key>.rete --json \
  | python3 -c 'import json,sys; print(json.load(sys.stdin)["signals"]["text_index"])'
# {'bytes': 1879287762, 'present': True, 'token_table_bytes': 193295361}
```

`signals.text_index` is **measured** from the section directory by the reader,
not stored in the card ([Dataset Cards](../../../docs/dataset-cards.md#the-full-text-signal-measured-not-stored)),
so it is right for every published file today and cannot go stale. The catalog
flag stays a separate, hand-written **declaration** on purpose: it is the claim
the two checks above hold the bucket to, and a flag derived from the bytes could
not detect the bytes changing.

## 2. `datasetMeta`

```js
"foo": { triples: "1.2 M", size: "23 MB", license: "CC0-1.0",
         source: "https://example.org",
         provenance: "source -> scripts/foo_to_nt.py -> rete build --pyramid-algo types --card" },
```

## 3. `datasetExtra`

```js
"foo": { icon: "book", tags: ["domain", "GeoSPARQL", "federation", "CC0"] },
```

Use the established icon style in neighboring catalog entries.

## 4. `examples`

```js
"foo": [
  {"family": "Select", "label": "Short human label", "view": "table",
   "cols": {"s": "Subject", "label": "Name"},
   "tip": "Use at least two lines. Explain what the query shows and name any opaque IDs.",
   "q": "PREFIX ex: <https://foo/>\nSELECT ?s ?label WHERE {\n  ?s a ex:Thing ; rdfs:label ?label .\n} LIMIT 50"}
]
```

`view` is `table`, `graph`, `map`, `tiles`, or `time`. `family` must be in
`CATALOG.families`. Map results need a `geo:asWKT` column. Tile results need a
`CATALOG.pmtiles[key]` entry. `fed: ["other-key"]` can preload a federation
partner. SHACL examples live in the parallel `shacl` map.

## 5. `pmtiles`

```js
// Separate object next to the graph.
"foo": { url: "https://data.graphplaza.com/foo/foo.pmtiles",
         label: "foo basemap", size: "113 MB",
         layers: { countries: "shapeName" } },

// PMTiles section embedded inside the .rete file.
"foo-tiles": { url: "https://data.graphplaza.com/foo-tiles/foo-tiles.rete",
               embedded: true, label: "embedded basemap",
               layers: { countries: "shapeName" } },
```

Build PMTiles with the relevant converter and `tippecanoe`; use
`scripts/embed_tiles.py` for an embedded section.

## 6. `companions`

```js
companions: {
  "foo": { duckdb: "foo.duckdb", parquet: "parquet/", sqlite: "foo.sqlite" }
},
```

Paths are relative to the dataset folder under `companionBase`. Only datasets
with a companion entry show the backend switch.

## After editing

Run `uv run python scripts/build_playground.py`, then the Playwright browser
gate. Finally run `uv run python scripts/check_dataset_catalog.py --all` to
verify the public R2 Range/CORS contract and `web/datasets.lock.json`.
