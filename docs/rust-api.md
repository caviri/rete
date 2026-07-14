# Rust API

`rete-core` is the embeddable Rust implementation of the `.rete` format. Starting
with `1.0.0-rc.1`, its supported SemVer surface is organized into five facade
modules:

| Module | Supported purpose |
|---|---|
| [`format`](https://docs.rs/rete-core/latest/rete_core/format/) | Parse RDF, build or open a `.rete`, inspect its header, and verify the container |
| [`query`](https://docs.rs/rete-core/latest/rete_core/query/) | Evaluate SPARQL and graph patterns, format results, and configure federation |
| [`range`](https://docs.rs/rete-core/latest/rete_core/range/) | Implement byte-range readers and open summary data without loading the full file |
| [`validation`](https://docs.rs/rete-core/latest/rete_core/validation/) | Validate an eager graph or a `.rete` index with SHACL |
| [`reasoning`](https://docs.rs/rete-core/latest/rete_core/reasoning/) | Run the supported RDFS/OWL rules and inspect schema coherence |

Use those module paths in application code. The crate retains some hidden root
exports and implementation namespaces for the other workspace crates, but they
are not part of the 1.x compatibility contract and may change without a major
release.

## Add the release candidate

Until the final release, pin the release candidate explicitly:

```toml
[dependencies]
rete-core = "=1.0.0-rc.1"
```

The default feature set supports compressed `.rete` files. Optional parallel or
browser-thread features remain experimental; they do not change the five facade
module names.

## Build, open, and query

The in-memory path is useful for applications that already have RDF text or a
complete file image:

```rust
use rete_core::format::{assemble_dataset, parse_statements, Rete};
use rete_core::query::{eval_query, QueryOutput};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let rdf = "<urn:alice> <urn:knows> <urn:bob> .";
    let quads = parse_statements(rdf, "nt")?;
    let (bytes, stats) = assemble_dataset(quads, br#"{"source":"example"}"#);
    assert_eq!(stats.statements, 1);

    let graph = Rete::open(&bytes)?;
    match eval_query(&graph, "SELECT ?o WHERE { <urn:alice> <urn:knows> ?o }")? {
        QueryOutput::Select(vars, rows) => {
            assert_eq!(vars, ["o"]);
            assert_eq!(rows.len(), 1);
        }
        _ => unreachable!("the query is SELECT"),
    }
    Ok(())
}
```

`QueryOutput`, public error enums, and other result-shape enums are
non-exhaustive. Always keep a wildcard arm when matching them; new variants can
then be added in a compatible 1.x release.

## Read by byte range

Implement `RangeReader` for a local file, object-store client, or HTTP client.
Every range must either return exactly the requested bytes or an error; offsets
from an untrusted remote file must not be used as unchecked slice indexes.

```rust
use rete_core::range::{RangeReader, SliceReader, SummaryView};

fn inspect_summary(bytes: &[u8]) -> Result<(), Box<dyn std::error::Error>> {
    let reader = SliceReader::new(bytes);
    let _file_len = reader.len();
    if let Some(summary) = SummaryView::open_ranged(&reader)? {
        println!("{} communities", summary.community_count());
    }
    Ok(())
}
```

Remote readers should preserve HTTP `Range` semantics. See [Hosting your
`.rete`](hosting.md) for required CORS response headers and [WASM & JavaScript
API](browser.md) for the browser-specific reader.

## Validate and reason

SHACL can operate directly against a `.rete` index, so a lazy remote graph only
fetches ranges used by the shapes:

```rust
use rete_core::format::Rete;
use rete_core::validation::{validate_shacl, ReteGraph, ShaclShapes};

fn validate(bytes: &[u8], shapes_turtle: &str) -> Result<bool, Box<dyn std::error::Error>> {
    let rete = Rete::open(bytes)?;
    let shapes = ShaclShapes::parse_turtle(shapes_turtle)?;
    let report = validate_shacl(&ReteGraph::new(&rete), &shapes);
    Ok(report.conforms)
}
```

The reasoning facade exposes the documented ruleset and both eager reasoning
and reasoned query entry points. It is deliberately a bounded RDFS/OWL profile,
not a complete OWL reasoner; see [Reasoning & coherence](reasoning.md) for the
supported entailments and inconsistency checks.

## Compatibility policy

- `.rete` format compatibility and Rust API compatibility are versioned
  independently. The format byte and read window live in `format`.
- The five facade modules follow SemVer from `1.0.0-rc.1` onward. Hidden modules
  and experimental feature-gated APIs do not.
- Public errors and result shapes are non-exhaustive. Treat unfamiliar variants
  as unsupported input or output rather than panicking.
- Rustdoc is built with warnings denied, and release CI compares the facade with
  the previous release using `cargo-semver-checks`.

For the byte-level contract, read the [format specification](SPEC.md).
