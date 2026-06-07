# rete

A **cloud-native, range-queryable RDF graph file**. Put one `.rete` file on S3,
GitHub, or any HTTP server that honors `Range`, give a client the URL, and run
SPARQL — no database server. Like **Parquet** for tables and **PMTiles** for
maps, but for **RDF graphs**, with a **pyramid** of progressively-refined detail.

> **Documentation:** [`docs/`](docs/index.md) — overview, getting started, CLI
> reference, SPARQL support, browser/WASM, the format spec, and benchmarks.
> Browsable as Markdown on GitHub, or as a rendered HTML site (`docs/*.html`,
> regenerated with `cargo run -p docgen`). Format version `0.1` (draft).

## Status

Working v0 end-to-end (single file, community pyramid, SPARQL, HTTP range,
browser/wasm):

- `crates/rete-core` — `varint`, `header` (§4.1), `dict` (front-coded sections
  §5.1), `dictionary` (HDT four-section role IDs §5), `triples` (grouped-delta
  blocks + zone maps §6.1), `index` (SPO/POS/OSP permutation set §6), and `file`
  (assemble/read a `.rete` image + `query` by triple pattern). All under test.
- `crates/rete-cli` — `rete build in.{nt,ttl,nq} -o out.rete` (N-Triples,
  Turtle, or N-Quads datasets), `rete info`, `rete query`, plus `verify`,
  `summary`, `predicates`, `graphs`, `bgp`, `sparql` (incl. `GRAPH`), `query-url`.
- **RDF datasets / named graphs**: N-Quads build groups triples per graph under
  one shared dictionary; SPARQL `GRAPH <iri> { … }` / `GRAPH ?g { … }` query them.

- pyramid (SPEC §7): `pyramid` (Louvain communities + quotient coarsening),
  `tiling` (size-targeted per-community tiles + quotient summary), `meta`
  (on-disk pyramid section). `rete build` embeds it; `rete summary` shows the
  coarse community graph.
- `bgp` — Basic Graph Pattern (multi-pattern join) evaluation; `rete bgp`.
- `sparql` — SPARQL (via spargebra): SELECT · ASK · CONSTRUCT over BGP · JOIN ·
  UNION · OPTIONAL · MINUS · FILTER (incl. EXISTS, arithmetic, built-ins like
  CONTAINS/STRLEN/isIRI) · VALUES · GROUP BY/aggregates (COUNT/SUM/AVG/MIN/MAX) ·
  BIND · property paths (`p+`/`p*`/`p?`/`/`/`|`/reverse) · ORDER BY · DISTINCT ·
  LIMIT/OFFSET. `rete sparql`.
- per-section **zstd** compression (codec in the header). ~8.8× on a 3k-triple
  graph (300 KB `.nt` → 34 KB `.rete`, pyramid included).
- `reader` — `RangeReader` trait + `Rete::open_ranged` (a full query touches ≤4
  byte ranges, never a linear scan) and `SummaryView::open_ranged` (coarse graph
  first, skips the index). The "give it a URL, fetch only what you need" path.
- `schema` — an **ontology-level** coarse graph: classes (by `rdf:type`) with
  their populations, plus the class→predicate→class relations between them. A
  *semantic* summary (what kinds of things relate how), complementary to the
  community pyramid's *structural* one. `rete schema`.

```sh
rete build examples/social.nt -o social.rete
rete build part1.nt part2.nt part3.nt -o merged.rete   # merge several inputs
curl -s https://host/data.nt | rete build - -o data.rete   # build from stdin
rete query social.rete --predicate '<http://ex/knows>'    # all "knows" edges
rete query social.rete --object '<http://ex/Alice>'       # who knows Alice?

rete build examples/clusters.nt -o clusters.rete
rete summary clusters.rete
#  pyramid round 0 — 2 communities summarized as 3 superedge(s):
#    C0 (internal) C0  via <http://ex/knows>  x4
#    C0 -> C1          via <http://ex/knows>  x1   (the bridge)
#    C1 (internal) C1  via <http://ex/knows>  x4

# ontology profile — the semantic coarse graph (by rdf:type), no clustering:
rete build examples/typed.nt -o typed.rete
rete schema typed.rete
#  classes (2 types):
#         2  <http://ex/Person>
#         1  <http://ex/Org>
#  relations:
#    <http://ex/Person> --<http://ex/knows>--> <http://ex/Person>  ×1
#    <http://ex/Person> --<http://ex/name>--> (literal)  ×1
#    <http://ex/Person> --<http://ex/worksAt>--> <http://ex/Org>  ×1

# multi-pattern (BGP) joins — `?x` is a variable, ` . ` separates patterns:
rete bgp clusters.rete "?x <http://ex/knows> ?y . ?y <http://ex/knows> ?z . ?z <http://ex/knows> ?x"
#  finds both knows-triangles

# real SPARQL SELECT (via spargebra) — joins, DISTINCT, LIMIT, FILTER:
rete sparql clusters.rete "PREFIX ex: <http://ex/> SELECT ?x ?z WHERE { ?x ex:knows ?y . ?y ex:knows ?z }"
rete sparql social.rete   "PREFIX ex: <http://ex/> SELECT ?p ?age WHERE { ?p ex:age ?age . FILTER(?age > 27) }"

# query straight from a URL — fetches only the needed byte ranges over HTTP(S):
python3 scripts/range_server.py 8000 .            # range-capable static server
rete query-url http://127.0.0.1:8000/clusters.rete --object '<http://ex/Dave>'
#  → answers in 4 bounded range requests, no full download

# https works too (rustls) — point it at S3, GitHub, or any CDN that honors Range:
rete query-url https://my-bucket.s3.amazonaws.com/clusters.rete --predicate '<http://ex/knows>'
```

### RDF datasets (named graphs)

Build from N-Quads — triples are grouped per graph under one shared dictionary —
then query specific graphs with SPARQL `GRAPH`:

```sh
rete build examples/dataset.nq -o dataset.rete       # default graph + named graphs
rete graphs dataset.rete                              # list named graph IRIs

# which graph holds the age triples, and whose?
rete sparql dataset.rete \
  "PREFIX ex: <http://ex/> SELECT ?g ?p ?age WHERE { GRAPH ?g { ?p ex:age ?age } }"

rete export dataset.rete > out.nq                     # dump back to N-Quads (lossless)
```

## Benchmarks

Synthetic social graph, **139k triples** (20k people in ~200 communities),
release build, dev container. Full writeup + harness in
[`docs/BENCHMARK.md`](docs/BENCHMARK.md) (`scripts/bench.sh`).

| | |
|---|--:|
| raw N-Triples | 8.38 MB |
| `gzip -9` | 565 KB |
| **`.rete`** (queryable, zstd + pyramid) | **708 KB** (1.25× gzip) |
| build | 660 ms |
| triple-pattern query | 20 ms |
| 2-hop BGP join | 27 ms |
| transitive path `knows+` (reaches whole graph) | 53 ms |
| GROUP BY COUNT (every node's degree) | 93 ms |
| per-predicate totals (summary only, index unread) | 15 ms |
| HTTP point query | 4 bounded range requests |

`.rete` is ~1.25× the size of gzip but is *queryable in place and over HTTP
ranges* — gzip answers no query without a full download + scan.

## Develop (in Docker only — nothing runs on the host)

Open the folder in a dev container (VS Code: *Reopen in Container*), then inside:

```sh
cargo test          # 94 tests: round-trip, malformed-input robustness, range
                    # access invariants, HTTP range reader, SPARQL features
cargo run -p rete-cli -- info some.rete
cargo clippy
bash scripts/smoke.sh   # end-to-end acceptance test of every CLI subcommand
```

CI ([`.github/workflows/ci.yml`](.github/workflows/ci.yml)) runs fmt, clippy, the
test matrix (default / `--no-default-features` / `--all-features`), the CLI smoke
test, and the wasm build — all inside the same `rust:1.92-bookworm` image, so
nothing ever builds on the host.

The container ([`.devcontainer/`](.devcontainer/)) carries the Rust toolchain,
the `wasm32-unknown-unknown` target, and `wasm-pack` for the future browser
client.

`rete-core` builds for the browser today (the same SPARQL/query code runs in
wasm). zstd is an optional `compression` feature for the C *encoder*; decoding
always uses the pure-Rust `ruzstd`, so the browser reads compressed files fine.

### Run SPARQL in the browser

`crates/rete-wasm` exposes the engine to JS (`info`, `schema`, `graph_names`,
`query_triples`, `query_sparql`, plus `header_ranges`/`summary_overview` for
progressive loading); `web/index.html` is a serverless explorer that fetches a
`.rete` and queries it client-side — no server-side query. It renders the
**ontology overview first** (the `schema` coarse graph: classes + their
relations), and each class/relation is clickable to drill into the data with a
generated SPARQL query, results shown as a table.

It also has a **progressive load** button: using `header_ranges` +
`summary_overview`, the page reads bytes `0..128`, then range-fetches only the
dictionary and pyramid summary and computes the coarse graph from them — the
(large) triple index is **never downloaded**. The same "overview first" path as
`rete summary-url`, in the browser. (Verified end-to-end in `rete-wasm`'s Node
test: the index region can be left zero-filled and the overview still computes —
typically ~25 % of the file fetched in 3 ranges.)

```sh
wasm-pack build crates/rete-wasm --target web --out-dir ../../web/pkg
rete build examples/typed.nt -o web/typed.rete   # ontology demo (People & Orgs)
rete build examples/deps.nt  -o web/deps.rete    # CVE-impact demo (dependsOn+ path)
python3 scripts/range_server.py 8000 web         # then open http://localhost:8000
```

## Roadmap

See `docs/SPEC.md` §8 (SPARQL status) and §11 (open questions). Next up:

- **Tile-routed queries** — use the pyramid to fetch only relevant community
  tiles over HTTP instead of the whole index (re-enables stored tiles).
- **Range-fetch the *full* query path in wasm** — the progressive *overview* is
  now range-fetched in the browser (`header_ranges` + `summary_overview`); the
  full triple-query path still loads the whole file client-side.

Done since first draft: named graphs / quads, ORDER BY, DESCRIBE, GROUP BY on
integer IDs end-to-end, the `schema` ontology profile, and progressive
overview-only loading over HTTP ranges in the browser.