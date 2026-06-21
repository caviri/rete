# rete

**A single-file, range-queryable RDF graph format — put it on a URL, run SPARQL, no server.**

[`github.com/caviri/rete`](https://github.com/caviri/rete) · crate **v0.1.0** · on-disk format **v0.4** (clean break — rebuild older files)

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

## Documentation

### Start here

- **[Graph data 101](intro.md)** — new to graphs/RDF? A beginner's tour, framed by the questions you can ask.
- **[Getting started](getting-started.md)** — install (Docker-only), build, query, deploy, generate test data.
- **[Real-world scenario](scenario.md)** — publish a queryable SBOM to a URL; curl examples.
- **[Interactive playground](playground.html)** — a static, no-server page that runs the engine in your browser: SELECT/ASK/CONSTRUCT, progressive summaries, SHACL, reachability — all offline (even from `file://`).

### Guides

- **[CLI reference](cli.md)** — every `rete` subcommand.
- **[SPARQL support](sparql.md)** — exactly what the engine evaluates.
- **[SHACL validation](shacl.md)** — validate `.rete` graphs against SHACL Core shapes.
- **[Dataset Cards](dataset-cards.md)** — self-describing metadata embedded in the file.
- **[Reasoning & coherence](reasoning.md)** — prototype OWL RL / RDFS reasoner; find incoherent points.
- **[Federated queries](federation.md)** — query several `.rete` files (local paths and/or URLs) as one; union merge + predicate routing.
- **[Compatibility & interop](compatibility.md)** — RDF, validation, and Cypher.

### Graph analysis

- **[Topic modeling (LDA)](topic-modeling.md)** — label each community's theme: `rete communities` + scikit-learn LDA.
- **[Multi-criteria communities](multi-criteria.md)** — partition the same graph by different relations/attributes; combine criteria.

### In the browser

- **[Browser / WASM](browser.md)** — query in the browser; progressive loading.
- **[Parallel in the browser (experimental)](parallel-browser.md)** — a working Web Worker reachability demo, plus the more fragile shared-memory WASM thread path.

### Internals

- **[Architecture](architecture.md)** — workspace map, build/read/query pipelines, range model, and extension points.
- **[Format specification](SPEC.md)** — the on-disk byte layout, for implementers.
- **[Benchmarks](BENCHMARK.md)** — sizing, the OpenCitations/Oxigraph comparison, and the LUBM-style suite, generated from JSON snapshots.

## Version & status

Developed in the open at [github.com/caviri/rete](https://github.com/caviri/rete).
The crates are **v0.1.0** (experimental); the on-disk format is **v0.4**, and each
version step is a clean break (readers accept only the current version), so rebuild
older files. The format is not yet stable across versions.
Everything is built and tested in Docker — see
[Getting started](getting-started.md).
