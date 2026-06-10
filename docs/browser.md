# Browser / WASM

`crates/rete-wasm` compiles the **same** engine (dictionary, permutation indexes,
SPARQL, zstd decode) to WebAssembly, so a web page queries a `.rete` file
client-side with no server. `web/index.html` is a working serverless explorer.

## Build

```sh
wasm-pack build crates/rete-wasm --target web --out-dir ../../web/pkg
wasm-pack build crates/rete-wasm --target no-modules --out-dir ../../web/pkg-nomodules
rete build examples/typed.nt -o web/typed.rete   # ontology demo (People & Orgs)
rete build examples/deps.nt  -o web/deps.rete    # CVE-impact demo (dependsOn+)
uv run python scripts/build_playground.py
python3 scripts/range_server.py 8000 web          # open http://localhost:8000
```

zstd's C encoder isn't used on wasm; decoding uses the pure-Rust `ruzstd`, so the
browser reads compressed files fine. `rete-wasm` depends on `rete-core` with
`--no-default-features`.

## JS API

All functions take the file bytes (`Uint8Array`) and return JSON strings.

| Function | Returns |
|---|---|
| `info(bytes)` | `{ quads, terms, pyramidLevels, namedGraphs }` |
| `graph_names(bytes)` | array of named-graph IRIs |
| `query_triples(bytes, s?, p?, o?)` | `[[s,p,o], …]` (omit a position for a wildcard) |
| `why_triples(bytes, s?, p?, o?)` | `{ pattern, resultCount, results:[{ terms, ids, provenance }] }` for triple-pattern provenance |
| `query_sparql(bytes, query)` | SELECT-only compatibility wrapper; array of solution objects `{ var: value, ... }` |
| `schema(bytes)` | `{ classes: [["<iri>",count]], relations: [["s","p","o",count]] }` |
| `header_ranges(headerBytes)` | `{ dictOffset, dictLen, pyramidOffset, pyramidLen, indexOffset, indexLen }` |
| `summary_overview(bytes)` | `{ round, communities, predicateTotals: [["<iri>",count]] }` |
| `progressive_query(bytes, query)` | SELECT/ASK envelope for summary-safe COUNT/ASK shapes, plus `progressive` metadata |
| `query(bytes, query, format)` | any SPARQL form, tagged by `kind` (see below) |
| `communities(bytes, round?)` | `[{ community, size, triples }, …]` (Louvain decomposition) |
| `reach(bytes, predicate, seeds, reverse)` | `[{ seed, count, reached:["<iri>",…] }, …]` (serial transitive reach) |

`query` runs SELECT / ASK / CONSTRUCT / DESCRIBE via `eval_query` and returns a
single JSON envelope with a `kind` field:

- SELECT → `{ "kind":"select", "vars":[…], "rows":[ {var:value,…} ] }`
- ASK → `{ "kind":"ask", "boolean": true|false }`
- CONSTRUCT/DESCRIBE → `{ "kind":"construct", "format":"ttl"|"jsonld", "text":"…" }`
  when `format` is `"ttl"`/`"jsonld"`, else `{ "kind":"construct", "triples":[[s,p,o],…] }`.

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

`why_triples` exposes the same result-provenance path as `rete why`. It resolves
the optional triple pattern through `Rete::query_with_provenance` and returns
browser-style camelCase fields: `resultCount`, `matchedPattern`,
`indexPermutation`, `indexSection`, `dictionaryRange`, `indexRange`,
`indexSectionRange`, and `pyramidRange`. `indexRange` is the full permutation
container; `indexSectionRange` is the selected SPO/POS/OSP payload inside it.
Tile provenance reports the physical tile for tiled (v0.2) files —
`{ "available": true, "id": "SPO/3", "range": { … } }` — and is explicit when
a pre-tiling file cannot provide one:
`{ "available": false, "reason": "not_materialized" }`.

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
browser: read bytes `0..128`, learn where the dictionary and pyramid summary
live, range-fetch **only those**, and compute the coarse graph — the large triple
index is never downloaded.

```js
import init, { header_ranges, summary_overview } from "./pkg/rete_wasm.js";
await init();

const range = async (off, len) => new Uint8Array(await (await fetch(url, {
  headers: { Range: `bytes=${off}-${off + len - 1}` }
})).arrayBuffer());

const total  = +(await fetch(url, { method: "HEAD" })).headers.get("content-length");
const header = await range(0, 128);
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
