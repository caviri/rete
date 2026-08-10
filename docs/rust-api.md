# Rust API

Welcome to the `rete-core` Rust API! This crate provides the embeddable Rust implementation of the `.rete` format.

To keep things simple and stable, the API is organized into **five core facade modules**. These will form the SemVer-stable API when we reach `1.0.0`.

### The Five Core Modules

- **[`format`](https://docs.rs/rete-core/latest/rete_core/format/)**: Your starting point. Use it to parse RDF, build or open a `.rete` file, inspect headers, and verify the container.
- **[`query`](https://docs.rs/rete-core/latest/rete_core/query/)**: The engine room. Evaluate SPARQL queries and graph patterns, format your results, and configure federation.
- **[`range`](https://docs.rs/rete-core/latest/rete_core/range/)**: For the network-savvy. Implement byte-range readers and open summary data lazily without downloading the entire file.
- **[`validation`](https://docs.rs/rete-core/latest/rete_core/validation/)**: Ensure data quality. Validate an eager graph or a `.rete` index against SHACL shapes.
- **[`reasoning`](https://docs.rs/rete-core/latest/rete_core/reasoning/)**: Add logic. Run supported RDFS/OWL rules and inspect your schema's coherence.

> **Tip:** Always use these five module paths in your application code. While the crate contains hidden exports for internal workspace use, they are not part of the stable 1.x contract and might change.

## Getting Started

### 1. Add the Dependency

Until the `1.0.0` release, we recommend pinning the exact `0.x` version in your `Cargo.toml`:

```toml
[dependencies]
rete-core = "=0.3.0"
```

The default feature set handles compressed `.rete` files out of the box. (Note: Optional parallel or browser-thread features are experimental but won't change the five facade module names).

### 2. Build, Open, and Query

If you already have RDF text or a complete file image in memory, here's how you can quickly build, open, and run a query:

```rust
use rete_core::format::{assemble_dataset, parse_statements, Rete};
use rete_core::query::{eval_query, QueryOutput};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // 1. Parse your RDF data
    let rdf = "<urn:alice> <urn:knows> <urn:bob> .";
    let quads = parse_statements(rdf, "nt")?;
    
    // 2. Assemble it into a .rete format
    let (bytes, stats) = assemble_dataset(quads, br#"{"source":"example"}"#);
    assert_eq!(stats.statements, 1);

    // 3. Open the graph and run a SPARQL query
    let graph = Rete::open(&bytes)?;
    match eval_query(&graph, "SELECT ?o WHERE { <urn:alice> <urn:knows> ?o }")? {
        QueryOutput::Select(vars, rows) => {
            assert_eq!(vars, ["o"]);
            assert_eq!(rows.len(), 1);
        }
        _ => unreachable!("The query is SELECT"),
    }
    
    Ok(())
}
```

> **Note on Enums:** Result-shape enums like `QueryOutput` and public error enums are **non-exhaustive**. Always include a wildcard (`_ => ...`) arm when matching against them to ensure your code remains compatible with future 1.x releases.

### 3. Read by Byte Range

If you're reading large files locally or over the network, you don't need to load everything at once. Implement the `RangeReader` trait for your storage backend (local file, S3, HTTP, etc.).

```rust
use rete_core::range::{RangeReader, SliceReader, SummaryView};

fn inspect_summary(bytes: &[u8]) -> Result<(), Box<dyn std::error::Error>> {
    let reader = SliceReader::new(bytes);
    let _file_len = reader.len();
    
    // Lazily open the summary view without reading the full index
    if let Some(summary) = SummaryView::open_ranged(&reader)? {
        println!("Found {} communities!", summary.community_count());
    }
    
    Ok(())
}
```

**Network Readers:** When implementing remote readers, ensure they preserve standard HTTP `Range` semantics. For browser specifics, check out the [WASM & JavaScript API](browser.md), and for CORS requirements, see [Hosting your `.rete`](hosting.md).

### 4. Validate and Reason

Need to ensure your data follows specific rules? You can run SHACL validation directly against a `.rete` index. Because it's lazy, a remote graph will only fetch the byte ranges required by the shapes!

```rust
use rete_core::format::Rete;
use rete_core::validation::{validate_shacl, ReteGraph, ShaclShapes};

fn validate(bytes: &[u8], shapes_turtle: &str) -> Result<bool, Box<dyn std::error::Error>> {
    let rete = Rete::open(bytes)?;
    let shapes = ShaclShapes::parse_turtle(shapes_turtle)?;
    
    // Validate the graph against the parsed shapes
    let report = validate_shacl(&ReteGraph::new(&rete), &shapes);
    
    Ok(report.conforms)
}
```

For logic and schemas, the `reasoning` facade provides both eager reasoning and reasoned query entry points. It implements a focused RDFS/OWL profile (not full OWL DL) — see [Reasoning & coherence](reasoning.md) for details on supported entailments.

## Compatibility Policy

We take stability seriously. Here is what you can expect:

- **Format vs. API:** The `.rete` file format and the Rust API are versioned independently. The format byte and read window are managed in the `format` module.
- **SemVer:** The five facade modules will strictly follow Semantic Versioning starting from `1.0.0`. Hidden modules and experimental APIs are exempt from this guarantee.
- **Non-Exhaustive Types:** Public errors and result enums are non-exhaustive. Treat unfamiliar variants as unsupported rather than panicking.
- **Safety Checks:** We build rustdocs with warnings denied and use `cargo-semver-checks` in our CI to catch accidental breaking changes.

For the byte-level contract, refer to the [format specification](SPEC.md).
