# rete

**A single-file, range-queryable RDF graph format — put it on a URL, run SPARQL, no server.**

[`github.com/caviri/rete`](https://github.com/caviri/rete) · crates **v0.3.0** · stable file-format generation **1** (`0x05`)

`rete` packs an RDF graph (or a full dataset of named graphs) into one immutable
`.rete` file with its own dictionary, permutation indexes, and a pyramidal
community summary. Drop the file on S3, GitHub, or any HTTP host that honors
`Range` requests, hand a client the URL, and query it in place — fetching only
the bytes a query needs. The same engine compiles to WebAssembly, so the browser
queries the file directly with no backend.

Think **Parquet / SQLite-over-HTTP / PMTiles**, but for RDF + SPARQL.

<img src="img/lazy-open.svg" alt="How lazy opening works. Any client — a browser, a notebook, the CLI, a server — sends HTTP byte-range reads to one .rete file that stays where it is, on a bucket or on local disk. Only the 1 KiB header, the few dictionary chunks and the few index tiles a query actually touches are fetched; the rest of the file is never transferred, and a block cache of at most 256 MiB keeps hot blocks resident. Measured on the 52 GB datacite.rete, 9.83 billion triples: a COUNT returns 779,399 rows in 4 seconds inside a 2 GiB container, because aggregation streams and nothing is ever read whole.">

*Lazy opening: the file stays where it is — on a bucket or on disk — and a query fetches only the bytes it touches. Aggregation streams, so even a `COUNT` over 9.83 billion triples fits in 2 GiB.*

## Why

- **No database server.** The file *is* the database. Publish once to static
  hosting; clients query by URL.
- **Bounded, progressive reads.** A query touches a handful of byte ranges, never
  a linear scan. The coarse "overview" graph can be fetched first (3 ranges —
  23.3 % of `davidrumsey.rete`, and the share tracks how big the dictionary is)
  before drilling into detail — PMTiles-style zoom, for graphs.
- **Real SPARQL.** SELECT / ASK / CONSTRUCT / DESCRIBE over BGPs, joins, OPTIONAL,
  UNION, MINUS, FILTER, VALUES, property paths, GROUP BY / aggregates, and named
  graphs — evaluated against the file.
- **In the browser.** `rete-wasm` runs the identical engine client-side; the demo
  page loads the overview over HTTP ranges and runs SPARQL with no server.
- **Safe on untrusted input.** A truncated or corrupt file from an arbitrary URL
  yields an error, never a panic (fuzz-tested).

<img src="img/pyramid.svg" alt="The pyramid stores a graph at several levels of detail so a client can read an overview before touching the data. Level 0 at the top is the coarsest: a handful of supernodes with aggregated edges. Middle levels split those into finer communities, each tile targeted at about 64 KiB so one zoom is one range read. Level N-1 at the base is the full triple graph, fetched only where a query drills in. On the published davidrumsey.rete the whole pyramid is 1,332,512 bytes — 1.8 percent of the 74.8 MB file — so the overview is cheap and the base is not.">

*The pyramid: read a coarse summary first, then drill into detail only where a query needs it.*

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
| **Blender** — add-on (engine bundled) | Install the `rete-*.zip` from `clients/blender` | Blender ≥ 4.2; SPARQL results become 3D scenes | [Blender add-on](blender.md) |
| **Browser, zero install** | — | any modern browser | [Playground](playground-guide.md) · [SPARQL IDE](yasgui-guide.md) |
| **Agents** — MCP server + Claude Code plugin | `/plugin marketplace add caviri/rete` — or point any MCP client at the [gateway](https://katospiegel-rete.hf.space/mcp/) | ChatGPT, Claude, pydantic-ai, any MCP host | [Agentic interfaces](agents.md) |
| **Agent frameworks** — the graph as tools, in process | `pip install rete-graph` + your framework | LangChain / LangGraph, Pydantic AI; local or remote graphs, no server | [LangChain & Pydantic AI](agent-frameworks.md) |

## Documentation

### Start here

- **[Graph data 101](intro.md)** — new to graphs/RDF? A beginner's tour, framed by the questions you can ask.
- **[Getting started](getting-started.md)** — install (Docker-only), build, query, deploy.
- **[Real-world scenario](scenario.md)** — publish a queryable SBOM to a URL; curl examples.

### Explore in the browser

- **[Playground](playground-guide.md)** — the flagship demo: 65 real datasets queried live over HTTP ranges, with SPARQL + SQL + semantic search, media viewers, and AI helpers. [Launch it →](playground.html)
- **[Plaza — dataset gallery](plaza-guide.md)** — browse published datasets as live cards. [Open it →](plaza/index.html)
- **[SPARQL IDE — yasgui·wasm](yasgui-guide.md)** — a Yasgui-style IDE where the endpoint is a `.rete` file: paste a URL (read lazily over HTTP range) or drop a local file; tabs, autocomplete from the dataset's own labels, pivot/turtle views, share links. [Launch it →](yasgui.html)
- **[Historical atlas](atlas.md)** — SPARQL + GIS: border polygons, timeline, five projections.
- **[2D match replay](pitch.html)** — a football match replayed on a canvas pitch from a spatiotemporal `.rete` (player + ball positions, 5 fps; pick any match). [Launch it →](pitch.html)
- **[Subtitle timeline](subtitles-guide.md)** — one film subtitled in 20 languages; scrub the timeline and watch a line of dialogue appear in every language at once. [Launch it →](subtitles.html)
- **[World Cup 2022 final](wcfinal.html)** — Argentina 3–3 France replayed from real StatsBomb positional freeze-frames: every player's place at every moment, with a live scoreboard and goal jumps. [Launch it →](wcfinal.html)
- **[Ask the graph](ask-the-graph.md)** — graphRAG search over a `.rete`, entirely in the browser.
- **[WebGPU coherence (experimental)](webgpu-guide.md)** — several sources making causal claims, none of them certain: where do they contradict each other, which fallacies does the graph itself expose, and can a GPU find them faster than one CPU core? Circular reasoning, causes-vs-prevents, slippery slopes and confounders are all found by arithmetic — no language model reading anything — in an editable sandbox that draws your argument, traces belief spreading step by step, and exports it as RDF-star you can `rete build`. Write your own disagreement down, share it as a link, and see where it actually breaks; pairs with [the fallacy-annotation experiment](fallacies.md). Then a live benchmark: same-line checks cap at ~2×, chain checks reach 15–17×. [Try it →](webgpu.html)
- **[Graph-map, topic-map & 3D (experimental)](graph-map.md)** — the community pyramid as a slippy map.

**In 3D — a SPARQL answer becomes geometry**

- **[Human anatomy](anatomy-guide.md)** — pick any bone, muscle, organ or nerve in the `z-anatomy` graph and see its real 3D neighbours: what touches it, shares its tissue or is thermally coupled to it, plus the diseases located there. [Launch it →](anatomy.html)
- **[A building, queried](building-guide.md)** — the FZK-Haus IFC model as a graph: pick a wall, door, window, slab or room and see its floor, the rooms it encloses and everything within reach in 3D, with real SPARQL + **geo3** (GeoSPARQL in three dimensions). [Launch it →](building.html)
- **[Architecture vs structure (BIM pair)](bim-pair-guide.md)** — the same house modelled twice by one TUM *BIM Project* team: the architectural envelope (walls, curtain walls, doors, windows, furniture) and the structural skeleton. Diff them, or overlay the skeleton inside the translucent envelope. [Launch it →](bim-pair.html)
- **[Neurons & astrocytes](neuro-showcase-guide.md)** — a WebGL viewer that streams electron-microscopy meshes out of the file and rebuilds each cell in your browser: rotate an astrocyte, or show the whole neuron cluster coloured by neurotransmitter. [Launch it →](neuro-showcase.html)

**Drawing & scripting**

- **[Mark Lombardi's networks](lombardi-guide.md)** — 51 of Lombardi's hand-drawn conspiracy diagrams (banks, shell companies, arms deals) read live out of one `.rete` over HTTP range and redrawn in the browser. [Launch it →](lombardi.html)
- **[JS lab](jslab-guide.md)** — an Observable-style notebook: a JavaScript editor beside a live visualization, the code querying a remote `.rete` as you type. [Launch it →](jslab.html)
- **[JupyterLite notebook](jupyterlite-guide.md)** — a full Jupyter notebook running the `rete-graph` Python client in your tab, on a Pyodide kernel: `pip install`-free, queries a remote `.rete` over HTTP range. [Launch it →](jupyterlite/index.html)

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
The crates are **v0.3.0**, the first release on crates.io. Stable file-format
generation 1 (header byte `0x05`) was frozen on 2026-07-14 and first released in
0.3.0; the experimental generations `0x01`–`0x04` predate it and must be rebuilt
from RDF source. **No backwards-compatibility promise is made before 1.0.0** —
neither for the `.rete` format nor for the public Rust, CLI and WASM APIs. See
[compatibility](compatibility.md#stable-rete-file-compatibility).
Everything is built and tested in Docker — see
[Getting started](getting-started.md).
