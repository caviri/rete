# SPARQL Queries

Rete's SPARQL engine (located in `rete-core::sparql`) evaluates queries locally or in the browser over `.rete` files. It translates SPARQL queries into a fast, integer-space plan algebra, only resolving final values when returning the results.

You can run queries in two ways:
- **CLI**: `rete sparql <file> "<query>" [--json]`
- **Browser (WASM)**: Use the `query` method in `rete-wasm` to execute any SPARQL query type.

If you need spatial queries (like point-in-polygon or distances), see our guide on [GeoSPARQL](geosparql.html).

<figure class="fig-right">
  <img src="img/bgp-join.svg" alt="Two triple patterns sharing the variable ?f are joined on it, producing a binding table with columns for the bound variables.">
  <figcaption>A basic graph pattern is a join on shared variables: patterns that share <code>?f</code> are intersected via the permutation indexes.</figcaption>
</figure>

## What's Supported?

Rete supports a large subset of SPARQL 1.1:

| Feature Area | Supported Capabilities |
|---|---|
| **Query Forms** | `SELECT`, `ASK`, `CONSTRUCT`, and `DESCRIBE`. |
| **Patterns** | Triple patterns and Basic Graph Patterns (BGPs) evaluated as fast integer-space hash joins. Blank nodes act as non-distinguished variables. |
| **Algebra** | `OPTIONAL` (left join), `UNION`, `MINUS`, `FILTER EXISTS` / `NOT EXISTS`, and nested `SELECT` **subqueries**. |
| **Filters** | Comparisons, `&&`/`\|\|`/`!`, arithmetic, `BOUND`, and `COALESCE`. Built-ins include `CONTAINS`, `STRLEN`, `SUBSTR`, `CONCAT`, `STR`, `isIRI`/`isLiteral`/`isBlank`, `DATATYPE`, `LANG`, and `REGEX`. |
| **Property Paths** | `p+`, `p*`, `p?`, reverse `^p`, sequence `a/b`, and alternative `a\|b`. Evaluated goal-directed from a bound endpoint. |
| **Solution Modifiers** | `DISTINCT`, `ORDER BY` (ASC/DESC), `LIMIT`, `OFFSET`, `VALUES`, and `BIND`. |
| **Aggregation** | `GROUP BY`, `HAVING`, and functions like `COUNT` (incl. `DISTINCT`), `SUM`, `AVG`, `MIN`, `MAX`. |
| **Datasets** | `GRAPH <iri>` / `GRAPH ?g`, `FROM`, and `FROM NAMED`. Plus, an opt-in [union default graph](#union-default-graph) mode. |
| **Output Formats** | SPARQL Results JSON (`--json`), fully typed with proper URIs, literals, `xml:lang`, and datatypes. |
| **RDF-star** | Native support for quoted triples (`<< s p o >>`) in both data and queries. See [RDF-star](#rdf-star). |
| **Reasoning (OWL 2 QL)** | Opt-in ontology-mediated reasoning without materialization via query rewriting. See [Reasoning](#reasoning-owl-2-ql). |

## Property Paths

Property paths (`*`, `?`, `+`, etc.) are evaluated directly from known (bound) starting points.

### Understanding Zero-Length Paths
The paths `*` and `?` will return the starting node itself (zero-length), whereas `+` strictly requires at least one hop.

```sparql
# Returns Alice AND everyone she transitively knows (including herself):
SELECT ?y WHERE { ex:Alice ex:knows* ?y }

# Returns everyone who knows Carol directly, PLUS Carol herself:
SELECT ?x WHERE { ?x ex:knows? ex:Carol }
```

### Fast, Index-Free Aggregates
If you just want the total count of a specific predicate, Rete pulls this straight from the dataset's summary metadata—no index scanning required!

```sh
# Instantly print predicate counts from the CLI
rete predicates data.rete
```

### How Queries are Evaluated
Rete evaluates queries using a **lazy pull pipeline**:
- **Streaming evaluation**: Operations like joins, `MINUS`, `DISTINCT`, filters, and `GRAPH` stream their results. This means `LIMIT` and `ASK` can stop the execution early to save time.
- **Low memory footprint**: Memory isn't scaled by the number of rows. For example, a `GROUP BY` operation folds rows into accumulators, so memory usage is proportional to the number of *groups*, not rows.
- **Materialization triggers**: The only things that force Rete to materialize all results before returning are an unbounded `ORDER BY` or a multi-graph `FROM` clause.

### "Split-by-Community" Execution
For massive datasets, Rete can use a **split-where-sound** strategy. It splits the query into smaller "stars" (groups of patterns sharing a subject), executes them independently across data communities, and hashes them back together. 
- You can test this in the playground using the "Split by community" toggle. 
- It gracefully falls back to normal execution if the query shape doesn't allow splitting.

## Union Default Graph (⛁ All graphs) {#union-default-graph}

By default, SPARQL looks at the **default graph** when no `GRAPH` block is specified. But many modern datasets put *all* their data into named graphs (like when converting from N-Quads). On such datasets, a standard `SELECT * WHERE { ?s ?p ?o }` will return nothing, which can be confusing.

To fix this, Rete provides an **opt-in** union mode where patterns outside a `GRAPH` block match against the **merged union of all graphs**.

**How it works:**
- It performs a **set union**: triples found in multiple graphs only match once.
- If your query has explicit `FROM` or `GRAPH` clauses, those take priority and are unaffected by this mode.

**Where to enable it:**
- **Playground**: Click the **⛁ All graphs** toggle.
- **Browser/WASM**: Pass `union: true` in `QueryOpts`.
- **Rust API**: Set `union_default_graph: true` in `QueryOpts`.
- *(Note: Not currently available via the CLI or SPARQL endpoint. Use explicit `GRAPH ?g { ... }` instead).*

> [!WARNING]
> **Performance Note:** Merging graphs can be slow on lazily-opened remote files because it might need to download indexes for *every* named graph. This is why this feature is strictly opt-in per query.

## Output Views (Playground)

The Rete playground can render your results as a Table, Graph, Map, or Timeline. Each view requires specific columns to be returned in your `SELECT` query (like a geometry column for Maps). 

Check out the [Playground Guide](playground-guide.md#output-views) for the exact column requirements.

## Federation with `SERVICE`

You can blend data from a `.rete` file with live data from external SPARQL endpoints (like Wikidata or DBpedia) using the `SERVICE` keyword.

```sparql
# Fetch local books and get their English labels from live DBpedia
SELECT ?book ?label WHERE {
  ?book <http://ex/about> ?ent .
  
  SERVICE <https://dbpedia.org/sparql> {
    VALUES ?ent { <http://dbpedia.org/resource/Douglas_Adams> }
    ?ent rdfs:label ?label . 
    FILTER(lang(?label) = "en")
  }
}
```

**Key Federation Tips:**
- **Push constraints down:** The `SERVICE` block is sent *exactly as written*. Put constants or `VALUES` inside the block to avoid accidentally downloading the entire remote database.
- **Rete as an endpoint:** You can serve a `.rete` file using `rete serve <file>`, allowing one Rete dataset to federate against another!
- **CORS required:** If you are running federated queries from the browser playground, the target endpoint must support CORS.

## RDF-star (Statements about Statements) {#rdf-star}

Rete has native support for **RDF-star**, meaning you can embed quotes (`<< s p o >>`) as the subject or object of another triple. This is perfect for tracking provenance, timestamps, or confidence scores.

```turtle
# The core fact
:occ1 a :BarnSwallow .

# Data about the fact
<< :occ1 a :BarnSwallow >> :recordedBy :jsmith ;
                           :observedOn "2023-05-01"^^xsd:date .
```

### Querying RDF-star
You can query quoted triples using standard SPARQL syntax or specific built-in functions.

```sparql
# Find who recorded a specific fact:
SELECT ?who WHERE { << :occ1 a :BarnSwallow >> :recordedBy ?who }

# Find everything recorded, extracting the core fact's subject and object:
SELECT ?occ ?species ?who WHERE {
  << ?occ a ?species >> :recordedBy ?who
}
```

### Helpful Built-in Functions
- `isTRIPLE(t)`: Checks if a term is a quoted triple.
- `SUBJECT(t)`, `PREDICATE(t)`, `OBJECT(t)`: Extracts components from a quoted triple.
- `TRIPLE(s, p, o)`: Constructs a new quoted triple.

> [!NOTE]
> Rete also accepts the new **RDF 1.2** object triple-term syntax `<<( s p o )>>`, mapping it seamlessly to standard RDF-star quotes.

## Reasoning (OWL 2 QL) {#reasoning-owl-2-ql}

Rete can intelligently expand your queries using an ontology (like a taxonomy or schema) to infer answers that aren't explicitly written in the data.

Crucially, it does this via **query rewriting** rather than pre-computing (materializing) all possibilities into the file. This keeps your `.rete` files tiny and fast.

**To enable reasoning:**
- Pass `--entail` in the CLI (`rete sparql --entail ...`)
- Turn on the **🧠 Reason** toggle in the playground.

### What it can infer:
- **Subclasses (`rdfs:subClassOf`)**: Searching for `?x a Animal` will return all instances of `Bird` and `Dog`.
- **Subproperties (`rdfs:subPropertyOf`)**: Searching for `?x knows ?y` will return pairs connected by `bestFriendOf`.
- **Domains and Ranges (`rdfs:domain` / `rdfs:range`)**: Infers the type of a node based on the properties it has.
- **Inverses (`owl:inverseOf`)**: Automatically flips relationships if needed.

```sparql
# Without reasoning: Returns 0 results (data only has specific species types)
# With reasoning: Returns all birds by automatically walking up the subclass taxonomy
SELECT ?o WHERE { 
  ?o a <https://w3id.org/rete/gbif/taxon/class/Aves> 
} LIMIT 20
```

## What is NOT Supported?

The following features are not supported and will result in a clear error:
- **Variable endpoints**: `SERVICE ?var { ... }` is not allowed.
- **Complex `ORDER BY` keys**: You must order by a bare variable or constant; expressions inside `ORDER BY` are not yet evaluated.

## Quick Examples

```sparql
# 1. Finding a 2-hop connection
PREFIX ex: <http://ex/>
SELECT ?z WHERE { ex:Alice ex:knows ?y . ?y ex:knows ?z }

# 2. Fetching optional data safely
SELECT ?p WHERE { 
  ?p ex:name ?n . 
  OPTIONAL { ?p ex:age ?a } . 
  FILTER(BOUND(?a)) 
}

# 3. Grouping and counting connections
SELECT ?p (COUNT(?f) AS ?degree) WHERE { 
  ?p ex:knows ?f 
} GROUP BY ?p ORDER BY DESC(?degree)

# 4. Searching a specific named graph
SELECT ?g ?s WHERE { GRAPH ?g { ?s ex:knows ?o } }

# 5. Finding all dependencies recursively (Reverse path)
SELECT DISTINCT ?d WHERE { ?d ex:dependsOn+ ex:log4x }
```
