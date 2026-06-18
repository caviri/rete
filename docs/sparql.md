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
| **Algebra** | `OPTIONAL` (left join), `UNION`, `MINUS`, `FILTER EXISTS` / `NOT EXISTS` |
| **Filters** | Comparisons, `&&`/`\|\|`/`!`, arithmetic, `BOUND`, `COALESCE`; built-ins incl. `CONTAINS`, `STRLEN`, `SUBSTR`, `CONCAT`, `STR`, `isIRI`/`isLiteral`/`isBlank`, `DATATYPE`, `LANG`, `REGEX` |
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

`eval_select_communities` evaluates a SELECT with a **split-where-sound,
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

The WASM playground's **Output** menu renders one result several ways. Each view
expects a particular query shape — write your `SELECT`/`CONSTRUCT` so the columns
it needs are present.

| Output | Needs | Query shape |
| --- | --- | --- |
| **Table** | anything | any `SELECT`, `ASK`, or `CONSTRUCT` |
| **Graph** | edges to draw | a `CONSTRUCT { ?a ?p ?b } …`, or a `SELECT` with **≥ 2 variables** — 2 vars are read as `v1 → v2` ("related") edges; **3 vars** are read as (subject, predicate, object). A 1-variable `SELECT` has nothing to connect. |
| **Map** | a WKT geometry column | a `SELECT` that binds a variable to a `geo:wktLiteral` (e.g. `?w` via `geo:hasGeometry/geo:asWKT ?w`). `POINT` / `LINESTRING` / `POLYGON` / `MULTI*` all plot; the first **non-geometry** column becomes each feature's hover label. |
| **Time** | a year / date column | a `SELECT` that binds a variable to `xsd:gYear`, `xsd:date` / `xsd:dateTime`, or a plain **year integer** (e.g. `ex:year ?y`). The other selected column(s) become the items listed in each cell's tooltip. |
| **TTL / JSON-LD** | triples | a `CONSTRUCT` query — these serialise the constructed graph. A `SELECT` has no triples to serialise. |

Rules of thumb:

- **Map, Time and Graph render the bindings of a `SELECT`**, so put the geometry /
  year / edge columns in your `SELECT` list. (`CONSTRUCT` also feeds Graph / TTL /
  JSON-LD directly.)
- **Map** and **Time** are available on **every** query (no per-dataset gating):
  each detects its column in the actual result and renders, or shows a short note
  if it's absent — so geometry or dates a query surfaces unexpectedly (e.g. from a
  federated join, or data the dataset's examples never touch) still plot.
- Run these views under the **Whole index** (or **Split by community**) strategy.
  **Progressive** answers only from the pyramid summary (counts and community
  structure), so it has no per-row geometry or dates to plot.
- **Time** buckets years automatically to fit the span (per-year for short ranges,
  up to per-1000-year for very long ones); negative years are read as **BCE**. A
  cell's colour encodes the **number of items** in that bucket — hover for the list.
- The map is an **offline equirectangular plot** of the WKT coordinates (no tiles /
  network), auto-fit to the bounding box of the returned geometries.

```sparql
# MAP — labelled territories as polygons (history dataset)
PREFIX geo:  <http://www.opengis.net/ont/geosparql#>
PREFIX ex:   <http://ex/>
PREFIX rdfs: <http://www.w3.org/2000/01/rdf-schema#>
SELECT ?territory ?w WHERE {
  ?t ex:year 1914 ; rdfs:label ?territory ;
     geo:hasGeometry/geo:asWKT ?w .
}

# TIME — how many territories exist per year, as a multi-year heatmap
PREFIX ex:   <http://ex/>
PREFIX rdfs: <http://www.w3.org/2000/01/rdf-schema#>
SELECT ?year ?territory WHERE { ?t ex:year ?year ; rdfs:label ?territory }

# GRAPH — a 3-variable SELECT read as (subject, predicate, object)
PREFIX cito: <http://purl.org/spar/cito/>
SELECT ?a ?p ?b WHERE { ?a ?p ?b . FILTER(?p = cito:cites) } LIMIT 50

# TTL / JSON-LD — a CONSTRUCT produces serialisable triples
PREFIX cito: <http://purl.org/spar/cito/>
CONSTRUCT { ?a cito:cites ?b } WHERE { ?a cito:cites ?b } LIMIT 50
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
