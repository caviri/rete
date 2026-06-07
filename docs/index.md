# rete

**A single-file, range-queryable RDF graph format — put it on a URL, run SPARQL, no server.**

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
rete sparql social.rete "PREFIX e: <http://ex/> SELECT ?p ?age WHERE { ?p e:age ?age . FILTER(?age > 27) }"

# Query straight from a URL — fetches only the byte ranges needed (http or https):
rete query-url https://my-bucket.s3.amazonaws.com/social.rete --object '<http://ex/Alice>'

# Look at the coarse graphs without reading the index:
rete summary social.rete    # structural (Louvain communities)
rete schema  social.rete    # semantic (by rdf:type)
```

## Documentation

- **[Graph data 101](intro.md)** — new to graphs/RDF? A beginner's tour, framed by the questions you can ask.
- **[Getting started](getting-started.md)** — install (Docker-only), build, query, deploy.
- **[Real-world scenario](scenario.md)** — publish a queryable SBOM to a URL; curl examples.
- **[CLI reference](cli.md)** — every `rete` subcommand.
- **[SPARQL support](sparql.md)** — exactly what the engine evaluates.
- **[Compatibility & interop](compatibility.md)** — RDF, validation, and Cypher.
- **[Reasoning & coherence](reasoning.md)** — prototype OWL RL / RDFS reasoner; find incoherent points.
- **[Topic modeling (LDA)](topic-modeling.md)** — label each community's theme: `rete communities` + scikit-learn LDA.
- **[Multi-criteria communities](multi-criteria.md)** — partition the same graph by different relations/attributes; combine criteria.
- **[Federated queries](federation.md)** — query several `.rete` files (local paths and/or URLs) as one; union merge + predicate routing.
- **[Browser / WASM](browser.md)** — query in the browser; progressive loading.
- **[Interactive playground](playground.html)** — a static, no-server, runs-entirely-in-your-browser page: it embeds the WASM engine and the example datasets, so SELECT/ASK/CONSTRUCT, Table/Turtle/JSON-LD, the community view, reachability, and history all work offline (even from `file://`).
- **[Format specification](SPEC.md)** — the on-disk byte layout, for implementers.
- **[Benchmarks](BENCHMARK.md)** — size and latency on a 139 k-triple graph.
- **[Parallel in the browser (experimental)](parallel-browser.md)** — real WASM threads via `wasm-bindgen-rayon` (served, cross-origin-isolated).

## Status

Experimental (v0). The format is not yet stable across versions. Everything is
built and tested in Docker — see [Getting started](getting-started.md).
