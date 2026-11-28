---
name: network-analysis
description: Analyse a .rete knowledge graph as a network. Use when the user asks who or what is most connected, central or influential, how two things are linked, what is near an entity, which groups or communities exist, how a graph is structured, or wants a network picture. Covers degree and hub queries, ego networks and k-hop neighbourhoods, reachability and shortest paths, co-occurrence projections, exporting an edge list for networkx or igraph, and showing the result with graph_view.
---

# The graph as a network

Entities are nodes and the links between them are edges. SPARQL answers the
local questions (degree, neighbours, reachability, projections) where the data
lives. Questions that need the whole graph at once (centrality, communities,
components) need an edge list and a library. So the pattern is always:

1. **Choose the edges.** One predicate, or a few, between one or two classes.
2. **Ask locally** for what SPARQL does well.
3. **Export the edge list** and run networkx or igraph when you need more.
4. **Show it** with `graph_view`, a few hundred nodes at most.

Examples use `ex:` for `https://example.org/`, with `ex:Paper` linked by
`ex:cites` and `ex:author` linking papers to people. Take the real classes and
predicates from `dataset_schema(dataset)`; its relation list is a map of the
possible edges. Predicates that link a class to itself (paper cites paper)
make a network directly. Two classes joined through a middle node (people
through papers) need a projection (section 4).

## 1. Degree and hubs

Out-degree, in-degree and total degree of the papers in a citation network:

```sparql
SELECT ?node (SUM(?o) AS ?out) (SUM(?i) AS ?in) WHERE {
  { ?node ex:cites ?x BIND(1 AS ?o) BIND(0 AS ?i) }
  UNION
  { ?x ex:cites ?node BIND(0 AS ?o) BIND(1 AS ?i) }
}
GROUP BY ?node
ORDER BY DESC(?in)
LIMIT 10
```

Hubs by total degree, for one predicate:

```sparql
SELECT ?hub (COUNT(?x) AS ?degree) WHERE {
  { ?hub ex:cites ?x } UNION { ?x ex:cites ?hub }
}
GROUP BY ?hub
ORDER BY DESC(?degree)
LIMIT 10
```

The degree distribution (how many nodes have degree 1, 2, 3 ...). A heavy tail,
a few hubs and many leaves, is typical of real graphs:

```sparql
SELECT ?degree (COUNT(?node) AS ?nodes) WHERE {
  { SELECT ?node (COUNT(?paper) AS ?degree) WHERE { ?paper ex:author ?node } GROUP BY ?node }
}
GROUP BY ?degree
ORDER BY ?degree
```

Hubs deserve a label. Join `rdfs:label` (or whatever the schema shows) after
you have the top ten, with `VALUES`, not before: ranking first keeps the query
cheap on a large file.

## 2. Neighbourhoods and ego networks

Everything one step from a node, in either direction:

```sparql
SELECT DISTINCT ?neighbour WHERE { ex:p3 (ex:cites|^ex:cites) ?neighbour }
```

Two steps, with `p/p?` (one or two). Counted repeats like `p{1,2}` do not parse:

```sparql
SELECT DISTINCT ?node WHERE { ex:p5 (ex:cites|^ex:cites)/(ex:cites|^ex:cites)? ?node }
```

The ego network as edges, ready for `graph_view` or an edge list: the links
of one node, in both directions, as `?s ?p ?o` rows. Each branch starts from the
bound node, so it stays cheap:

```sparql
SELECT ?s ?p ?o WHERE {
  { ex:p3 ex:cites ?o BIND(ex:p3 AS ?s) }
  UNION
  { ?s ex:cites ex:p3 BIND(ex:p3 AS ?o) }
  BIND(ex:cites AS ?p)
}
```

Add the links among the neighbours with a second query and
`graph_view_update(view_id, query=..., mode="add")`.

Keep the radius small. Each extra hop multiplies the node count; in a graph
with hubs, two hops from a hub can be most of the graph. Count first:

```sparql
SELECT (COUNT(DISTINCT ?node) AS ?size) WHERE { ex:p5 (ex:cites|^ex:cites)/(ex:cites|^ex:cites)? ?node }
```

`describe_entity(dataset, iri)` is the quick look at one node's own statements.

## 3. Reachability and paths

Property paths need one end bound. Who is reached by following citations from
`ex:p5`, at any distance, and who reaches `ex:p1`:

```sparql
SELECT DISTINCT ?older WHERE { ex:p5 ex:cites+ ?older }
```

```sparql
SELECT DISTINCT ?newer WHERE { ?newer ex:cites+ ex:p1 }
```

Is B reachable from A at all (cheap, stops at the first witness):

```sparql
ASK { ex:p5 ex:cites+ ex:p1 }
```

SPARQL answers whether a path exists, not what it is. For a shortest path,
search outwards one hop at a time from both ends (or one) and stop when the
frontiers meet. With the MCP tools, each hop is one `sparql_query` with the
frontier in `VALUES`:

```sparql
SELECT DISTINCT ?next WHERE {
  VALUES ?frontier { ex:p5 ex:p4 }
  ?frontier (ex:cites|^ex:cites) ?next .
}
```

On a network you have exported, `nx.shortest_path` does it (section 5). Cap
the number of hops and the frontier size, and warn the user if you hit the cap. In a graph with hubs the frontier
explodes after two or three hops; start from the end with the lower degree.

## 4. Projections: people through the papers they share

Co-authorship, co-citation, shared membership. Two nodes are linked when they
point to the same middle node. `STR(?a) < STR(?b)` keeps one direction of each
pair and drops self-pairs:

```sparql
SELECT ?a ?b (COUNT(DISTINCT ?paper) AS ?weight) WHERE {
  ?paper ex:author ?a , ?b .
  FILTER(STR(?a) < STR(?b))
}
GROUP BY ?a ?b
ORDER BY DESC(?weight)
```

Co-citation (two papers cited by the same third paper):

```sparql
SELECT ?a ?b (COUNT(DISTINCT ?citing) AS ?weight) WHERE {
  ?citing ex:cites ?a , ?b .
  FILTER(STR(?a) < STR(?b))
}
GROUP BY ?a ?b
```

A projection can be much bigger than the data it comes from: a paper with 500
authors makes 125,000 pairs. Count the middle nodes by size first, and cut the
giants:

```sparql
SELECT ?paper (COUNT(?a) AS ?authors) WHERE { ?paper ex:author ?a }
GROUP BY ?paper
HAVING (COUNT(?a) > 1)
ORDER BY DESC(?authors)
LIMIT 5
```

## 5. Centrality, components and communities: export an edge list

SPARQL has no betweenness, PageRank or modularity. Pull the edges out and use a
library.

**Through the MCP tools.** `sparql_query` returns `table`, a list of rows. The
server caps a result (10,000 rows by default; see `truncated`), which suits a
sample or a filtered subgraph.

**Locally, for the whole network.** `list_datasets` gives each graph's `url`,
range-readable by any rete client. Open it and the query runs on your machine,
reading only the bytes it needs:

```python
import networkx as nx
import rete_graph as rete

g = rete.open("https://example.org/papers.rete")   # the `url` from list_datasets
rows = g.query("""
    PREFIX ex: <https://example.org/>
    SELECT ?a ?b (COUNT(DISTINCT ?paper) AS ?w) WHERE {
      ?paper ex:author ?a , ?b . FILTER(STR(?a) < STR(?b))
    } GROUP BY ?a ?b""")

G = nx.Graph()
for r in rows:
    G.add_edge(r["a"].value, r["b"].value, weight=r["w"].to_python())

print(G.number_of_nodes(), G.number_of_edges())
print(nx.number_connected_components(G))
```

Install with `pip install rete-graph networkx scipy` (`pagerank` needs scipy). Run the analyses:

```python
# hubs and brokers
degree = dict(G.degree(weight="weight"))
between = nx.betweenness_centrality(G)          # fine up to ~10k nodes; sample (k=200) above
top = sorted(between, key=between.get, reverse=True)[:10]

# components: the size of the biggest one tells you if the graph is one network
parts = sorted(nx.connected_components(G), key=len, reverse=True)
print([len(p) for p in parts[:5]])

# communities
communities = nx.community.louvain_communities(G, weight="weight", seed=1)
print(len(communities), [len(c) for c in communities])

# shortest path
print(nx.shortest_path(G, "https://example.org/alan", "https://example.org/kathy")
      if nx.has_path(G, "https://example.org/alan", "https://example.org/kathy") else "no path")
```

On a directed network (citations), build `nx.DiGraph()` and use
`nx.pagerank(D)`, `nx.in_degree_centrality(D)`. igraph (`pip install
python-igraph`) takes the same edge list: `igraph.Graph.TupleList(edges,
directed=True)`; prefer it above a few hundred thousand edges.

Mind the cost on large remote files:

- Count the edges first (`SELECT (COUNT(*) AS ?n) ...`). A few hundred thousand edges is comfortable; tens of millions is a job, not a query.
- Project one predicate and, if possible, one pair of classes. `?s ?p ?o` as an edge list pulls literals and types too.
- Limit by a filter that means something (a year, a type, a lab) before you reach for `LIMIT`; an arbitrary LIMIT gives a biased sample.
- Say what you analysed: the predicate, the filter, and whether it was a sample.

The engine can also do some of this where the file lives: files built with a
community pyramid carry a Louvain-style summary, and the CLI has `rete
communities`, `rete reach` and `rete predicates`. Those are for someone with
the file and the CLI, not through this server.

## 6. Show it

`graph_view(dataset, query=...)` draws a SELECT with `?s ?p ?o` columns (or any
CONSTRUCT) as nodes and edges, in clients that render MCP Apps; elsewhere the
answer is text. Keep it to a few hundred nodes. A LIMIT on a SELECT is honoured;
a CONSTRUCT ignores it (see the `query` skill).

The arguments of a `graph_view` call for the ego network of `ex:p3`, grouped by
type and laid out in rings:

```json
{
  "dataset": "papers",
  "query": "PREFIX ex: <https://example.org/> SELECT ?s ?p ?o WHERE { { ex:p3 ex:cites ?o BIND(ex:p3 AS ?s) } UNION { ?s ex:cites ex:p3 BIND(ex:p3 AS ?o) } BIND(ex:cites AS ?p) } LIMIT 300",
  "title": "Around p3",
  "options": { "group_by": "type", "layout": "concentric", "edge_labels": false }
}
```

The look is in `options`: `layout` is one of `cose` (force-directed, default
for graphs), `layered`, `concentric`, `breadthfirst`, `circle` or `grid`;
`group_by` colours nodes by `type`, `namespace`, `none` or a predicate IRI, and
`group_boxes` draws each group in a box; `diagram` is `graph`, `boxes` or
`classes`. `graph_view(dataset, schema=true)` draws the dataset's own schema, a
good first picture of an unfamiliar graph.

After you compute something locally, put it back on the picture with
`graph_view_update(view_id, highlight=[iri, ...])` for the top brokers, `focus`
to centre one node, `aggregate` to collapse a community to one node, or
`query` to add another edge set. `graph_view_state(view_id)` tells you what the
user has selected, so "these" and "this one" mean something.

A picture of 5,000 nodes is a hairball. Show the top hubs and their links, one
community, or one ego network, and say what was left out.

## 7. What to report

Give the answer first (who, how many), then how you got it: the edge definition,
the size (nodes and edges), whether it was the whole graph or a sample, and the
measure. "Most central" means nothing without saying central by what: degree,
betweenness, PageRank answer different questions.
