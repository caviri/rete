---
name: dataset-download
description: Acquire and organize a NEW raw dataset under ./data/<name>/ before it becomes a .rete — investigate a source (download page, API, S3/Tigris bucket) to find ALL of its data, scaffold the standard folder layout (raw/ + scripts/ + README + checksums), download robustly (reproducible + resumable + parallel), follow manifest→asset links (nested-JSON URLs, .cif/.png/etc.), and document the schema. Use whenever the task is "download this dataset", "grab all the data from <site>", or "get <source> into ./data" — the step that feeds the rete-from-graph skill.
---

# Download & organize a new dataset into `./data/<name>/`

This is the **acquisition** step that comes before `rete-from-graph`. The goal:
land *all* of a source's data under a predictable folder layout, reproducibly,
so a future `<name>.rete` build is a clean hand-off.

```
investigate ─▶ scaffold ─▶ download ─▶ verify ─▶ profile ─▶ document ─▶ (rete-from-graph)
   (§1)         (§2)        (§3)        (§4)      (§5)        (§6)
```

> **Repo conventions that bind here** (see [skills/README.md](../README.md)):
> run **everything in Docker** (python/uv/npm/cargo) — only plain `curl`/`hf`
> may run on the host; `data/` is **gitignored** (the *scripts* get committed,
> not the downloaded bytes); **never scrape robots-blocked sites** — go to the
> official dump / API / open bucket. Commit **without** the Claude co-author trailer.

## Standard layout (the contract)

Every dataset gets exactly this shape:

```
data/<name>/
  README.md              # provenance, license, schema, layout, reproduce steps
  SHA256SUMS.txt         # checksums of the raw file(s)
  raw/                   # the downloaded bytes, AS-IS (never hand-edited)
    <source-file(s)>
    assets/              # only if the source links out to files (see §3.3)
      <type>/…
      *.urls.txt, all_assets.tsv   # asset manifests
  scripts/               # every script used, committed to git
    download.sh          # reproducible re-download of the primary file(s)
    inspect_csv.py       # schema/statistics profiler (NOT inspect.py — see gotchas)
    …                    # extract/convert/harvest helpers as needed
```

Scaffold it in one shot:

```bash
bash skills/dataset-download/scripts/scaffold.sh <name>
# creates data/<name>/{raw,scripts}, a README.md skeleton, SHA256SUMS.txt,
# and a download.sh stub to fill in.
```

## 1. Investigate — find ALL the data (don't stop at the obvious file)

A "download page" almost always hides more than the one button. Work through
**[reference/investigate.md](reference/investigate.md)**; the essentials:

- **Grep the page, don't trust the render.** `curl -sSL <page> -o page.html`
  then extract every link: `grep -oE 'https?://[^"'\''<> ]+' page.html | sort -u`.
  Look for storage hosts (`*.s3.*`, `*.t3.storage.dev`, `storage.<site>`,
  `*.r2.dev`, `zenodo.org/record`), and `.csv/.json/.parquet/.zip/.nt/.ttl`.
- **Probe the bucket.** Many S3/Tigris/R2 buckets disallow listing
  (`?list-type=2` → AccessDenied) — then the page URL is the only entry point.
  If listing *is* allowed, page through it for other files / older snapshots.
- **HEAD before GET.** `curl -sSLI <url>` gives `Content-Length`, `Content-Type`,
  `Last-Modified`, `Accept-Ranges`. Record the size; note dated snapshot names.
- **The primary file is often a manifest.** A CSV/JSON row may embed URLs to the
  *real* payload (3D structures, images, PDFs) — sometimes nested inside a JSON
  column (`{"url": …}` or `{"file": {"url": …}}`), so a naive top-level URL scan
  finds nothing. Profile the file (§5) and extract those links (§3.3).
- **License + attribution.** Capture the license and the exact attribution string
  now — it goes in the README and later the Dataset Card.
- **Snapshots/versions.** A dated filename (`..._28_01_2026.csv`) *is* the
  snapshot; note the date and whether newer ones are reachable.

## 2. Scaffold

```bash
bash skills/dataset-download/scripts/scaffold.sh <name>
```
Then fill `scripts/download.sh` with the real URL(s) you found in §1.

## 3. Download

### 3.1 Primary file(s) — reproducible
Put the real download in `scripts/download.sh` (a `curl -sSL --fail` per file into
`raw/`, writing `SHA256SUMS.txt`). Plain `curl` on the host is fine. Run it:
```bash
bash data/<name>/scripts/download.sh
```

### 3.2 Check disk space FIRST for anything large
```bash
df -h .        # confirm free space >> total download size
```
A full bind-mount surfaces as `OSError: [Errno 5] Input/output error` on *write*
(not a network error) — it means the disk is full, not the server failing. Never
start a tens-of-GB harvest onto a near-full drive.

### 3.3 Linked assets — extract a manifest, then harvest in parallel
If the primary file references external files (§1), extract them to a URL list
per type, then download resumably in Docker:

```bash
# 1. extract URLs into data/<name>/raw/assets/<type>.urls.txt  (write a small
#    per-source extractor; model on data/proteinbase/scripts/extract_asset_urls.py)
# 2. harvest — parallel, resumable, atomic, retries; stdlib-only so plain python:slim works
MSYS_NO_PATHCONV=1 docker run --rm -v "$PWD:/w" -w //w python:3.12-slim \
  python skills/dataset-download/scripts/fetch_urls.py \
    data/<name>/raw/assets/<type>.urls.txt data/<name>/raw/assets/<type> --workers 16
```

`fetch_urls.py` skips already-present files, writes `.part` then renames (no
half-files), retries with backoff, and logs misses to `download_failures.txt`
(re-run to retry only those). **Use fewer workers (~8) for large files** (multi-MB
each) — high concurrency of big writes is what tips a bind-mount over.
Map `s3://<bucket>/<key>` URIs to the bucket's public https host before fetching.

## 4. Verify

```bash
cd data/<name>/raw && sha256sum <file> | tee ../SHA256SUMS.txt   # matches download.sh?
wc -c <file>                                                     # == Content-Length from §1?
find assets -name '*.part'                                       # none = clean
```
For asset sets, confirm the on-disk file count equals the manifest line count per
type, and that `download_failures.txt` is absent (or empty) — if present, re-run
the harvester until it's clean, then delete the stale list.

## 5. Profile the schema

Write `scripts/inspect_csv.py` (a real profiler, run in Docker) that reports
columns, fill/uniqueness, category distributions, and — for embedded JSON — the
object keys/types. Model on `data/proteinbase/scripts/inspect_csv.py`. This report
is what you paste into the README and reuse when modelling the graph in
`rete-from-graph`.

```bash
MSYS_NO_PATHCONV=1 docker run --rm -v "$PWD:/w" -w //w python:3.12-slim \
  python data/<name>/scripts/inspect_csv.py
```

## 6. Document

Fill `data/<name>/README.md`: source URL, **license + attribution string**,
snapshot date, the layout tree, the schema table (from §5), the asset inventory
(counts + sizes + status), and the exact **reproduce** commands. Record a memory
entry for the dataset (type `project`) noting what's non-obvious — the entry
point, gotchas, and that the `.rete` is pending. Then hand off to
**rete-from-graph**.

## Gotchas (hard-won)

- **Docker + Git Bash path mangling (Windows).** `-v /d/...:/w -w /w` gets rewritten
  to `W:/`. Fix: `MSYS_NO_PATHCONV=1 docker run -v "D:/pro/rete:/w" -w //w …`
  (Windows-style mount, double-slash workdir).
- **`[Errno 5] I/O error` on write == disk full**, not a network fault. `df -h .`
  before and after; free space before resuming.
- **Buckets don't list.** `AccessDenied` on `?list-type=2` is normal — the download
  page URL is then the only handle. `s3://bucket/key` → `https://bucket.<host>/key`.
- **BOM.** CSV/JSON exports often start with a UTF-8 BOM — read as `utf-8-sig`, or
  the first column name comes back as `\ufeffid`.
- **Never name a script after a stdlib module** — `inspect.py`, `profile.py`,
  `code.py`, `types.py`, `json.py`. Python puts the script's own dir on
  `sys.path[0]`, so it shadows the real module and unrelated imports crash later
  (e.g. `attrs` doing `inspect.signature` → `AttributeError`). Use `inspect_csv.py`.
- **Manifest-in-manifest.** The headline file can be a list of URLs to the real
  payload, sometimes nested in a JSON cell. Always profile before assuming "the
  CSV is the dataset".
- **Reproducibility over cleverness.** The committed `download.sh` + `extract` +
  `fetch_urls.py` must reconstruct `raw/` from scratch — `data/` is gitignored, so
  the scripts *are* the dataset in the repo.
