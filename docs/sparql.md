# SPARQL support

The engine lives in `rete-core::sparql`. Queries are parsed with
[`spargebra`](https://crates.io/crates/spargebra) and lowered to a small plan
algebra (`Bgp`/`Join`/`Union`/`Minus`/`LeftJoin`/`Filter`/`Path`/`Values`/
`Graph`), evaluated in the unified integer node space and resolved back to terms
only for final bindings.

Run via the CLI (`rete sparql <file> "<query>" [--json]`) or in the browser
(`query` in `rete-wasm` for any query form; `query_sparql` is the older
SELECT-only wrapper).

<figure class="fig-right">
  <img src="img/bgp-join.svg" alt="Two triple patterns sharing the variable ?f are joined on it, producing a binding table with columns for the bound variables.">
  <figcaption>A basic graph pattern is a join on shared variables: patterns that share <code>?f</code> are intersected via the permutation indexes.</figcaption>
</figure>

## Supported

| Area | Details |
|---|---|
| **Query forms** | `SELECT`, `ASK`, `CONSTRUCT`, `DESCRIBE` |
| **Patterns** | Triple patterns and BGPs evaluated as integer-space hash joins on shared variables; blank nodes as non-distinguished variables |
| **Algebra** | `OPTIONAL` (left join), `UNION`, `MINUS`, `FILTER EXISTS` / `NOT EXISTS` |
| **Filters** | Comparisons, `&&`/`\|\|`/`!`, arithmetic, `BOUND`, `COALESCE`; built-ins incl. `CONTAINS`, `STRLEN`, `SUBSTR`, `CONCAT`, `STR`, `isIRI`/`isLiteral`/`isBlank`, `REGEX` |
| **Property paths** | `p+`, `p*`, `p?` (zero-length included for `*`/`?`), reverse `^p`, sequence `a/b`, alternative `a\|b` — evaluated goal-directed from a bound endpoint |
| **Solution modifiers** | `DISTINCT`, `ORDER BY` (ASC/DESC), `LIMIT`, `OFFSET`, `VALUES`, `BIND` |
| **Aggregation** | `GROUP BY`, `HAVING`, `COUNT`/`SUM`/`AVG`/`MIN`/`MAX` (incl. `COUNT(DISTINCT …)`) |
| **Datasets** | `GRAPH <iri>` / `GRAPH ?g`, `FROM` (RDF-merge default graph), `FROM NAMED` (scope which graphs `GRAPH` sees); `EXISTS` honors the active graph |
| **Output** | SPARQL Results JSON (`--json`), with correct `uri`/`literal`/`bnode` typing, datatype, and `xml:lang`; literal values are properly unescaped |

### Property-path zero-length semantics

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
```sparql
# (the summary path; see SummaryView::predicate_totals)
```

### Evaluation model

The algebra evaluates as a lazy pull pipeline over integer slot rows: joins,
`MINUS`, `DISTINCT`, filters, and `GRAPH` stream, so `LIMIT` and `ASK` stop the
underlying index scans early, and under a small known demand joins switch to
index-nested-loop probes. Aggregation, `ORDER BY` (a bounded top-k when `LIMIT`
is present), and hash-join build sides are the only blocking points; terms are
resolved to strings only at projection. It is still not a *cost-based* planner —
join order is a selectivity heuristic — and the benchmark page separates
correctness coverage from latency and calls out the shapes where Oxigraph still
wins.

### Community-split evaluation

`eval_select_communities` evaluates a SELECT **per pyramid community** and
merges: each community's subjects are pushed into the plan as a `VALUES`
binding, the partial rows are concatenated, and the solution modifiers
(`GROUP BY` / `ORDER BY` / `LIMIT` / `DISTINCT`) run once on the union —
"compute per community, aggregate globally", with rows identical to the
whole-graph answer. This is sound only for **subject-star** queries over the
default graph — one basic graph pattern (FILTERs allowed) whose every triple
pattern shares the same subject variable — because tiles partition triples by
their subject's community, so each solution lives entirely inside one
community. Any other shape (multi-hop joins, UNION, paths) is **rejected with
a clear error** rather than answered from a split that could drop
cross-community rows. The playground's "Split by community" strategy uses
this; natively it is the seam for per-community parallel evaluation.

## Not supported

These are **rejected with a clear error** — never silently mis-evaluated:

- **Subqueries** (nested `SELECT`).
- **`SERVICE`** (federation) — out of scope for a single self-contained file.
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
