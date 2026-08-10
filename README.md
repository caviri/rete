<p align="center">
  <img src="docs/img/logo.svg" alt="rete — a queryable RDF graph in a single file" width="520">
</p>

<p align="center">
  <b>Put an RDF graph in one file. Drop it on a URL. Query it with SPARQL — no database server.</b>
</p>

<p align="center">
  <a href="https://caviri.github.io/rete/playground.html"><b>▶ Try it in your browser</b></a> ·
  <a href="https://caviri.github.io/rete/jslab.html">JS lab (D3)</a> ·
  <a href="https://caviri.github.io/rete/atlas.html">Historical atlas</a> ·
  <a href="https://caviri.github.io/rete/">Docs</a>
</p>

<p align="center">
  <a href="https://caviri.github.io/rete/jupyterlite/lab/index.html?path=graph-data-science.ipynb"><img src="https://img.shields.io/badge/JupyterLite-in--browser-F37626?logo=jupyter&logoColor=white" alt="Open in JupyterLite"></a>
  <a href="https://colab.research.google.com/github/caviri/rete/blob/main/clients/python/examples/graph-data-science.ipynb"><img src="https://img.shields.io/badge/Colab-open-F9AB00?logo=googlecolab&logoColor=white" alt="Open in Colab"></a>
  <a href="https://mybinder.org/v2/gh/caviri/rete/main?labpath=clients%2Fpython%2Fexamples%2Fgraph-data-science.ipynb"><img src="https://img.shields.io/badge/Binder-launch-579ACA?logo=jupyter&logoColor=white" alt="Launch Binder"></a>
</p>

---

## What is rete?

The name comes from Latin **rēte**, meaning "net" (pronounced **RAY-teh**).

`rete` is a tool that takes a complete RDF knowledge graph and compiles it down into **one immutable `.rete` file**. This file contains a dictionary, permutation indexes, a community summary, and a schema pyramid. 

You can host this file anywhere that supports HTTP range requests (like AWS S3 or GitHub Pages). When a client runs a SPARQL query against the file's URL, it **only downloads the exact bytes needed** to answer the query, entirely skipping the rest of the file.

> Think of it as **Parquet** (for tables) or **PMTiles** (for maps) — but built for **RDF graphs and SPARQL queries**.

<p align="center">
  <img src="docs/img/lazy-open.svg" alt="A diagram showing how clients only fetch the bytes they need from a .rete file" width="680">
</p>

### Key Superpowers
- **Serverless querying:** The file *is* the database. You publish it once, and clients query it directly. No triplestore required.
- **Full SPARQL support:** Supports SELECT, ASK, CONSTRUCT, joins, OPTIONAL, UNION, MINUS, FILTER, and GeoSPARQL. Passes ~75% of the W3C conformance suite.
- **Lazy HTTP loading:** A 1 GB graph remains fully interactive in the browser because selective queries only fault in the needed index tiles.
- **Bounded memory usage:** Aggregations stream efficiently. A `COUNT` query over a 9.83-billion-triple graph completes in 4 seconds using less than 2 GB of RAM.
- **Self-describing:** Every file includes a **Dataset Card** and a **schema pyramid** (a zoomable overview of the graph's structure) that can be read index-free.
- **Instant open:** Files are pre-indexed and compressed (zstd). They open in milliseconds.
- **Browser native:** The engine compiles to WebAssembly (WASM), allowing tools like our [interactive playground](https://caviri.github.io/rete/playground.html) to run entirely offline.

## Is rete right for your project?

**✅ Use rete when:**
- You want to **publish a public dataset** and let users query it without paying for a dedicated database server.
- Your data consists of **read-mostly snapshots** (e.g., daily dumps, releases, archives).
- You want to run **SPARQL queries in the browser**, on the edge, or inside a Jupyter notebook with no backend.
- You need to distribute **many graphs** (e.g., per-tenant or sharded by year) as cheap, cacheable files.

**🚫 Use a traditional triplestore (like Oxigraph or Jena) when:**
- Your graph requires **frequent writes, updates, or transactions** (rete files are immutable).
- You need an **always-on endpoint** optimized for maximum server throughput.
- You rely on dynamic, query-time **OWL/RDFS entailment** (rete pre-computes inferences at build time).

## Quick Start

We develop and run `rete` entirely inside Docker to ensure a clean, reproducible toolchain.

### 1. Setup the Dev Container
```sh
docker compose build dev
docker compose run --rm dev cargo build --release -p rete-cli
```

### 2. Build and Query a Graph
```sh
# Build a .rete file from N-Triples (--card adds self-describing metadata):
rete build examples/social.nt -o social.rete --card --title "Social graph"

# Run a simple pattern match:
rete query social.rete --predicate '<http://ex/knows>'

# Run a full SPARQL query:
rete sparql social.rete "PREFIX e: <http://ex/> SELECT ?p ?age WHERE { ?p e:age ?age . FILTER(?age > 27) }"
```

### 3. Explore the Schema (Index-Free)
```sh
# View metadata, vocabulary, and starter queries:
rete card social.rete

# View the schema pyramid at its highest, most abstract level:
rete summary social.rete --level 0
```

### 4. Query Over the Web
You can query a `.rete` file hosted on any static web server:
```sh
# Fetch only the exact bytes needed to answer the query:
rete query-url https://my-bucket.s3.amazonaws.com/social.rete --object '<http://ex/Alice>'
```

## Available Clients

You can query `.rete` files from almost any language. All clients use the same core engine and support lazy HTTP range-reads:

| Client | How to Install | Best For |
|---|---|---|
| **Python / Pyodide** | `pip install rete-graph` | Data science, Jupyter, Colab |
| **JavaScript / TS** | `npm install rete-graph` | Node.js backends and browser apps |
| **Java** | `mvn install` (in `clients/java`) | JVM ecosystems, RDF4J integration |
| **R** | `remotes::install_github(...)` | Data analysis, data frames |
| **Rust** | Use `rete-core` / `rete-cli` crates | Native performance, CLI tooling |
| **Claude MCP** | Install the [MCP Bundle](https://data.graphplaza.com/mcpb/rete.mcpb) | Giving AI agents access to your graphs |

### Claude Code Plugin
You can give Claude direct access to public knowledge graphs and your local `.rete` files:
```sh
/plugin marketplace add caviri/rete
/plugin install rete-graph@rete
```

## Learn More

- **[Graph Data 101](https://caviri.github.io/rete/intro.html)** — A beginner's guide to RDF and graphs.
- **[Getting Started Guide](https://caviri.github.io/rete/getting-started.html)** — Full tutorial on building and querying files.
- **[Architecture](https://caviri.github.io/rete/architecture.html)** — How the engine and file format actually work.
- **[SPARQL Support](https://caviri.github.io/rete/sparql.html)** — Supported syntax and functions.
- **[Semantic Zoom](https://caviri.github.io/rete/semantic-zoom.html)** — How the index-free schema pyramid works.

## License & Support
`rete` is free and open-source software licensed under [Apache-2.0](LICENSE). 
If you find it useful, consider supporting its development on [Ko-fi](https://ko-fi.com/M1W723PEW3).
