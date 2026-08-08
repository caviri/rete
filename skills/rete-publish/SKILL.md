---
name: rete-publish
description: Publish a built `.rete` dataset to the rete playground: generate optional Parquet/DuckDB/SQLite companions, upload files to Cloudflare R2, register the dataset in the catalog, and verify the public Range/CORS contract. Use after rete-from-graph has produced and verified a `.rete` whenever the task is to make a dataset explorable in the playground.
---

# Publish a `.rete` dataset to the playground

After **rete-from-graph** produces and verifies `data/foo/foo.rete` (the `.rete`
lives WITH its dataset, not in `web/`), this workflow makes it available in the
browser playground:

```text
build companions -> upload to R2 -> register catalog -> rebuild -> verify -> back up
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

## 6. Back it up — R2 is not a backup

**A published file lives on exactly two disks, or it does not exist.** This step
is not optional and it is not "later": the day it was first measured, 44 of 273
local `.rete` files (177.55 GB) existed on **one** disk — the whole OpenAlex
corpus among them, five ~30 GB `works` shards plus the 15.67 GB
`openalex-authors.rete`, the largest `.rete` ever built — on a volume with
195 GB free. R2 is a serving surface, not a safe: a bucket-level mistake, an
expired card or a bad `aws s3 rm --recursive` takes the only copy with it.

```bash
# After publishing one dataset — the normal case.
scripts/backup_to_hf.sh data/foo/foo.rete

# Plan first, upload nothing (also a pre-flight: non-zero if a source is unsound).
scripts/backup_to_hf.sh --source both --dry-run

# Whole-corpus sweep: every data/**/*.rete plus every .rete object on R2.
scripts/backup_to_hf.sh --source both
```

### Why the backup is a *different* store, and why it cannot serve

The two are not interchangeable, and the difference is the reason there must be
two of them:

- **R2 serves.** `https://data.graphplaza.com/<key>` answers an HTTP Range
  request **directly** — `206` with `Content-Range`, no redirect — which is the
  only thing the playground's default synchronous-XHR worker reader can consume,
  and it benchmarks ~3.5× faster than the HF Space. The CORS policy **must** put
  `Content-Range` in `ExposeHeaders`: the open probe fetches `Range: bytes=0-0`
  and reads the file length out of that header, so without it every remote open
  fails before the first query.
- **An HF bucket cannot serve rete's reader at all.** It is Xet
  content-addressed chunk storage. The public resolve URL
  `huggingface.co/buckets/<ns>/<bucket>/resolve/<file>` (no revision segment)
  does work, but it **302-redirects** to a per-range-signed Xet-bridge CDN — and
  a synchronous XHR cannot follow a 302. The CLI is fine; the browser is not.

So HF is the durable **backup**, never a second origin. Never put a
`huggingface.co/buckets/...` URL in `catalog.js`.

### Destination keys

One prefix, mirroring the R2 key layout exactly, so an object's identity is the
same in both stores:

```text
local   data/<dataset>/<file>.rete
R2           <dataset>/<file>.rete  ->  https://data.graphplaza.com/<dataset>/<file>.rete
HF      rete/<dataset>/<file>.rete  ->  hf://buckets/katospiegel/rete-public/rete/<dataset>/<file>.rete
```

The R2 key is the local path minus `data/`; the HF key is the R2 key under one
`rete/` prefix (the bucket also holds non-`.rete` material, which is why the
prefix exists). Override the bucket with `RETE_HF_BUCKET`.

### What the script guarantees

- **Resumable.** It snapshots the bucket first and skips any object already
  present at the identical byte size, so a killed sweep is restarted by
  re-running it. A local file whose size *differs* from the backed-up one is not
  skipped — that is a divergent build, and it gets its own object.
- **Verified after upload, not on exit code.** `hf buckets cp` returning 0 is not
  evidence that the object landed whole; the size is read back out of the bucket
  per object, and the run ends by re-listing and confirming every object in the
  work list — the number worth quoting is of the form *297 objects present at
  matching byte counts*.
- **Single instance, by atomic `mkdir` lock.** Two concurrent sweeps each passed
  their own free-space check and then jointly exhausted the disk: two 56 GB pulls
  took 68 GB. A free-space decision is only sound while one process is making it.
- **Nothing truncated is ever uploaded.** A killed download once left a
  **zero-byte** file that a naive uploader would have shipped as a valid-looking,
  completely empty graph. Every source is size-checked against what was expected,
  and a `.rete` must carry the 4-byte `RETE` magic at **both** ends — 8 bytes that
  prove the file is whole end to end.

### Verify

```bash
# Whole corpus, read-only. Ends with
#   verified N/N object(s) present at matching byte counts
#   DRY RUN: planned=0 skipped=N unsound=0 ...
# planned=0 with N/N verified IS the all-clear; it exits non-zero if any local
# source is unsound, so it also works as a pre-flight before a real run.
scripts/backup_to_hf.sh --source both --dry-run

# One object.
hf buckets ls katospiegel/rete-public/rete/foo/foo.rete --recursive --json
```

Anything short of N/N is listed by key in `dev/backup-hf/missing.txt`; the logs
and the resume state live beside it in `dev/backup-hf/`.

## Commit

Commit `catalog.js`, `web/datasets.lock.json`, the rebuilt
`docs/playground.html`, and any converter or companion scripts. Dataset files
and raw `data/` remain gitignored; R2 holds the published bytes while the repo
holds their recipes. Commit without a co-author trailer.
