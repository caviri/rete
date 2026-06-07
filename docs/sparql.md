# SPARQL support

The engine lives in `rete-core::sparql`. Queries are parsed with
[`spargebra`](https://crates.io/crates/spargebra) and lowered to a small plan
algebra (`Bgp`/`Join`/`Union`/`Minus`/`LeftJoin`/`Filter`/`Path`/`Values`/
`Graph`), evaluated in the unified integer node space and resolved back to terms
only for final bindings.

Run via the CLI (`rete sparql <file> "<query>" [--json]`) or in the browser
(`query_sparql` in `rete-wasm`).

## Supported

| Area | Details |
|---|---|
| **Query forms** | `SELECT`, `ASK`, `CONSTRUCT`, `DESCRIBE` |
| **Patterns** | Triple patterns, BGPs, nested-loop join on shared variables; blank nodes as non-distinguished variables |
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
