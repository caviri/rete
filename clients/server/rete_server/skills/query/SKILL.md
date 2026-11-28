---
name: query
description: Explore and query .rete knowledge graphs with SPARQL on this server. Use when the user asks a question about the data in a graph, wants to find, list, count or compare things, or when a query returns zero rows, too many rows or runs slowly. Covers the discovery workflow (card, schema, examples), reading warnings and stats, fast and slow query shapes on lazily-read files, federation, and known engine issues with their workarounds.
---

# Query a .rete graph

A `.rete` file describes itself and is read lazily: a query fetches only the
byte ranges it touches. So you never need to guess what is in a graph. Ask the
file, then write the query. Examples below use `ex:` for `https://example.org/`;
in a real query declare every prefix you use, and copy IRIs from the schema.

## 1. The order that works

1. `list_datasets` returns the graphs on this server, with key, label, size and license. Pick a `key`.
2. `dataset_card(dataset)` is the graph's own description: what it is, license, provenance, counts, creators. Read it before you answer a question about the data.
3. `dataset_schema(dataset)` lists classes with instance counts and (class, predicate, class, count) relations. These are the exact IRIs to use. Never invent a namespace.
4. `example_queries(dataset)` gives queries the file ships with. Start from the nearest one and edit it.
5. `validate_query(query, dataset)` checks syntax and probes every class and predicate IRI against the graph with a cheap ASK. An IRI that matches nothing is a wrong namespace or a typo, the usual cause of zero rows.
6. `sparql_query(dataset, query)` runs it.

When the question names a thing ("Marie Curie", "Geneva"), resolve it first:
`find_entities(dataset, text)` returns IRIs by label prefix (and full text when
the file has a text index), and `describe_entity(dataset, iri)` shows everything
the graph says about one. Use the returned IRI in your query, not a guessed one.

A public `.rete` file that is not in the list can be queried with
`sparql_query(url=...)`; the same tools take a `url`.

## 2. Write small queries, add LIMIT

Always `LIMIT` while exploring. Look at 5 rows, then widen.

```sparql
SELECT ?paper ?title ?year WHERE {
  ?paper a ex:Paper ;
         rdfs:label ?title ;
         ex:year ?year .
}
ORDER BY DESC(?year)
LIMIT 5
```

`a` is `rdf:type` and always works. Count before you list:

```sparql
SELECT ?class (COUNT(?s) AS ?n) WHERE { ?s a ?class } GROUP BY ?class ORDER BY DESC(?n)
```

Questions of the form "how many / which / who has the most" are an aggregate:
`GROUP BY`, `COUNT(DISTINCT ?x)`, `ORDER BY DESC`, `LIMIT`.

```sparql
SELECT ?person (COUNT(DISTINCT ?paper) AS ?papers) WHERE {
  ?paper ex:author ?person .
}
GROUP BY ?person
ORDER BY DESC(?papers)
LIMIT 3
```

With `reason=true` the engine adds OWL 2 QL entailment (subclasses,
subproperties, domain and range). It can reach the same node by several paths,
so count with `COUNT(DISTINCT ?x)`.

## 3. When you get zero rows

A FILTER that hits a type error is false, and SPARQL raises no error, so rows
vanish. Check in this order:

1. Read `warnings` in the result. It names the function, the argument, a sample value and the fix.
2. `CONTAINS`, `STRSTARTS`, `STRENDS` and `REGEX` need strings. Wrap an IRI or a number in `STR()`.
3. Run `validate_query` with the dataset. An IRI it flags is the wrong namespace.
4. Matching is case-sensitive. Use `LCASE(STR(?x))` on both sides, or a text index.
5. A typed literal (`"1843"^^xsd:integer`) does not equal the plain string `"1843"`. Compare with the same type, or compare `STR(?y)`.

This returns no rows, and `warnings` says why:

```sparql
# no rows: CONTAINS gets an IRI
SELECT ?paper WHERE { ?paper ex:author ?a FILTER(CONTAINS(?paper, "p1")) }
```

This is the fixed version:

```sparql
SELECT ?paper WHERE { ?paper ex:author ?a FILTER(CONTAINS(STR(?paper), "p1")) }
```

## 4. Reading a result

`sparql_query` returns `table` (plain values, easiest to read) and `results`
(W3C SPARQL JSON), plus:

- `warnings`: FILTER type errors. Empty when there are none.
- `stats`: bytes and requests actually fetched. Compare `bytes` with the file size. A small query on a large file should fetch a small part.
- `elapsed_seconds`, and `truncated` when the server's row cap cut the answer. If it is true, narrow the query; do not report the partial table as complete.
- CONSTRUCT and DESCRIBE return `triples` instead of `table`.

## 5. Fast and slow shapes

The file is read by byte range, so cost follows how much of the index a query
must touch, not how many rows it returns.

Cheap: a bound subject or object (`<iri> ?p ?o`, `?s ex:author <iri>`), a
bound predicate with a LIMIT, `GROUP BY` over one predicate, an aggregate the
file's summary can answer, a property path anchored at one end.

Expensive: `?s ?p ?o` with no LIMIT, a full-graph `COUNT`, `ORDER BY` over a
large unlimited result, `CONTAINS`/`REGEX` over every literal (unless the file
has a text index), a property path with both ends free, a join that matches
millions of intermediate rows before a selective filter.

Habits that keep it cheap:

- Put the most selective pattern (a bound IRI) first and the optional ones last.
- Anchor paths: `ex:p4 ex:cites+ ?x` is fine, `?a ex:cites+ ?b` enumerates the graph.
- Ask for the columns you need, not `SELECT *`.
- Count with a small query before you fetch rows.
- Split a heavy question into steps. Find the entities first, then ask for details with `VALUES`:

```sparql
SELECT ?paper ?year WHERE {
  VALUES ?paper { ex:p1 ex:p3 ex:p4 }
  ?paper ex:year ?year .
}
```

Property paths work from a bound end. `p+`, `p*`, `p?`, `^p`, `a/b` and `a|b`
all parse; counted repeats such as `p{1,2}` do not. For "within two hops" write
`p/p?`.

```sparql
SELECT DISTINCT ?cited WHERE { ex:p4 ex:cites/ex:cites? ?cited }
```

## 6. Federation

Each dataset is a SPARQL 1.1 endpoint at `/sparql/<key>` on this server, and
any other SPARQL endpoint works too, so one query can join a graph with
another graph or a live endpoint such as Wikidata or DBpedia with `SERVICE`.
The block is sent to the remote side as written, with no bound join, so an
open pattern asks it for everything it knows. Put `VALUES` or constants inside
the block and keep it selective. `SERVICE ?var` is rejected.

```sparql
SELECT ?paper ?name WHERE {
  VALUES ?paper { ex:p1 ex:p3 }
  ?paper ex:author ?person .
  SERVICE <https://example.org/sparql/people> {
    VALUES ?person { ex:ada ex:alan }
    ?person rdfs:label ?name .
  }
}
LIMIT 10
```

`SERVICE SILENT` degrades a failed call to one empty solution instead of
failing the query. A server that needs a token will refuse a call that carries
none; then run two queries and join the results yourself.

## 7. Known engine issues (rete 0.7.8)

Each has a workaround. Details and runnable repros are in
[references/engine-issues.md](references/engine-issues.md).

- **CONSTRUCT and LIMIT.** Engines after 0.7.8 honour LIMIT, OFFSET and ORDER BY on CONSTRUCT and DESCRIBE; 0.7.8 and older ignore them, even inside a subquery. If `triples` comes back longer than your LIMIT, the server runs an older engine: use a SELECT with `?s ?p ?o` columns and a LIMIT, or pass `limit` to `sparql_query`, which trims the answer after the engine ran.
- **MIN, SUM and AVG come back unbound** when a group has an unbound value from `OPTIONAL`. MAX and COUNT are fine. Take the `OPTIONAL` out so only bound values reach the aggregate, or wrap with `COALESCE`.
- **Two star patterns that share two variables** can exhaust memory for a small answer. Split them into two queries and join client-side.
- **`CONTAINS`/`REGEX` on an IRI or a number** are type errors. Use `STR()`.

If a result looks wrong, check it against a second formulation (COUNT vs
listing, or a SELECT without the aggregate) before trusting it.

## 8. Showing results

For a picture of the answer, `graph_view(dataset, query=...)` opens an
interactive graph in clients that render MCP Apps. A SELECT with `?s ?p ?o`
columns, or a CONSTRUCT, is drawn as nodes and edges. Keep it to a few hundred
nodes. See the `network-analysis` skill for what to ask the graph.

For one question, answer in words with the numbers; offer the query and the
dataset key so the user can rerun it.
