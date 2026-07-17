# rete

**A single-file, range-queryable RDF graph format — put it on a URL, run SPARQL, no server.**

[`github.com/caviri/rete`](https://github.com/caviri/rete) · crates **v1.0.0-rc.1** · stable file-format generation **1** (`0x05`)

`rete` packs an RDF graph (or a full dataset of named graphs) into one immutable
`.rete` file with its own dictionary, permutation indexes, and a pyramidal
community summary. Drop the file on S3, GitHub, or any HTTP host that honors
`Range` requests, hand a client the URL, and query it in place — fetching only
the bytes a query needs. The same engine compiles to WebAssembly, so the browser
queries the file directly with no backend.

Think **Parquet / SQLite-over-HTTP / PMTiles**, but for RDF + SPARQL.

<img src="img/pyramid.svg" alt="The rete pyramid: a coarse community summary at the top, communities in the middle, and full triples at the base; a client reads the top first and drills down only where needed.">

*The pyramid: read a coarse summary first, then drill into detail only where a query needs it.*

## Why

- **No database server.** The file *is* the database. Publish once to static
  hosting; clients query by URL.
- **Bounded, progressive reads.** A query touches a handful of byte ranges, never
  a linear scan. The coarse "overview" graph can be fetched first (≈25 % of a
  file, 3 ranges) before drilling into detail — PMTiles-style zoom, for graphs.
- **Real SPARQL.** SELECT / ASK / CONSTRUCT / DESCRIBE over BGPs, joins, OPTIONAL,
  UNION, MINUS, FILTER, VALUES, property paths, GROUP BY / aggregates, and named
  graphs — evaluated against the file.
- **In the browser.** `rete-wasm` runs the identical engine client-side; the demo
  page loads the overview over HTTP ranges and runs SPARQL with no server.
- **Safe on untrusted input.** A truncated or corrupt file from an arbitrary URL
  yields an error, never a panic (fuzz-tested).

## 60-second tour

```sh
# Build a file from N-Triples (or .nq / .ttl; merge several; or read stdin):
rete build examples/social.nt -o social.rete

# Query a triple pattern, a BGP, or full SPARQL:
rete query  social.rete --predicate '<http://ex/knows>'
rete why    social.rete --predicate '<http://ex/knows>'   # result provenance
rete sparql social.rete "PREFIX e: <http://ex/> SELECT ?p ?age WHERE { ?p e:age ?age . FILTER(?age > 27) }"

# Query straight from a URL — fetches only the byte ranges needed (http or https):
rete query-url https://my-bucket.s3.amazonaws.com/social.rete --object '<http://ex/Alice>'

# Look at the coarse graphs without reading the index:
rete summary social.rete    # structural (Louvain communities)
rete schema  social.rete    # semantic (by rdf:type)
```

## Clients

The same engine, in your language of choice — every client opens local files
*and* remote URLs (lazy HTTP range reads) and returns parsed SPARQL results:

| Client | Get it | Runs on | Docs |
|---|---|---|---|
| **Python** — [`rete-graph` on PyPI](https://pypi.org/project/rete-graph/) | `pip install rete-graph` | CPython ≥ 3.9 everywhere, plus **Pyodide** (JupyterLite, marimo WASM) | [Python API](python.md) · [build tutorial](python-build-tutorial.md) |
| **JavaScript** — [`rete-graph` on npm](https://www.npmjs.com/package/rete-graph) | `npm install rete-graph`, or one [`<script>` tag via CDN](javascript.md) | Node ≥ 18, browsers (bundlers or script-tag) | [JavaScript API](javascript.md) |
| **R** — `rete` (from this repo; CRAN/R-universe pending) | `remotes::install_github("caviri/rete", subdir = "clients/r", build = FALSE)` | R ≥ 4.2 + Rust toolchain; results as data frames | [R API](r.md) |
| **Rust** — `rete-core` / `rete-cli` (in this repo; crates.io release pending) | `cargo add rete-core --git https://github.com/caviri/rete` | anywhere Rust runs — native + wasm | [Rust API](rust-api.md) · [CLI](cli.md) |
| **Browser, zero install** | — | any modern browser | [Playground](playground-guide.md) · [SPARQL IDE](yasgui-guide.md) |

## Documentation

### Start here

- **[Graph data 101](intro.md)** — new to graphs/RDF? A beginner's tour, framed by the questions you can ask.
- **[Getting started](getting-started.md)** — install (Docker-only), build, query, deploy.
- **[Real-world scenario](scenario.md)** — publish a queryable SBOM to a URL; curl examples.

### Explore in the browser

- **[Playground](playground-guide.md)** — the flagship demo: 40+ real datasets queried live over HTTP ranges, with SPARQL + SQL + semantic search, media viewers, and AI helpers. [Launch it →](playground.html)
- **[Plaza — dataset gallery](plaza/index.html)** — browse published datasets as live cards.
- **[SPARQL IDE — yasgui·wasm](yasgui-guide.md)** — a Yasgui-style IDE where the endpoint is a `.rete` file: paste a URL (read lazily over HTTP range) or drop a local file; tabs, autocomplete from the dataset's own labels, pivot/turtle views, share links. [Launch it →](yasgui.html)
- **[Historical atlas](atlas.md)** — SPARQL + GIS: border polygons, timeline, five projections.
- **[2D match replay](pitch.html)** — a football match replayed on a canvas pitch from a spatiotemporal `.rete` (player + ball positions, 5 fps; pick any match). [Launch it →](pitch.html)
- **[Subtitle timeline](subtitles.html)** — one film subtitled in 20 languages; scrub the timeline and watch a line of dialogue appear in every language at once. [Launch it →](subtitles.html)
- **[World Cup 2022 final](wcfinal.html)** — Argentina 3–3 France replayed from real StatsBomb positional freeze-frames: every player's place at every moment, with a live scoreboard and goal jumps. [Launch it →](wcfinal.html)
- **[Ask the graph](ask-the-graph.md)** — graphRAG search over a `.rete`, entirely in the browser.
- **[Graph-map, topic-map & 3D (experimental)](graph-map.md)** — the community pyramid as a slippy map.

### Guides

- **[CLI reference](cli.md)** — every `rete` subcommand.
- **[SPARQL support](sparql.md)** — exactly what the engine evaluates, including `SERVICE` federation.
- **[GeoSPARQL](geosparql.md)** — geometry filters + time: "which territory contained this point in year Y?"
- **[SHACL validation](shacl.md)** — validate `.rete` graphs against SHACL Core shapes, locally or over a URL.
- **[Reasoning & coherence](reasoning.md)** — prototype OWL RL / RDFS reasoner; find incoherent points.
- **[Federated queries](federation.md)** — query several `.rete` files (local paths and/or URLs) as one.
- **[Semantic zoom](semantic-zoom.md)** — the schema pyramid: overview first, drill into detail.
- **[Compatibility & Cypher](compatibility.md)** — RDF interop, validation paths, and the Cypher subset.

### Publish & share

- **[Dataset Cards](dataset-cards.md)** — self-describing metadata embedded in the file.
- **[Hosting your .rete](hosting.md)** — put the file on R2, Zenodo, GitHub Pages, or S3 and query it by URL.
- **[Media & SQL companions](media-companions.md)** — images, IIIF, 3D and audio in query results; Parquet/SQLite companions.

### Graph analysis

- **[Topic modeling (LDA)](topic-modeling.md)** — label each community's theme: `rete communities` + scikit-learn LDA.
- **[Multi-criteria communities](multi-criteria.md)** — partition the same graph by different relations/attributes; combine criteria.

### Development

- **[Architecture](architecture.md)** — workspace map, build/read/query pipelines, range model, and extension points.
- **[Format specification](SPEC.md)** — the on-disk byte layout, for implementers.
- **[Rust API](rust-api.md)** — the stable `rete-core` facade modules and embedding examples.
- **[WASM & JavaScript API](browser.md)** — the browser bindings: query, remote reads, caching.
- **[Parallel in the browser (experimental)](parallel-browser.md)** — Web Worker reachability + shared-memory threads.
- **[Tables, VKG & big builds](data-engineering.md)** — entity/property tables, virtual knowledge graphs, large ingestions.
- **[Benchmarks](BENCHMARK.md)** — sizing, the OpenCitations/Oxigraph comparison, and the LUBM-style suite.
- **[SPARQL 1.1 conformance](conformance.md)** — the W3C test-suite scorecard.

## Version & status

Developed in the open at [github.com/caviri/rete](https://github.com/caviri/rete).
The crates are **v1.0.0-rc.1**. Stable file-format generation 1 (header byte
`0x05`) is the compatibility baseline for Rete 1.x. Stable readers keep reading
it; a future incompatible layout must retain generation-1 read support and ship
a documented migration path. Pre-1.0 experimental files must be rebuilt from
RDF source. The public Rust, CLI, and WASM APIs remain release candidates until
1.0.0 final.
Everything is built and tested in Docker — see
[Getting started](getting-started.md).
