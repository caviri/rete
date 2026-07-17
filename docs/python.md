# Python API

`rete-graph` is the Python client for `.rete` files: native bindings (PyO3) to
the same Rust engine behind the [CLI](cli.md) and the
[browser playground](playground-guide.md). It opens a graph from a **local
path, an HTTP(S) URL, in-memory bytes, or a custom reader object** and queries
it with SPARQL — remote files are read lazily over HTTP `Range` requests, so a
selective query over a multi-GB file fetches kilobytes, never the file.

```sh
uv pip install rete-graph          # or: pip install rete-graph
```

Works anywhere real CPython ≥ 3.9 runs: scripts, Jupyter, **marimo**
(desktop/server), Colab, uv/pip/conda environments, on Linux, macOS, and
Windows. From **0.2.0** there are also Pyodide wheels for browser Pythons —
see [JupyterLite & marimo WASM](#browser-python-jupyterlite--marimo-wasm)
below. A runnable
[Jupyter notebook tour](https://github.com/caviri/rete/blob/main/clients/python/examples/tutorial.ipynb)
covers everything below with captured outputs.

## Open a graph

```python
import rete_graph as rete

g = rete.open("https://data.graphplaza.com/boe/boe.rete")     # remote, lazy
g = rete.open("data/example.rete")                            # local file, lazy too
g = rete.open(file_bytes)                                     # bytes image, eager
g = rete.open(url, headers={"Authorization": "Bearer ..."})   # authed hosts
```

All four paths return the same `Graph`. Remote and local opens are **lazy**:
the header, dictionary directory, and index tile directories load up front;
tile payloads fault in per query and stay cached on the handle, so repeated
queries get faster. The host serving a remote file must answer `Range`
requests with `206 Partial Content` (any S3/R2/CDN/GitHub URL does — see
[Hosting your .rete](hosting.md)); anything else is a loud error, never a
silently wrong slice.

`g.stats()` reports the physical traffic since open —
`{"fileLength": ..., "bytes": ..., "requests": ...}` — the number that makes
the lazy story visible: a `LIMIT 3` scan of a 6.9 MB remote file fetches
~24% of it in ~13 requests; a cache-hit re-run adds ~0.

### Custom readers (fsspec, S3, anything)

Any object with `read_at(offset, length) -> bytes` and a length (a `len()`
method or `__len__`) can back a graph — so fsspec reaches authenticated
S3/GCS/Azure with no rete-specific code:

```python
import fsspec, rete_graph as rete

class FsspecReader:
    def __init__(self, url, **kw):
        self.f = fsspec.open(url, "rb", **kw).open()
        self.size = self.f.size
    def len(self):
        return self.size
    def read_at(self, offset, length):
        self.f.seek(offset)
        return self.f.read(length)

g = rete.open(reader=FsspecReader("s3://my-bucket/data.rete"))
```

## Query

```python
rows = g.query("""
    SELECT ?s ?label WHERE {
        ?s <http://www.w3.org/2000/01/rdf-schema#label> ?label
    } LIMIT 10
""")
for row in rows:                       # SELECT -> list of {var: Term}
    print(row["s"].value, row["label"].to_python())

g.query("ASK { ?s ?p ?o }")            # -> bool
g.query("CONSTRUCT { ... } WHERE { ... }")   # -> list of (s, p, o) Term triples
```

Every value is a `Term` with `.kind` (`"iri"` / `"literal"` / `"bnode"` /
`"triple"` for RDF-star), `.value`, `.datatype`, `.lang`, plus:

- `.to_python()` — int/float/bool for the common XSD datatypes, else the string
- `.n3` — the term back in N-Triples surface form

`g.query_raw(q)` returns the engine's JSON envelope unparsed (the same shape
the playground uses), and `g.query_df(q)` returns a pandas DataFrame
(`pip install rete-graph[pandas]`).

### Run the file's own example queries

A `.rete` can carry **example SPARQL queries inside the file** (in its
[Dataset Card](dataset-cards.md)); CLI-built datasets ship a whole tiered
starter library. `g.examples()` reads them — on a remote file that costs one
small ranged read:

```python
g = rete.open("https://data.graphplaza.com/boe/boe.rete")
for ex in g.examples():                     # 20 entries on this dataset
    print(ex["title"], "—", ex["question"])
g.query(g.examples()[0]["sparql"])          # and they just run
```

Embed your own with `Builder.example()` (rich: title + question + SPARQL) or
`Builder.card(example_queries=[...])` (plain strings).

### Reasoning

`g.query(q, reason=True)` turns on OWL 2 QL entailment — `rdfs:subClassOf`,
`subPropertyOf`, `domain`/`range` and friends, computed by **query rewriting**
over the file's ontology, so it works over lazy remote files without
materializing anything. See [Reasoning & coherence](reasoning.md).

### Federation

SPARQL 1.1 `SERVICE` works out of the box: join a `.rete` file against any
public SPARQL endpoint (Wikidata, DBpedia, …) in one query. See
[Federated queries](federation.md).

## Build `.rete` files from Python

`rete.build()` produces a complete file image, ready to `open()`, save, or
upload:

```python
data = rete.build(nt_text)                   # N-Triples text ("nt", "nq", "ttl")
pathlib.Path("out.rete").write_bytes(data)
```

It also accepts a **graph object from another RDF library** — anything with a
`.serialize(format=...)` method (duck-typed; rdflib is not a dependency). An
rdflib `Graph` round-trips as N-Triples; a context-aware `Dataset` /
`ConjunctiveGraph` as N-Quads, so named graphs survive:

```python
import rdflib, rete_graph as rete

g = rdflib.Graph()
g.parse("mydata.ttl")

rg = rete.open(rete.build(g))                # rdflib -> .rete -> SPARQL
```

For step-by-step preparation — an embedded Dataset Card, pyramid options, the
full-text index — use the **lazy `Builder`**: configure, then `run()` and
`export()`:

```python
builder = (
    rete.Builder()
    .add_file("people.ttl")
    .card(title="People", license="CC0-1.0")
    .pyramid(algo="louvain")        # or "types", or .pyramid(False)
    .text_index()
)
builder.run()                        # bytes; stats in builder.stats
builder.export("people.rete")
```

The full walkthrough (every card field, pyramid trade-offs, verification) is
in [Python: build a .rete](python-build-tutorial.md). In-memory builds suit
tests and small-to-medium graphs (say, up to a few million triples). For big
datasets use the [`rete build` CLI](cli.md), which streams from disk,
compresses harder, and derives the enriched card profile — see
[Tables, VKG & big builds](data-engineering.md).

## Search and overview

```python
g.schema()                    # {"classes": [(iri, count)], "relations": [(s, p, o, count)]}
g.prefix_search("Berl")       # label autocomplete -> [(label, subject_iri)]
g.text_search("volcano")      # full-text; needs a file built with --text-index
g.info()                      # {"quads": ..., "terms": ..., "pyramidLevels": ...}
g.quads, g.terms              # header counts, as properties
g.graph_names()               # named graphs in a dataset
g.content_hash()              # blake3-16 hex — a stable cache key
```

## Browser Python: JupyterLite & marimo WASM {#browser-python-jupyterlite--marimo-wasm}

From version 0.2.0 the release also ships **PyEmscripten (Pyodide) wheels**,
so the client runs inside browser Pythons — a JupyterLite site, marimo's WASM
playground — with no server anywhere. **Try it right now** in the
[JupyterLite experiment](jupyterlite-guide.md) bundled with these docs.

```python
# JupyterLite / Pyodide 0.29 (Python 3.13)
%pip install https://data.graphplaza.com/wheels/rete_graph-0.2.0-cp39-abi3-pyodide_2025_0_wasm32.whl

import rete_graph as rete
g = rete.open("https://data.graphplaza.com/boe/boe.rete")
g.query("SELECT ?s ?p ?o WHERE { ?s ?p ?o } LIMIT 5")
```

(The URL install is a transition-window workaround: PyPI requires the new
PEP 783 wheel-tag spelling, which Pyodide 0.29's installer doesn't recognize
yet — the hosted wheel is byte-identical to the PyPI one, just retagged. Once
your runtime's installer understands `pyemscripten` tags, a plain
`%pip install rete-graph` resolves from PyPI.)

Under the hood, remote reads use synchronous `XMLHttpRequest` range requests —
allowed only in **web workers**, which is where JupyterLite and marimo run
their kernels, so it just works there. The file's host must send CORS headers
and honor `Range`, the same contract as the [playground](playground-guide.md).

Differences from the native wheels, by browser necessity:

- Range fetches are sequential (no threads in wasm) — fine in practice, since
  the block cache already coalesces reads.
- `SERVICE` federation is unavailable.
- In-browser `build()` writes uncompressed sections (like the playground's
  Build tab); every reader accepts them.
- wasm32 means a 4 GiB memory ceiling — lazy *remote* querying is the
  intended use, not giant in-memory builds. (A wasm64 build lifts this, and
  is tracked as future work pending Pyodide support.)

Pyodide wheels are per-ABI-year (not abi3): current Pyodide releases are
covered; if yours isn't, `micropip.install("<url of the .whl>")` works from
any CORS-enabled host.

## Guarantees and threading

- **No silent partial results**: if a range fetch fails mid-query on a lazy
  handle, the query raises instead of returning fewer rows.
- **The GIL is released** around every engine call, and remote range reads fan
  out over an internal thread pool (16-way, like the CLI and the playground) —
  other Python threads keep running during long queries.
- Errors surface as ordinary exceptions: `ValueError` for bad SPARQL or RDF
  input, `RuntimeError` for I/O and format problems.

## Feature matrix

| Capability | Python | Notes |
|---|---|---|
| SPARQL SELECT / ASK / CONSTRUCT / DESCRIBE | ✅ | `query()` |
| Lazy remote open (HTTP Range) | ✅ | `open(url)`, custom `headers` |
| Lazy local open | ✅ | positional reads, no whole-file load |
| Custom reader objects | ✅ | `open(reader=...)` — fsspec/S3 |
| OWL 2 QL reasoned queries | ✅ | `query(..., reason=True)` |
| `SERVICE` federation | ✅ | host-injected HTTP client |
| Build from RDF text / rdflib objects | ✅ | `build()`, `Builder` |
| Dataset Card: embed + read back | ✅ | `Builder.card()`, `Graph.card()` (ranged on remote) |
| Example queries in the file | ✅ | `Builder.example()`, `Graph.examples()` |
| Build options: pyramid algo, text index | ✅ | `Builder.pyramid()/text_index()/type_predicate()` |
| Schema profile, prefix & text search | ✅ | `schema()`, `prefix_search()`, `text_search()` |
| pandas DataFrames | ✅ | `query_df()`, `[pandas]` extra |
| Browser Python (Pyodide) | ✅ 0.2.0 | JupyterLite / marimo WASM; no SERVICE, sequential fetches |
| SHACL validation | ⏳ | use the [CLI](cli.md) meanwhile |
| Reachability / communities / provenance (`why`) | ⏳ | planned bindings |
| Multi-shard federated open | ⏳ | planned |
| Writes / SPARQL UPDATE | — | `.rete` is immutable; use [`rete serve`](cli.md) |

## For contributors

This page is for *using* the package. Building from source, the CI layout,
the release/publishing procedure, and the checklist for adding new language
clients live in [Client development & releases](clients-dev.md).
