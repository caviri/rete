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

Wheels are abi3: one wheel per platform covers every CPython ≥ 3.9. The source
lives in the repository under `clients/python/`; a runnable
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
| Build options: pyramid algo, text index | ✅ | `Builder.pyramid()/text_index()/type_predicate()` |
| Schema profile, prefix & text search | ✅ | `schema()`, `prefix_search()`, `text_search()` |
| pandas DataFrames | ✅ | `query_df()`, `[pandas]` extra |
| SHACL validation | ⏳ | use the [CLI](cli.md) meanwhile |
| Reachability / communities / provenance (`why`) | ⏳ | planned bindings |
| Multi-shard federated open | ⏳ | planned |
| Writes / SPARQL UPDATE | — | `.rete` is immutable; use [`rete serve`](cli.md) |

## Development

The package is a maturin project at `clients/python/` (excluded from the cargo
workspace). Build and test without a host toolchain:

```sh
docker run --rm -v "$PWD":/io ghcr.io/pyo3/maturin build \
    --release -m clients/python/Cargo.toml --out clients/python/dist
uv pip install clients/python/dist/*.whl pytest && pytest clients/python/tests
```

CI: `python-test.yml` runs lint + wheel + pytest whenever `clients/python/` or
`rete-core` changes; a `py-v*` tag runs `python.yml`, which builds the full
wheel matrix and publishes to PyPI.
