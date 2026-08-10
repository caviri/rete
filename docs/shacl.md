# SHACL Validation

The `rete shacl` command validates a `.rete` file against **SHACL Core** shapes. It's the perfect tool for release checks and CI pipelines: build your graph, run your shapes against it, and fail the pipeline if the data is non-conformant.

```sh
# 1. Build the data
rete build data.ttl -o data.rete

# 2. Validate against shapes
rete shacl data.rete --shapes shapes.ttl

# 3. Output as JSON
rete shacl data.rete --shapes shapes.ttl --format json

# 4. Validate a specific named graph
rete shacl data.rete --shapes shapes.ttl --graph '<http://ex/releases/2026-06>'
```

If the validation fails (finds non-conformant results), the command exits with a non-zero status.

## Validating Remote Graphs (Lazy Loading)

SHACL validation works node-by-node. Because of Rete's indexed architecture, you can validate a remote `.rete` file over HTTP **without downloading the whole file!** Rete will perform targeted range-reads to fetch only the nodes the SHACL shapes actually care about.

```sh
rete shacl-url https://host/data.rete --shapes shapes.ttl
# Result: Fetched 38KB (7 requests) out of a 1MB file!
```

> [!TIP]
> **Performance Note:** Targeted shapes (`sh:targetClass`, `targetNode`, etc.) are blazing fast over the network. However, "target-less" shapes (which implicitly target *every* node) will force the engine to read the entire graph. Keep your shapes targeted for best performance.

## A Minimal Example Shape

Here is a simple SHACL shape enforcing that every `ex:Person` must have exactly one email string:

```turtle
@prefix sh: <http://www.w3.org/ns/shacl#> .
@prefix ex: <http://ex/> .
@prefix xsd: <http://www.w3.org/2001/XMLSchema#> .

ex:PersonShape
  a sh:NodeShape ;
  sh:targetClass ex:Person ;
  sh:property [
    sh:path ex:email ;
    sh:minCount 1 ;
    sh:maxCount 1 ;
    sh:datatype xsd:string ;
    sh:pattern "@" ;
    sh:message "Every person needs exactly one email string." ;
  ] .
```

Run it:
```sh
rete shacl people.rete --shapes person-shapes.ttl
```

The output is clean and human-readable:
```text
conforms: false

- focus: <http://ex/Alice>
  severity: http://www.w3.org/ns/shacl#Violation
  component: http://www.w3.org/ns/shacl#MinCountConstraintComponent
  path: <http://ex/email>
  shape: _:b1
  message: Every person needs exactly one email string.
```

*(You can also use `--format json` for CI parsing, or `--format ttl` for a standard Turtle report).*

## What SHACL Features are Supported?

Rete targets the stable **W3C SHACL Core 2017 Recommendation**. (SHACL-SPARQL, SHACL-JS, and ShEx are not supported).

### Targets Supported
- `sh:targetNode`, `sh:targetSubjectsOf`, `sh:targetObjectsOf`
- `sh:targetClass` (including `rdfs:subClassOf` closure in the graph)
- Implicit class targets (`rdfs:Class`, `owl:Class`)
- Metadata: `sh:deactivated`, `sh:severity`, `sh:message`

### Property Paths Supported
- Predicate IRIs
- `sh:inversePath`, `sh:alternativePath`
- `sh:zeroOrMorePath`, `sh:oneOrMorePath`, `sh:zeroOrOnePath`
- RDF list sequence paths

### Constraint Components Supported
| Category | Supported Constraints |
|---|---|
| **Cardinality** | `sh:minCount`, `sh:maxCount` |
| **Value Type** | `sh:class`, `sh:datatype`, `sh:nodeKind` |
| **Value Range** | `sh:minInclusive`, `sh:maxInclusive`, `sh:minExclusive`, `sh:maxExclusive` |
| **Strings** | `sh:minLength`, `sh:maxLength`, `sh:pattern`, `sh:flags`, `sh:languageIn`, `sh:uniqueLang` |
| **Property Pairs**| `sh:equals`, `sh:disjoint`, `sh:lessThan`, `sh:lessThanOrEquals` |
| **Value Sets** | `sh:hasValue`, `sh:in` |
| **Nested Shapes** | `sh:node`, `sh:property` |
| **Logic** | `sh:not`, `sh:and`, `sh:or`, `sh:xone` |
| **Closed Shapes** | `sh:closed`, `sh:ignoredProperties` |
| **Qualified** | `sh:qualifiedValueShape`, `sh:qualifiedMinCount`, `sh:qualifiedMaxCount`, `sh:qualifiedValueShapesDisjoint` |

## Limitations
- Shapes must be provided in **Turtle** format.
- Recursive shape cycles are reported as validation results (they won't crash the engine).
- If your `.rete` file was built with `--materialize`, SHACL validates the **entire materialized graph**. Otherwise, it only validates the explicitly asserted data.

## Rust Core API

You can use the SHACL engine programmatically in Rust. `validate_shacl` accepts either an eager, fully-loaded `DataGraph`, or a lazy `ReteGraph` that streams only what it needs over HTTP!

```rust
use rete_core::{validate_shacl, DataGraph, ReteGraph, Rete, ShaclShapes};

// 1. Parse the shapes
let shapes = ShaclShapes::parse_turtle(&std::fs::read_to_string("shapes.ttl")?)?;

// 2. EAGER METHOD: Load the whole graph in memory
let rete = Rete::open(&std::fs::read("data.rete")?)?;
let report = validate_shacl(&DataGraph::from_rete(&rete, None), &shapes);

// 3. LAZY METHOD: Validate over a range reader (HTTP backend)
let rete = Rete::open_ranged_lazy(reader)?;
let report = validate_shacl(&ReteGraph::new(&rete), &shapes);
```
*(Pass `Some("<graph-iri>")` to `DataGraph::from_rete` to validate a specific named graph).*
