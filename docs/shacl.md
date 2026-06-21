# SHACL validation

`rete shacl` validates a `.rete` file against SHACL Core shapes. It is meant for
release checks and CI gates: build the graph once, run the shape file against the
default graph or a named graph, and fail the job if the validation report is not
conformant.

```sh
rete build data.ttl -o data.rete
rete shacl data.rete --shapes shapes.ttl
rete shacl data.rete --shapes shapes.ttl --format json
rete shacl data.rete --shapes shapes.ttl --graph '<http://ex/releases/2026-06>'
```

The command exits with status `0` when the report conforms and non-zero when it
finds at least one result. Shapes are read as Turtle. Data comes from the `.rete`
file, so validation runs over exactly the graph you publish and query.

## Validating a remote graph (lazy)

SHACL validation is **node-by-node**: a shape names its targets (a class, a node,
the subjects/objects of a predicate), then checks each focus node's own property
values. That maps onto **routed range reads**, so a remote `.rete` can be
validated over HTTP *without downloading it* — each lookup faults only the tiles
holding the target nodes and their values:

```sh
rete shacl-url https://host/data.rete --shapes shapes.ttl
# (fetched 38912 bytes in 7 range request(s); file is 1048576 bytes)
```

A **targeted** shape (`sh:targetClass` / `targetNode` / `targetSubjectsOf` /
`targetObjectsOf`) fetches only its targets — the win grows with the file. The one
exception is a **target-less shape** (one that implicitly targets every node) or a
general inverse path, which must read the whole graph; `^predicate` inverse paths
are routed (they become a single `(?, p, focus)` lookup). `shacl-url` validates the
default graph.

## A minimal shape

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

```sh
rete shacl people.rete --shapes person-shapes.ttl
```

Text output is compact and human-oriented:

```text
conforms: false

- focus: <http://ex/Alice>
  severity: http://www.w3.org/ns/shacl#Violation
  component: http://www.w3.org/ns/shacl#MinCountConstraintComponent
  path: <http://ex/email>
  shape: _:b1
  message: Every person needs exactly one email string.
```

Use `--format json` for automation and `--format ttl` for a Turtle validation
report.

## Supported SHACL surface

This implementation targets the stable W3C SHACL Core 2017 Recommendation. It
does not try to implement SHACL-SPARQL, SHACL-AF, SHACL-JS, SHACL 1.2 drafts, or
ShEx.

Supported targets:

| Feature | Status |
|---|---|
| `sh:targetNode` | yes |
| `sh:targetClass` | yes, including `rdfs:subClassOf` closure in the data graph |
| `sh:targetSubjectsOf` | yes |
| `sh:targetObjectsOf` | yes |
| implicit class targets (`rdfs:Class`, `owl:Class`) | yes |
| `sh:deactivated`, `sh:severity`, `sh:message` | yes |

Supported property paths:

| Path form | Status |
|---|---|
| predicate IRI | yes |
| `sh:inversePath` | yes |
| RDF list sequence paths | yes |
| `sh:alternativePath` | yes |
| `sh:zeroOrMorePath` | yes |
| `sh:oneOrMorePath` | yes |
| `sh:zeroOrOnePath` | yes |

Supported constraint components:

| Constraint family | Components |
|---|---|
| Cardinality | `sh:minCount`, `sh:maxCount` |
| Value type | `sh:class`, `sh:datatype`, `sh:nodeKind` |
| Value range | `sh:minInclusive`, `sh:maxInclusive`, `sh:minExclusive`, `sh:maxExclusive` |
| Strings and languages | `sh:minLength`, `sh:maxLength`, `sh:pattern`, `sh:flags`, `sh:languageIn`, `sh:uniqueLang` |
| Property pairs | `sh:equals`, `sh:disjoint`, `sh:lessThan`, `sh:lessThanOrEquals` |
| Value sets | `sh:hasValue`, `sh:in` |
| Nested shapes | `sh:node`, `sh:property` |
| Logical constraints | `sh:not`, `sh:and`, `sh:or`, `sh:xone` |
| Closed shapes | `sh:closed`, `sh:ignoredProperties` |
| Qualified values | `sh:qualifiedValueShape`, `sh:qualifiedMinCount`, `sh:qualifiedMaxCount`, `sh:qualifiedValueShapesDisjoint` |

## Current limits

- Shape files must be Turtle.
- SHACL-SPARQL constraints, custom functions, rules, and JavaScript extensions
  are out of scope for this command.
- Recursive shape cycles are reported as validation results instead of being
  treated as fatal engine errors.
- Rete files are immutable snapshots. If your build uses `--materialize`, SHACL
  validates the already-materialized graph; otherwise it validates asserted data
  only.

## Core API

The CLI uses the reusable core API. `validate_shacl` is generic over a
[`GraphView`] — the data graph can be an in-memory `DataGraph` (eager) or a
`ReteGraph` that routes every lookup as a range read (lazy / remote):

```rust
use rete_core::{validate_shacl, DataGraph, ReteGraph, Rete, ShaclShapes};

let shapes = ShaclShapes::parse_turtle(&std::fs::read_to_string("shapes.ttl")?)?;

// Eager: whole graph in memory.
let rete = Rete::open(&std::fs::read("data.rete")?)?;
let report = validate_shacl(&DataGraph::from_rete(&rete, None), &shapes);

// Lazy: validate over a range reader (e.g. an HTTP backend) — fetches only the
// shapes' targets.
let rete = Rete::open_ranged_lazy(reader)?;
let report = validate_shacl(&ReteGraph::new(&rete), &shapes);
```

Pass `Some("<graph-iri>")` to `DataGraph::from_rete` to validate one named graph
(the lazy `ReteGraph` validates the default graph).
