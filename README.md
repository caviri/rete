<p align="center">
  <img src="docs/img/logo.svg" alt="rete — a queryable RDF graph in a single file, no server" width="520">
</p>

<p align="center">
  <b>An RDF graph in one file. Put it on any static host and query it with SPARQL, without a database server.</b>
</p>

<p align="center">
  <a href="https://caviri.github.io/rete/playground.html"><b>Try it in your browser</b></a> ·
  <a href="https://caviri.github.io/rete/">Documentation</a> ·
  <a href="CHANGELOG.md">Changelog</a>
</p>

<p align="center">
  <a href="https://github.com/caviri/rete/actions/workflows/ci.yml"><img src="https://img.shields.io/github/actions/workflow/status/caviri/rete/ci.yml?branch=main&label=CI" alt="CI status"></a>
  <a href="https://crates.io/crates/rete-cli"><img src="https://img.shields.io/crates/v/rete-cli?label=crates.io%20rete-cli&logo=rust&logoColor=white" alt="rete-cli on crates.io"></a>
  <a href="https://pypi.org/project/rete-graph/"><img src="https://img.shields.io/pypi/v/rete-graph?label=PyPI%20rete-graph&logo=pypi&logoColor=white" alt="rete-graph on PyPI"></a>
  <a href="https://www.npmjs.com/package/rete-graph"><img src="https://img.shields.io/npm/v/rete-graph?label=npm%20rete-graph&logo=npm&logoColor=white" alt="rete-graph on npm"></a>
  <a href="LICENSE"><img src="https://img.shields.io/badge/license-Apache--2.0-blue" alt="License: Apache-2.0"></a>
  <a href="https://doi.org/10.5281/zenodo.21546287"><img src="https://zenodo.org/badge/DOI/10.5281/zenodo.21546287.svg" alt="DOI: 10.5281/zenodo.21546287"></a>
</p>

## What is rete?

rete (Latin for "net", pronounced RAY-teh) stores a whole RDF graph in one
immutable `.rete` file: the term dictionary, the sorted triple indexes, and
optional extras such as a Dataset Card or a full-text index. Put the file on
any host that serves HTTP range requests (S3, Cloudflare R2, GitHub Pages,
Zenodo) and clients run SPARQL against it where it sits. A query fetches only
the parts of the file it needs, so there is no server to run and no import
step. The same engine runs in the CLI, in Python and JavaScript, and in the
browser through WebAssembly.

If you know Parquet or PMTiles, it is the same idea applied to RDF graphs.

Each file can also describe itself. Its
[Dataset Card](https://caviri.github.io/rete/dataset-cards.html) (title,
license, counts, vocabularies, starter queries) and its
[schema summary](https://caviri.github.io/rete/semantic-zoom.html) are read in
a couple of small range requests, without touching the indexes.

### When it fits

rete is built for graphs that you publish and other people read: a dataset
release, a nightly dump, a knowledge-base export, one file per tenant or per
year. You host a file instead of running a triplestore, and people query it
from a browser tab, a notebook or a shell.

It is not a database. Files are immutable, so a change means a rebuild. If you
need writes, transactions or an always-on endpoint under load, use a
triplestore such as [Oxigraph](https://github.com/oxigraph/oxigraph), Jena or
GraphDB. There is no full OWL/RDFS entailment at query time either: rete
rewrites queries for OWL 2 QL (`--entail`) and can bake RDFS/OWL RL inferences
into the file at build time (`--materialize`); see
[reasoning](https://caviri.github.io/rete/reasoning.html).

## Install

```sh
cargo install rete-cli --locked   # the rete command-line tool
pip install rete-graph            # Python 3.9+, also in Pyodide
npm install rete-graph            # JavaScript: Node 18+ and browsers
```

No Rust toolchain? The CLI is also published as a small container image,
`ghcr.io/caviri/rete-cli` (amd64 and arm64). With this alias the commands
below work as written on the files in your current directory:

```sh
alias rete='docker run --rm -i -v "$PWD:/data" -w /data ghcr.io/caviri/rete-cli:latest'
```

Read the three container pitfalls in
[Getting started](https://caviri.github.io/rete/getting-started.html#getting-the-cli)
first: piping needs `-i`, the image runs as root on Linux, and Git Bash on
Windows rewrites paths.

## Quick start

Build a file from an RDF dump. N-Triples, N-Quads, Turtle, TriG and RDF/XML
are detected by extension, and several inputs merge into one file.
`--card` embeds a Dataset Card.

```sh
rete build dump.nt -o graph.rete --card --title "My graph"
```

Inspect and query it locally:

```sh
rete card   graph.rete
rete sparql graph.rete 'SELECT ?s ?name WHERE { ?s <http://xmlns.com/foaf/0.1/name> ?name } LIMIT 10'
```

A remote file works the same way, with nothing to download first. OpenCitations
Meta is a single 36 GB file with 5.2 billion triples; reading its card takes
two small requests:

```sh
rete card-url https://data.graphplaza.com/opencitations/opencitations.rete
# fetched 1740 of 35852509508 bytes in 2 range request(s) — index NOT fetched

rete sparql-url https://data.graphplaza.com/opencitations/opencitations.rete \
  'SELECT * WHERE { ?s ?p ?o } LIMIT 3'
```

The [CLI reference](https://caviri.github.io/rete/cli.html) covers every
command, and [hosting](https://caviri.github.io/rete/hosting.html) explains how
to put your own file online.

To try it without installing anything, open the
[playground](https://caviri.github.io/rete/playground.html). It runs the same
engine in your browser, on example datasets that range from small graphs
embedded in the page to remote files with billions of triples.

## Clients

All of these run the same engine.

- **CLI and Rust**: the `rete-cli` binary, or the [`rete-core`](https://docs.rs/rete-core) crate as a library ([CLI](https://caviri.github.io/rete/cli.html), [Rust API](https://caviri.github.io/rete/rust-api.html)).
- **Python**: `rete-graph` on PyPI, for CPython and Pyodide (JupyterLite, marimo) ([docs](https://caviri.github.io/rete/python.html)).
- **JavaScript**: `rete-graph` on npm, for Node and browsers, with TypeScript types ([docs](https://caviri.github.io/rete/javascript.html)).
- **R**: installed from GitHub with `remotes` (needs Rust); not on CRAN ([docs](https://caviri.github.io/rete/r.html)).
- **Java**: pure JVM with an RDF4J binding, built from [`clients/java`](clients/java/README.md); not on Maven Central yet.
- **Browser, no install**: the [playground](https://caviri.github.io/rete/playground.html) and a [SPARQL IDE](https://caviri.github.io/rete/yasgui.html) query any `.rete` URL.
- **Claude Desktop**: download [rete.mcpb](https://github.com/caviri/rete/releases/latest/download/rete.mcpb) and double-click it; the engine runs on your machine ([docs](https://caviri.github.io/rete/agents.html)).
- **Blender**: an add-on from the [releases page](https://github.com/caviri/rete/releases) that turns query results into 3D scenes ([docs](https://caviri.github.io/rete/blender.html)).
- **Hosted gateway**: [katospiegel-rete.hf.space](https://katospiegel-rete.hf.space/) serves MCP, a REST API and a SPARQL endpoint for each published dataset.

## Claude Code plugin

This repository is also a Claude Code plugin. It adds skills for building and
publishing `.rete` files, and the rete MCP server for querying the public
datasets:

```
/plugin marketplace add caviri/rete
/plugin install rete-graph@rete
```

The same MCP server works in other MCP clients, ChatGPT included; see
[agentic interfaces](https://caviri.github.io/rete/agents.html).

## How a file is laid out

A 1 KiB header points to every section, so a reader fetches the header and
then only the sections a query needs. Optional sections a file does not carry
are not listed. The [format spec](https://caviri.github.io/rete/SPEC.html)
and [architecture notes](https://caviri.github.io/rete/architecture.html) have
the details.

<p align="center">
  <img src="docs/img/rete-anatomy.svg" alt="Anatomy of a .rete file, drawn to scale on the real dblp.rete — 2.27 GB, 179,328,188 triples, 64,276,736 terms. A 1 KiB header carries a 64-byte core and a directory of 24-byte entries; then a 731-byte dataset card, the front-coded dictionary (413 MB, 18.2%), and six permutation indexes cut into roughly 64 KiB tiles with a per-tile min/max synopsis (1.85 GB, 81.8%). The file ends in a 4-byte RETE magic that a truncated download cannot fake. This specimen has no pyramid, named graphs, text index or build info — those section kinds are optional, and the directory simply does not list them." width="760">
</p>

The indexes are built into the file, so opening it needs no parsing or
indexing, and aggregates stream: counting the 779,399 software records in the
9.83-billion-triple DataCite file takes 4 s inside a 2 GiB container
([benchmark](https://caviri.github.io/rete/BENCHMARK.html#billion-triple-scale-bounded-memory-queries-datacite)).

## Documentation

- Start: [Getting started](https://caviri.github.io/rete/getting-started.html) · [Graph data 101](https://caviri.github.io/rete/intro.html) (if RDF is new to you)
- Querying: [CLI](https://caviri.github.io/rete/cli.html) · [SPARQL](https://caviri.github.io/rete/sparql.html) · [GeoSPARQL](https://caviri.github.io/rete/geosparql.html) · [SHACL](https://caviri.github.io/rete/shacl.html) · [Federation](https://caviri.github.io/rete/federation.html)
- Publishing: [Dataset Cards](https://caviri.github.io/rete/dataset-cards.html) · [Hosting](https://caviri.github.io/rete/hosting.html) · [Browser / WASM](https://caviri.github.io/rete/browser.html)
- Reference: [Format spec](https://caviri.github.io/rete/SPEC.html) · [Compatibility](https://caviri.github.io/rete/compatibility.html) · [Benchmarks](https://caviri.github.io/rete/BENCHMARK.html) · [W3C conformance](https://caviri.github.io/rete/conformance.html)
- Demos: [playground](https://caviri.github.io/rete/playground.html) · [JS lab (D3)](https://caviri.github.io/rete/jslab.html) · [historical atlas](https://caviri.github.io/rete/atlas.html)
- Notebooks: [JupyterLite](https://caviri.github.io/rete/jupyterlite/lab/index.html?path=graph-data-science.ipynb) (in the browser) · [Colab](https://colab.research.google.com/github/caviri/rete/blob/main/clients/python/examples/graph-data-science.ipynb) · [Binder](https://mybinder.org/v2/gh/caviri/rete/main?labpath=clients%2Fpython%2Fexamples%2Fgraph-data-science.ipynb)

## Status

rete is pre-1.0, and both the API and the file format can still change. The
format (byte `0x05`, stable generation 1) has not changed since it was frozen
in July 2026, but that is a track record, not a promise: until 1.0.0 a release
may require you to rebuild files you have published, so keep the RDF you built
them from. See [compatibility](https://caviri.github.io/rete/compatibility.html).

New in 0.4.0 for people who build files: a label index (`--label-index`) and a
schema section that no longer depend on the pyramid, `--text-index` in
memory-bounded builds, an opt-in quad index for named graphs
(`--named-graphs quad`, which older readers refuse), and `rete recard`, which
replaces a file's Dataset Card without a rebuild. The
[CHANGELOG](CHANGELOG.md) has the rest.

Known limits:

- Reads are lazy, but evaluation is not fully streaming. A query fetches only
  the tiles and dictionary chunks it touches, while hash-join build sides,
  `GROUP BY` groups, `DISTINCT` sets and an `ORDER BY` without `LIMIT` are
  held in memory. A small local file is read whole.
  [Details](https://caviri.github.io/rete/sparql.html#memory-budget).
- `rete federate` unions the results of several files; it does not join
  across them. The playground can split a basic graph pattern across files,
  with limits. [Details](https://caviri.github.io/rete/federation.html#limitations).
- The browser engine is single-threaded by default; multi-threading is
  experimental. [Details](https://caviri.github.io/rete/parallel-browser.html).

## Develop

Everything builds in Docker, nothing on the host:

```sh
docker compose build dev
docker compose run --rm dev cargo build --release -p rete-cli
docker compose run --rm dev cargo test --workspace --exclude rete-bench
docker compose run --rm dev bash scripts/smoke.sh
bash tests/gate/gate.sh    # browser regression gate
```

You can also open the folder in its devcontainer.
[CONTRIBUTING.md](CONTRIBUTING.md) and [AGENTS.md](AGENTS.md) list the full set
of checks and the rules for generated files.

## Support

rete is free and open source. If it is useful to you, you can support its
development on Ko-fi:

<a href='https://ko-fi.com/M1W723PEW3' target='_blank'><img height='36' style='border:0px;height:36px;' src='https://storage.ko-fi.com/cdn/kofi2.png?v=6' border='0' alt='Buy Me a Coffee at ko-fi.com' /></a>

## License

[Apache-2.0](LICENSE).
