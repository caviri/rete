# API and technical reference

Use this directory when you need an exact interface, supported behavior or
format detail. If you are still choosing a workflow, begin with
[Build and publish](build-and-publish.md) or [Query and analyze](query-and-analyze.md).

## Interfaces and behavior

| Reference | What it answers |
|---|---|
| [CLI](cli.md) | Commands, arguments, defaults and output |
| [SPARQL](sparql.md) | Query syntax and evaluation support |
| [Python](python.md), [JavaScript](javascript.md), [R](r.md), [Rust](rust-api.md) | Client usage and result types |
| [WASM bindings](browser.md) | Low-level browser engine interfaces |
| [Compatibility](compatibility.md) | Reader/file compatibility and feature restrictions |
| [Interoperability](interop.md) | Import/export formats and graph preservation |
| [GeoSPARQL](geosparql.md), [SHACL](shacl.md), [reasoning](reasoning.md) | Spatial functions, validation and inference |
| [Federation](federation.md) | Combining files and remote endpoints |
| [Writable graphs](manifest.md) | Manifest and journal semantics |
| [Dataset Card fields](dataset-card-reference.md) | Metadata schema, projections and maintenance contracts |
| [Hosting](hosting.md) | HTTP and browser hosting requirements |
| [Media and SQL companions](media-companions.md) | Additional representations and assets |

## How rete works

The [format specification](SPEC.md) describes the bytes on disk.
[Architecture](architecture.md) explains the implementation, while
[semantic zoom](semantic-zoom.md) explains the summaries used to explore a graph.

[Large builds and tables](data-engineering.md) covers data preparation and
scale. [Parallel browser execution](parallel-browser.md) describes the
experimental threading paths and their requirements.

## Evidence and development

[Benchmarks](BENCHMARK.md) records measurements and their conditions.
[SPARQL conformance](conformance.md) records test coverage and limitations;
neither is a promise about every workload.

For building and changing the project, see [Contributing](contributing.md),
[Client development](clients-dev.md) and the [Release guide](release.md).
