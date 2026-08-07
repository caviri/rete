---
name: rete-publish
description: Publish a built `.rete` dataset to the rete playground: generate optional Parquet/DuckDB/SQLite companions, upload files to Cloudflare R2, register the dataset in the catalog, and verify the public Range/CORS contract. Use after rete-from-graph has produced and verified a `.rete` whenever the task is to make a dataset explorable in the playground.
---

# Publish a `.rete` dataset to the playground

After **rete-from-graph** produces and verifies `data/foo/foo.rete` (the `.rete`
lives WITH its dataset, not in `web/`), this workflow makes it available in the
browser playground:

```text
build companions -> upload to R2 -> register catalog -> rebuild -> verify
```

## 0. Pick the load mode

- **Remote-lazy** (default; anything more than a few MB): serve it straight from
  Cloudflare R2 from `data/foo/foo.rete`. The browser fetches only the ranges each
  query touches. The file never needs to enter `web/`.
- **Embedded** (small, no more than a few MB): base64-inline it into
  `docs/playground.html` with `scripts/build_playground.py`. This is the ONLY case
  that needs the file in `web/` — copy it there as a staging step
  (`cp data/foo/foo.rete web/foo.rete`) just before the playground rebuild.

Both modes are mirrored in R2 so users can cache or lazy-load the graph.

## 1. Build optional SQL companions

The Explore tab can compare rete with DuckDB and SQLite over the same data:

```bash
python skills/rete-publish/scripts/make_companions.py data/foo/foo.nt -o data/foo/foo \
  --parquet --duckdb --sqlite
```

For class-partitioned Wikidata-style layouts, model
`scripts/rdf_to_entity_tables.py` or `scripts/rdf_to_property_tables.py`.
Companions are optional.

## 2. Upload to Cloudflare R2

```bash
# Defaults to foo/foo.rete.
skills/rete-publish/scripts/upload_bucket.sh data/foo/foo.rete

# Explicit object key and recursive companion prefix.
skills/rete-publish/scripts/upload_bucket.sh data/foo/foo.rete foo/foo.rete
skills/rete-publish/scripts/upload_bucket.sh data/foo/foo-tables/ foo
```

It runs the upload in a container (like everything else here); the host needs
only Docker. The uploader reads `ACCESS_KEY_ID`, `SECRET_ACCESS_KEY`, and
`S3_API_ENDPOINT` from the environment or the repository's gitignored `.env`.
Objects are public at `https://data.graphplaza.com/<key>` without a redirect or
token. The bucket defaults to `rete`; override it with `RETE_BUCKET`.

## 3. Register the dataset

Edit `web/playground-src/catalog.js`. The full field reference and copy-paste
templates are in **[reference/catalog.md](reference/catalog.md)**. Add:

1. `datasets`: `{key, kind:"remote-lazy", url, label, description}`. Omit `url`
   for embedded data; remote URLs derive from
   `remoteBase/<key>/<key>.rete`. Add `textIndex: true` if the file was built
   with `--text-index`, and say so in the description — the gate compares the
   flag to the prose, and the weekly catalog probe compares it to the header.
2. `datasetMeta`: `{triples, size, license, source, provenance}`.
3. `datasetExtra`: `{icon, tags}`.
4. Two to five `examples` with family, label, view, columns, a useful tip, and
   the SPARQL query. Add SHACL shapes where relevant.
5. Optional `pmtiles` metadata for a geographic dataset.
6. Optional `companions` metadata for DuckDB, Parquet, or SQLite.

## 4. Rebuild

```bash
uv run python scripts/build_playground.py
```

This regenerates `docs/playground.html`. If an embedded dataset disappears,
check that its gitignored source file is staged under `web/` before rebuilding.

## 5. Verify

- Run the repo's Playwright browser gate: load the dataset, execute at least
  one example, and assert rows plus no console errors.
- Run `rete card-url <url>` and `rete sparql-url <url> "<query>"` for CLI-level
  range-read checks.
- Run `uv run python scripts/check_dataset_catalog.py --all`. It checks exact
  URLs (no redirects), `206`, Range/CORS response headers, stable format byte,
  file size, content hash, and `web/datasets.lock.json`.
- Verify R2 CORS in a browser. `Content-Range` must be in
  `Access-Control-Expose-Headers`; curl alone cannot prove that contract.

## Commit

Commit `catalog.js`, `web/datasets.lock.json`, the rebuilt
`docs/playground.html`, and any converter or companion scripts. Dataset files
and raw `data/` remain gitignored; R2 holds the published bytes while the repo
holds their recipes. Commit without a co-author trailer.
