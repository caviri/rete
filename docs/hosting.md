# Hosting your `.rete`

A `.rete` file needs **no special server** — the whole promise of the format is
that any HTTP host can serve it queryably. This page says exactly what a host
must support, how to check it in ten seconds, and gives working recipes for the
hosts this project uses in production.

## What a host must support

1. **HTTP range requests.** A `Range: bytes=…` request must get back
   `206 Partial Content` with exactly those bytes. A host that ignores `Range`
   and replies `200` with the whole file is never silently mis-read: the CLI
   rejects it, and the browser reader treats that body as the whole file —
   it keeps a body of up to 256 MiB and serves every read from it (so the
   file is downloaded whole, once per open, and the result line counts it),
   and refuses a larger one with a message that says the host ignores `Range`.
2. **CORS — for browser clients only.** The CLI (`rete query-url`,
   `sparql-url`, …) needs nothing else. But the playground / any web page
   fetches cross-origin, so the host must send
   `Access-Control-Allow-Origin` (`*` is fine for public data) and allow the
   `Range` request header in a preflight (`Access-Control-Allow-Headers: Range`).
3. **Nothing has to be exposed for the length** (browser clients). A browser
   can read `Content-Range` only when the host lists it in
   `Access-Control-Expose-Headers`, and `HEAD`'s `Content-Length` may describe
   a compressed representation (GitHub Pages, #95). So the reader takes a
   `.rete`'s length from the file itself: the first ranged `GET` reads the 1 KiB
   header, whose section directory fixes the length, and one 4-byte read of the
   closing `RETE` footer confirms it. A visible `Content-Range` that agrees
   saves that footer read, which is why the production bucket still exposes
   it. A resource that is not a plain `.rete` (a polyglot page) falls back to
   `Content-Range`, then to `HEAD`'s `Content-Length`, which is
   CORS-safelisted and readable everywhere; the opt-in persistent range cache
   does the same when `Content-Range` is hidden.

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
[Federated queries](federation.md#beyond-union--cross-source-joins-and-sharded-datasets).

### Zenodo (free, permanent, citable)

An academic repository works as a live query host: Zenodo serves uploads with
`206` ranges and CORS, and gives the deposit a **DOI** — so a paper's data
citation can point at a file that is *directly queryable in the browser*. No
account tier, no server, and takedown-resistant persistence.

The query URL is the file API:
`https://zenodo.org/api/records/<id>/files/<name>/content`. It answers a range
request with `206` and `Access-Control-Allow-Origin: *` and allows `Range` in
the preflight, but its `Access-Control-Expose-Headers` lists only
`Content-Type`, `ETag`, `Link` and the rate-limit headers, so a browser cannot
read `Content-Range`. That costs nothing (requirement 3 above): the reader
takes the length from the file's own header and confirms it with a 4-byte
footer read, and the playground labels the source with the file name rather
than the URL's trailing `content`. `tests/gate/checks/check_zenodo_host.mjs`
serves a file with exactly that header set, in both browser engines, so this
stays true without depending on zenodo.org (whose API rate-limits with `403`).

The playground's `wikidata-zenodo` entry still serves an R2 twin of record
21168821, for a different reason: the deposited file is pre-1.0 format
(header byte `0x04`), which current readers refuse; it has to be rebuilt to
`0x05` and re-deposited before the DOI itself can be the query URL.

Upload through the normal Zenodo web UI or REST API.

### GitHub Pages / raw.githubusercontent.com

Both serve ranges and send `Access-Control-Allow-Origin: *`, so a small `.rete`
can live right next to your project site (this repo's own docs site hosts demo
files that way). The constraint is GitHub's **100 MB per-file limit** — fine
for demos and small graphs, not for the big ones.

### S3 / GCS / any static host

Standard object stores work out of the box for the CLI; for browsers, attach
the same kind of CORS policy as the R2 recipe (S3 and GCS both take a JSON CORS
configuration; make sure `GET` + `HEAD` are allowed; exposing `Content-Range`
saves one request per open). For local development, the bundled dev server
speaks ranges:

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
