# Understanding graph data {#graph-data-for-dummies}

A graph records things and the relationships between them. A software dependency
graph is one example: an application uses a web library, which uses a logging
library. Following those relationships helps answer which packages a vulnerable
library might affect.

## Start with three facts {#what-is-a-graph-aka-a-network}

Here is the same small graph used in [Getting started](getting-started.md):

```text
app     --dependsOn-> web
web     --dependsOn-> logging
logging --hasVulnerability-> CVE-2099-0001
```

The packages and vulnerability are **nodes**. Each arrow is an **edge**, with a
name describing the relationship. We can ask about one edge (what does `app`
depend on?) or a path (what eventually depends on `logging`?). Tables can
represent these facts too, and relational databases can follow paths with
recursive queries. Graph notation makes the relationships explicit.

## Give each fact a shared meaning {#two-big-families-rdf-vs-labeled-property-graphs}

RDF writes each fact as a **triple**: subject, predicate, object. The first edge
above becomes this N-Triples statement:

```nt
<http://ex/app> <http://ex/dependsOn> <http://ex/web> .
```

An **IRI** identifies a resource, including the relationship itself. Using
`http://ex/dependsOn` rather than a bare `dependsOn` gives that name a namespace.
These example IRIs are identifiers; a query need not visit them as web pages.
An object can also be a **literal**, such as a package name `"Web library"`,
a number, or a date. A named graph groups statements under another identifier;
RDF datasets contain a default graph and may contain named graphs.

Labeled Property Graphs, used by systems such as Neo4j, organize data as nodes
and edges with properties. RDF uses shared IRIs and triples, with additional
modeling for statements about edges. Rete stores RDF; its limited read-only
Cypher translation is described in [Compatibility](compatibility.md).

<a id="the-standards-plainly"></a>
The surrounding standards serve different purposes. **RDFS/OWL** express schema
and logical relationships; **SHACL** checks required data shapes; **SPARQL** asks
questions. N-Triples, Turtle and JSON-LD are ways to serialize RDF, rather than
different graph models. Rete supports [SHACL Core](shacl.md), not SHACL-SPARQL,
SHACL-AF or ShEx, and a documented [reasoning subset](reasoning.md), not full
OWL DL.

## Ask for a path {#what-kinds-of-questions-can-you-actually-ask-rete}

SPARQL matches patterns against RDF. A name beginning with `?` is a variable;
`PREFIX` abbreviates an IRI. This query asks for every package that reaches
`logging` by following `dependsOn` one or more times:

```sparql
PREFIX e: <http://ex/>
SELECT ?package WHERE {
  ?package e:dependsOn+ e:logging
}
ORDER BY ?package
```

The answer is `http://ex/app` and `http://ex/web`. The `+` includes both the
direct edge from `web` and the two-edge path from `app`. Without `+`, only
`web` matches. More patterns sharing a variable form a join; aggregates such
as `COUNT` summarize the matches. [Getting started](getting-started.md) builds
this exact graph and runs the query.

See [SPARQL support](sparql.md) for query forms, property paths, named graphs,
and execution-mode support for local files and remote URLs.

## Read a graph from a file {#where-rete-fits}

Rete compiles RDF into one immutable `.rete` file with a dictionary and indexes.
A client can open it locally or read byte ranges from a static HTTP URL and
execute queries itself. The host serves bytes; it does not run SPARQL.

For the dependency query, the client needs the identifiers and index data that
match the path. That does not guarantee a small transfer: broad patterns and
long traversals may read much more than a point lookup, and a tiny graph may
arrive in full. An optional community pyramid supports coarse overviews;
summary-based progressive answers are approximate, rather than full graph
results. The playground reports fetched bytes so you can see the cost.

The file is a snapshot. Check [Compatibility](compatibility.md) when sharing it
with older readers. For changes over time, rebuild the snapshot or use [`rete serve`](cli.md), which keeps updates
in a journal while the base file stays immutable.

Try [Getting started](getting-started.md), explore the
[playground](playground-guide.md), or follow the
[dependency-impact scenario](scenario.md) to publish a graph.
