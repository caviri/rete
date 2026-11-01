# Build and publish a graph

Start with RDF data, build a `.rete` file, then give readers a local file or a
URL. You do not need to run a database service to share it: a suitable static
host can serve the byte ranges that clients request.

## Make a file readers can understand

The [Quickstart](getting-started.md) walks through a small build and query.
For your own data, include a [Dataset Card](dataset-cards.md) with its title,
licence and source. Readers can inspect the card before choosing what to query.

- [Build from Python](python-build-tutorial.md) when the graph comes from your code.
- [Import, export and interoperability](interop.md) covers RDF syntaxes, named
  graphs and exchanging data with other stores.
- [Large builds and tabular data](data-engineering.md) explains the choices when
  the source is a table or the graph exceeds available memory.

Use the [CLI reference](cli.md) for exact build flags and defaults.

## Put it on a URL

[Hosting a graph](hosting.md) explains HTTP Range requests and browser CORS
settings. Test the published URL with a real query before sharing it; a URL
that downloads successfully is not necessarily suitable for browser queries.

[Media and SQL companions](media-companions.md) describes how images, geometry,
audio and table exports can accompany a graph. These are optional: start with
the graph and add the representations your readers need.

## Maintain a published graph

A `.rete` file is immutable. Rebuilding it or changing its card changes its
identity, even if the data is otherwise unchanged. See
[updating a card](dataset-cards.md#maintaining-a-published-card) and
[compatibility](compatibility.md) before replacing a published file.

For applications that need edits between rebuilds,
[writable graphs](manifest.md) explains the manifest and journal approach,
including its experimental status and limits.
