# SPARQL support

The engine lives in `rete-core::sparql`. Queries are parsed with
[`spargebra`](https://crates.io/crates/spargebra) and lowered to a small plan
algebra (`Bgp`/`Join`/`Union`/`Minus`/`LeftJoin`/`Filter`/`Path`/`Values`/
`Graph`), evaluated in the unified integer node space and resolved back to terms
only for final bindings.

Run via the CLI (`rete sparql <file> "<query>" [--json]`) or in the browser
(`query` in `rete-wasm` for any query form; `query_sparql` is the older
SELECT-only wrapper).

Spatial queries over `geo:wktLiteral` geometry — point-in-polygon, intersection,
distance — are covered by a focused set of GeoSPARQL functions; see
[GeoSPARQL (geometry + time)](geosparql.html).

<figure class="fig-right">
  <img src="img/bgp-join.svg" alt="A basic graph pattern is a join on a shared variable. The pattern ?p :knows ?f and the pattern ?f :age ?age both mention ?f, so the engine joins them on it: it walks each pattern through a permutation index and intersects the two streams on ?f. The result is a binding table with one column per variable — ?p, ?f and ?age — here :ann :bob 31 and :ann :cleo 27.">
  <figcaption>A basic graph pattern is a join on shared variables: patterns that share <code>?f</code> are intersected via the permutation indexes.</figcaption>
</figure>

## Supported

| Area | Details |
|---|---|
| **Query forms** | `SELECT`, `ASK`, `CONSTRUCT`, `DESCRIBE` |
| **Patterns** | Triple patterns and BGPs evaluated as integer-space hash joins on shared variables; blank nodes as non-distinguished variables |
| **Algebra** | `OPTIONAL` (left join), `UNION`, `MINUS`, `FILTER EXISTS` / `NOT EXISTS`, nested `SELECT` **subqueries** (evaluated independently, then joined on shared projected variables) |
| **Filters** | Comparisons, `&&`/`\|\|`/`!`, arithmetic, `BOUND`, `COALESCE`; built-ins incl. `CONTAINS`, `STRLEN`, `SUBSTR`, `CONCAT`, `STR`, `isIRI`/`isLiteral`/`isBlank`, `DATATYPE`, `LANG`, `REGEX` |
| **Property paths** | `p+`, `p*`, `p?` (zero-length included for `*`/`?`), reverse `^p`, sequence `a/b`, alternative `a\|b` — evaluated goal-directed from a bound endpoint |
| **Solution modifiers** | `DISTINCT`, `ORDER BY` (ASC/DESC), `LIMIT`, `OFFSET`, `VALUES`, `BIND` |
| **Aggregation** | `GROUP BY`, `HAVING`, `COUNT`/`SUM`/`AVG`/`MIN`/`MAX` (incl. `COUNT(DISTINCT …)`) |
| **Datasets** | `GRAPH <iri>` / `GRAPH ?g`, `FROM` (RDF-merge default graph), `FROM NAMED` (scope which graphs `GRAPH` sees); `EXISTS` honors the active graph. Plus an **opt-in, non-standard** [union default graph](#union-default-graph) mode for named-graph-heavy files. |
| **Output** | SPARQL Results JSON (`--json`), with correct `uri`/`literal`/`bnode` typing, datatype, and `xml:lang`; literal values are properly unescaped |
| **RDF-star** | Quoted triples `<< s p o >>` in subject/object position — ingest (N-Triples-star & Turtle-star), storage, and SPARQL-star: quoted-triple patterns (incl. inner variables `<< ?s :p ?o >>`) and the `isTRIPLE` / `TRIPLE` / `SUBJECT` / `PREDICATE` / `OBJECT` built-ins. See [below](#rdf-star). |
| **Reasoning (OWL 2 QL)** | Opt-in ontology-mediated answering by **query rewriting** — no materialization: `rdfs:subClassOf` / `subPropertyOf` hierarchy + `rdfs:domain` / `range` type inference, computed over the raw data. `rete sparql … --entail`, or the playground **🧠 Reason** toggle. See [below](#reasoning-owl-2-ql). |

## Property paths

Property paths are evaluated goal-directed from a bound endpoint and support
`p+`, `p*`, `p?`, reverse `^p`, sequence `a/b`, and alternative `a|b`.

### Zero-length semantics

`*` and `?` include the zero-length path (a node reaches itself); `+` does not.
This holds in every binding direction:

```sparql
# Alice plus everyone she transitively knows (includes Alice herself):
SELECT ?y WHERE { ex:Alice ex:knows* ?y }
# Everyone who reaches Carol in ≤1 hop (includes Carol):
SELECT ?x WHERE { ?x ex:knows? ex:Carol }
```

### Index-free aggregates

Exact per-predicate totals come straight from the pyramid summary's superedge
counts, without reading the triple index:

```sh
rete predicates data.rete            # CLI
```

The same per-predicate totals back the playground's index-free aggregate path.

### Evaluation model

The algebra evaluates as a lazy pull pipeline over integer slot rows: joins,
`MINUS`, `DISTINCT`, filters, and `GRAPH` stream, so `LIMIT` and `ASK` stop the
underlying index scans early, and under a small known demand joins switch to
index-nested-loop probes. Aggregation, `ORDER BY` (a bounded top-k when `LIMIT`
is present), and hash-join build sides are the only blocking points — and
*blocking* is about ordering, not memory: aggregation folds rows through
per-group accumulators, so resident memory is **O(groups), not O(rows)** — a
bare `COUNT(*)` is a single counter, and a `GROUP BY` over the 1.38 billion
`rdf:type` rows of the 9.83 B-triple DataCite graph completes inside a 4 GiB
container (numbers on the [benchmark page](BENCHMARK.md)). What still
materializes its input: a no-`LIMIT` `ORDER BY`, and a multi-graph `FROM` (a
single `FROM <g>` borrows that graph's index without copying). Terms are
resolved to strings only at projection. It is still not a *cost-based* planner —
join order is a selectivity heuristic — and the benchmark page separates
correctness coverage from latency and calls out the shapes where Oxigraph still
wins.

### Community-split evaluation

The engine can also evaluate a SELECT with a **split-where-sound,
global-where-not** strategy that always returns exactly the whole-graph
answer. The one place the pyramid partition genuinely applies is a *subject
star* — a group of triple patterns sharing one variable subject — because
tiles partition triples by their subject's community, so a star's solutions
partition cleanly by community. Each BGP is decomposed into its stars; each
star is evaluated per community (the community's subjects pushed in as a
`VALUES` binding, which the engine turns into index probes); and the stars
are recombined with **global hash joins**, so multi-hop joins work and
solutions that cross communities survive. `FILTER` / `UNION` / `OPTIONAL` /
`MINUS` recurse through the same machinery; property paths, inline `VALUES`,
and `GRAPH` blocks evaluate globally inside the split (exact by definition);
and `GROUP BY` / `ORDER BY` / `LIMIT` / `DISTINCT` run once on the merged
rows. A query is refused only when nothing in it can split (no BGP with a
variable subject — the strategy would add nothing) or under `FROM` / `FROM
NAMED`. The playground's "Split by community" strategy uses this; natively
the per-star, per-community partials are the seam for parallel evaluation.

## Union default graph (⛁ All graphs) {#union-default-graph}

Standard SPARQL scopes a pattern outside `GRAPH` to the **default graph**, and
the engine keeps exactly that — the W3C conformance suite runs on it. But many
datasets keep **every statement in named graphs** (anything built from N-Quads:
DCAT catalogs, provenance stores), so on such a file
`SELECT (COUNT(*) AS ?n) WHERE { ?s ?p ?o }` answers `0`. That answer is
*correct*, and nearly every newcomer reads it as breakage — the published Czech
national open-data catalog (2.28 M quads across 31,974 named graphs, default
graph empty) is the case that proved it.

Virtuoso, GraphDB and Jena TDB (`tdb:unionDefaultGraph`) all offer a
store-level switch for exactly this, and rete offers the same mode, **opt-in
per query**: with it on, a pattern outside `GRAPH` matches the **RDF merge of
the default graph and every named graph**. Because it is not standard SPARQL,
it is off by default and is never applied implicitly — a plain query never
changes meaning.

Semantics, precisely:

- The merge is a **set union**: a triple present in several graphs matches
  once. The file's own default-graph triples are **included** (unlike a bare
  `FROM`, which replaces the default graph).
- A query that brings its own `FROM` keeps its `FROM` dataset — the query
  named its dataset explicitly, and that wins.
- `GRAPH <iri>` / `GRAPH ?g` and `FROM NAMED` are completely unaffected: they
  enumerate and scope the named graphs exactly as before.

Where it exists today:

- **The playground** — the ⛁ All graphs toggle beside 🧠 Reason. Flipping it
  is announced, and every result computed under it says so in its meta line,
  so a non-standard answer is never silently presented as a standard one. See
  [the playground guide](playground-guide.md).
- **The browser engine** — `Graph.query_opts(query, format, reason, union)`
  and the same method on `RemoteGraph` (see
  [WASM & JavaScript API](browser.md)).
- **Rust** — `eval_query_with(&rete, query, QueryOpts { union_default_graph:
  true, ..QueryOpts::default() })` in `rete-core::sparql`.
- **Not** on the CLI — `rete sparql` has no union flag — and **not** in
  `rete serve`'s endpoint. If you need the union semantics there, write it
  into the query with `GRAPH ?g { … }` (and `UNION` with a default-graph
  branch when both sides hold data), which works everywhere.

Costs and limits worth knowing before flipping it on:

- The common converted-file shape — **empty default graph, exactly one named
  graph** — stays a zero-copy borrow of that graph's index: no copying, no
  extra bytes.
- Any other shape **materializes the merged index** for the query. On a
  lazily-opened remote file that can mean faulting the index tiles of every
  named graph the merge touches — a real cost on a many-graph file, and
  precisely why the mode is opt-in per query rather than a file-level default.
- The playground's progressive and community-split strategies answer from
  default-graph structures, so a union run is evaluated on the whole index
  instead (the result line says so) rather than silently answering with
  standard semantics.
- Federated multi-source runs and live SPARQL endpoints keep standard
  semantics — the toggle does not reach them (a live endpoint decides its own
  dataset).
- Under union, `DESCRIBE`'s per-resource expansion — the triples returned
  *about* each matched resource — still reads the default graph; the union
  applies to the pattern matching that selects the resources.

## Output views & query shapes

The playground renders one result several ways — Table, Graph, Map, Time,
TTL/JSON-LD — and each view expects a particular query shape (a geometry column
for Map, a year/date column for Time, …). That matrix lives with the rest of the
playground documentation: see
[Playground — output views](playground-guide.md#output-views).

## Federation: `SERVICE`

SPARQL 1.1 federated query is supported: a `SERVICE <endpoint> { … }` block is
shipped (as written) to the remote SPARQL endpoint at evaluation time and its
solutions join the surrounding pattern on shared variables — so one query can
span a `.rete` file *and* a live endpoint (Wikidata, DBpedia, …):

```sparql
# Local entities enriched with live DBpedia labels, in one query.
SELECT ?book ?label WHERE {
  ?book <http://ex/about> ?ent .
  SERVICE <https://dbpedia.org/sparql> {
    VALUES ?ent { <http://dbpedia.org/resource/Douglas_Adams> }
    ?ent rdfs:label ?label . FILTER(lang(?label) = "en")
  }
}
```

Notes:

- rete can also **be** the endpoint: `rete serve <file>` (see [cli](cli.md))
  exposes a `.rete` over the SPARQL Protocol — queries *and* SPARQL Update —
  so one rete file can `SERVICE` against another rete served live.
- `SERVICE SILENT` follows the spec: a failed call degrades to one empty
  solution instead of failing the query.
- The block is sent **as written** (no bound-join injection yet), so keep it
  selective — an unconstrained pattern asks the remote endpoint for everything
  it knows. Put `VALUES`/constants inside the block, as above.
- The engine performs no I/O itself: the CLI and the browser client attach the
  HTTP transport (`ServiceClient`); in the browser the endpoint must allow
  CORS (the big public ones do).
- `SERVICE ?endpoint { … }` (a variable endpoint) is not supported.

## RDF-star

rete supports **RDF-star**: a *quoted triple* `<< s p o >>` may stand in the
subject or object position of another triple, so you can make statements **about
statements** — the natural home for provenance and annotation (who recorded a
fact, when, with what confidence).

```turtle
:occ1 a :BarnSwallow .
# annotate the statement above:
<< :occ1 a :BarnSwallow >> :recordedBy :jsmith ;
                           :individualCount 5 ;
                           :observedOn "2023-05-01"^^xsd:date .
```

**Ingest & storage.** Quoted triples parse from **N-Triples-star**,
**Turtle-star** and their **RDF 1.2** equivalents (`rete build data.ttls`,
`rete validate`) and are stored as ordinary dictionary terms — no format change,
no version bump, and an old reader stays forward-compatible. A file that
contains any quoted triple sets a header flag (`FLAG_HAS_QUOTED_TRIPLES`), so a
plain-RDF consumer can tell from the header alone, without scanning; `rete info`
shows it. Quoted triples round-trip losslessly through `rete export`, and
`rete verify` covers them.

**Input surface.** In Turtle/TriG, `<< s p o >>` is a quoted triple under
RDF-star and a *reifier* under RDF 1.2, so `rete build` and `rete validate` take
`--quoted-triple-syntax rdf12|rdf-star` (default `rdf-star`) to say which. Under
`rdf12` a reifier expands to `_:r rdf:reifies <<( s p o )>>` plus a statement
about `_:r` — ordinary triples, queried with ordinary SPARQL, no `<< >>` pattern
needed. That is the shape to write a query against if your source is RDF 1.2
reification rather than RDF-star quoting; the two are different data models and
rete stores whichever one the file actually contains. See
[`rete build`](cli.md#quoted-triple-syntax-on-input).

**Export surface.** `rete export --format nq|ttl|trig` writes the ratified
**RDF 1.2 triple term** `<<( s p o )>>` by default — that is what current
parsers read — and `--quoted-triple-syntax rdf-star` writes the stored
`<<s p o>>` surface instead. Both re-ingest losslessly; see
[Triple-store interop](interop.md#quoted-triples-two-surfaces-one-graph) for
which consumer takes which. RDF 1.2 places a triple term in *object position
only*, so a subject-position quoted triple is refused by name under `rdf12`
rather than written as something no parser accepts.

**Query — SPARQL-star.** A quoted triple can appear in a query pattern, with
constants or inner variables:

```sparql
# Who recorded that occ1 is a Barn Swallow?  (concrete quoted triple)
SELECT ?who WHERE { << :occ1 a :BarnSwallow >> :recordedBy ?who }

# Every recorded identification, with the sighting and species bound from the
# quoted triple (inner variables):
SELECT ?occ ?species ?who WHERE {
  << ?occ a ?species >> :recordedBy ?who
}

# A quoted variable that is also bound by a regular pattern joins on it:
SELECT ?who WHERE {
  ?occ :place ?p .
  << ?occ a ?species >> :recordedBy ?who     # ?occ unifies across both
}
```

**Built-in functions** inspect and construct quoted triples:

| Function | Result |
|---|---|
| `isTRIPLE(t)` | whether `t` is a quoted triple |
| `SUBJECT(t)` / `PREDICATE(t)` / `OBJECT(t)` | the component of a quoted triple |
| `TRIPLE(s, p, o)` | build a quoted triple from three terms |

```sparql
# Equivalent to the inner-variable pattern above, spelled with the built-ins:
SELECT ?occ ?who WHERE {
  ?qt :recordedBy ?who
  FILTER(isTRIPLE(?qt))
  BIND(SUBJECT(?qt) AS ?occ)
}
```

`CONSTRUCT` (and `rete serve`'s SPARQL Update) may build quoted triples in their
templates. Nested quoting (`<< << … >> :p ?o >>`) works. rete follows the
RDF-star community-group / SPARQL-star syntax that its parser (Oxigraph) implements.

**You do not have to write the first one.** A dataset built with `--card` whose
statements quote other statements ships a `qt-*` family of starter queries in
its [dataset card](dataset-cards.md#querying-the-star-layer), instantiated with
its own annotation vocabulary — what gets said about statements, who says it,
and how much of the data is qualified — and every one of them is run against the
finished file at build time, so a shipped query is one that answers. Note that
those bodies are written in the `<< s p o >>` surface above: the ratified RDF 1.2
`<<( s p o )>>` is an **export** spelling and does not parse in a query.

**RDF 1.2 interop.** rete's **N-Triples/N-Quads** reader also accepts the
ratified RDF 1.2 object triple-term syntax `<<( s p o )>>`, mapped to the *same*
canonical token as `<< s p o >>` — so an RDF 1.2 N-Triples file and an RDF-star
file are interchangeable. (Turtle and TriG go through `oxttl` 0.1, which reads
the RDF-star surface only; the export prints a note when it writes a TriG dump
rete itself could not read back.) RDF 1.2
**base-direction strings** (`"…"@lang--dir`) are modelled: `DATATYPE` reports
`rdf:dirLangString` and `LANG` returns the language subtag; a leading SPARQL 1.2
`VERSION "1.2"` declaration is accepted. RDF 1.2 reification (`rdf:reifies`) and
the new SPARQL 1.2 direction functions are not yet supported — see
[Compatibility](compatibility.md#is-it-compatible-with-rdf).

## Reasoning (OWL 2 QL)

rete answers ontology-mediated queries by **rewriting the query**, not by
materializing entailments. That is the OWL 2 QL idea, and it is the profile that
fits a cloud-native, range-queried file: the TBox is small, the ABox is huge and
maybe remote, so instead of baking inferences into the data (bloating the file,
forcing a rebuild — what `rete build --materialize` does) the *query* is expanded
so that evaluating it over the **raw** data yields the entailed answers. A remote
`.rete` becomes ontology-aware with no rebuild, and only the bytes the rewritten
query touches are fetched.

Reasoning is **opt-in** — `rete sparql|sparql-url … --entail`, or the playground's
**🧠 Reason** toggle. A plain query is never changed.

**What is entailed** (the RDFS-plus core of OWL 2 QL):

| Axiom | A query for … also returns … |
|---|---|
| `rdfs:subClassOf` | `?x a C` → instances of every subclass of `C` (transitively) |
| `rdfs:subPropertyOf` | `?x P ?y` → pairs related by any subproperty of `P` |
| `rdfs:domain` | `?x a C` → subjects of a property whose domain is `⊑ C` |
| `rdfs:range` | `?x a C` → objects of a property whose range is `⊑ C` |
| `owl:inverseOf` | `?x P ?y` → pairs `?y Q ?x` for any `Q` inverse to `P` |
| `owl:someValuesFrom` (`A ⊑ ∃P`) | `?x P ?_` (existential object) → every `?x` that is (transitively) an `A` |
| existential inverse (`A ⊑ ∃P⁻`) | `?_ P ?x` (existential subject) → every such `?x`, via `P`'s inverse |
| `domain`/`range` ∘ `subPropertyOf` | type inferred through a *subproperty* of a domain/range-declared property |

```sparql
# Over gbif-birds (occurrences are typed to their SPECIES, and each species has a
# subClassOf chain up to :Aves). WITHOUT reasoning this matches nothing directly;
# WITH --entail it returns real occurrences via the taxonomy — no hand-written path.
SELECT ?o WHERE { ?o a <https://w3id.org/rete/gbif/taxon/class/Aves> } LIMIT 20
```

**How** — a hierarchy atom is lowered to the property path that already walks the
hierarchy: `?x a C` becomes `?x a ?c . ?c rdfs:subClassOf* C` (reflexive, so a
direct type still matches), and likewise `subPropertyOf*` for roles; `domain` /
`range` add `UNION` branches. A small TBox read gates the rewrite, so an atom
whose class/property has no sub-terms — and every non-reasoned query — is
untouched. The reasoning reaches nested patterns (`UNION` / `OPTIONAL` /
subqueries).

The existential rewrite is **sound by construction**: it fires only when the
object variable is purely existential — it occurs exactly once in the whole query
and is not returned — because an anonymous `∃P` successor can neither be projected
nor joined. Where the object is bound, shared, or in the `SELECT`, the rewrite is
skipped.

**Boundary.** Every DL-Lite_R axiom *type* is covered. The one remaining gap is
the PerfectRef *reduction* step — existential **chaining**, where a shared join
constraint is itself entailed by an existential (e.g. a query joins `?x P ?y`
with `?y a C` and `∃P⁻ ⊑ C` makes the `?y a C` atom redundant). That query shape
is rare, and reasoning is never *unsound* regardless: with it off you get exact
matches; with it on you get the entailed answers for the supported cases — it can
only ever be *incomplete* for that one chaining shape. The whole-graph RL reasoner
(`rete reason` / the Coherence tab) is a separate, materializing tool for
coherence checking.

## Expression errors {#errors}

A SPARQL expression evaluates to an RDF term **or to an error**: a function
given the wrong kind of argument (`CONTAINS` on an IRI), an unbound variable, a
division by zero, a term with no boolean value. rete carries that error as a
value of its own and resolves it only where SPARQL 1.1 says to (§17.2, §17.3,
§17.4.1, §18.5.1):

| Where the error meets… | Result |
|---|---|
| any function or operator (`CONTAINS`, `STR`, `+`, `=`, `isIRI`, `sameTerm`, …) | an error |
| `!` | an error. `!CONTAINS(?iri, "x")` is **not** true |
| `\|\|` | `true \|\| error` and `error \|\| true` are `true`; with `false` or another error, an error |
| `&&` | `false && error` and `error && false` are `false`; with `true` or another error, an error |
| `IF(cond, a, b)` | an error in `cond` is an error; only the chosen branch is evaluated |
| `COALESCE(…)` | skipped: the first argument without an error wins (an error if none) |
| `BOUND(?x)` | never an error |
| `x IN (…)` | `true` if some member equals `x`; otherwise an error if a member errored, else `false`. `IN ()` is `false`. `NOT IN` is `!(… IN …)` |
| effective boolean value | `xsd:boolean`, numeric and `xsd:string` literals have one (an ill-typed `"abc"^^xsd:integer` is `false`); an IRI, a blank node, a language-tagged or other-typed literal is an error |
| `FILTER`, `HAVING`, an `OPTIONAL`'s filter | the row is dropped |
| `BIND`, a projected `(expr AS ?v)`, a `GROUP BY` key | `?v` is left unbound; the row stays |
| `ORDER BY` | sorts as "no value", before everything else |
| `COUNT(expr)` | the error is not counted |
| `SUM`, `GROUP_CONCAT`, `MIN` | the aggregate is unbound for that group. An unbound variable is an error here too, so `SUM(?x)` over a group where some row lacks `?x` is unbound |
| `AVG` | unbound if some rows error and others do not; `0` if every row errors, because errors are not counted and the average of zero values is `0` (§18.5.1.4) |
| `MAX` | the largest value that is not an error: MAX orders like `ORDER BY DESC`, where "no value" comes last (§18.5.1.6, §15.1). MIN orders like `ORDER BY ASC`, where it comes first, hence the row above |
| `SAMPLE` | returns one of the values that is not an error |

Through v0.3.2 rete turned an error into `false` at the function that raised
it, so `FILTER(!CONTAINS(?iri, "x"))` kept every row, `BIND(!CONTAINS(?iri,
"x") AS ?b)` bound `true`, and `SUM` / `AVG` / `MIN` / `MAX` / `GROUP_CONCAT`
skipped the rows they could not use (`MAX` still does, as the spec says). Those results were wrong, and the
[CHANGELOG](https://github.com/caviri/rete/blob/main/CHANGELOG.md) lists this as a result-changing fix.

Where rete deliberately differs from Oxigraph 0.5, each time because the spec
text says otherwise: `COUNT(expr)` removes errors and counts the rest
(§18.5.1.2; Oxigraph makes the count unbound); `MAX` ignores error elements
and an all-error `AVG` is `0` (§18.5.1.4-6; Oxigraph makes both unbound); `x IN ()` is `false` and
`x NOT IN ()` is `true` (the examples in §17.4.1.9-10; Oxigraph raises an
error); and the effective boolean value of an ill-typed numeric literal is
`false` (§17.2.2; Oxigraph raises an error).

The boolean built-ins (`isIRI`, `isLiteral`, `isBlank`, `isNumeric`,
`isTRIPLE`, `CONTAINS`, `STRSTARTS`, `STRENDS`, `REGEX`, `LANGMATCHES`) work
outside FILTER too: `BIND(CONTAINS(?label, "x") AS ?hit)` binds an
`xsd:boolean`, or leaves `?hit` unbound on an error. `isNumeric` is true only
for a literal of a numeric datatype with a valid lexical form, so it is false
for the string `"10"`.

## Type errors and query warnings {#warnings}

The string functions (`CONTAINS`, `STRSTARTS`, `STRENDS`, `STRBEFORE`,
`STRAFTER`, `REGEX`, `REPLACE`, `STRLEN`, `UCASE`, `LCASE`, `SUBSTR`, `CONCAT`,
`ENCODE_FOR_URI`, the hashes) take **string literals**. Given an IRI, a blank
node, a number or another typed literal, they raise a SPARQL **type error**.
Inside `FILTER` the row is dropped, with no error, and an enclosing `!` does
not rescue it (see [Expression errors](#errors)). In `BIND` or a projected
expression the variable is left unbound instead. rete follows the spec here,
as Oxigraph and Jena do, so these queries return no rows:

| Query | Rows | Why |
|---|---|---|
| `FILTER(CONTAINS(?s, "geneva"))`, `?s` an IRI | 0 | an IRI is not a string: use `CONTAINS(STR(?s), "geneva")` |
| `FILTER(STRSTARTS(?year, "15"))`, `?year` an `xsd:integer` | 0 | a number is not a string: use `STRSTARTS(STR(?year), "15")` |
| `FILTER(REGEX(?s, "geneva"))`, `?s` an IRI | 0 | same: `REGEX(STR(?s), "geneva")` |
| `FILTER(CONTAINS(?label, "Geneva"@fr))`, `?label` is `@en` | 0 | the arguments are *incompatible* (§17.4.3): a tagged needle must carry the haystack's tag. Use `STR()` on both |
| `FILTER(CONTAINS(?label, "geneva"))`, label `"Geneva …"` | 0 | not an error: matching is case-sensitive. Use `LCASE(?label)` or `REGEX(?label, "geneva", "i")` |
| `FILTER(ABS(?note) > 0)`, `?note` the string `"a note"` | 0 | `ABS` / `CEIL` / `FLOOR` / `ROUND` need a number (rete accepts any literal whose text parses as one) |
| `FILTER(?label + 1 > 0)`, `?label` a string | 0 | `+ - * /` take numeric-typed literals only: cast first, `xsd:decimal(?label)` |
| `BIND(?n / 0 AS ?r)` | 1, `?r` unbound | division by zero is an error |
| `OPTIONAL { ?s ex:age ?a } FILTER(?a > 18)` | rows without an age dropped | a comparison (or `IN`) with an unbound operand is an error |
| `FILTER(LANG(?s) = "en")`, `?s` an IRI | 0 | `LANG` / `DATATYPE` take a literal |
| `BIND(STRDT(?label, xsd:token) AS ?t)`, `?label` is `@en` | 1, `?t` unbound | `STRDT` / `STRLANG` need a simple literal: `STRDT(STR(?label), …)`; `STRDT`'s datatype must be an IRI |

Results are never changed to be helpful. What rete adds is a **side channel**:
every such error is counted, and the query reports **warnings** next to its
results. `rete sparql` and `rete sparql-url` print them to **stderr**, so stdout
stays the result:

```text
$ rete sparql maps.rete 'SELECT ?s WHERE { ?s rdfs:label ?l FILTER(CONTAINS(?s, "geneva")) }'
0 solution(s)
warning: CONTAINS received an IRI as argument 1 in 1 row: a SPARQL type error, which makes a FILTER drop the row, even under ! (BIND leaves the variable unbound); e.g. <http://ex.org/map/geneva-1572>. Hint: wrap it in STR() to match the IRI's text.
```

With `--json`, the result object also gets a `warnings` array. The array is left
out when there is nothing to report. Each entry has these fields:

| Field | Meaning |
|---|---|
| `severity` | `type-error` (an error was raised and absorbed) or `hint` (a suggestion; nothing went wrong) |
| `function` | the SPARQL function, e.g. `CONTAINS`, or for an operator its spelling: `+` `-` `*` `/`, `=` `!=` `<` `<=` `>` `>=`, `IN` |
| `argument` | 1-based argument position; for an operator 1 is the left operand, 2 the right |
| `argKind` | `iri`, `blank-node`, `quoted-triple`, `numeric`, `typed-literal`, `string`, `language-tagged`, `invalid-number` (a numeric datatype whose text is not a number), `language-mismatch`, `unbound`, `invalid-regex`, `not-a-datetime`, `division-by-zero` (argument 2 of `/`), or `case-sensitive` for the hint |
| `count` | how many evaluations raised it. A row is evaluated only when it reaches the expression, and LIMIT / ASK stop early, so this is not a count of the data |
| `sample` | the first offending value, cut to 80 characters |
| `hint` | what to change, e.g. `wrap it in STR() to match the IRI's text` |
| `message` | all of the above as one sentence |

Notes:

- **`unbound`** is reported for a bare variable passed to `CONTAINS`,
  `STRSTARTS`, `STRENDS`, `REGEX`, the numeric functions, `LANG`,
  `DATATYPE`, `STRDT` / `STRLANG`, an arithmetic operator, a comparison or
  `IN`. Such a variable usually comes from an `OPTIONAL` that did not match,
  or is misspelled. An error inside a nested call (`CONTAINS(LCASE(?s), …)`,
  `STRLEN(?s) > 3`) is reported once, by the inner function.
- **Comparisons** are reported only where rete already raises an error,
  which today is an operand that is itself an error (most often unbound).
  rete compares any two bound terms, numerically when both parse as numbers
  and lexically otherwise, so `"10" = 10` is true and `?year > "abc"` is
  silently lexical. Where that differs from the spec it is a semantics
  question, deliberately not changed by the warnings.
- **Division by zero** is an error for every numeric type in rete, as it
  is for `xsd:integer` and `xsd:decimal` in the spec (for `xsd:double` the
  spec gives `INF`).
- **`invalid-regex`**: rete's regex engine uses Rust syntax, which has no
  look-around and no back-references. An invalid pattern is an error (the row
  is dropped, even under `!`) and is reported along with the parser's message.
- **The case-sensitivity hint** is given only when all of these hold: the
  result is empty, no type error was raised, `CONTAINS` / `STRSTARTS` /
  `STRENDS` / `REGEX` without the `i` flag actually ran on string values and
  returned no match, and the needle is a constant with upper/lower-case
  letters. It says the matching *might* be the reason; rete doesn't claim it is.
- Cost: errors are recorded only where they happen, at most one entry (a
  count and one sample) per function, argument and kind. A query that raises
  no errors does no extra work beyond resetting the counter once per query.
- From Rust: `rete_core::eval_query_with_warnings(&rete, query, opts)` returns
  `(QueryOutput, Vec<QueryWarning>)`. `eval_query` is unchanged.
- **The playground** shows them in a box above the result, in every output
  view, and counts them in the run summary (`0 row(s) · ⚠ 1 warning`).
- **`rete serve`** sends them as response headers, so the body stays the
  standard SPARQL result document: `Rete-Warnings` holds the same JSON array
  (non-ASCII escaped as `\uXXXX`, as a header value must be), and
  `Rete-Warning-Count` the total. Both are left out when there is nothing to
  report. Browsers can read them (`Access-Control-Expose-Headers`). See
  [`rete serve`](cli.html#rete-serve-file---bind-addr---token-t---journal-path).

A function rete doesn't know is never silently false. An unknown name
(`NOSUCHFN(?x)`) is a parse error, and an extension-function IRI that rete
doesn't implement (e.g. `<http://ex.org/fn#match>(?o)`) fails with
`unsupported query feature: function <http://ex.org/fn#match> is not
implemented`.

## Not supported

These are **rejected with a clear error** — never silently mis-evaluated:

- **`SERVICE ?var`** — federation to a variable-bound endpoint.
- **Extension functions** other than the XSD casts, GeoSPARQL `geof:` and
  `geo3:` ones — the error names the function IRI.
- Complex `ORDER BY` **key expressions** beyond a bare variable/constant are not
  yet evaluated for ordering.

## Examples

```sparql
# 2-hop join
PREFIX ex: <http://ex/>
SELECT ?z WHERE { ex:Alice ex:knows ?y . ?y ex:knows ?z }

# FILTER + OPTIONAL
SELECT ?p WHERE { ?p ex:name ?n . OPTIONAL { ?p ex:age ?a } . FILTER(BOUND(?a)) }

# GROUP BY with aggregate
SELECT ?p (COUNT(?f) AS ?degree) WHERE { ?p ex:knows ?f } GROUP BY ?p ORDER BY DESC(?degree)

# Named graph
SELECT ?g ?s WHERE { GRAPH ?g { ?s ex:knows ?o } }

# Transitive impact (reverse property path)
SELECT DISTINCT ?d WHERE { ?d ex:dependsOn+ ex:log4x }
```
