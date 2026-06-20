<p align="center">
  <img src="docs/img/logo.svg" alt="rete — a queryable RDF graph in a single file" width="520">
</p>

<p align="center">
  <b>Put an RDF graph in one file. Drop it on a URL. Query it with SPARQL — no database server.</b>
</p>

<p align="center">
  <a href="https://caviri.github.io/rete/playground.html"><b>▶ Try it in your browser</b></a> ·
  <a href="https://caviri.github.io/rete/explore-100mb.html">100 MB / 1 GB explorer</a> ·
  <a href="https://caviri.github.io/rete/atlas.html">Historical atlas</a> ·
  <a href="https://caviri.github.io/rete/">Docs</a>
</p>

---

## What is rete?

The name comes from Latin **rēte**, meaning "net". Pronounce it **RAY-teh**
(Classical Latin: roughly `/ˈreː.te/`).

`rete` packs a whole RDF graph — its dictionary, permutation indexes, a community
summary, and a self-describing **schema pyramid** — into **one immutable `.rete`
file**. Put that file on S3, GitHub Pages, or any HTTP host that supports range
requests, hand a client the URL, and it runs **real SPARQL** against the file in
place, **fetching only the bytes a query needs**. The same engine compiles to
WebAssembly, so a **browser can query the file directly with no backend**.

> Think **Parquet** (for tables) or **PMTiles** (for maps) — but for **RDF
> graphs + SPARQL**.

<p align="center">
  <img src="docs/img/lazy-query.svg" alt="A browser sends HTTP Range requests to a remote .rete file shown as a stack of tiles; only the few tiles a query touches are fetched, while most of the file is never transferred. A 100 MB / 1 GB graph stays interactive because only a few MB cross the wire per query." width="620">
</p>

- **No server.** The file *is* the database. Publish once to static hosting.
- **Query in place.** SELECT / ASK / CONSTRUCT / DESCRIBE, joins, OPTIONAL,
  UNION, MINUS, FILTER, subqueries, property paths, GROUP BY / aggregates, XSD
  casts and the SPARQL 1.1 function library, named graphs, and **GeoSPARQL**
  (geometry + time) — **[~75% of the W3C query-evaluation suite](https://caviri.github.io/rete/conformance.html)**
  (309 tests; ≈89% excluding the RDFS/OWL-entailment and SERVICE-federation
  regimes rete leaves out by design).
- **Lazy over HTTP.** Range-read a remote file: a selective query faults in only
  the dictionary chunks and index tiles it touches, so a **1 GB graph stays
  interactive** in the browser ([try it](https://caviri.github.io/rete/explore-100mb.html)).
- **Self-describing.** Every file can carry its own **[Dataset Card](https://caviri.github.io/rete/dataset-cards.html)**
  (counts, vocabulary, detected signals, and a library of **ready-to-run starter
  queries**) plus a **[schema pyramid](https://caviri.github.io/rete/semantic-zoom.html)** —
  a *leveled* `rdf:type` legend you read **index-free** in a couple of range
  requests. See below.
- **Small + fast to open.** Compressed (zstd) with indexes prebuilt, so it
  *opens in ~16 ms* — no load/index step. (See [benchmarks](#how-fast).)
- **Runs in the browser.** Same engine in WASM — the
  **[interactive playground](https://caviri.github.io/rete/playground.html)** is
  a single offline HTML page.

## Self-describing & semantic zoom

Open a `.rete` you've never seen and the **cold-start problem** bites: which
classes exist, how do they connect, where do you even start? rete fixes this by
shipping the answer *inside the file*, read over a couple of HTTP range requests
**without ever touching the triple index**:

- The **Dataset Card** (`rete card-url <url>`) — title/license, term and class
  counts, the vocabulary, detected affordances (label / time / geo predicates,
  spatial bbox, …), and an auto-generated, **runnable** starter-query library.
- The **schema pyramid** (`rete summary --level k`) — a leveled `rdf:type`
  histogram where abstract classes describe the coarse view and leaf classes
  resolve as you zoom in. It also keeps the **lateral** relations between classes
  (not just the `is-a` tree), as a *non-exclusive* graph.

<p align="center">
  <img src="docs/img/semantic-zoom.svg" alt="A four-band pyramid: the abstract level (Agent ×4) at the narrow top widens to leaf classes at the base — Level 0 Agent, Level 1 Person and Organisation, Level 2 Scientist and Artist, Level 3 Astronomer. A footer notes it is read index-free from the file over HTTP." width="560">
</p>

```text
$ rete summary big.rete --level 0     # the big picture, index-free
   12048  <http://schema.org/Person>
    3017  <http://schema.org/Organization>
$ rete summary big.rete --level 2     # zoom in — leaves resolve
    4102  <http://schema.org/Scientist>  …
```

Full guide: **[Semantic zoom (schema pyramid)](https://caviri.github.io/rete/semantic-zoom.html)**.

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
  [federated across several files](https://caviri.github.io/rete/federation.html)
  (UNION federation: per-source results merged; cross-file joins are *not* resolved).
- You care about **bounded, progressive reads** — fetch a coarse overview first,
  drill into detail only where a query needs it (PMTiles-style, for graphs).

🚫 **Reach for a real triplestore instead when…**
- You need **frequent writes / transactions / live updates** (rete files are
  immutable — you rebuild to change them).
- You need an **always-on SPARQL endpoint** with a mature, battle-tested planner
  (e.g. [Oxigraph](https://github.com/oxigraph/oxigraph), Jena, GraphDB). rete's
  engine wins or ties most benchmark shapes against Oxigraph, but it is
  optimized for *publish-and-query*, not server throughput — see the honest
  [comparison](https://caviri.github.io/rete/BENCHMARK.html#comparison-vs-oxigraph-real-opencitations-network).
- You need full **OWL/RDFS entailment** at query time — rete bakes a fixed set of
  RDFS/OWL-RL inferences at build (`rete build --materialize`) rather than
  entailing during evaluation.

## Quick start

Everything runs **in Docker** (nothing builds on your host):

```sh
# Build the repo dev image once, then build the CLI inside it:
docker build -t rete-dev -f .devcontainer/Dockerfile .
docker run --rm -it -v "${PWD}:/work" -w /work rete-dev \
  cargo build --release -p rete-cli       # binary at target/release/rete
```

```sh
# 1. Build a .rete from N-Triples / Turtle / N-Quads (--card embeds the self-description):
rete build examples/social.nt -o social.rete --card --title "Social graph"

# 2. Query it — a triple pattern, or full SPARQL:
rete query  social.rete --predicate '<http://ex/knows>'
rete why    social.rete --predicate '<http://ex/knows>'   # explain result provenance
rete sparql social.rete "PREFIX e: <http://ex/> \
  SELECT ?p ?age WHERE { ?p e:age ?age . FILTER(?age > 27) }"

# 3. Read the self-description (no index touched) and the leveled type legend:
rete card    social.rete            # counts, vocabulary, signals, starter queries
rete summary social.rete --level 0  # the schema pyramid at its most abstract level

# 4. Or query straight from a URL — fetches only the byte ranges it needs:
rete card-url   https://my-bucket.s3.amazonaws.com/social.rete   # 2 range requests
rete query-url  https://my-bucket.s3.amazonaws.com/social.rete --object '<http://ex/Alice>'
rete sparql-url https://my-bucket.s3.amazonaws.com/social.rete "SELECT * WHERE { ?s ?p ?o } LIMIT 5"
```

`query-url` resolves bound terms from the dictionary, then range-fetches only the
selected SPO/POS/OSP permutation payload for that triple pattern; `sparql-url`
faults in index tiles as a query touches them. `rete cost --explain` shows when a
query can use the summary-only or routed-pattern budgets.

### Try it in your browser (no install)

The **[playground](https://caviri.github.io/rete/playground.html)** is a
self-contained offline page bundling the WASM engine and **21 example datasets**
— from tiny embedded graphs to **remote, lazily-queried** ones served over HTTP
range requests:

- a real **588k-triple OpenCitations** citation network,
- the **getty-ulan** artist-mentorship lineage (~205k triples, remote),
- the **entire 98 MB OpenHistoricalMap** planet (~6.1M dated, geolocated features), and
- a **104 MB and a 1 GB slice of Wikidata** — queried in the browser **without
  downloading them**.

Pick a dataset, run filterable SPARQL examples, inspect progressive exactness
metadata, validate SHACL, explore reachability, render schema summaries, and
explain triple-pattern provenance — all offline. Two focused demos:

- **[100 MB / 1 GB explorer](https://caviri.github.io/rete/explore-100mb.html)** —
  the same Wikidata slice four ways (rete / Parquet / DuckDB / SQLite), lazy over HTTP.
- **[Historical atlas](https://caviri.github.io/rete/atlas.html)** — GeoSPARQL +
  time over a `.rete`, with 80+ map overlays (battles, castles, treaties, …).

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

rete opens ~100–160× faster, and after the 2026 engine rework (lazy slot-row
pipeline, adaptive index-nested-loop joins, top-k ORDER BY) it wins or ties most
SPARQL shapes — aggregates, GROUP BY, DISTINCT, OPTIONAL, UNION/VALUES, paths,
sorted pagination. Oxigraph keeps a fractional edge on ASK, the tightest LIMIT
joins, and non-literal REGEX scans. File size: ~11.7× smaller than raw
N-Triples, ~1.27× of `gzip` — but *queryable*.

> **Honest note on the pyramid.** Two structures ship in the file: the
> *community summary* scales with the graph and helps overview/aggregate queries
> but adds build time and gives node-selective queries no benefit (use
> `--no-pyramid` to drop it); the *schema pyramid* is bounded by the ontology and
> stays cheap at any scale. See the
> [cost-vs-benefit benchmark](https://caviri.github.io/rete/BENCHMARK.html#the-pyramid-cost-vs-benefit).

## How it's laid out

A 1 KB header — a small fixed core plus a typed **section directory** — points at
every section, so a client reads only what it needs (and new sections are just new
directory entries).

<p align="center">
  <img src="docs/img/file-layout.svg" alt="On-disk layout: a fixed 1 KB header — a typed section directory holding the byte offset and length of every section — metadata (Dataset Card), dictionary, index (SPO/POS/OSP), pyramid-meta (community summary + schema pyramid), optional named graphs, and a footer magic." width="600">
</p>

See the **[format spec](https://caviri.github.io/rete/SPEC.html)** and
**[architecture](https://caviri.github.io/rete/architecture.html)** for the details.

## Documentation

- **[Graph data 101](https://caviri.github.io/rete/intro.html)** — new to RDF/graphs? Start here.
- **[Getting started](https://caviri.github.io/rete/getting-started.html)** · **[Architecture](https://caviri.github.io/rete/architecture.html)** · **[CLI reference](https://caviri.github.io/rete/cli.html)** · **[SPARQL support](https://caviri.github.io/rete/sparql.html)** · **[GeoSPARQL](https://caviri.github.io/rete/geosparql.html)** · **[SHACL validation](https://caviri.github.io/rete/shacl.html)**
- **[Dataset Cards](https://caviri.github.io/rete/dataset-cards.html)** · **[Semantic zoom (schema pyramid)](https://caviri.github.io/rete/semantic-zoom.html)** · **[Reasoning & coherence](https://caviri.github.io/rete/reasoning.html)** · **[Federated queries](https://caviri.github.io/rete/federation.html)**
- **[Interactive playground](https://caviri.github.io/rete/playground.html)** · **[100 MB / 1 GB explorer](https://caviri.github.io/rete/explore-100mb.html)** · **[Historical atlas](https://caviri.github.io/rete/atlas.html)**
- **[Format spec](https://caviri.github.io/rete/SPEC.html)** · **[Benchmarks](https://caviri.github.io/rete/BENCHMARK.html)** · **[SPARQL conformance](https://caviri.github.io/rete/conformance.html)** · **[Browser / WASM](https://caviri.github.io/rete/browser.html)**

The docs render as Markdown on GitHub, or as an HTML site (`docs/*.html`,
regenerated with `cargo run -p docgen`).

## Status

**Experimental — v0.1.0** (first tagged release; see [CHANGELOG](CHANGELOG.md)).
Working end-to-end — the single-file format, dictionary + permutation indexes, the
community summary and a self-describing **schema pyramid**, SPARQL + GeoSPARQL,
lazy HTTP-range queries (with per-tile synopses that prune a routed tile before
fetching it), and the browser/WASM engine. The **on-disk format (header version 3,
a 1 KB section directory) is still a draft and is not guaranteed stable across
releases** — rebuild to upgrade (v0.3 is a clean break from the earlier
128-byte-header layouts). SPARQL evaluation is exact for supported shapes (no OWL/RDFS query-time
entailment), and federation is UNION-only.

## Develop (Docker only)

```sh
cargo test --workspace --exclude rete-bench   # round-trip, robustness, range access, SPARQL
cargo run -p rete-cli -- info some.rete
bash scripts/smoke.sh                          # end-to-end test of every CLI subcommand
uv run python scripts/build_playground.py      # rebuild the offline playground page
```

CI runs fmt, clippy, the test matrix, the CLI smoke test, a query-engine
regression check (`qbench --check`: per-query row counts + time ceilings), and
the WASM build in containers, so nothing builds on the host. See
[Getting started](https://caviri.github.io/rete/getting-started.html) for the dev-container setup.

## License

[Apache-2.0](LICENSE).
