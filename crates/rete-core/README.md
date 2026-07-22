# rete-core

`rete-core` is the embeddable Rust engine for the range-queryable `.rete` RDF
graph format. It builds and opens immutable graph files, evaluates SPARQL,
supports lazy byte-range readers, validates SHACL, and provides a bounded
RDFS/OWL reasoning profile.

This is a 0.x release: the `.rete` format is already at stable generation 1, but
the Rust API carries no semantic-versioning promise until 1.0.0. Pin it
explicitly while the crates are 0.x:

```toml
[dependencies]
rete-core = "=0.3.0"
```

## Open and query a file

```rust
use rete_core::format::Rete;
use rete_core::query::{eval_query, QueryOutput};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let bytes = std::fs::read("graph.rete")?;
    let graph = Rete::open(&bytes)?;

    match eval_query(&graph, "SELECT ?s ?o WHERE { ?s <urn:knows> ?o }")? {
        QueryOutput::Select(vars, rows) => {
            println!("variables: {vars:?}; {} row(s)", rows.len());
        }
        _ => unreachable!("the query is SELECT"),
    }
    Ok(())
}
```

Use the stable facade modules in application code:

- `rete_core::format` — file headers, building, opening, and verification
- `rete_core::query` — SPARQL, graph patterns, results, and federation
- `rete_core::range` — local or remote byte-range readers
- `rete_core::validation` — SHACL validation
- `rete_core::reasoning` — RDFS/OWL reasoning and schema coherence

Public error and result enums are non-exhaustive; keep a wildcard arm when
matching them. Hidden root exports and implementation namespaces are used by
the Rete workspace but are not part of the 1.x SemVer contract.

See the [Rust API guide](https://caviri.github.io/rete/rust-api.html), the
[format specification](https://caviri.github.io/rete/SPEC.html), and
[docs.rs](https://docs.rs/rete-core).

## License

Apache-2.0. See `LICENSE`.
