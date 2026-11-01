# rete

Rete stores RDF data in one `.rete` file. Query it on your computer, or put it
on a web host that supports byte-range requests. A client can then read the
parts needed for a SPARQL query without downloading the file first.

**[Try the Playground](playground-guide.md)** or **[build your first graph](getting-started.md)**.
If RDF is new to you, begin with [Graph concepts](intro.md).

<span id="why"></span>

## A file you can share and query

The file contains the data and indexes used to find it. Optional summaries
and a Dataset Card help readers understand what is inside. The same engine
runs natively and in WebAssembly, so a browser can query a hosted file without
a dedicated database backend.

<img src="img/lazy-open.svg" alt="A client reads selected byte ranges from a hosted .rete file to answer a query.">

This is useful for publishing snapshots, distributing graphs and embedding
queryable data in an application. Selective queries can read little data;
broad scans and large results can still require substantial I/O and memory.
An immutable file also has different update trade-offs from a database service.

<span id="60-second-tour"></span>
<span id="documentation"></span>
<span id="start-here"></span>
<span id="explore-in-the-browser"></span>
<span id="guides"></span>
<span id="publish--share"></span>
<span id="graph-analysis"></span>
<span id="clients"></span>

## Choose a task

| I want to… | Start here |
|---|---|
| Run a first query | [Quickstart](getting-started.md) or [Playground](playground-guide.md) |
| Turn my data into a shareable graph | [Build and publish](build-and-publish.md) |
| Ask questions and analyze relationships | [Query and analyze](query-and-analyze.md) |
| Use rete in an application | [Language clients](clients.md) and [integrations](integrations.md) |
| Learn from a complete example | [Worked examples](recipes.md) |
| Explore visual demonstrations | [Apps and experiments](apps.md) |

<span id="development"></span>
<span id="version--status"></span>

## Know the boundaries

Check [compatibility and limits](compatibility.md) when choosing a reader
version or a build feature. Rete remains a 0.x project; format generation
alone does not guarantee that an older reader supports every newer feature.

The [technical reference](reference.md) links exact command, API and format
contracts. For implementation work, use [Contributing](contributing.md).
Source and release history are available in the
[repository](https://github.com/caviri/rete).
