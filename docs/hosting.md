# Hosting your `.rete`

A `.rete` file needs **no special server** — the whole promise of the format is
that any HTTP host can serve it queryably. This page says exactly what a host
must support, how to check it in ten seconds, and gives working recipes for the
hosts this project uses in production.

## What a host must support

1. **HTTP range requests.** A `Range: bytes=…` request must get back
   `206 Partial Content` with exactly those bytes. A host that ignores `Range`
   and replies `200` with the whole file is rejected loudly by every rete
   client — never silently mis-read.
2. **CORS — for browser clients only.** The CLI (`rete query-url`,
   `sparql-url`, …) needs nothing else. But the playground / any web page
   fetches cross-origin, so the host must send
   `Access-Control-Allow-Origin` (`*` is fine for public data).
3. **A way to learn the file's length** (browser clients). The reader first
   tries a `HEAD` request — `Content-Length` is CORS-safelisted, so this works
   almost everywhere. If the host rejects `HEAD`, it falls back to a one-byte
   ranged `GET` and reads the total from `Content-Range` — which is **not**
   safelisted, so that fallback additionally needs
   `Access-Control-Expose-Headers: Content-Range`. Satisfy either one.

### The ten-second check

```sh
# 206 + a content-range header = ranges work:
curl -sI -r 0-1023 https://host/path/data.rete | grep -iE "^HTTP|content-range"
# HTTP/2 206
# content-range: bytes 0-1023/104857600

# CORS for browsers — expect access-control-allow-origin in the answer:
curl -sI -r 0-1023 -H "Origin: https://example.org" https://host/path/data.rete \
  | grep -i access-control
```

If both pass, `rete card-url https://host/path/data.rete` should print the
dataset card in two range requests — and the same URL pasted into the
[playground](playground-guide.md)'s **+ Add source → Connect (lazy)** queries it
live.

## Recipes

### Cloudflare R2 (what the playground uses)

All the playground's datasets are served from an R2 bucket behind a custom
domain (`data.graphplaza.com`): direct `206` responses, free egress, CORS-open.
Two things to configure once on the bucket:

- **A public hostname** (custom domain or the `r2.dev` subdomain).
- **A CORS policy** that exposes what the readers need:

```json
[{
  "AllowedOrigins": ["*"],
  "AllowedMethods": ["GET", "HEAD"],
  "AllowedHeaders": ["*"],
  "ExposeHeaders": ["Content-Range", "Content-Length", "ETag"],
  "MaxAgeSeconds": 86400
}]
```

Upload over the S3 API (`aws s3 cp`, rclone, or the repo's small helper):

```sh
# env: S3_API_ENDPOINT, ACCESS_KEY_ID, SECRET_ACCESS_KEY
python3 dev/r2_s3.py put <bucket> data.rete datasets/mygraph/data.rete
```

One sizing note: Cloudflare's cache handles objects up to ~512 MB, so files at
or below that stay CDN-cacheable. A bigger graph is better published as several
**shards** queried as one — see
[Federated queries](federation.md#beyond-union-cross-source-joins-and-sharded-datasets).

### Zenodo (free, permanent, citable)

An academic repository works as a live query host: Zenodo serves uploads with
`206` ranges and CORS, and gives the deposit a **DOI** — so a paper's data
citation can point at a file that is *directly queryable in the browser*. No
account tier, no server, and takedown-resistant persistence. Zenodo doesn't
expose `Content-Range` cross-origin, which is exactly why the readers probe
length with `HEAD` first (requirement 3 above) — it just works. The
playground's `wikidata-zenodo` dataset is a 1 GB `.rete` served straight from
a Zenodo DOI, and it benchmarks on par with R2 (~4 s to first answer).

Upload through the normal Zenodo web UI or REST API; the file URL under
`https://zenodo.org/records/<id>/files/<name>` is the query URL.

### GitHub Pages / raw.githubusercontent.com

Both serve ranges and send `Access-Control-Allow-Origin: *`, so a small `.rete`
can live right next to your project site (this repo's own docs site hosts demo
files that way). The constraint is GitHub's **100 MB per-file limit** — fine
for demos and small graphs, not for the big ones.

### S3 / GCS / any static host

Standard object stores work out of the box for the CLI; for browsers, attach
the same kind of CORS policy as the R2 recipe (S3 and GCS both take a JSON CORS
configuration; make sure `GET` + `HEAD` are allowed and `Content-Range` is
exposed). For local development, the bundled dev server speaks ranges:

```sh
python3 scripts/range_server.py 8000 .
rete card-url http://127.0.0.1:8000/data.rete
```

### Hosts that do *not* work (from the browser)

A host can serve the CLI fine and still fail in a browser. The known offender:
Hugging Face's `buckets/…/resolve` endpoint answers `405` to a cross-origin
browser `GET` (the CLI sends no `Origin`, so it never notices). If a host
can't be made CORS-friendly, put a CORS-open front (a proxy, or R2) in front of
the bytes.

## After hosting: publish it properly

- Embed a [Dataset Card](dataset-cards.md) at build time (`rete build --card`)
  so anyone who finds the URL gets the title, license, counts, and runnable
  starter queries from two range requests — `rete card-url <url>`.
- Register it in the [playground](playground-guide.md) catalog to give it a
  browsable home with examples, media rendering, and SQL companions — see
  [Media & SQL companions](media-companions.md).
- For a *living* dataset, `rete serve` (see the [CLI reference](cli.md)) runs a
  live SPARQL endpoint — updates journal beside the immutable base file, and
  `GET /snapshot.rete` emits a fresh publishable snapshot at any time.
