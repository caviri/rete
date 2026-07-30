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
  <img src="img/bgp-join.svg" alt="Two triple patterns sharing the variable ?f are joined on it, producing a binding table with columns for the bound variables.">
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
| **Datasets** | `GRAPH <iri>` / `GRAPH ?g`, `FROM` (RDF-merge default graph), `FROM NAMED` (scope which graphs `GRAPH` sees); `EXISTS` honors the active graph |
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

**Ingest & storage.** Quoted triples parse from both **N-Triples-star** and
**Turtle-star** (`rete build data.ttls`, `rete validate`) and are stored as
ordinary dictionary terms — no format change, no version bump, and an old reader
stays forward-compatible. A file that contains any quoted triple sets a header
flag (`FLAG_HAS_QUOTED_TRIPLES`), so a plain-RDF consumer can tell from the header
alone, without scanning; `rete info` shows it. Quoted triples round-trip
losslessly through `rete export`, and `rete verify` covers them.

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

**RDF 1.2 interop.** Ingest also accepts the ratified RDF 1.2 object triple-term
syntax `<<( s p o )>>`, mapped to the *same* canonical token as `<< s p o >>` — so
an RDF 1.2 N-Triples file and an RDF-star file are interchangeable. RDF 1.2
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

## Not supported

These are **rejected with a clear error** — never silently mis-evaluated:

- **`SERVICE ?var`** — federation to a variable-bound endpoint.
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
