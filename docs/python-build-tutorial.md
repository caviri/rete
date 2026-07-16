# Python: build a `.rete`, step by step

This tutorial walks through preparing a `.rete` file from Python with the
`rete_graph.Builder` — a **lazy** builder: every step just records
configuration and returns the builder, so calls chain freely; nothing parses
or builds until you say `run()`. You'll configure the sources, the embedded
Dataset Card, the community pyramid, and the full-text index, then run the
build, verify the result, and export the file.

```sh
uv pip install rete-graph        # rdflib optional: uv pip install rete-graph[rdflib]
```

The whole flow, before we take it apart:

```python
import rete_graph as rete

builder = (
    rete.Builder()
    .add_file("people.ttl")                      # 1. sources
    .card(title="People", license="CC0-1.0")     # 2. dataset card
    .pyramid(algo="louvain")                     # 3. pyramid
    .text_index()                                # 4. full-text index
)
data = builder.run()                             # 5. build
builder.export("people.rete")                    # 6. ship
```

## 1. Start a builder and add sources

A `Builder` starts empty. `add()` queues RDF **text** (`"nt"`, `"nq"`,
`"ttl"`, `"rdfxml"`), `add_file()` queues a file (format inferred from the
suffix), and either accepts a **graph object from another RDF library** —
anything with a `.serialize()` method, e.g. an rdflib `Graph` or `Dataset`:

```python
import rdflib
import rete_graph as rete

people = rdflib.Graph()
people.parse("people.ttl")

builder = (
    rete.Builder()
    .add(people)                                       # rdflib object
    .add_file("places.nt")                             # file, format inferred
    .add("<urn:a> <urn:knows> <urn:b> .")              # inline N-Triples
)
```

All sources merge into one graph. N-Quads sources keep their named graphs
(they become a dataset; rdflib `Dataset` objects round-trip the same way).
Nothing has been parsed yet — the builder only holds text and settings.

## 2. The Dataset Card

A `.rete` file can embed a [Dataset Card](dataset-cards.md) — a data-catalog
record living in the file's metadata section, so the data travels with its own
documentation. `card()` sets the **curated** fields; repeat calls merge:

```python
builder.card(
    title="People & places",                     # short display name
    description="Who knows whom, and where they lived (demo).",
    license="CC0-1.0",                           # SPDX id or a URL
    source="https://example.org/source-dump",    # where the data came from
    created="2026-07-16",                        # ISO date of this snapshot
    example_queries=[                            # starter SPARQL, shown by tools
        "SELECT ?s ?o WHERE { ?s <urn:knows> ?o } LIMIT 10",
    ],
)
```

Field by field:

| Field | Meaning |
|---|---|
| `title` | Human name of the dataset |
| `description` | A paragraph of context — what, where from, caveats |
| `license` | The data license (SPDX identifier or URL) |
| `source` | Provenance: the upstream dump/API this was built from |
| `created` | Snapshot date (ISO 8601 string) |
| `example_queries` | Runnable SPARQL strings for a newcomer's first click |

The **statistics** — `triple_count`, `quad_count`, `named_graph_count`,
`term_count`, plus the `format_version` — are stamped **automatically** at
build time; you never write them. The card is readable from every client:
`Graph.card()` here (on a *remote* file it fetches only the metadata
section's byte range), `rete card` / `rete info` in the CLI, and the
playground's catalog view.

One honest limit: the CLI's `rete build` additionally derives an **enriched
profile** (top predicates and classes, vocabularies, hubs, datatype/language
histograms, a tiered starter-query library, an optional coherence verdict).
The Python builder embeds the curated fields + counts only — if you want the
full auto-profile, rebuild the exported data with the CLI.

## 3. The pyramid

The [community pyramid](semantic-zoom.md) is the file's "zoom-out" structure:
communities summarized level by level, powering the playground's overview,
progressive rendering, and label prefix search.

```python
builder.pyramid(algo="louvain")     # default: topological communities
builder.pyramid(algo="types")       # one community per rdf:type class
builder.pyramid(False)              # skip it entirely
```

- **`louvain`** (default) clusters by graph structure — "what hangs together
  densely?". Deterministic, byte-identical across runs.
- **`types`** partitions by `rdf:type` class — communities are self-naming,
  and the summary becomes the class→class relation graph. It falls back to
  Louvain when the graph carries no usable typing. If your data types
  entities with something other than `rdf:type`, force it:

```python
builder.type_predicate("http://www.wikidata.org/prop/direct/P31")
```

- **`pyramid(False)`** writes no pyramid section at all. SPARQL, SHACL, and
  reachability are unaffected — the file stays fully queryable and gets
  markedly smaller — but summary/progressive views and `prefix_search` are
  gone. Good for pure query workloads.

## 4. The full-text index

Opt-in, because it costs file size: a word index over literals that powers
`Graph.text_search()` and fast `CONTAINS` filters (~39× a raw scan):

```python
builder.text_index()
```

## 5. Run

`run()` executes the whole configuration — parse every source, build the
dictionary, the triple indexes, the pyramid, the text index, embed the card —
and returns the complete file image as `bytes`:

```python
data = builder.run()
print(builder.stats)
# {'statements': 6, 'defaultTriples': 6, 'namedGraphs': 0,
#  'terms': 10, 'pyramidLevels': 1}
```

The result is cached: `run()` twice returns the same bytes without
rebuilding, and `export()`/`graph()` reuse it. Changing **any** setting
(another `add`, a card edit, a pyramid flag) invalidates the cache, and the
next `run()` rebuilds.

## 6. Verify before shipping

The image opens like any other graph — check it holds what you meant:

```python
g = builder.graph()                  # == rete.open(builder.run())

assert g.quads == builder.stats["statements"]
g.query("SELECT ?s ?p ?o WHERE { ?s ?p ?o } LIMIT 5")
g.schema()                           # classes + relations profile
g.card()                             # the embedded card, counts included
g.text_search("alice")               # the index built in step 4
g.content_hash()                     # blake3-16 — cite it when publishing
```

## 7. Export

```python
path = builder.export("people.rete")
```

The exported file is **complete and immutable** — build-once, read-many.
Reopen it from disk (lazily, like a remote file) or verify it with the CLI:

```python
g = rete.open("people.rete")
g.card()["title"]                    # ranged metadata read — a few KB
```

```sh
rete card people.rete                # the same card, rendered as a catalog
rete info people.rete
```

Host it on anything that serves HTTP `Range` (S3, R2, GitHub, any CDN — see
[Hosting your .rete](hosting.md)) and it is queryable in place from Python,
the CLI, and the browser playground, with no server.

## Scaling up, and the CLI equivalents

In-memory builds comfortably handle test and small-to-medium graphs (up to a
few million triples). Past that, switch to the streaming CLI — every builder
step has a flag twin:

| Python builder | `rete build` flag |
|---|---|
| `.add_file("x.nt")` | positional input files |
| `.card(title=..., ...)` | `--card-file card.json` / `--title` … |
| `.pyramid(algo="types")` | `--pyramid-algo types` |
| `.pyramid(False)` | `--no-pyramid` |
| `.text_index()` | `--text-index` |
| `.type_predicate(iri)` | `--type-predicate <iri>` |
| `.export("out.rete")` | `-o out.rete` |

The CLI also compresses sections with zstd at higher levels, streams input
twice instead of holding it, and derives the enriched card profile. See the
[CLI reference](cli.md) and [Tables, VKG & big builds](data-engineering.md).

## The complete script

```python
"""people.ttl (+ an rdflib graph) -> people.rete, with card + pyramid + text index."""
import rdflib
import rete_graph as rete

enrichment = rdflib.Graph()
enrichment.parse("extra-labels.ttl")

builder = (
    rete.Builder()
    .add_file("people.ttl")
    .add(enrichment)
    .card(
        title="People & places",
        description="Who knows whom, and where they lived (demo).",
        license="CC0-1.0",
        source="https://example.org/source-dump",
        created="2026-07-16",
        example_queries=["SELECT ?s ?o WHERE { ?s <urn:knows> ?o } LIMIT 10"],
    )
    .pyramid(algo="louvain")
    .text_index()
)

builder.run()
print("built:", builder.stats)

g = builder.graph()
assert g.card()["title"] == "People & places"

print("exported:", builder.export("people.rete"))
print("content hash:", g.content_hash())
```
