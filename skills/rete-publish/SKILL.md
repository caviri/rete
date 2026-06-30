---
name: rete-publish
description: Publish a built `.rete` dataset to the rete playground — generate Parquet/DuckDB/SQLite companions, upload the file to the HF bucket, and register it in the playground catalog (dataset entry, metadata, icon/tags, SPARQL/SHACL examples, optional geo/PMTiles basemap). Use after the rete-from-graph skill has produced and verified a `.rete`, whenever the task is "make this dataset show up / be explorable in the playground".
---

# Publish a `.rete` to the playground

After **rete-from-graph** produced and verified `web/foo.rete`, this makes it
appear in the browser playground. Pipeline:

```
build companions ──▶ upload to bucket ──▶ register in catalog.js ──▶ rebuild playground ──▶ verify
   (§1, optional)        (§2)                  (§3)                      (§4)               (§5)
```

## 0. Embedded vs remote-lazy — pick the load mode

- **Embedded** (small, ≲ a few MB): base64-inlined into `docs/playground.html` by
  `scripts/build_playground.py`. Instant, fully offline. The file must be in `web/`.
- **Remote-lazy** (anything bigger): served from the HF bucket and HTTP-range-queried;
  only the bytes each query touches are fetched. Use for ≳ a few MB up to multi-GB.

Both are *also* mirrored in the bucket, so any dataset can be cached/lazy-loaded.
Register remote-lazy datasets with their `url`; embedded ones are picked up from
`RETE_DATASETS_B64` at build.

## 1. Companions (optional — enables the Explore SQL backends)

The playground's Explore tab can compare the rete engine against DuckDB/SQLite over
the same data. Generate a relational companion from the N-Triples:

```bash
# generic flat triples table → Parquet (+ optional DuckDB / SQLite):
python skills/rete-publish/scripts/make_companions.py data/foo/foo.nt -o data/foo/foo \
  --parquet --duckdb --sqlite
```

For a richer, class-partitioned layout (Wikidata-style property/entity tables) model
`scripts/rdf_to_entity_tables.py` / `scripts/rdf_to_property_tables.py` instead.
Upload the companions next to the `.rete` (see §2). Datasets without a
`CATALOG.companions[key]` entry simply don't show the backend switch — companions
are optional.

## 2. Upload to the bucket

```bash
# single file:
skills/rete-publish/scripts/upload_bucket.sh web/foo.rete            # → playground/foo.rete
# a directory of companions:
skills/rete-publish/scripts/upload_bucket.sh data/foo/foo-tables/ foo-tables
```

Served at `https://<space>/data/playground/<name>?token=<read-token>` with CORS +
HTTP Range. Writes use your `hf` CLI auth (not the read token). The bucket repo
defaults to the project's; override with `RETE_BUCKET`.

## 3. Register in the catalog

Edit `web/playground-src/catalog.js` — full field reference + copy-paste templates in
**[reference/catalog.md](reference/catalog.md)**. You add (keyed by the dataset key):

1. **`datasets`** — `{key, kind:"remote-lazy", url, label, description}` (omit `url`
   for embedded; the URL derives from `remoteBase/playground/<key>.rete`).
2. **`datasetMeta`** — `{triples, size, license, source, provenance}`.
3. **`datasetExtra`** — `{icon, tags}`.
4. **`examples`** — 2–5 example queries `{family, label, view, cols, tip, q}`. Tips
   must be ≥2 lines, name the human label for any ID, and the `view` picks the output
   (table/map/tiles/graph/time). Add SHACL shapes under `shacl` if relevant.
5. (geo) **`pmtiles`** — a paired/embedded PMTiles vector basemap for the Tiles view.
6. (optional) **`companions`** — the DuckDB/Parquet/SQLite backends from §1.

## 4. Rebuild the playground

```bash
python scripts/build_playground.py     # inlines app.js/catalog.js/styles + wasm + embedded datasets
```
This regenerates `docs/playground.html`. **Git-clean gotcha**: if an embedded dataset
disappears, check it's actually in `web/` and not gitignored away before the build read it.

## 5. Verify (browser-only features need a real browser)

- Headless E2E with the repo's Playwright harness (`dev/playwright/serve.mjs` +
  a probe in the `mcr.microsoft.com/playwright` image) — load the dataset, run an
  example, assert rows/cells/no-console-errors. CodeMirror, WebGL/Canvas maps, and
  IIIF can't render in jsdom; use the browser.
- Remote sanity from the CLI: `rete card-url <url>` and `rete sparql-url <url> "<q>"`
  exercise the same HTTP-range path the playground uses.

## Commit

Commit `catalog.js` + the rebuilt `docs/playground.html` (and any new converter/
companion scripts). The `web/*.rete` and `data/` stay gitignored — the file lives in
the bucket, the *recipe* lives in the repo. In this repo, commit **without** the
Claude co-author trailer.
