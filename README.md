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
  UNION, MINUS, FILTER, property paths, GROUP BY / aggregates, named graphs.
- **Small + fast to open.** Compressed (~zstd) with indexes prebuilt, so it
  *opens in ~20 ms* — no load/index step. (See [benchmarks](#how-fast).)
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
- You need an **always-on SPARQL endpoint** with a mature, lazy/streaming planner
  squeezing every millisecond (e.g. [Oxigraph](https://github.com/oxigraph/oxigraph),
  Jena, GraphDB). rete is competitive but optimized for *publish-and-query*, not
  server throughput — see the honest [comparison](https://caviri.github.io/rete/BENCHMARK.html#comparison-vs-oxigraph-real-opencitations-network).

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

### Try it in your browser (no install)

Open **[the playground](https://caviri.github.io/rete/playground.html)** — a self-contained page
that embeds the WASM engine and example datasets (including a real ~588k-triple
**OpenCitations** network). Pick a dataset, run the **Easy → Hard** example
queries, visualise the ontology, and explore reachability — all offline.

## How fast?

Real **OpenCitations** citation network (539,246 sanitized triples from the
~588k-triple playground dataset), vs
[Oxigraph](https://github.com/oxigraph/oxigraph) (in-memory), in the dev
container — full writeup in [Benchmarks](https://caviri.github.io/rete/BENCHMARK.html):

| | rete | Oxigraph |
|---|--:|--:|
| **Open / load the graph** | **16 ms** (indexes prebuilt in the file) | 2,415 ms (parse + index) |
| SPARQL queries (24 operators) | tens of ms · **24/24 results identical** | tens of ms |
| Batch reachability, 300 seeds | 455 ms → **34 ms** with `--parallel` | 2,105 ms |

rete opens ~150× faster and its **parallel reachability is ~61× faster** than a
general property-path. Oxigraph's broader lazy planner still wins some selective
and early-out workloads; rete now has lazy fast paths for simple BGP/FILTER
`LIMIT` shapes, but ORDER BY, DISTINCT, aggregation, and many algebra forms still
materialize intermediate rows. File size: ~11.8× smaller than raw N-Triples,
~1.25× of `gzip` — but *queryable*.

## Documentation

- **[Graph data 101](https://caviri.github.io/rete/intro.html)** — new to RDF/graphs? Start here.
- **[Getting started](https://caviri.github.io/rete/getting-started.html)** · **[Architecture](https://caviri.github.io/rete/architecture.html)** · **[CLI reference](https://caviri.github.io/rete/cli.html)** · **[SPARQL support](https://caviri.github.io/rete/sparql.html)** · **[SHACL validation](https://caviri.github.io/rete/shacl.html)** · **[Dataset Cards](https://caviri.github.io/rete/dataset-cards.html)**
- **[Interactive playground](https://caviri.github.io/rete/playground.html)** — query in the browser, offline.
- **[Real-world scenario](https://caviri.github.io/rete/scenario.html)** · **[Federated queries](https://caviri.github.io/rete/federation.html)** · **[Reasoning & coherence](https://caviri.github.io/rete/reasoning.html)**
- **[Format spec](https://caviri.github.io/rete/SPEC.html)** · **[Benchmarks](https://caviri.github.io/rete/BENCHMARK.html)** · **[Browser / WASM](https://caviri.github.io/rete/browser.html)**

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
```

CI runs fmt, clippy, the test matrix, the CLI smoke test, and the WASM build in
containers, so nothing builds on the host. See
[Getting started](https://caviri.github.io/rete/getting-started.html) for the dev-container setup.

## License

[Apache-2.0](LICENSE).
