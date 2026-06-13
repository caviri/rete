<p align="center">
  <img src="docs/img/logo.svg" alt="rete — a queryable RDF graph in a single file" width="520">
</p>

<p align="center">
  <b>Put an RDF graph in one file. Drop it on a URL. Query it with SPARQL — no database server.</b>
</p>

---

## What is rete?

The name comes from Latin **rēte**, meaning "net". Pronounce it **RAY-teh**
(Classical Latin: roughly `/ˈreː.te/`).

`rete` packs a whole RDF graph — its dictionary, indexes, and a community
"summary pyramid" — into **one immutable `.rete` file**. Put that file on S3,
GitHub Pages, or any HTTP host that supports range requests, hand a client the
URL, and it runs **real SPARQL** against the file in place, fetching only the
bytes a query needs. The same engine compiles to WebAssembly, so a **browser can
query the file directly with no backend**.

> Think **Parquet** (for tables) or **PMTiles** (for maps) — but for **RDF
> graphs + SPARQL**.

- **No server.** The file *is* the database. Publish once to static hosting.
- **Query in place.** SELECT / ASK / CONSTRUCT / DESCRIBE, joins, OPTIONAL,
  UNION, MINUS, FILTER, subqueries, property paths, GROUP BY / aggregates, XSD
  casts and the SPARQL 1.1 function library, named graphs — **[~75% of the W3C
  query-evaluation suite](https://caviri.github.io/rete/conformance.html)** (≈89%
  excluding the RDFS/OWL-entailment and SERVICE-federation regimes rete leaves
  out by design).
- **Small + fast to open.** Compressed (~zstd) with indexes prebuilt, so it
  *opens in ~15 ms* — no load/index step. (See [benchmarks](#how-fast).)
- **Runs in the browser.** Same engine in WASM — try the
  **[interactive playground](https://caviri.github.io/rete/playground.html)** (a single offline HTML page).

## When does rete make sense?

✅ **A good fit when…**
- You want to **publish a graph dataset** and let people query it without
  standing up (and paying for) a triplestore — just static hosting + a URL.
- The data is **read-mostly / versioned snapshots** (a release, a daily dump, a
  knowledge-base export) rather than constantly mutated.
- You need **SPARQL in the browser** / at the edge / in a notebook, with no
  backend.
- You ship **many graphs** (per-tenant, per-dataset, sharded by year) and want
  each to be one cheap, cacheable, queryable artifact — even
  [federated across several files](https://caviri.github.io/rete/federation.html).
- You care about **bounded, progressive reads** — fetch a coarse overview first,
  drill into detail only where a query needs it (PMTiles-style, for graphs).

🚫 **Reach for a real triplestore instead when…**
- You need **frequent writes / transactions / live updates** (rete files are
  immutable — you rebuild to change them).
- You need an **always-on SPARQL endpoint** with a mature, battle-tested planner
  (e.g. [Oxigraph](https://github.com/oxigraph/oxigraph), Jena, GraphDB). rete's
  engine now wins or ties most benchmark shapes against Oxigraph, but it is
  optimized for *publish-and-query*, not server throughput — see the honest
  [comparison](https://caviri.github.io/rete/BENCHMARK.html#comparison-vs-oxigraph-real-opencitations-network).

## Quick start

Everything runs **in Docker** (nothing builds on your host):

```sh
# Build the repo dev image once, then build the CLI inside it:
docker build -t rete-dev -f .devcontainer/Dockerfile .
docker run --rm -it -v "${PWD}:/work" -w /work rete-dev \
  cargo build --release -p rete-cli       # binary at target/release/rete
```

```sh
# 1. Build a .rete file from N-Triples / Turtle / N-Quads:
rete build examples/social.nt -o social.rete

# 2. Query it — a triple pattern, or full SPARQL:
rete query  social.rete --predicate '<http://ex/knows>'
rete why    social.rete --predicate '<http://ex/knows>'   # explain result provenance
rete sparql social.rete "PREFIX e: <http://ex/> \
  SELECT ?p ?age WHERE { ?p e:age ?age . FILTER(?age > 27) }"

# 3. Or query straight from a URL — fetches only the byte ranges it needs:
rete query-url https://my-bucket.s3.amazonaws.com/social.rete \
  --object '<http://ex/Alice>'
```

`query-url` resolves bound terms from the dictionary, then range-fetches only
the selected SPO/POS/OSP permutation payload for that triple pattern. Full
SPARQL-over-URL still uses the broader ranged open path; `rete cost --explain`
shows when a query can use the summary-only or routed-pattern budgets.

### Try it in your browser (no install)

Open **[the playground](https://caviri.github.io/rete/playground.html)** — a self-contained page
that embeds the WASM engine and example datasets (including a real ~588k-triple
**OpenCitations** network). Pick a dataset, run filterable SPARQL examples,
inspect progressive exactness metadata, validate SHACL, explore reachability,
review schema summaries, and explain triple-pattern provenance — all offline.

## How fast?

Real **OpenCitations** citation network (539,246 sanitized triples from the
~588k-triple playground dataset), vs
[Oxigraph](https://github.com/oxigraph/oxigraph) (in-memory), in the dev
container — full writeup in [Benchmarks](https://caviri.github.io/rete/BENCHMARK.html):

| | rete | Oxigraph |
|---|--:|--:|
| **Open / load the graph** | **16 ms** (indexes prebuilt in the file) | ~2,200 ms (parse + index) |
| SPARQL queries (24 operators) | **wins or ties 20/24** · 24/24 results identical | sub-ms to tens of ms |
| Batch reachability, 300 seeds | 453 ms → **36 ms** with `--parallel` | 2,591 ms |

rete opens ~100–160× faster, and after the 2026-06 engine rework (lazy
slot-row pipeline, adaptive index-nested-loop joins, top-k ORDER BY) it wins
or ties most SPARQL shapes — aggregates, GROUP BY, DISTINCT, OPTIONAL,
UNION/VALUES, paths, sorted pagination. Oxigraph keeps a fractional edge on
ASK, the tightest LIMIT joins, and non-literal REGEX scans. File size: ~11.8×
smaller than raw N-Triples, ~1.25× of `gzip` — but *queryable*.

## Documentation

- **[Graph data 101](https://caviri.github.io/rete/intro.html)** — new to RDF/graphs? Start here.
- **[Getting started](https://caviri.github.io/rete/getting-started.html)** · **[Architecture](https://caviri.github.io/rete/architecture.html)** · **[CLI reference](https://caviri.github.io/rete/cli.html)** · **[SPARQL support](https://caviri.github.io/rete/sparql.html)** · **[SHACL validation](https://caviri.github.io/rete/shacl.html)** · **[Dataset Cards](https://caviri.github.io/rete/dataset-cards.html)**
- **[Interactive playground](https://caviri.github.io/rete/playground.html)** — query in the browser, offline.
- **[Real-world scenario](https://caviri.github.io/rete/scenario.html)** · **[Federated queries](https://caviri.github.io/rete/federation.html)** · **[Reasoning & coherence](https://caviri.github.io/rete/reasoning.html)**
- **[Format spec](https://caviri.github.io/rete/SPEC.html)** · **[Benchmarks](https://caviri.github.io/rete/BENCHMARK.html)** · **[SPARQL conformance](https://caviri.github.io/rete/conformance.html)** · **[Browser / WASM](https://caviri.github.io/rete/browser.html)**

The docs render as Markdown on GitHub, or as an HTML site (`docs/*.html`,
regenerated with `cargo run -p docgen`).

## Status

**Experimental (v0).** Working end-to-end — single-file format, community
pyramid, SPARQL, HTTP-range queries, and the browser/WASM engine — but the file
format is **not yet stable across versions**. Format version `0.1` (draft).

## Develop (Docker only)

```sh
cargo test                 # round-trip, malformed-input robustness, range access, SPARQL
cargo run -p rete-cli -- info some.rete
bash scripts/smoke.sh      # end-to-end test of every CLI subcommand
uv run python scripts/build_playground.py
```

CI runs fmt, clippy, the test matrix, the CLI smoke test, a query-engine
regression check (`qbench --check`: per-query row counts + time ceilings), and
the WASM build in containers, so nothing builds on the host. See
[Getting started](https://caviri.github.io/rete/getting-started.html) for the dev-container setup.

## License

[Apache-2.0](LICENSE).
