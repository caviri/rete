# R API

Welcome to `rete` for R! This package provides native R bindings (via extendr) to the same powerful Rust engine that drives the [CLI](cli.md), the [Python client](python.md), and the [browser playground](playground-guide.md). 

With this client, you can open a graph from a **local file, an HTTP(S) URL, or a raw vector** in memory and query it using SPARQL. Best of all, results are returned as **standard R data frames**. 

Because remote files are read lazily using HTTP `Range` requests, running a selective query over a multi-gigabyte file only downloads a few kilobytes—never the entire file!

## Installation

Currently, the package is installed directly from GitHub.

**Prerequisites:** You must have Rust ≥ 1.87 installed on your system (get it from [rustup.rs](https://rustup.rs)).

```r
# Install from the main branch
install.packages("remotes")
remotes::install_github("caviri/rete", subdir = "clients/r", build = FALSE)

# Or, install from a specific branch, tag, or commit:
remotes::install_github("caviri/rete@main", subdir = "clients/r", build = FALSE)
```

> **Why `build = FALSE`?**
> The R package lives inside a monorepo and its Rust crate references the core engine at the repository root. `build = FALSE` ensures it installs from the full source tree rather than a disconnected subdirectory tarball.
> *(Note: Binary installs via R-universe/CRAN that won't require Rust are planned for the future!)*

Once installed, you can open the offline tutorial using `vignette("rete")` or check the reference pages (`?rete_open`, `?rete_query`, `?rete_build`).

## 1. Open a Graph and Query It

Opening a graph is straightforward, whether it's local or remote.

```r
library(rete)

# Open lazily from a remote URL (fetches only what it needs)
g <- rete_open("https://data.graphplaza.com/boe/boe.rete")   

# Open lazily from a local file
g <- rete_open("data/example.rete")                          

# Open eagerly from a raw vector in memory
g <- rete_open(file_image)                                   

# Run a SPARQL query
results <- rete_query(g, "
  SELECT ?title WHERE {
    ?law <http://data.europa.eu/eli/ontology#title> ?title
  } LIMIT 5
")
```

### Understanding Results

`rete_query()` automatically converts SPARQL results into familiar R structures:

- **`SELECT`**: Returns a `data.frame` with one column per variable. 
  - IRI brackets are stripped.
  - `xsd:integer` family literals become R integers (or doubles on overflow).
  - `xsd:decimal`, `double`, and `float` become R doubles.
  - `xsd:boolean` becomes R logicals.
  - Everything else remains character data.
- **`ASK`**: Returns a logical scalar (`TRUE` or `FALSE`).
- **`CONSTRUCT` / `DESCRIBE`**: Returns a `data.frame` with three columns: `subject`, `predicate`, and `object`.

Need the raw, uncoerced data? Use `rete_query_raw()` to get the engine's JSON envelope parsed into an R list, preserving full N-Triples tokens (like `<iri>`, `"lit"^^<datatype>`, `_:bnode`).

### The Magic of Lazy Loading

Both `rete_open(url)` and `rete_open(path)` are **lazy**. 
- The file header, dictionary directory, and index tile directories are loaded upfront.
- Tile payloads are fetched exactly when your query needs them and are cached on the graph handle, making subsequent queries blazing fast.

> **Host Requirements:** To query a remote file, the host must support HTTP `Range` requests and return `206 Partial Content` (standard for S3, R2, CDNs, and GitHub). If it doesn't, the client will immediately throw an error rather than silently reading the wrong data. See [Hosting your .rete](hosting.md).

You can check your network efficiency at any time:

```r
# Check physical traffic since the graph was opened
rete_stats(g)
#> $fileLength  … $bytes  … $requests
```
*(Typically, a query over a multi-hundred MB remote file fetches well under 1% of its total size!)*

## 2. Reasoning (OWL 2 QL)

Turn on OWL 2 QL entailment simply by adding `reason = TRUE`. 

```r
rete_query(g, query, reason = TRUE)
```

Because reasoning is computed dynamically via **query rewriting** over the file's embedded ontology, it requires no upfront materialization. This means it works seamlessly and instantly over remote files too. See [Reasoning](reasoning.md) for details.

## 3. Explore a Graph

Even if you didn't build the `.rete` file, you can easily inspect its contents, metadata, and schema:

```r
rete_info(g)                        # Overview: quads, terms, pyramid levels, named graphs
rete_card(g)                        # The embedded Dataset Card as an R list (or NULL)
rete_examples(g)                    # Starter queries included in the card, as a data.frame
rete_schema(g)                      # Class and predicate profiles (returns two data.frames)
rete_prefix_search(g, "Mad")        # Fast autocomplete for labels starting with "Mad"
rete_text_search(g, "madrid ley")   # Full-text search (requires a file built with text-index)
rete_content_hash(g)                # The blake3-16 hex hash of the file
```

> **Fast Metadata:** Functions like `rete_card()` and `rete_examples()` only fetch the metadata byte range. Reading them from a remote file costs just a few tiny network requests.

You can even run the embedded starter queries directly:

```r
ex <- rete_examples(g)
rete_query(g, ex$sparql[[1]])
```

## 4. Build a `.rete` File from R

You can build a `.rete` file completely in memory using R strings. This is perfect for testing, small graphs, and data pipelines.

```r
# Define some raw RDF text
nt <- '
<urn:x:alice> <http://xmlns.com/foaf/0.1/knows> <urn:x:bob> .
<urn:x:alice> <http://xmlns.com/foaf/0.1/name> "Alice" .
'

# Build the .rete image
img <- rete_build(nt,
  format = "nt",                       # Options: "nt", "nq", "ttl", "rdfxml"
  card = list(
    title = "Tiny demo",
    description = "Two triples about Alice",
    license = "CC0-1.0"
  ),
  pyramid = "louvain",                 # Community detection: "louvain", "types", or "none"
  text_index = TRUE                    # Enable full-text search
)

# Save it to disk...
writeBin(img, "demo.rete")             

# ...or query it directly in memory!
rete_query(rete_open(img), "SELECT ?n WHERE { ?s <http://xmlns.com/foaf/0.1/name> ?n }")
```

When you build a graph, statistics like `triple_count` and `term_count` are automatically injected into the Dataset Card. 

> **For Big Data:** In-memory building is great for small graphs. For massive datasets, we recommend using the [`rete build` CLI](cli.md), which streams from disk and uses advanced compression.

## Write Once, Query Everywhere

The true power of the `.rete` format is its portability. A `.rete` file built in R can be read exactly the same way by the [Python client](python.md), the [JavaScript client](javascript.md), the [CLI](cli.md), and the [browser playground](playground-guide.md). 

Publish a single file to any static host, and every environment can query it lazily!
