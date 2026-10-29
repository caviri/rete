# SPARQL support

The engine lives in `rete-core::sparql`. Queries are parsed with
[`spargebra`](https://crates.io/crates/spargebra) and lowered to a small plan
algebra (`Bgp`/`Join`/`Union`/`Minus`/`LeftJoin`/`Filter`/`Path`/`Values`/
`Graph`), evaluated in the unified integer node space and resolved back to terms
only for final bindings.

Run via the CLI (`rete sparql <file> "<query>" [--json]`) or in the browser
(`query` in `rete-wasm` for any query form; `query_sparql` is the older
SELECT-only wrapper). A query too long for the command line — a large
`VALUES` block passes the 128 KiB a Linux argument may hold, or Windows'
32,767-character command line — goes in a file: `rete sparql <file>
--query-file q.rq`, or `--query-file -` to read standard input (every CLI
command that takes a query accepts it; see [the CLI](cli.html)).

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
| **Datasets** | `GRAPH <iri>` / `GRAPH ?g`, `FROM` (RDF-merge default graph), `FROM NAMED` (scope which graphs `GRAPH` sees); `EXISTS`, sub-`SELECT`s and `DESCRIBE` honor the active graph. The full-text index (`--text-index`) covers the default graph only, so it speeds up `FILTER(CONTAINS(…))` there and is not used inside `GRAPH`, under `FROM` or over the union default graph, where the filter scans. Plus an **opt-in, non-standard** [union default graph](#union-default-graph) mode for named-graph-heavy files. |
| **Output** | SPARQL Results JSON (`--json`), with correct `uri`/`literal`/`bnode` typing, datatype, and `xml:lang`; literal values are properly unescaped |
| **RDF-star** | Quoted triples `<< s p o >>` in subject/object position — ingest (N-Triples-star & Turtle-star), storage, and SPARQL-star: quoted-triple patterns (incl. inner variables `<< ?s :p ?o >>`) and the `isTRIPLE` / `TRIPLE` / `SUBJECT` / `PREDICATE` / `OBJECT` built-ins. See [below](#rdf-star). |
| **Reasoning (OWL 2 QL)** | Opt-in ontology-mediated answering by **query rewriting** — no materialization: `rdfs:subClassOf` / `subPropertyOf` hierarchy + `rdfs:domain` / `range` type inference, computed over the raw data. `rete sparql … --entail`, or the playground **🧠 Reason** toggle. See [below](#reasoning-owl-2-ql). |

## Property paths

Property paths are evaluated goal-directed from a bound endpoint and support
`p+`, `p*`, `p?`, reverse `^p`, sequence `a/b`, and alternative `a|b`.

### Duplicates

Paths follow SPARQL 1.1's bag semantics (§9.3, §18.4). An alternative `a|b` is
the union of its branches, so a pair that both branches connect is returned
twice: with `:x :cites :t` and `:x :references :t`, `?s (:cites|:references)
:t` returns `:x` twice. A sequence `a/b` is a join through a hidden variable,
so it returns a pair once per intermediate node. The connectivity operators
`*`, `+` and `?`, and a negated property set `!(…)`, return each reachable
node once, however many ways it is reached. Use `SELECT DISTINCT` (or
`COUNT(DISTINCT …)`) to count nodes rather than paths.

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
resolved to strings only at projection.

**Join order and join method.** A basic graph pattern is joined most selective
pattern first, keeping the join connected. The size of each pattern comes from
the pyramid's exact per-predicate statistics when the pyramid is resident, and
otherwise from the index's tile directory, which every file has and which a
lazy open already holds: the number of tiles a pattern routes to, times the
average triples per tile. That second figure needs no read and counts a class's
own instances, so on a 9.53 B-triple file `?s a :Service ; :publicationYear
?y` starts from the 517 services, not from the 109 M years. Then, per pattern:

- when the rows found so far are few next to the pattern (at least 16 times
  fewer, and the pattern above 65,536 triples), each row **looks the pattern
  up** in the index instead of scanning it — a bound subject becomes an SPO
  range read, so `<doi> :p ?r . ?r ?q ?o` reads the handful of `?r` rows and
  never the whole graph;
- when even the most selective pattern is large (above about two million
  matches), the join **streams**: that pattern is read from its index cursor
  and every later pattern is joined per row, through a hash table when it is
  small and by lookup otherwise; two large patterns that share their subject
  variable stream through a merge join. Nothing proportional to the large
  pattern is held;
- otherwise the small side is materialized and hash-joined, as before.

The same rule applies between a basic graph pattern and what joins it from the
outside — a property path, an `OPTIONAL`, another block: the outside rows are
pulled first, and if there are few of them next to the pattern's most selective
triple pattern (16 times fewer, that pattern above 65,536 triples), the pattern
is looked up once per row instead of being evaluated whole and hash-joined.
`<work> frbr:partOf+ ?j . ?j a fabio:Journal ; dcterms:title ?t` reads the
three containers of one work, not every journal of the file.

A sub-select's rows enter the outer query as dictionary ids, never resolved to
strings in between. It is still not a cost-based optimizer over the whole
algebra (only within a basic graph pattern), and the benchmark page separates
correctness coverage from latency and calls out the shapes where Oxigraph
still wins.

### Memory budget {#memory-budget}

A file opened lazily — every `-url` command, a local file larger than a
quarter of the budget, and every client that opens through the ranged reader
(Python, R, Java, the relay) — keeps three caches: decoded dictionary chunks,
decoded index tiles (with the per-tile group directories that point lookups
build) and raw file blocks. They are capped by one memory budget, **4096 MiB by
default** (`rete sparql --memory-budget-mb N`, `0` = unlimited; a client that
wants another figure calls `set_memory_budget`). Least recently used entries
are evicted and re-read when touched again, so the budget trades speed for
memory and never changes an answer. The per-query term and number memos are
capped at 64 MiB each. The browser engine is not capped: its 32-bit heap is
smaller than the default, and its callers bound their own range budgets.

`COUNT(DISTINCT …)` is bounded by the same figure: its distinct values are
kept as a sorted, deduplicated buffer of dictionary ids (8 bytes each), and
past an eighth of the budget (512 MiB by default) the buffer is written to the
temporary directory (`TMPDIR`) as a sorted, delta-encoded run; the count is
then a merge of the runs. It is exact at any budget. On DataCite,
`COUNT(DISTINCT ?r)` over the 610 M `dcite:PidRelation`s ran out of memory at
16 GB; it now returns 609,765,497 in 47 s at 1.5 GB peak RSS. A resident
(eager) open spills at the same threshold when the command was given a budget
(`rete sparql` always is); the browser engine never spills.

The budget does not cover the rest of the query's own working state, which
follows from what the query asks for: the build side of a hash join,
`GROUP BY` groups, `SELECT DISTINCT` sets, and an `ORDER BY` without `LIMIT`.
A `GROUP BY` with a few hundred groups over billions of rows stays small; one
with hundreds of millions of groups does not.

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

### Splitting a query by a variable {#partition}

A long extraction — title, venue and year of every work citing a paper, over
a remote graph where each scattered work is a network round trip — can run as
many short queries instead of one long one, each finished, retried or resumed
on its own. `partition_plan(query, var)` plans that and
`eval_query_partitioned(query, opts, var, keys)` runs one chunk (JS:
`g.partitionPlan(q, "?work")` and `g.queryRaw(q, { partition: { var, keys } })`;
the Claude Desktop extension's `partition_by` drives both).

- **Exact, or refused.** Every solution must bind the variable: then each
  solution belongs to exactly one chunk, the one holding its value, and the
  chunks' rows together are the query's rows. The plan is refused, with the
  reason, for ASK and DESCRIBE, an `ORDER BY` (it needs every row at once), an
  aggregate not grouped by the variable (it would be computed per chunk), a
  variable a select expression computes, and a variable some solution may
  leave unbound (matched only inside `OPTIONAL`, one side of a `UNION`, a
  `BIND`, a `SERVICE`, or a sub-SELECT that does not project it).
- **The keys.** The plan's `keys_query` lists the variable's values, and
  `keys_from` says from where:
  - `values`: a required `VALUES` list on the variable (bound in every row),
    taken as it stands — the solutions can take no other value, and listing
    them reads nothing. The shortest one when there are several.
  - `pattern`: the one required triple pattern that binds it with constants
    alone (`?work cx:cites <paper>`), the one whose tiles are smallest
    according to the tile directory — one range scan, where the query is a
    read per entity.
  - `where`: the whole WHERE clause, when neither exists; listing them may
    cost as much as the query.

  Either way that is a superset of the values the solutions take; a key with
  no solution makes an empty chunk. A caller may also run chunks for keys it
  got elsewhere — the works a query listed in another graph: the chunks then
  answer the query for those values only.
- **A chunk** is the query without its top-level `OFFSET` / `LIMIT`, with its
  keys either cutting the query's own `VALUES` list on the variable down to
  them (when every key is in it as written), or joined in as `VALUES` on the
  first required pattern block that binds the variable, where the VALUES
  pushdown turns each key into an index lookup.
- **Merging** is the caller's: the chunks' rows in key order, deduplicated when
  the plan says `dedupe` (a `DISTINCT` that does not project the variable, and
  every `CONSTRUCT`), then the plan's `offset` / `limit`. A `LIMIT` without
  `ORDER BY` asks for any rows, so a caller may stop starting chunks once the
  finished ones hold enough.
- Keys are term tokens as the keys query returns them (`<https://…>`,
  `"…"^^<…>`), matched against the dictionary as stored. A variable bound to
  blank nodes cannot be split by: a blank node cannot be named in a chunk.

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

## Comparisons and ordering {#compare}

`=`, `!=`, `<`, `>`, `<=`, `>=`, `IN` / `NOT IN`, `ORDER BY`, `MIN` / `MAX`,
`SUM` / `AVG`, arithmetic and the numeric functions read a term by its
**datatype**, as SPARQL 1.1 §17.3 maps each operator to an XPath function.
What a string's text looks like never matters: a string is not a number.

| Operands | `=` / `!=` | `<` `>` `<=` `>=` |
|---|---|---|
| two numbers (`xsd:integer`, `decimal`, `float`, `double` and the types derived from them, such as `xsd:int`) | by value, with type promotion: `1 = 1.0` and `"01"^^xsd:integer = 1` are true; integers and decimals compare exactly; a `NaN` equals nothing, itself included; `-0 = 0` | by value; any comparison with `NaN` is false |
| two strings (simple literals or `xsd:string`) | by code point: `"01" = "1"`, `"1.0" = "1"`, `"1e3" = "1000"` and `"inf" = "Infinity"` are **false**; `"NaN" = "NaN"` is true | by code point, so `"10" < "9"` |
| two language-tagged literals | same text and same tag (tags compare case-insensitively: `"a"@en = "a"@EN`) | when the tags match; otherwise a type error |
| two `xsd:boolean`s | by value (`"1"^^xsd:boolean = true`) | `false < true` |
| two `xsd:dateTime`s, or two of another date/time type (`date`, `time`, `gYear`, `gYearMonth`, `gMonth`, `gDay`, `gMonthDay`) | by value, timezones applied: `"2020-01-01T00:00:00Z" = "2020-01-01T01:00:00+01:00"`. A value with a timezone and one without less than 14 hours apart cannot be decided without an implicit timezone, which rete does not assume: a type error | by value; the same pair is a type error |
| two durations (`xsd:duration`, `dayTimeDuration`, `yearMonthDuration`) | by value: `PT60M = PT1H`, `P1Y = P12M` | by value where XSD orders them (`P1M` vs `P30D` is a type error) |
| an IRI or blank node and anything | equal only to the same term | a type error |
| two different literals of a datatype rete does not know, or of a known one with an invalid text (`"abc"^^xsd:integer`) | a type error (RDFterm-equal, §17.4.1.7); the same term is equal to itself | a type error |
| two literals of different known kinds (a string and a number, a date and a boolean) | `=` is false, `!=` true | a type error |

`IN` is a chain of `=`. `sameTerm` is term identity, with no value
comparison: `sameTerm(1, 1.0)` is false.

`ORDER BY` puts no value first, then blank nodes, IRIs, literals and quoted
triples (§15.1). Among literals: first text — simple, `xsd:string`,
language-tagged literals, literals of an unknown datatype and ill-typed values
such as `"820"^^xsd:gYear` (XSD wants four digits), together by text, then
datatype, then tag, so labels with and without a tag sort alphabetically
together — then numbers by value (a `NaN` after every number), booleans,
date/time values and durations. Within one kind this is the `<` above. The
order between kinds is implementation-defined in SPARQL; rete's puts text
first so that a stray string or an ill-typed date never tops an `ORDER BY
DESC(…)` or a `MAX`. `MIN` / `MAX` use the same order (§18.5.1.5-6).

`SUM`, `AVG`, `+ - * /` and `ABS` / `CEIL` / `FLOOR` / `ROUND` take numbers:
a string is a type error there, whatever its text. When your data holds
numbers as strings, cast them — `xsd:integer(?year) > 2000`,
`SUM(xsd:decimal(?amount))` — or compare them as text on both sides,
`?year > "2000"` (fine for same-length years, wrong for `"999"`).

Through 0.3.6 rete compared any two terms numerically when both texts parsed
as numbers and by their token text otherwise, so `"01" = "1"`, `"1e3" = "1000"`
and `"inf" = "Infinity"` were true, `"NaN" = "NaN"` was false, `?x < 5`
compared strings with numbers, and `ORDER BY` put literals before IRIs. The
[CHANGELOG](https://github.com/caviri/rete/blob/main/CHANGELOG.md) lists this
as a result-changing fix. An `=` that is false only because a string's text
spells the number it is compared with (`"2020" = 2020`) is reported as a hint,
and a comparison with no defined operator as a type error; see
[warnings](#warnings).

Where rete differs from Oxigraph 0.5, because the spec says otherwise:
`NaN < 1` is false (Oxigraph: an error), `NaN <= NaN` is false and
`<a> <= <a>` a type error (Oxigraph: true, from its identity shortcut),
`false < true` is true (Oxigraph: an error), and a zoned and an unzoned
date/time less than 14 hours apart are a type error under `=` (Oxigraph:
false; the W3C test `open-world/date-2` requires the error). Language tags:
rete keeps a tag as the data spells it, so `sameTerm`, joins and `DISTINCT`
see `"a"@EN` and `"a"@en` as different terms, while `=` treats them as equal;
Oxigraph lower-cases tags when it loads data.

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
| `FILTER(ABS(?note) > 0)`, `?note` the string `"12"` | 0 | `ABS` / `CEIL` / `FLOOR` / `ROUND` need a number, and a string is not one whatever its text: `ABS(xsd:decimal(?note))` |
| `FILTER(?label + 1 > 0)`, `?label` a string | 0 | `+ - * /` take numeric-typed literals only: cast first, `xsd:decimal(?label)` |
| `FILTER(?year > 2000)`, `?year` the string `"2001"` | 0 | `<` is not defined between a string and a number (§17.3): `xsd:integer(?year) > 2000` |
| `FILTER(?year = 2001)`, `?year` the string `"2001"` | 0 | not an error: a string never equals a number, so `=` is false. Reported as a hint |
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
| `argument` | 1-based argument position; for an operator 1 is the left operand, 2 the right; 0 when the warning is about the pair (`incomparable`, `string-vs-number`) |
| `argKind` | `iri`, `blank-node`, `quoted-triple`, `numeric`, `typed-literal`, `string`, `language-tagged`, `invalid-number` (a numeric datatype whose text is not a number), `language-mismatch`, `unbound`, `invalid-regex`, `not-a-datetime`, `division-by-zero` (argument 2 of `/`), `incomparable` (a comparison whose two operands no operator is defined for), or, for the hints, `case-sensitive` and `string-vs-number` |
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
- **Comparisons** (`=` … `>=`, `IN`) report an operand that is itself an
  error (most often unbound) and, as `incomparable`, a pair §17.3 defines no
  operator for: a string and a number under `<`, an IRI under `<`, two
  different literals of an unknown datatype under `=`. The message names the
  two kinds (`compared a string with xsd:integer`). An `=` / `!=` / `IN`
  between a string and a number whose value the string's text spells
  (`"2020" = 2020`, false) is reported as a `string-vs-number` **hint**, since
  nothing went wrong but the answer reads as one; see
  [Comparisons and ordering](#compare).
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
