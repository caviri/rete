# Investigating a source — finding ALL of its data

The download button is rarely the whole story. Work these angles before you
decide "that's everything".

## 1. Read the page as HTML, not as rendered text

```bash
curl -sSL "<page-url>" -o page.html
# every absolute URL on the page:
grep -oE 'https?://[^"'\''<> ]+' page.html | sort -u
# data-ish links and storage hosts:
grep -oiE '(storage\.|\.s3\.|t3\.storage\.dev|r2\.dev|zenodo\.org/record|figshare|\.csv|\.json|\.parquet|\.tsv|\.zip|\.gz|\.nt|\.ttl|\.rdf|/api/|download)[^"'\''<> ]*' page.html | sort -u
```

Many sites are SPAs (Next.js, etc.) — URLs may appear escaped (`https:\/\/…`).
Grep tolerates that; just strip trailing `\`. If the data links are injected by
JS and not in the HTML, drive the page with the browser tools
(`mcp__claude-in-chrome__*`) and read the network requests / DOM.

## 2. HEAD every candidate before downloading

```bash
curl -sSLI "<file-url>"        # Content-Length, Content-Type, Last-Modified, Accept-Ranges
```

- **Content-Length** → record the exact byte size (verify after download).
- **Last-Modified** / a dated filename (`..._28_01_2026.csv`) → the snapshot date.
- **Accept-Ranges: bytes** → range-readable (good; relevant later for .rete hosting).

## 3. Probe the storage bucket

Data usually lives on object storage (S3, Tigris `*.t3.storage.dev`, R2 `*.r2.dev`,
GCS). Try to list it:

```bash
curl -sSL "https://<bucket-host>/?list-type=2"   # S3 ListObjectsV2
```

- **`AccessDenied` / `<Error>`** → listing is disabled; the page's file URL is the
  only entry point. That's normal and fine.
- **XML `<ListBucketResult>`** → page through `<Key>` entries for other files and
  older snapshots (`&continuation-token=…`).
- `s3://bucket/key` URIs map to `https://bucket.<public-host>/key` — e.g.
  `s3://proteinbase-pub/X.json` → `https://proteinbase-pub.t3.storage.dev/X.json`.

Common open-data hosts with real APIs: **Zenodo** (`/api/records/<id>`),
**Figshare**, **Hugging Face** (`/api/datasets/<id>`), **GBIF** (DwC-A),
**data portals** (CKAN `/api/3/action/package_show`).

## 4. The primary file is often a manifest

A CSV/JSON export frequently *points at* the real payload rather than containing
it. After downloading it, profile it (see the `inspect.py` pattern) and look for:

- columns/fields that hold **URLs** (structures `.cif/.pdb`, images, PDFs, media);
- URLs **nested inside a JSON column** — `{"url": …}` or `{"file": {"url": …}}` —
  which a top-level "does this string start with http" scan will miss entirely
  (this is exactly how ProteinBase stores its 3D structures and PAE matrices);
- ID columns that resolve via a documented URL template.

Extract those into per-type URL manifests
(`raw/assets/<type>.urls.txt`, plus a combined `all_assets.tsv` keyed by record
id) — model on `data/proteinbase/scripts/extract_asset_urls.py` — then harvest
with `fetch_urls.py`. **Estimate total size first** (sample a handful with HEAD)
and `df -h .` — asset sets can be far bigger than the manifest (ProteinBase: a
40 MB CSV → 33 GB of linked assets, ~26 GB of it PAE JSON alone).

## 5. Licence, attribution, snapshots

- Capture the **licence** and the **exact attribution string** verbatim — it goes
  in the README now and the Dataset Card (`--card`) later.
- Note the **snapshot date/version**. If newer snapshots are reachable, record how
  to find the latest (a dated URL pattern, an API `latest` field, a bucket prefix).
- **Robots / ToS**: if the site blocks crawlers, do **not** scrape — find the
  official dump, API, or open bucket instead. If there's genuinely no open route
  and the user still wants it, surface that trade-off rather than deciding silently.
