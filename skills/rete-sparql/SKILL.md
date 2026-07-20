---
name: rete-sparql
description: Write correct SPARQL for .rete graphs — the supported SPARQL 1.1 surface, rete's extensions (OWL 2 QL reasoning, RDF-star, SERVICE federation, text index), result formats, and the recurring gotchas that silently return 0 rows. Use whenever authoring or debugging a SPARQL query against a .rete file, the playground, the Space API, or any client.
---

# SPARQL against `.rete` graphs

The engine speaks SPARQL 1.1 with a few opt-in extensions. This skill is
the practical dialect guide; `docs/sparql.md` and `docs/conformance.md`
are the full references.

## Before writing a single triple pattern

`.rete` files are self-describing — **never guess vocabulary**:

1. **card** → what the dataset is;
2. **schema** → classes with instance counts + (subject class, predicate,
   object class) relations — **copy these IRIs verbatim**;
3. **examples** → runnable queries showing the graph's idioms; start from
   one and edit.

(Client calls: `g.card()/schema()/examples()`; Space:
`/api/datasets/<key>/{card,schema,examples}`; CLI: `rete card|schema`.)

## Supported surface (condensed)

- **Forms**: SELECT, ASK, CONSTRUCT, DESCRIBE.
- **Algebra**: OPTIONAL, UNION, MINUS, FILTER (NOT) EXISTS, nested SELECT
  subqueries, VALUES, BIND.
- **Filters/built-ins**: comparisons, `&&`/`||`/`!`, arithmetic, BOUND,
  COALESCE, CONTAINS, STRLEN, SUBSTR, CONCAT, STR, isIRI/isLiteral/
  isBlank, DATATYPE, LANG, REGEX.
- **Property paths**: `p+ p* p? ^p a/b a|b` — evaluated goal-directed
  **from a bound endpoint**; anchor one end (an IRI or an already-bound
  variable) or the path enumerates far too much.
- **Modifiers/aggregation**: DISTINCT, ORDER BY, LIMIT/OFFSET, GROUP BY,
  HAVING, COUNT/SUM/AVG/MIN/MAX (incl. COUNT(DISTINCT)).
- **Datasets**: `GRAPH <iri>` / `GRAPH ?g`, FROM, FROM NAMED.
- **RDF-star**: `<< s p o >>` patterns (inner variables allowed) +
  isTRIPLE/TRIPLE/SUBJECT/PREDICATE/OBJECT.
- **Federation**: `SERVICE <endpoint> { … }` (+ SILENT) against any
  SPARQL 1.1 endpoint — including other .rete files via the gateway's
  `/sparql/<key-or-url>` endpoints.
- **Reasoning (OWL 2 QL, opt-in)**: subclass/subproperty hierarchy +
  domain/range type inference by query rewriting — `reason=true`
  (clients/Space), `--entail` (CLI), 🧠 toggle (playground).

**Rejected loudly, never mis-evaluated**: `SERVICE ?var`, and ORDER BY
key *expressions* (bare variables/constants only — compute the key in a
BIND first).

## The gotchas that return 0 rows (learned the hard way)

1. **Wrong namespace on types.** `?s a bx:Ley` silently matches nothing
   if `bx:` isn't the dataset's actual namespace. The schema profile is
   the source of truth — copy, don't reconstruct.
2. **Undeclared prefixes.** Every prefixed name needs its `PREFIX` line;
   there are no baked-in defaults. When in doubt use full IRIs in
   angle brackets — `rdf:type` has the shortcut `a`, which always works.
3. **Reasoned COUNT without DISTINCT.** Rewriting derives the same
   instance via several paths (subclass AND property domains/ranges), so
   `COUNT(?x)` over-counts under `reason=true`. Use
   `COUNT(DISTINCT ?x)`.
4. **Unanchored property paths.** `?a p+ ?b` with both ends free explodes
   on big graphs — bind one endpoint.
5. **Case-sensitive literals.** Exact-match FILTERs miss real data;
   prefer `CONTAINS(LCASE(STR(?x)), "…")` — or better, the text index.

## Fast text search

`FILTER(CONTAINS(...))` scans every literal. Files built with
`--text-index` carry a word index that is ~40× faster — but it is a
**separate API**, not a SPARQL builtin: `g.text_search([...])`,
`rete search --contains`, the Space's `find_entities`, or the
playground's search box. Resolve entities there first, then use the IRIs
in SPARQL.

## Result formats

- **Engine envelope** (clients' `query_raw`, Space `/api/query`):
  `{kind: select|ask|construct, vars, rows, …}` with terms as N-Triples
  tokens (`<iri>`, `"lit"^^<dt>`, `_:b0`); clients parse them into Terms.
- **W3C SPARQL Results JSON** (`head`/`results.bindings` with
  uri/literal/bnode typing): CLI `--json`, the Space's `results` field,
  and the SPARQL 1.1 protocol endpoints (`/sparql/<key>`), where
  CONSTRUCT/DESCRIBE answer N-Triples.
- Lazy sanity check: after a remote query, `stats()` should show a small
  fraction of `fileLength` fetched. Always `LIMIT` while exploring.

## Worked pattern

```sparql
PREFIX eli: <http://data.europa.eu/eli/ontology#>
PREFIX rdfs: <http://www.w3.org/2000/01/rdf-schema#>

SELECT ?law ?title (COUNT(DISTINCT ?amendment) AS ?amendments) WHERE {
  ?law a eli:LegalResource ;              # class IRI copied from schema()
       eli:title ?title .
  OPTIONAL { ?amendment eli:amends ?law }
}
GROUP BY ?law ?title
ORDER BY DESC(?amendments)
LIMIT 10
```

With `reason=true` the same `a eli:LegalResource` also matches every
subclass (bx:Ley, bx:RealDecreto, …) — that is the reasoning payoff.
