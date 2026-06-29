# Compatibility, validation & interop

## Is it compatible with RDF?

**Yes — `rete` is RDF.** It is not a new graph model; it's a storage + query
format *for* RDF, built on the standard Rust RDF stack
([`oxrdf`](https://crates.io/crates/oxrdf),
[`oxttl`](https://crates.io/crates/oxttl),
[`spargebra`](https://crates.io/crates/spargebra)).

It implements the RDF 1.1 data model:

| RDF concept | Support |
|---|---|
| IRIs | ✅ |
| Literals (plain) | ✅ |
| Typed literals (`"30"^^xsd:integer`) | ✅ — datatype preserved |
| Language-tagged literals (`"hi"@en`) | ✅ — tag preserved |
| Blank nodes | ✅ (as terms; non-distinguished variables in patterns) |
| Named graphs / RDF datasets | ✅ — N-Quads in, `GRAPH`/`FROM`/`FROM NAMED` in SPARQL |
| SPARQL 1.1 query | ✅ — see [SPARQL support](sparql.md) |
| SHACL Core validation | ✅ — see [SHACL validation](shacl.md) |

**Input formats:** N-Triples (`.nt`), N-Quads (`.nq`), Turtle (`.ttl`), and
RDF/XML (`.rdf` / `.owl` / `.rdfxml`).
**Output:** N-Quads (`rete export`, lossless round-trip), Turtle and expanded
JSON-LD (`rete export --format ttl|jsonld`, default graph only), and SPARQL
Results JSON (`rete sparql --json`).

**Interop in practice:** anything that emits N-Triples/N-Quads/Turtle/RDF-XML can
feed `rete build`, and `rete export` round-trips back to N-Quads for any other RDF
tool. So `rete` slots in as a *publishing + query* layer next to your existing RDF
pipeline.

**OWL:** OWL is a *language*, not a file format — an ontology is a set of RDF
triples that can be serialized several ways. The two common RDF serializations,
**Turtle** and **RDF/XML**, both ingest directly (`.ttl`, `.rdf`/`.owl`), so most
published OWL ontologies build with no conversion. The non-RDF serializations —
**OWL/XML** (functional XML) and **OWL Functional Syntax** — are *not* RDF, so
convert them to RDF first (e.g. `owlready2`, `robot convert`, or Protégé "Save
as → RDF/XML"). Once ingested, OWL axioms are just triples you can query; to
*materialize* OWL RL / RDFS entailments see [Reasoning](reasoning.md)
(`rete build --reason` / `rete reason`).

**Current limits (not RDF-incompatible, just unimplemented):** no RDF-star
(quoted triples); OWL/XML and Functional Syntax need an external convert-to-RDF
step (above); and there is no SPARQL Update (the file is immutable by design).
Turtle/JSON-LD export covers the default graph only (use N-Quads export for named
graphs).

## Validation

There are five independent ways to check correctness:

1. **Syntactic — `rete validate`.** Parses N-Triples/N-Quads/Turtle without
   building, and fails with a precise line/column error on malformed input:

   ```sh
   rete validate data.ttl
   #  valid: 4210 statement(s) — 4180 in the default graph, 2 named graph(s)
   rete validate broken.ttl
   #  Error: broken.ttl: Parser error at line 12 ... not a valid subject
   ```

   `rete build` runs the same parse, so a successful build is also a validation.

2. **Shape validation — `rete shacl`.** Validates a `.rete` graph against SHACL
   Core shapes read from Turtle. It exits non-zero when the validation report is
   non-conformant, so it works as a CI gate. See [SHACL validation](shacl.md) for
   the supported target, path, and constraint components.

   ```sh
   rete shacl data.rete --shapes shapes.ttl
   rete shacl data.rete --shapes shapes.ttl --format json
   ```

3. **Integrity — `rete verify`.** Recomputes the blake3 content hash and compares
   it to the header, detecting any corruption or truncation of a `.rete` file.

4. **Round-trip — `rete export`.** Dumps back to N-Quads; diff against the source
   (or re-validate the output) to confirm nothing was lost.

5. **Logical coherence — `rete reason`.** A prototype OWL RL / RDFS reasoner
   materializes RDFS/OWL entailments and flags *incoherent points* — logical
   contradictions such as disjoint-class violations, `sameAs`/`differentFrom`
   clashes, functional-property conflicts, and `owl:Nothing` membership. It exits
   non-zero on incoherence, so it doubles as a CI coherence gate. See
   [Reasoning & coherence](reasoning.md) for the full rule set and scope (it is a
   documented subset, not full OWL DL).

   ```sh
   rete reason data.rete
   #  inferred 9 new triple(s)
   #  1 inconsistency(ies) found:
   #    [disjoint-classes] <http://ex/p> is typed as both … owl:disjointWith
   ```

`rete shacl` is SHACL Core support, not the whole shape-language universe:
SHACL-SPARQL, SHACL-AF, JavaScript extensions, SHACL 1.2 draft features, and ShEx
are not implemented.

## Could it speak Cypher too?

Short answer: **not today, and it's a different data model — but a useful subset
could be translated to SPARQL.** Here's the honest picture.

Cypher targets the **labeled property graph (LPG)** model (Neo4j): nodes and
relationships both carry a label and arbitrary key/value *properties*. RDF is a
**triple** model: everything is `(subject, predicate, object)`. They overlap a
lot, but not perfectly:

| Cypher (LPG) | RDF / SPARQL equivalent |
|---|---|
| `(a:Person)` (node label) | `?a rdf:type ex:Person` |
| `(a)-[:KNOWS]->(b)` | `?a ex:knows ?b` (a triple / BGP) |
| `(a)-[:KNOWS*]->(b)` (var-length) | `?a ex:knows+ ?b` (property path) |
| node property `a.age` | `?a ex:age ?age` (triple with a literal) |
| **relationship property** `[:KNOWS {since: 2020}]` | no direct triple — needs reification or RDF-star |
| `RETURN`, `WHERE`, `LIMIT` | `SELECT`, `FILTER`, `LIMIT` |

So a **"loose Cypher"** front-end — read-only `MATCH … WHERE … RETURN`, including
variable-length relationships — maps cleanly onto the BGP + property-path + filter
machinery this engine already has. The genuine gaps are LPG features with no plain
RDF triple: **relationship properties**, and the distinction between a node label
and a node property. Those need a modeling convention (reification, or RDF-star
once supported).

What this is *not*: full openCypher (no writes/`CREATE`/`MERGE`, no `APOC`,
no stored procedures) — the file is immutable and server-less by design.

**Status: available as a prototype** via `rete cypher`. It is a translation
layer, not a second engine: a small Cypher subset is parsed into an AST, emitted
as an equivalent SPARQL `SELECT` string, and evaluated by the existing SPARQL
engine — so it reuses the same BGP/join, property-path, and `FILTER` machinery.

### Supported subset (read-only)

```text
query      := MATCH patterns [WHERE conditions] RETURN items [LIMIT n]
patterns   := pattern ("," pattern)*
pattern    := node (rel node)*
node       := "(" [var] [":" Label] ")"
rel        := "-" "[" ":" REL ["*"] "]" "->"      (forward)
            | "<-" "[" ":" REL ["*"] "]" "-"      (reverse)
conditions := condition (("AND" | "OR") condition)*
condition  := var "." prop  OP  value             (property comparison)
            | var          "=" value              (node identity)
OP         := "=" | "<>" | "!=" | "<" | "<=" | ">" | ">="
value      := number | "string" | <iri>
items      := item ("," item)*
item       := var | var "." prop
```

Variable-length `-[:REL*]->` lowers to the SPARQL property path `REL+`
(**one-or-more**) for the prototype; bounded forms (`*N..M`) are not supported.

### Name → IRI convention

A bare label/relationship/property name `X` maps to `<BASE + X>`, where `BASE`
defaults to `http://ex/` and is overridable with `--base`. With the default base:

| Cypher | Emitted SPARQL |
|---|---|
| `(a:Library)` | `?a a <http://ex/Library>` |
| `-[:dependsOn]->` | predicate `<http://ex/dependsOn>` |
| `a.name` | `?a <http://ex/name> ?a_name` |
| `(a)-[:dependsOn*]->(b)` | `?a <http://ex/dependsOn>+ ?b` |

### Out of scope (rejected with a clear error, never a panic)

Writes (`CREATE` / `MERGE` / `SET` / `DELETE`), `OPTIONAL MATCH`, `WITH`,
aggregations, `RETURN *`, relationship variables/properties
(`[r:REL {since: 2020}]`), and multiple labels per node. These genuinely depend
on LPG features (relationship properties) or write/aggregation semantics outside
this prototype's scope.

### Worked example

Against the bundled `examples/deps.nt` dependency graph:

```sh
rete build examples/deps.nt -o deps.rete

# Which packages transitively depend on the vulnerable log4x?
rete cypher deps.rete \
  "MATCH (a)-[:dependsOn*]->(b) WHERE b = <http://ex/log4x> RETURN a"
# ?a=<http://ex/app>
# ?a=<http://ex/web>
# ?a=<http://ex/auth>
# ?a=<http://ex/logging>
```

That query is translated to
`SELECT ?a WHERE { ?a <http://ex/dependsOn>+ ?b . FILTER(?b = <http://ex/log4x>) }`
and evaluated by the SPARQL engine's property-path machinery.
