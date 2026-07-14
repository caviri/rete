# WASM & JavaScript API

`crates/rete-wasm` compiles the **same** engine (dictionary, permutation indexes,
SPARQL, zstd decode) to WebAssembly, so a web page queries a `.rete` file
client-side with no server. `web/index.html` is a working serverless explorer.

## Build

```sh
docker compose build wasm
docker compose run --rm wasm
```

This runs `scripts/build_wasm.sh`: it builds `web/pkg`, `web/pkg-nomodules`, and
`web/pkg-nomodules-async` from one checkout, regenerates the playground, and
writes [wasm-build.json](wasm-build.json) with the source revision, pinned tool
versions, sizes, and SHA-256 digests. The gitignored embedded `.rete` datasets
listed in `scripts/build_playground.py` must already be staged under `web/`.

When the checkout is a Windows Git worktree mounted into Linux, pass the
revision because the worktree's host `.git` indirection is not visible:

```sh
docker compose run --rm -e RETE_SOURCE_REVISION=$(git rev-parse HEAD) wasm
```

The image carries Binaryen v108 for the Asyncify transform. That version
corrupts modern wasm-bindgen extern-reference tables, so regular `web` and
`no-modules` builds deliberately use `wasm-pack --no-opt`; Rust's release
profile still optimizes them. The Asyncify build disables reference types
before its explicit, pinned Binaryen transform.

zstd's C encoder isn't used on wasm; decoding uses the pure-Rust `ruzstd`, so the
browser reads compressed files fine. `rete-wasm` depends on `rete-core` with
`--no-default-features`.

## Stable JS API

The 1.0 stable surface is `Graph`, `RemoteGraph`, `build`, `query`,
`query_sparql`, `header_ranges`, `summary_overview`, and the validation and
reasoning functions documented below. `reach_parallel` and the `threads`
feature are experimental; the Asyncify artifact is an alternative transport,
not a different graph/query API.

Functions take file bytes as `Uint8Array` and return JSON strings. Every
Rete-owned JSON **object** envelope includes `"schemaVersion": 1`; compatibility
functions that return a bare array (`graph_names`, `query_triples`,
`query_sparql`, searches, communities, and reachability) remain arrays.
Binding failures throw JavaScript `Error` objects rather than strings.

| Function | Returns |
|---|---|
| `info(bytes)` | `{ schemaVersion, quads, terms, pyramidLevels, namedGraphs }` |
| `graph_names(bytes)` | array of named-graph IRIs |
| `query_triples(bytes, s?, p?, o?)` | `[[s,p,o], …]` (omit a position for a wildcard) |
| `why_triples(bytes, s?, p?, o?)` | `{ pattern, resultCount, results:[{ terms, ids, provenance }] }` for triple-pattern provenance |
| `query_sparql(bytes, query)` | SELECT-only compatibility wrapper; array of solution objects `{ var: value, ... }` |
| `prefix_search(bytes, prefix, limit)` | label-prefix autocomplete from the label index: `[{ label, subject }]` (no literal scan) |
| `text_search(bytes, words, prefix, limit)` | full-text (`--text-index`) word/AND search: `[{ subject }]`; `words` is a JSON array string, `prefix` an optional token-prefix |
| `schema(bytes)` | `{ classes: [["<iri>",count]], relations: [["s","p","o",count]] }` |
| `shacl(bytes, shapes, graph?, format?)` | SHACL Core validation against Turtle `shapes`; report as text/json/ttl per `format` |
| `file_layout(bytes)` | the physical section map (offsets/lengths of header, dictionary, index, pyramid, …) behind the playground's layout view |
| `header_ranges(headerBytes)` | `{ schemaVersion, dictOffset, dictLen, pyramidOffset, pyramidLen, indexOffset, indexLen }` |
| `summary_overview(bytes)` | `{ schemaVersion, round, communities, predicateTotals: [["<iri>",count]] }` |
| `progressive_query(bytes, query)` | SELECT/ASK envelope for summary-safe COUNT/ASK shapes, plus `progressive` metadata |
| `query(bytes, query, format)` | any SPARQL form, tagged by `kind` (see below) |
| `communities(bytes, round?)` | `[{ community, size, triples }, …]` (Louvain decomposition) |
| `query_communities(bytes, query, round?)` | a SELECT evaluated with the community-split strategy (stars per community, global joins — exact rows), plus `communities: [{ community, subjects, rows }]` |
| `pyramid_tree(bytes)` | the full community pyramid: per dendrogram round, every community's node/triple counts and its parent at the next-coarser round |
| `reach(bytes, predicate, seeds, reverse)` | `[{ seed, count, reached:["<iri>",…] }, …]` (serial transitive reach) |
| `build(text, format)` | a complete `.rete` file image (`Uint8Array`) built from RDF text |
| `sparql_url(url, query, format)` | **worker-only**: the `query` envelope evaluated against a remote URL via lazy HTTP range reads, plus `remote: { fileLength, bytes, requests }` |
| `why_url(url, s?, p?, o?)` | **worker-only**: triple-pattern provenance over a remote URL (`why_triples` lazily), plus `remote:{…}` |
| `reach_url(url, predicate, seeds, reverse)` | **worker-only**: transitive reachability over a remote URL, faulting only the traversed tiles, plus `remote:{…}` |
| `schema_url(url)` | **worker-only**: the Schema view (`classes`/`relations`) from a remote file's schema pyramid, plus `remote:{…}` |
| `shacl_url(url, shapes, format?)` | **worker-only**: lazy SHACL — validates the remote default graph, range-reading only each shape's targets, plus `remote:{…}` |
| `shacl_construct_url(url, shapes, construct)` | **worker-only**: SHACL over just the subgraph a CONSTRUCT selects (only its tiles fetched), plus `remote:{…}` |
| `new Graph(bytes)` → `.query`, `.query_triples`, `.prefix_search`, `.text_search`, `.why_triples`, `.schema`, `.reach`, `.shacl`, `.reason`, `.query_communities`, `.pyramid_tree`, `.file_layout`, `.info`, `.graph_names` | a file opened **once** and kept resident in memory, so repeated calls reuse the decoded dictionary/index — the stateful local mirror of the free functions |
| `new RemoteGraph(url)` → `.query(query, format)`, `.prefix_search`, `.text_search`, `.stats()`, `.content_hash()` | **worker-only**: a remote URL opened **once** and kept resident, so repeated queries reuse the block cache + faulted tiles + decoded dictionary (see [Caching remote reads](#caching-remote-reads)) |
| `reason(bytes, graph?)` | OWL RL / RDFS coherence over an in-memory graph: `{ kind:"reasoning", coherent, inferredCount, inconsistencies:[{kind,detail}] }` |
| `check_schema(bytes)` | index-free Tier-0 schema coherence: `{ kind:"schemaCoherence", coherent, schemaPoints:[{kind,detail}], readsIndex:false }` |
| `check_schema_url(url)` | **worker-only**: Tier-0 schema coherence over a remote URL from ~2–3 ranges (header + dictionary + pyramid-meta, never the triple index), plus `remote:{…}` |
| `reason_construct_url(url, construct)` | **worker-only**: Tier-1 *selective* coherence — reason over just the subgraph a CONSTRUCT selects (only its tiles are fetched), plus `remote:{…}` |
| `reason_url(url, graph?)` | **worker-only**: Tier-2 *full* coherence over a remote URL (materializes the whole graph), plus `remote:{…}` |

`query` runs SELECT / ASK / CONSTRUCT / DESCRIBE via `eval_query` and returns a
single JSON envelope with a `kind` field:

- SELECT → `{ "schemaVersion":1, "kind":"select", "vars":[…], "rows":[ {var:value,…} ] }`
- ASK → `{ "schemaVersion":1, "kind":"ask", "boolean": true|false }`
- CONSTRUCT/DESCRIBE → `{ "schemaVersion":1, "kind":"construct", "format":"ttl"|"jsonld", "text":"…" }`
  when `format` is `"ttl"`/`"jsonld"`, else
  `{ "schemaVersion":1, "kind":"construct", "triples":[[s,p,o],…] }`.

`communities` recomputes the Louvain community decomposition (optionally at a
given dendrogram `round`) and returns per-community member and triple counts —
the data behind the playground's "split by community" view.

`reach` computes multi-source transitive reachability over one `predicate`.
`seeds` is a JSON array string of seed IRI tokens (e.g. `'["<http://ex/app>"]'`);
a single bare IRI is also accepted. With `reverse=true` it traverses edges
backward ("who reaches the seed?" — impact analysis). It returns one entry per
seed in input order: `{ seed, count, reached }`, or `{ seed, error }` for a seed
not in the graph (so one unknown seed never fails the whole call). It runs
**serially** — the browser engine is single-threaded; the native CLI's
`rete reach --parallel` fans one task per seed for a real speedup.

`build` is the ingest path in reverse direction from everything above: it takes
RDF *text* (`format`: `"nt"` N-Triples, `"nq"` N-Quads — named graphs become a
dataset — or `"ttl"` Turtle) and assembles a complete `.rete` file image in the
browser: dictionary, permutation indexes, and the community pyramid. The bytes
it returns are immediately queryable by every other function, and downloadable
as a file. One caveat: the wasm engine ships only the pure-Rust zstd *decoder*,
so in-browser builds write uncompressed sections (codec `NONE`) — every reader
accepts them, but `rete build` produces a smaller file from the same input.
This powers the playground's **Build** tab.

`sparql_url` runs full SPARQL against a **remote `.rete` URL without
downloading it**: it reads the header, the dictionary chunk directories and
index tile directories, then faults in only the dictionary chunks and index
tiles the query touches — and full scans coalesce adjacent tiles into batched
range reads, so even `?s ?p ?o` costs a handful of requests, not one per
tile. The result envelope is the same as `query`, plus a `remote` object
reporting exactly how little of the file was fetched.

The design constraint, honestly: the engine is synchronous, and wasm cannot
block on `fetch`. Instead of an async engine refactor, the byte-range reads
use **synchronous XHR — which browsers permit only inside Web Workers**. So
call `sparql_url` from a worker (see `web/sparql-url-worker.js` and the
"Remote SPARQL" section of the demo page); on the main thread the browser
throws. The host must answer `Range` requests with `206 Partial Content`
(a host that ignores `Range` is rejected loudly, never silently mis-read)
and send CORS headers when cross-origin. A range fetch that fails mid-query
is an error — never a silently incomplete result.

The length probe uses a one-byte ranged `GET` (reading the total from
`Content-Range`) rather than `HEAD`, since some hosts reject `HEAD` —
notably Hugging Face's signed-redirect storage, which answers `405`.

## Caching remote reads

Range reads are **cached**, so re-running or refining a query on a remote dataset
re-fetches almost nothing. Two layers:

- **Within a query — a block cache.** The lazy reader wraps the raw HTTP-range
  backend in a `BlockCacheReader`: every read is served from a 64 KiB aligned
  block, fetched once and kept. A query's scattered tile reads that fall in the
  same block cost a single fetch (and a multi-range host coalesces the block
  fetches further). This works over any single-range backend — S3, a CDN — not
  just a multi-range gateway.
- **Across queries — a resident session.** `RemoteGraph` opens a URL once and
  keeps the `Rete` resident, so the block cache **and** the faulted index tiles
  **and** the decoded dictionary chunks all survive between queries. The
  playground's worker holds one `RemoteGraph` per URL, so exploring a remote
  dataset — refining a filter, paging entity tables, re-running — reuses
  everything already fetched: a fully cached re-run fetches **0 bytes**, and the
  result line shows *"served from cache, 0 new bytes."* (The free `sparql_url`
  opens a fresh file each call, so it gets only the within-query block cache —
  use `RemoteGraph` for cross-query reuse.)

`RemoteGraph.stats()` returns the session's cumulative `{ schemaVersion,
fileLength, bytes, requests }`; the worker diffs successive calls to report one
query's physical traffic versus the running session total.

- **Across reloads and sessions — an opt-in persistent range cache.** The
  playground's **Settings → Persist fetched ranges across reloads** toggle installs
  a tiny `XMLHttpRequest` shim in each engine worker that mirrors fetched bytes
  into IndexedDB in 1 MiB blocks (keyed by the file's origin + path) and warms them
  back on the next load — so a reload re-fetches nothing already held, and the
  cache survives browser sessions until cleared. Settings shows a **per-file
  breakdown** — each cached `.rete` with how much of it is held (e.g. *16 MB /
  1.04 GB · 1.6%*) and a fill bar — plus per-file and global **Clear**. It is off
  by default, so the default read path is byte-identical with the shim absent.

**Host CORS, in practice.** Range-querying from the browser needs a host that
serves the bytes *directly* to a cross-origin browser request. A plain static
server with CORS works; an S3/R2/GCS bucket with CORS works; same-origin always
works. What does **not** work is Hugging Face's `buckets/.../resolve` endpoint:
it returns `405` to a cross-origin browser `GET` even though it serves the
`rete` CLI fine (the CLI sends no `Origin`). The bytes themselves are reachable
— the resolved signed CDN URL answers `206` to the browser — but the `resolve`
hop refuses browser requests, so the lazy backends can't follow it. A client
should probe its data URL on load and surface a clear banner when the
host isn't browser-reachable; point at a CORS-enabled direct host to light up
the remote backends.

**Production catalog contract.** Playground datasets are served directly from
`https://data.graphplaza.com` (Cloudflare R2), one folder per dataset, without a
redirect or token. Every published `.rete` must use stable format generation 1
(header byte `0x05`) and expose `Content-Range`, `Content-Length`,
`Accept-Ranges`, and `ETag` to CORS callers. `web/datasets.lock.json` pins each
catalog object's size, content hash, and format byte. Run
`uv run python scripts/check_dataset_catalog.py --all` before a release to probe
all URLs with a browser-style Range request and verify the lock.

**Parallel range reads (opt-in).** Sequential synchronous XHR serialises a
query's round trips. With cross-origin isolation the explorer can read the
faulted ranges in parallel: a pool of fetch workers pulls them (each a
synchronous XHR, parallel across the pool) into a `SharedArrayBuffer`,
`Atomics`-coordinated, and the engine blocks until they land — `read_at` falls
back to sequential when isolation is unavailable, so there is never a
regression. Static hosts don't send COOP/COEP, so it is **opt-in via
`?parallel=1`** (a bundled `coi-serviceworker.js` injects the headers and the
page reloads once); the default page stays un-isolated so the cross-origin
DuckDB-WASM / SQLite backends keep working.

`why_triples` exposes the same result-provenance path as `rete why`. It resolves
the optional triple pattern through `Rete::query_with_provenance` and returns
browser-style camelCase fields: `resultCount`, `matchedPattern`,
`indexPermutation`, `indexSection`, `dictionaryRange`, `indexRange`,
`indexSectionRange`, and `pyramidRange`. `indexRange` is the full permutation
container; `indexSectionRange` is the selected permutation payload inside it.
Tile provenance reports the physical tile holding each match —
`{ "available": true, "id": "SPO/3", "range": { … } }`.

### Minimal example

```js
import init, { info, query_sparql } from "./pkg/rete_wasm.js";
await init();
const bytes = new Uint8Array(await (await fetch("/data.rete")).arrayBuffer());
console.log(JSON.parse(info(bytes)));
const rows = JSON.parse(query_sparql(bytes,
  `PREFIX e: <http://ex/> SELECT ?y WHERE { e:Alice e:knows ?y }`));
```

## Progressive loading (overview without the index)

<img src="img/progressive-fetch.svg" alt="A client issues three small range requests for the header, dictionary, and pyramid summary; the large index block is greyed out and never fetched.">

*Three small range requests (header + dictionary + summary, ~25% of the file) build the coarse graph; the large triple index is never downloaded.*

`header_ranges` + `summary_overview` implement the "overview first" path in the
browser: read the 1 KB header (bytes `0..1024`), learn where the dictionary and
pyramid summary live, range-fetch **only those**, and compute the coarse graph —
the large triple index is never downloaded.

```js
import init, { header_ranges, summary_overview } from "./pkg/rete_wasm.js";
await init();

const range = async (off, len) => new Uint8Array(await (await fetch(url, {
  headers: { Range: `bytes=${off}-${off + len - 1}` }
})).arrayBuffer());

const total  = +(await fetch(url, { method: "HEAD" })).headers.get("content-length");
const header = await range(0, 1024);
const r      = JSON.parse(header_ranges(header));

const buf = new Uint8Array(total);                 // index region left zero
buf.set(header, 0);
buf.set(await range(r.dictOffset, r.dictLen), r.dictOffset);
buf.set(await range(r.pyramidOffset, r.pyramidLen), r.pyramidOffset);

const overview = JSON.parse(summary_overview(buf)); // index never fetched
```

This is the same path as `rete summary-url` natively. It's verified end-to-end in
`rete-wasm`'s Node test: with the index region zero-filled, the overview still
computes — typically ~25 % of the file fetched in 3 ranges.

`progressive_query` uses the same summary-only path for query answering. It is
intentionally conservative and returns an error unless the query is exactly one
of these shapes:

- `SELECT (COUNT(*) AS ?n) WHERE { ?s <predicate> ?o }`
- `SELECT (COUNT(*) AS ?n) WHERE { ?s ?p ?o }`
- `SELECT ?p (COUNT(*) AS ?n) WHERE { ?s ?p ?o } GROUP BY ?p`
- `SELECT DISTINCT ?p WHERE { ?s ?p ?o }`
- `SELECT (COUNT(DISTINCT ?p) AS ?n) WHERE { ?s ?p ?o }`
- `ASK { ?s ?p ?o }`
- `ASK { ?s <predicate> ?o }`

Successful responses reuse the normal `query` envelopes and add
`progressive`, for example:

```json
{
  "schemaVersion": 1,
  "kind": "select",
  "vars": ["n"],
  "rows": [{ "n": "\"42\"^^<http://www.w3.org/2001/XMLSchema#integer>" }],
  "progressive": {
    "stage": "summary",
    "exact": true,
    "readsIndex": false,
    "queryShape": "predicate_count",
    "bytes": 9182,
    "requests": 3,
    "fileBytes": 37210
  }
}
```

## The demo page / playground

`docs/playground.html` is the static console build. It is generated from
`web/playground.template.html` plus the source fragments in
`web/playground-src/`, then inlines the no-modules WASM glue, WASM bytes, and
bundled `.rete` datasets. It opens directly from `file://`, defaults to SPARQL,
and keeps SHACL, reachability, schema, and provenance modes available without a
runtime server or bundler. The WASM initializer receives embedded bytes; the
generator removes wasm-bindgen's URL/fetch fallback so app boot cannot silently
go to the network.

Beyond the bundled datasets it also opens **remote** `.rete` files lazily over
HTTP range (a 120 MB / 1 GB graph stays interactive because only the touched
tiles cross the wire), caches those reads across queries (above), and federates a
query across **several** sources via the SPARQL console's **+ Add source** button
— see [Federated queries](federation.md#in-the-playground).

### Find a term

The console's **🔎 Find a term** button opens a picker so you don't have to know
a graph's IRIs by heart. It opens on the schema card's **classes and predicates**
(instant, from the resident card) and, as you type, also searches **entities by
label** — synchronously for an embedded graph, and over HTTP-range reads (with a
spinner) for a remote-lazy one, using the bounded label index. Click any result
to drop its `<IRI>` at the cursor.

Each predicate row also carries a **values ›** drill: click it for a faceted
browse of the distinct objects that predicate takes — IRIs resolved to their
human labels, literals shown verbatim — then click one to insert it into the
query. The values for a predicate are read once and cached, so re-opening is
instant; on a remote graph the read is a single bounded range query. The **Label**
selector at the top chooses which predicate is read as the human label
(`rdfs:label`, `skos:prefLabel`, `schema:name`, … or `auto`), and the same choice
drives the editor's inline **Labels** decode chips.

## Media & companions

Inline rendering of images / IIIF / 3D / audio / geo cells in playground
results — and the asset-preparation pipelines behind them — moved to their own
page: [Media & SQL companions](media-companions.md).
