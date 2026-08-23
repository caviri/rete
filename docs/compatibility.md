# Compatibility, validation & interop

## Stable `.rete` file compatibility

Format byte `0x05` is stable format generation 1, frozen on **2026-07-14** and
first released in Rete **0.3.0**. Files carrying the experimental generations
`0x01`–`0x04` predate that freeze and must be rebuilt from RDF source. Every
stable Rete reader from 0.3.0 onward reads format `0x05`. Newer readers may
add optional sections and flags that preserve `0x05` semantics. A required layout
change uses a new format byte. Older readers may reject a newer format cleanly;
silent misinterpretation is never permitted.

The current transition is exactly such a layout change. Ordinary builds that
carry all six permutations write `0x06`, pairing SPO/SOP, POS/PSO and OSP/OPS
into three physical index families. Current readers accept both generations and
reconstruct the same six logical orders. The memory-bounded external writer and
`--permutations 3` still write `0x05`; their cutovers are separate work. A reader
that predates `0x06` rejects it from header byte 4 before touching the index.
Published `0x05` files remain readable; this release performs no corpus
migration and removes no legacy path.

> **No backwards-compatibility promise before 1.0.0.** rete reserves the right to
> change the `.rete` format while it is pre-1.0, **including in ways that require
> rebuilding a file you have already published**. `0x05` has not moved since it
> froze on 2026-07-14 — that is a track record, not a guarantee. Keep the RDF
> source you built from; the durable compatibility promise starts at 1.0.0.

**The generation number is not a release version.** There is no Rete 1.0.0: the
workspace is 0.3.x, and the file format froze earlier and independently of any
release version ([release.md](release.md)). Two consequences worth stating,
because both have bitten:

- **`0x05` does not pin reader capability.** Nine days after the freeze,
  [#68](https://github.com/caviri/rete/pull/68) changed what the *writer* emits
  within `0x05` — a single index group larger than the tile budget is now split
  across consecutive tiles — with no generation bump, because the reader change
  was backward compatible in one direction only. A reader older than #68 returns
  **silently incomplete** results on a file that contains split groups, which is
  [#124](https://github.com/caviri/rete/issues/124). "It's `0x05`" therefore
  means "the layout is generation 1", not "any generation-1 reader is safe";
  read published files with a current reader, and rebuild every bundled engine
  (playground WASM, the single-file explorer pages, the clients) before
  publishing a file written by a newer writer.
- **A `min_reader_version` byte would have said that in the file**, and cannot be
  retrofitted into `0x05`. It is item B of
  [#206](https://github.com/caviri/rete/issues/206), the standing survey of what
  a future generation break should batch together.

### `--permutations 3`: a `0x05` file older readers refuse

`rete build --permutations 3` writes SPO, POS and OSP and omits the three
merge-join orders (SOP, PSO, OPS). **The default is six and this section is the
reason to think before changing it.**

The file is still format `0x05` — no layout moved, no section was added, the
header's byte 50 simply carries the permutation mask instead of zero. A reader
that knows the mask answers every query on it with the same rows, from the same
tiles, as it would on the six-permutation twin. A reader that predates the mask
**refuses it**:

```
$ rete stats three.rete          # a Rete built before the mask existed
Error: malformed container: expected 6 permutation sections
$ echo $?
1
$ rete sparql-url http://…/three.rete 'SELECT ?s ?p ?o WHERE { ?s ?p ?o }'
Error: malformed container: unexpected container section count
$ echo $?
1
```

Both the resident decoder and the ranged one check the index container's section
count before reading a payload, so the refusal is the same on a local file, a
lazily range-read one, and every `*-url` command. `rete info`, `rete verify` and
`rete card-url` still succeed, because they read only the header and the metadata
section and never claim to have read the index.

> **Guarantee.** A reader that does not understand the permutation mask never
> returns a row from a file that carries fewer than six permutations. Nothing is
> written empty: a lean file's index container holds **three sections, not six**,
> and both decoders compare that count against the six they expect *before*
> touching a payload. Verified by reading a 3-permutation file with an unmodified
> `rete` built from the previous `main`: `stats`, `sparql`, `export`, `why`,
> `query-url`, `sparql-url`, `cost` and a forced-resident open
> (`RETE_LOCAL_LAZY_ABOVE_MB=0`) each printed one of the two errors above and
> exited 1. That is what makes `--permutations 3` safe to ship without a
> format-generation bump. (A *current* reader accepts the file; it compares the
> section count against the header's mask instead, and says `index container
> section count does not match the header permutation mask` if they disagree.)

That is the good failure — loud, immediate, non-zero exit — and it is the
opposite of [#124](https://github.com/caviri/rete/issues/124), where a stale
reader returned 65,384 rows where 508,116 were correct. It is *not* forward
compatibility: a lean file cannot be published to a fleet of older readers and
be expected to work. Keep the default for anything published, and treat
`--permutations 3` as a choice about a specific consumer that you control.

Which set a file carries is visible without downloading it: `rete info`, `rete
stats` and the Dataset Card's `signals.permutations` all report it, derived from
the header byte rather than stored — see [dataset-cards.md](dataset-cards.md).

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

**RDF-star & RDF 1.2.** rete ingests, stores, and queries **quoted triples** —
statements about statements — in the widely-deployed RDF-star surface
`<< s p o >>` (subject or object), with the SPARQL-star patterns and built-ins
(see [SPARQL support](sparql.md#rdf-star)). It also accepts the ratified **RDF 1.2**
object triple-term syntax `<<( s p o )>>` on ingest, mapping it to the *same*
canonical token, so an RDF 1.2 file and an RDF-star file are interoperable.
**Base-direction language strings** (`"…"@lang--dir`, RDF 1.2's
`rdf:dirLangString`) are modelled — `DATATYPE` reports `rdf:dirLangString` and
`LANG` returns the language subtag — and a leading SPARQL 1.2 `VERSION "1.2"`
declaration is accepted. Not yet: RDF 1.2 **reification** (`rdf:reifies` /
Turtle-1.2 annotation syntax) and the new SPARQL 1.2 direction *functions*
(`LANGDIR`…), which would require swapping the parser to the RDF-1.2 model that
reinterprets `<< >>` as reification — deliberately deferred to keep the deployed
RDF-star data working.

**Current limits (not RDF-incompatible, just unimplemented):** OWL/XML and
Functional Syntax need an external convert-to-RDF step (above); and there is no
in-place SPARQL Update — the file is immutable by design, though
[`rete serve`](cli.md) runs a live endpoint that accepts SPARQL Update into a
journal beside the untouched base file. Turtle/JSON-LD export covers the default
graph only (use N-Quads export for named graphs).

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
