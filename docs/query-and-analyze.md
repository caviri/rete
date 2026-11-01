# Query and analyze

A useful first query answers a small question about a graph you understand.
Inspect its card, classes and example queries before writing your own: the
names used in RDF are exact identifiers, so a plausible name can still match
nothing.

## Find an answer

Use the [Playground](playground-guide.md) to query without installing anything,
or choose a [language client](clients.md). The [SPARQL reference](sparql.md)
describes supported syntax and limitations; the [CLI reference](cli.md)
covers query commands and output formats.

For a specific kind of question:

- [GeoSPARQL](geosparql.md) filters geometry and time.
- [Federation](federation.md) combines graphs; it explains the differences
  between a union, a cross-file join and a remote `SERVICE` call.
- [Reasoning](reasoning.md) uses ontology relationships to derive answers
  that are not written explicitly in the data.
- [Semantic zoom](semantic-zoom.md) gives an overview of classes and communities
  before you inspect individual entities.

## Check the data and the cost

[SHACL validation](shacl.md) checks a graph against shapes: for example,
whether each entity has a required property. Read the reported violations
alongside the shape that produced them.

Range reads avoid loading a whole file at open time. They do not make every
query small: a broad scan, join or large result set can still read and retain
substantial data. Start with a selective question and inspect its cost; see
the [CLI reference](cli.md) and [compatibility and limits](compatibility.md).

## Compute beyond SPARQL

[Graph algorithms](graph-algorithms.md) explains how to project an edge list,
compute with Python libraries, and write results back to a new graph.
[Topic modeling](topic-modeling.md) labels communities by their text;
[multi-criteria communities](multi-criteria.md) combines structural and
attribute-based partitions. These are different analyses, so choose by the
question you want to answer rather than by the visualization.
