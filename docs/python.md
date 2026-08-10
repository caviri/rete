# Python API

Welcome to the `rete-graph` Python client! This package provides native Python bindings (via PyO3) to the same high-performance Rust engine powering the [CLI](cli.md) and the [browser playground](playground-guide.md).

With this client, you can open a graph from a **local path, an HTTP(S) URL, in-memory bytes, or a custom reader object** and query it using SPARQL. Because remote files are queried lazily using HTTP `Range` requests, running a selective query over a multi-gigabyte file only fetches a few kilobytes!

## Installation

Install via pip or uv. The package works anywhere CPython ≥ 3.9 runs (scripts, Jupyter, marimo, Colab, Linux, macOS, Windows).

```sh
uv pip install rete-graph          # or: pip install rete-graph
```

*(Note: We also provide Pyodide wheels for browser Pythons! See [Browser Python](#browser-python-jupyterlite--marimo-wasm) below.)*

> **Try it out:** Check out our [interactive Jupyter notebook tour](https://github.com/caviri/rete/blob/main/clients/python/examples/tutorial.ipynb) which covers everything below with runnable examples.

## 1. Open a Graph

You can open a graph from virtually any source. All methods return the exact same `Graph` object.

```python
import rete_graph as rete

# 1. Remote and lazy (fetches bytes only as needed)
g = rete.open("https://data.graphplaza.com/boe/boe.rete")

# 2. Local and lazy (reads from disk only as needed)
g = rete.open("data/example.rete")

# 3. In-memory eager (from a bytes object)
g = rete.open(file_bytes)

# 4. Authenticated remote hosts
g = rete.open(url, headers={"Authorization": "Bearer ..."})
```

### How Lazy Loading Works

Local and remote opens are **lazy**:
- The file header, dictionary directory, and index tile directories are loaded immediately.
- Tile payloads (the actual data) are fetched *only* when your query needs them, and then they are cached on the graph handle for subsequent queries.

You can verify the physical network traffic using `.stats()`:
```python
print(g.stats())
# Example output: {'fileLength': 7234567, 'bytes': 15432, 'requests': 13}
```
*(A standard `LIMIT 3` query might only download a tiny fraction of the file, and a re-run will fetch 0 new bytes!)*

> **Host Requirements for URLs:** The host must support HTTP `Range` requests and return `206 Partial Content` (like S3, R2, CDNs, or GitHub). If it doesn't, you will get a clear error, not a silently corrupted file. See [Hosting your .rete](hosting.md).

### Using Custom Readers (S3, GCS, fsspec)

You aren't limited to standard files or HTTP. You can pass any Python object that implements `read_at(offset, length)` and a `len()` method. This makes integrating with `fsspec` (for S3, Azure, GCS) trivial!

```python
import fsspec
import rete_graph as rete

class FsspecReader:
    def __init__(self, url, **kw):
        self.f = fsspec.open(url, "rb", **kw).open()
        self.size = self.f.size
        
    def __len__(self):
        return self.size
        
    def read_at(self, offset, length):
        self.f.seek(offset)
        return self.f.read(length)

g = rete.open(reader=FsspecReader("s3://my-bucket/data.rete"))
```

## 2. Querying Data

Query your graph using standard SPARQL:

```python
rows = g.query("""
    SELECT ?s ?label WHERE {
        ?s <http://www.w3.org/2000/01/rdf-schema#label> ?label
    } LIMIT 10
""")

# SELECT queries return a list of dictionaries mapping variables to Terms
for row in rows:
    print(row["s"].value, row["label"].to_python())

# ASK returns a boolean
is_present = g.query("ASK { ?s ?p ?o }") 

# CONSTRUCT/DESCRIBE returns a list of (s, p, o) Term triples
triples = g.query("CONSTRUCT { ... } WHERE { ... }")
```

### The `Term` Object
Every value returned is a `Term` with useful properties:
- `.kind`: `"iri"`, `"literal"`, `"bnode"`, or `"triple"` (for RDF-star).
- `.value`, `.datatype`, `.lang`: Standard RDF components.
- `.to_python()`: Converts common XSD datatypes to native Python `int`, `float`, or `bool`.
- `.n3`: Returns the term formatted as an N-Triples string.

### Pandas & Raw Data
- Want dataframes? Run `pip install rete-graph[pandas]` and use `g.query_df(q)`.
- Want raw engine JSON? Use `g.query_raw(q)`.

## 3. Powerful Graph Features

### Example Queries
`.rete` files can embed starter queries inside their [Dataset Card](dataset-cards.md). You can list and run them directly—which only costs one small range read on a remote file!

```python
for ex in g.examples():
    print(ex["title"], "—", ex["question"])

# Run the first embedded example directly!
g.query(g.examples()[0]["sparql"])
```

### Reasoning (OWL 2 QL)
Want intelligent inference? Enable OWL 2 QL reasoning, which dynamically rewrites your query based on the graph's ontology (`rdfs:subClassOf`, `domain`, etc.). No materialization required, so it works flawlessly on lazy remote files!

```python
g.query(q, reason=True)
```
See [Reasoning & coherence](reasoning.md).

### Federation
Join your `.rete` file against public SPARQL endpoints (like Wikidata) in a single query using SPARQL 1.1 `SERVICE` blocks. See [Federated queries](federation.md).

## 4. Search and Explore Metadata

Easily explore graphs you didn't build:

```python
g.schema()                    # Get class and relation counts: {"classes": [...], "relations": [...]}
g.prefix_search("Berl")       # Fast label autocomplete -> [(label, subject_iri)]
g.text_search("volcano")      # Full-text search (if the file was built with --text-index)
g.info()                      # Returns {"quads": ..., "terms": ..., "pyramidLevels": ...}
g.graph_names()               # List named graphs in a dataset
g.content_hash()              # Get the blake3-16 hex hash (great as a cache key!)
```

## 5. SHACL Validation

Validate your graph against SHACL Core shapes written in Turtle. Validation is **lazy-aware**: when checking a remote graph, only the specific targets of the shapes are fetched over the network!

```python
shapes = """
@prefix sh: <http://www.w3.org/ns/shacl#> .
@prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> .

[] a sh:NodeShape ;
  sh:targetClass <https://example.org/Person> ;
  sh:property [ sh:path rdfs:label ; sh:minCount 1 ] .
"""

report = g.shacl(shapes)
print("Conforms?", report["conforms"])
print("Violations:", report["results"])

# Get the report as a Turtle string instead
ttl_report = g.shacl(shapes, format="ttl")
```
Use the `graph="<iri>"` argument to validate a specific named graph.

## 6. Build `.rete` Files from Python

You can easily build `.rete` files from RDF text or directly from `rdflib` objects.

```python
# 1. From raw text
data = rete.build(nt_text) # Handles "nt", "nq", "ttl"
with open("out.rete", "wb") as f:
    f.write(data)

# 2. From an rdflib Graph
import rdflib
g_rdf = rdflib.Graph()
g_rdf.parse("mydata.ttl")
rg = rete.open(rete.build(g_rdf))
```

### The `Builder` API
For granular control over metadata, community pyramids, and text indexing, use the lazy `Builder`:

```python
builder = (
    rete.Builder()
    .add_file("people.ttl")
    .card(title="People", license="CC0-1.0")
    .pyramid(algo="louvain")        # "louvain", "types", or False
    .text_index()
)

builder.run() # Builds the graph
builder.export("people.rete")
```
See the full tutorial: [Python: build a .rete](python-build-tutorial.md).

> **For Big Data:** In-memory building is great for tests and graphs up to a few million triples. For massive datasets, use the [`rete build` CLI](cli.md), which streams from disk and uses advanced compression.

## 7. Browser Python: JupyterLite & marimo WASM

We provide **PyEmscripten (Pyodide) wheels**, meaning this client runs directly inside browser-based Pythons (like JupyterLite or marimo) with zero servers required!

```python
# Inside JupyterLite (Python 3.14+)
%pip install rete-graph
import rete_graph as rete

g = rete.open("https://data.graphplaza.com/boe/boe.rete")
g.query("SELECT ?s ?p ?o WHERE { ?s ?p ?o } LIMIT 5")
```

**Key Details for Browser Environments:**
- Remote reads use synchronous `XMLHttpRequest` range requests. Because browsers only allow this in **web workers**, it runs perfectly in JupyterLite/marimo kernels, but would block the main thread.
- Due to WebAssembly limitations, range fetches are sequential and `SERVICE` federation is unavailable. 
- In-browser builds write uncompressed data.

See the [JupyterLite experiment guide](jupyterlite-guide.md).

## Guarantees and Threading

- **No Silent Failures:** If a range fetch fails mid-query, the query raises an exception rather than returning incomplete rows.
- **True Concurrency:** The Python GIL is released during engine execution. Remote reads fan out over an internal 16-way thread pool, keeping your other Python threads responsive!
- **Clear Exceptions:** Errors surface as native Python exceptions (`ValueError` for bad syntax, `RuntimeError` for I/O).
