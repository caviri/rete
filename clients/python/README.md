# rete-graph — Python client for `.rete` files

Query **local and remote `.rete` graph files with SPARQL** from Python. A
`.rete` file is a single, immutable, range-queryable RDF graph file
([rete](https://github.com/caviri/rete)): drop it on any HTTP host that
supports `Range` requests and query it in place — the client fetches only the
byte ranges a query touches, never the whole file.

These are native bindings (PyO3) to the same Rust engine that powers the rete
CLI and the [browser playground](https://caviri.github.io/rete/playground.html).

## Install

```sh
uv pip install rete-graph        # or: pip install rete-graph
```

A runnable tour with captured outputs lives in
[`examples/tutorial.ipynb`](examples/tutorial.ipynb).

## Use

```python
import rete_graph as rete

# Remote: lazy HTTP range reads — a selective query over a multi-GB file
# fetches KBs. Any S3/R2/CDN/GitHub URL with Range + CORS works.
g = rete.open("https://data.graphplaza.com/boe/boe.rete")

rows = g.query("""
    SELECT ?s ?label WHERE {
        ?s <http://www.w3.org/2000/01/rdf-schema#label> ?label
    } LIMIT 10
""")
for row in rows:
    print(row["s"].value, "→", row["label"].to_python())

print(g.stats())   # {'fileLength': ..., 'bytes': ..., 'requests': ...}
```

`query()` returns `{variable: Term}` rows for SELECT, a `bool` for ASK, and
`(s, p, o)` triples for CONSTRUCT/DESCRIBE. A `Term` carries `.kind` / `.value`
/ `.datatype` / `.lang`, plus `.to_python()` (int/float/bool for common XSD
types) and `.n3`.

More:

```python
g = rete.open("data/example.rete")            # local file, opened lazily too
g = rete.open(rete.build(nt_text))            # build a graph in memory ("nt"/"nq"/"ttl")
g = rete.open(rete.build(rdflib_graph))       # or straight from an rdflib Graph/Dataset
g = rete.open(url, headers={"Authorization": "Bearer ..."})   # authed hosts

g.examples()                                  # SPARQL examples embedded in the file — run as-is
g.query(q, reason=True)                       # OWL 2 QL entailment (query rewriting)
g.query_df("SELECT ...")                      # pandas DataFrame (pip install rete-graph[pandas])
g.schema()                                    # class/predicate profile
g.prefix_search("Berl")                       # label autocomplete
g.text_search("volcano eruption")             # full-text (files built with --text-index)
g.info(), g.quads, g.content_hash()
```

SPARQL 1.1 `SERVICE` federation works out of the box — join a `.rete` file
against any public SPARQL endpoint in one query.

### Prepare a `.rete` step by step

The lazy `Builder` configures a build — sources, embedded Dataset Card,
pyramid, full-text index — then `run()` / `export()`:

```python
builder = (
    rete.Builder()
    .add_file("people.ttl")                    # or .add(text) / .add(rdflib_graph)
    .card(title="People", license="CC0-1.0", created="2026-07-16")
    .pyramid(algo="louvain")                   # or "types", or .pyramid(False)
    .text_index()
)
builder.run()                                  # bytes; counts in builder.stats
builder.export("people.rete")                  # immutable, host-anywhere file
builder.graph().card()                         # read the embedded card back
```

See the "Python: build a .rete" tutorial in the docs for the full walkthrough.

### Custom storage (fsspec, S3, GCS, …)

Anything that can serve byte ranges can back a graph — pass an object with
`read_at(offset, length) -> bytes` and `len()`:

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

g = rete.open(reader=FsspecReader("s3://my-bucket/data.rete", anon=False))
```

## Building from source

The package is a [maturin](https://maturin.rs) project; the repo convention is
to build in Docker (nothing on the host):

```sh
# from the repository root
docker run --rm -v "$PWD":/io ghcr.io/pyo3/maturin build \
    --release -m clients/python/Cargo.toml --out clients/python/dist

uv pip install clients/python/dist/*.whl
```

Tests: `uv pip install pytest && pytest clients/python/tests`.

Wheels are abi3 (`>= 3.9`): one wheel per platform covers every CPython.
Releases are published to PyPI by
`.github/workflows/python-client-publish.yml` on a `py-v*` tag.
