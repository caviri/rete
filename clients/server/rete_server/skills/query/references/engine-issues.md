# Known engine issues (rete 0.7.8)

Each snippet below was run on a small test graph (7 papers, 7 people; `ex:` is
`https://example.org/`). Re-test on a newer engine before relying on a
workaround: a fix removes the need for it.

## CONSTRUCT ignores LIMIT

A `LIMIT` on a CONSTRUCT is accepted and not applied. The subquery form does
not help either: all 11 `ex:author` triples come back.

```sparql
CONSTRUCT { ?paper ex:author ?person } WHERE { ?paper ex:author ?person } LIMIT 2
```

Workarounds:

- Ask for the same data as a SELECT with three columns, which does honour LIMIT. `graph_view` reads `?s ?p ?o` columns as triples.

```sparql
SELECT ?s ?p ?o WHERE { ?s ?p ?o . VALUES ?p { ex:author } } LIMIT 2
```

- Pass `limit` to `sparql_query`. The server trims the returned triples and sets `truncated`. The engine still computes the whole CONSTRUCT, so use this only when the CONSTRUCT is selective.
- Make the CONSTRUCT itself selective (`VALUES`, a bound subject) instead of relying on LIMIT.

A full CONSTRUCT on a large graph builds every triple in memory. Never run an
unselective one.

## MIN, SUM and AVG unbound after OPTIONAL

If any row of a group has no value from an `OPTIONAL`, `MIN`, `SUM` and `AVG`
return nothing for that group. `MAX` and `COUNT` are not affected. Here Alan
Turing wrote one paper without `ex:pages`, so his `?total` and `?fewest` are
unbound while `?most` and `?n` are fine:

```sparql
SELECT ?person (SUM(?pages) AS ?total) (MIN(?pages) AS ?fewest) (MAX(?pages) AS ?most) (COUNT(?pages) AS ?n) WHERE {
  ?paper ex:author ?person .
  OPTIONAL { ?paper ex:pages ?pages }
}
GROUP BY ?person
ORDER BY ?person
```

Workarounds:

- Drop the `OPTIONAL` so only papers that have pages reach the aggregate. People with no pages then disappear from the result; list them separately if you need them.

```sparql
SELECT ?person (SUM(?pages) AS ?total) (MIN(?pages) AS ?fewest) WHERE {
  ?paper ex:author ?person ;
         ex:pages ?pages .
}
GROUP BY ?person
ORDER BY ?person
```

- For a sum, `COALESCE` turns a missing value into zero:

```sparql
SELECT ?person (SUM(COALESCE(?pages, 0)) AS ?total) WHERE {
  ?paper ex:author ?person .
  OPTIONAL { ?paper ex:pages ?pages }
}
GROUP BY ?person
ORDER BY ?person
```

`COALESCE` is wrong for MIN and AVG (a zero is not "no value"). For those, use
the first form.

## Two star patterns that share two variables

Two separate patterns around different subjects, joined on two variables at
once, can build an enormous intermediate join on a big graph and
exhaust memory, even when the final answer has a handful of rows (seen on a
4 GB limit with a 1,332-row answer). The shape, which runs fine on a small
graph and is only a risk on a big one:

```sparql
# no rows on this small graph; the shape is the point
SELECT ?a ?b ?x WHERE {
  ?a ex:memberOf ?org ; ex:born ?x .
  ?b ex:memberOf ?org ; ex:born ?x .
  FILTER(?a != ?b)
}
LIMIT 10
```

Workaround: split it. Run the first star, collect the values it binds, then run
the second with those values in `VALUES`, and join the two answers yourself. If
the first star is selective, `VALUES` makes the second one selective too.

## CONTAINS, REGEX and friends on IRIs and numbers

`CONTAINS`, `STRSTARTS`, `STRENDS` and `REGEX` need string arguments. An IRI
or a number is a type error, a FILTER that errors is false, and the rows
vanish. The result's `warnings` names the function and the fix. Wrap with `STR()`:

```sparql
SELECT ?paper ?year WHERE { ?paper ex:year ?year FILTER(STRSTARTS(STR(?year), "19")) }
```

## A habit that catches these

When an aggregate looks too small, run the same grouping as a plain SELECT and
compare. When a CONSTRUCT is slow, look at `stats` in the answer before running
it again.
