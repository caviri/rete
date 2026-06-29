# Getting started

## Everything runs in Docker

`rete` is developed and built **entirely inside a container** — nothing runs on
the host. The dev container ([`.devcontainer/`](../.devcontainer)) carries the
Rust 1.92 toolchain, rustfmt, clippy, Python, the `wasm32-unknown-unknown`
target, and `wasm-pack`.

Open the folder in a dev container (VS Code: *Reopen in Container*), or run the
same image directly:

```sh
docker build -t rete-dev -f .devcontainer/Dockerfile .
docker run --rm -it -v "${PWD}:/work" -w /work rete-dev bash
# then, inside:
cargo build --release -p rete-cli
```

The compiled CLI is at `target/release/rete`. The examples below assume it is on
your `PATH` (or substitute `cargo run -p rete-cli --`).

## Building a `.rete` file

<figure class="fig-right">
  <img src="img/build-pipeline.svg" alt="A pipeline: .nt, .ttl and .nq inputs feed into 'rete build', which produces one social.rete file containing a dictionary, indexes and a pyramid, ready to put on an HTTP host or URL.">
  <figcaption><code>rete build</code> packs your triples into one immutable file — dictionary, permutation indexes, and a community pyramid — that you can drop on any URL.</figcaption>
</figure>

`rete build` accepts N-Triples (`.nt`), N-Quads (`.nq`), Turtle (`.ttl`), and
RDF/XML (`.rdf` / `.owl` / `.rdfxml` — the usual OWL serialization), detected by
extension. Multiple inputs are merged under one shared dictionary, and `-` reads
standard input.

```sh
rete build data.nt -o data.rete                  # single file
rete build part1.nt part2.nt -o merged.rete      # merge several inputs
curl -s https://host/data.nt | rete build - -o data.rete   # from a pipe
rete build dump.unknown --format nt -o out.rete  # force a format
```

N-Quads inputs build a **dataset**: one shared dictionary, a default-graph index,
and one index per named graph.

### A full ontology: RDF/XML → an optimized `.rete`

Most OBO/W3C ontologies ship as **RDF/XML** (`*.owl`), which `rete build` does
**not** read — convert it to N-Triples first with `rapper` (from `raptor2-utils`;
it streams, so it handles gigabyte files):

```sh
# 1. RDF/XML (OWL) -> N-Triples
rapper -i rdfxml -o ntriples chebi.owl > chebi.nt        # 812 MB owl -> 8.83 M triples

# 2. assemble, with a self-describing Dataset Card (title/license/source/examples)
rete build chebi.nt -o chebi.rete --card \
  --title "ChEBI (full)" --license "CC BY 4.0" --source "https://www.ebi.ac.uk/chebi/"
#   -> 8.83 M triples, 3.15 M terms, 6 pyramid levels, 120 MB
```

The build is **parallel and allocation-frugal** by design (the CLI enables the
`parallel` feature): the dictionary dedups terms with a `HashSet` and sorts once,
and the six permutation indexes (SPO/POS/OSP/SOP/PSO/OPS) are built concurrently
with parallel sorts. This is what lets it scale to millions of *unique* terms
(definitions, synonyms, SMILES/InChI strings) without the build collapsing into
allocation churn — and the output is **byte-identical** to a serial build, so the
speedup is free. Turtle-native sources (e.g. a 239 MB `.ttl`) skip `rapper` and
feed `rete build --format ttl` directly.

The result is a single range-queryable file: drop it on any HTTP host and query
it lazily (below), or register it in the [playground](playground.html) as a
remote dataset. To go alongside it with a columnar/SQL view, generate
[lossless entity tables](#lossless-entity-tables-the-best-of-both-worlds)
straight from the same N-Triples.

## Querying locally

```sh
# Triple pattern — any of subject/predicate/object may be omitted (a wildcard):
rete query data.rete --predicate '<http://ex/knows>'
rete query data.rete --object   '<http://ex/Alice>'

# Basic Graph Pattern (multi-pattern join); ?x is a variable, ' . ' separates:
rete bgp data.rete "?x <http://ex/knows> ?y . ?y <http://ex/knows> ?z"

# Full SPARQL (SELECT / ASK / CONSTRUCT / DESCRIBE):
rete sparql data.rete "PREFIX e: <http://ex/> SELECT ?x ?z WHERE { ?x e:knows ?y . ?y e:knows ?z }"

# Standard SPARQL Results JSON, for piping into other tools:
rete sparql data.rete "SELECT ?s ?p ?o WHERE { ?s ?p ?o } LIMIT 10" --json
```

See the [SPARQL support](sparql.md) page for the full feature list.

## Validating shapes

Use `rete shacl` when you need semantic data-quality checks, not just syntax or
file integrity. Shapes are Turtle files; the command exits non-zero if the graph
does not conform.

```sh
rete shacl data.rete --shapes shapes.ttl
rete shacl data.rete --shapes shapes.ttl --format json
```

See [SHACL validation](shacl.md) for the supported SHACL Core surface.

## Inspecting a file

```sh
rete info   data.rete          # raw header
rete stats  data.rete          # size, counts, top predicates, planner stats, entity shapes
rete verify data.rete          # check the blake3 content hash (detect corruption)
rete graphs data.rete          # list named-graph IRIs
rete search data.rete gluc     # prefix-search labels (autocomplete; no literal scan)
rete export data.rete          # dump back to N-Quads (lossless)
```

Two **coarse-graph** views answer questions without reading the triple index:

```sh
rete summary data.rete   # structural: Louvain community quotient graph
rete schema  data.rete   # semantic: relations between rdf:type classes
rete predicates data.rete  # exact per-predicate totals, from the summary alone
```

`rete stats` also prints two index-free profiles read from the pyramid: the
**planner stats** (per predicate: distinct subjects/objects, multiplicities, and
functional / inverse-functional hints — the cardinality the cost-based join
planner uses) and the **entity shapes** (the most common *characteristic sets* —
which predicate-combinations subjects carry, e.g. `{type, name, age} ×N`).

### Label search (autocomplete)

`rete search data.rete <prefix>` resolves a case-insensitive label prefix
**without scanning the literals**. At build time rete extracts the display labels
(`rdfs:label`, `skos:prefLabel`/`altLabel`, `foaf:name`, `dc(terms):title`,
`schema:name`) of the most-connected subjects into a bounded, label-sorted block
in the pyramid-meta; a prefix query is then a binary search over that block:

```sh
rete search data.rete "alan"          # label<TAB><iri> rows, case-insensitive
rete search data.rete "alan" --limit 5 --json   # [{"label":…,"subject":…}]
rete search data.rete                 # empty prefix → the first --limit labels
```

This is the fast path for autocomplete: a binary search plus a short walk,
versus a `FILTER(STRSTARTS(LCASE(?l), …))` scan over every label triple
(measured **~22× faster** at 6k labeled subjects; the gap widens with size — the
scan is linear in the label count, the index is `O(log n + matches)`). The block
is **bounded** (top 8,192 labeled subjects by degree), so on a very large graph
it covers the prominent entities; an exhaustive match still needs the FILTER
scan. Files built before this feature have no label index — rebuild to add it
(the block is additive and backward-compatible, so old readers ignore it).

### Full-text search (word / CONTAINS)

Label prefix search finds an entity by the *start* of its label. To find entities
by a **word anywhere in any of their literals**, build with `--text-index` and
query with `rete search --contains`:

```sh
rete build data.nt -o data.rete --text-index   # opt-in TEXT_INDEX section

rete search data.rete --contains glucose            # subjects whose literals say "glucose"
rete search data.rete --contains glucose phosphate  # AND — both words (any literal)
rete search data.rete --contains-prefix einst       # word starting with "einst…"
rete search data.rete --contains glucose --json     # [{"subject":…}]
```

Matching is whole-word and case-insensitive (the same tokenizer builds and
queries: Unicode-alphanumeric runs, lowercased, length ≥ 2). The index maps each
word to its sorted subject ids as its own range-readable section (§6.3 of the
[SPEC](SPEC.md)): the token table is read once, then each queried word faults only
**its** posting list — so a `--contains` over a remote multi-GB file fetches
kilobytes, not the whole index. It is **opt-in** because it is sizable; a build
without `--text-index` is byte-identical to one built before the feature, and
`rete search --contains` on such a file reports that there is no text index.
`rete stats` shows the section's size when present.

## Deploying & querying over a URL

A `.rete` file is immutable and self-describing, so any static host that honors
HTTP `Range` requests works — S3, GCS, GitHub, a CDN, or the bundled dev server:

```sh
# Local range-capable server for testing:
python3 scripts/range_server.py 8000 .
rete query-url http://127.0.0.1:8000/data.rete --object '<http://ex/Dave>'

# Real https hosts (rustls; no http-only limitation):
rete query-url   https://my-bucket.s3.amazonaws.com/data.rete --predicate '<http://ex/knows>'
rete summary-url https://raw.githubusercontent.com/me/repo/main/data.rete
```

`query-url` resolves bound terms from the dictionary, then fetches only the
selected permutation payload (the best of the six) for the triple pattern. `summary-url`
reads just the header, dictionary, and summary — **the index is never
downloaded**. The host **must** return `206 Partial Content` to a `Range`
request; a host that ignores `Range` (returns `200`) is rejected with a clear
error rather than silently returning wrong bytes.

## Generating synthetic test data

`scripts/synth_graph.py` generates a realistic scholarly knowledge graph —
papers, authors, venues, institutions, grants, fields — with the statistics
real graphs have (power-law citations via preferential attachment, field
communities, Zipfian venue popularity, log-normal team sizes, typed literals,
per-year temporal structure). Two orthogonal knobs control it on demand:

```sh
# 10k papers (~315k triples), clean:
uv run python scripts/synth_graph.py --papers 10000 -o clean.nt

# Same size, 20% deliberate mess (cross-field rewires, temporal violations,
# missing attributes, mangled literals) — for robustness/quality testing:
uv run python scripts/synth_graph.py --papers 10000 --noise 0.2 --seed 7 -o messy.nt

# N-Quads with one named graph per publication year:
uv run python scripts/synth_graph.py --papers 5000 --quads -o by-year.nq

rete build clean.nt -o clean.rete
```

Identical arguments + seed reproduce the byte-identical graph; different seeds
give natural variability at the same size/noise point. A per-kind breakdown of
every noise event goes to stderr, so a test knows exactly what mess it got.
(`scripts/gen_graph.py` is the older, simpler social-graph generator used by
`scripts/bench.sh`.)

### Scaling to ~1 GB

The generator and `rete build` scale linearly, so a big stress-test graph is
just a bigger `--papers`. Output is roughly **85 bytes/triple** as N-Triples
and **31 triples/paper**, so ~1 GB is about 400k papers / 12.5M triples:

```sh
uv run python scripts/synth_graph.py --papers 400000 --seed 1 -o big.nt  # ~1.1 GB, ~22 s
rete build big.nt -o big.rete                                            # ~56 s -> ~100 MB
```

Measured on the dev container (12.5M triples, 2.0M terms, ~30k communities):
build is ~56 s and the `.rete` is ~100 MB (zstd). The point of the size is what
querying it then *doesn't* read: a selective pattern answers in under a second,
`rete predicates` reads ~20 MB of summary rather than the 80 MB index, and a
lazy query open (`rete cost big.rete "<query>"`) touches ~7 MB in ~50 range
requests instead of the whole file — the range-query promise, at 1 GB.

The playground's `scholar` / `scholar-noisy` demo datasets are built with this
generator (250 papers, seed 42, noise 0 and 0.25 — the exact commands are in
the `scripts/build_playground.py` docstring).

## Lossless entity tables (the best of both worlds)

`scripts/rdf_to_entity_tables.py` is the *lossless* counterpart: it keeps the
readable one-table-per-type shape **without dropping anything**. Each class
table has the frequent properties as named `LIST` columns (occupation,
citizenship, date of birth…) plus three things that make it complete: a
`types` column (all `P31` values, so a multi-typed entity lives in exactly one
table, never duplicated), an `extra` `MAP(predicate → objects)` column that
catches every other property (rare ones, all the multilingual labels), and an
`_untyped` residual table for subjects with no type. Object values are stored
as N-Triples term tokens (`<iri>`, `"lit"`, `"lit"@en`), so IRIs, literals and
language tags round-trip. Explode `types` + every column + `extra` across all
tables and you get back **exactly** the triples — `--verify` checks that
(reconstructed == input). It can emit Parquet, a DuckDB file, and a SQLite file
(list/map columns as JSON text) in one run:

```sh
uv run python scripts/rdf_to_entity_tables.py --parts 1 --limit 12000000 --props 24 \
  --min-entities 50 -o data/ent --duckdb data/ent.duckdb --sqlite data/ent.sqlite --verify
```

`--props` only changes how many properties get their own column vs. land in
`extra` — it never affects losslessness. The `_manifest.parquet` records each
class's column → predicate map so reconstruction is mechanical, and N-Triples
is the interchange hub (`rete export` ↔ `rete build` ↔ these tables).

It works on **any** RDF, not just the Wikidata Parquet source: pass `--nt
<file>` to read N-Triples directly (objects, language tags and datatypes
round-trip verbatim) and `--type-predicate <iri>` to group by something other
than Wikidata's `P31` — e.g. `rdf:type` for OBO ontologies. This is how the
`chebi-full` companions are built from the same `chebi.nt` as the `.rete`:

```sh
uv run python scripts/rdf_to_entity_tables.py --nt chebi.nt \
  --type-predicate "http://www.w3.org/1999/02/22-rdf-syntax-ns#type" \
  --props 24 --min-entities 50 -o data/chebi-tables \
  --duckdb data/chebi.duckdb --sqlite data/chebi.sqlite --verify
```

## Companion: columnar property tables (Parquet, split by type)

To compare the `.rete` graph against a columnar layout,
`scripts/rdf_to_property_tables.py` denormalizes the same Wikidata triples into
**one Parquet table per entity type** (the classic RDF "property table"): rows
are entities, grouped by `wdt:P31` (instance-of); columns are that class's most
common structured properties, multi-valued as `LIST(VARCHAR)`; an English
`label` column is added and the labelling/description predicates are excluded
so the columns are the real properties. It runs entirely in DuckDB from the
source Parquet:

```sh
pip install --break-system-packages duckdb
uv run python scripts/rdf_to_property_tables.py --parts 10 --limit 120000000 -o data/wd-tables
# -> data/wd-tables/Q5.parquet (human), Q16521.parquet (taxon), … + _manifest.parquet
```

Each class table is independently queryable (`SELECT … FROM 'Q5.parquet'`), the
`_manifest.parquet` maps class IRI → label/entity-count/file and each column id
back to its predicate, and a single DuckDB over the set is just views:
`CREATE VIEW human AS SELECT * FROM 'data/wd-tables/Q5.parquet'`. Match the
`.rete` slice by passing the same `--parts`/`--limit`. This is a property-table
companion for benchmarking storage/query tech against the graph format — not a
lossless graph encoding (sparse properties become NULLs, heterogeneous classes
get wide; that's the point of the comparison).

## Alternative approach: a *virtual* knowledge graph over the companions

The companions above are materialized the rete way: triples → a `.rete` (the
graph) **plus** tabular exports you query with SQL. The playground's Explore tab
already queries those exports **lazily over `httpfs`** — DuckDB-WASM / SQLite-WASM
fetch only the Parquet row-groups a query touches, the *same* range-read transport
the `.rete` uses — so you can compare the same class across the rete engine and
the columnar engines side by side.

A different school of thought skips materialization entirely. A **Virtual
Knowledge Graph** (VKG / OBDA) — e.g. [Ontop](https://ontop-vkg.org/) over DuckDB —
keeps the Parquet as the source of truth and answers **SPARQL by rewriting it to
SQL** at query time through declarative **R2RML mappings**; no RDF file is ever
built ([tech note](https://ontopic.ai/en/tech-notes/create-virtual-knowledge-graphs-from-parquet-files/)).
The two are complementary; the trade is materialized-vs-virtual:

| | rete (this project) | Virtual KG (Ontop + DuckDB) |
|---|---|---|
| RDF | **materialized** into a graph-native `.rete` | **virtual** — never materialized |
| Source of truth | the `.rete` file | the Parquet |
| SPARQL | answered directly over `.rete` (range reads) | rewritten to SQL over Parquet via mappings |
| Parquet's role | a tabular **companion/export** | **the** data |
| Trade-off | a build step; self-contained & graph-native (pyramid, communities, reachability, SHACL, coherence, provenance) | a mapping + a SPARQL→SQL engine at query time; always fresh, no ingestion |

Both lean on the **same lazy transport** — Parquet's footer + row groups are
range-friendly the way the `.rete` header + tiles are — so the honest comparison
isn't "lazy vs eager" but a **graph-native materialized file** vs a **virtual
SPARQL view over a columnar source**.

And rete's companions are already **VKG-ready**: the `_manifest.parquet` beside
each one records the column → predicate map (and class IRI → table) — *exactly the
R2RML mapping a VKG needs, generated for free*. So because the companions are
range-readable HF objects, a VKG can `ATTACH` them in place: a single Ontop/DuckDB
endpoint over *every* Parquet in the HF bucket — driven by the manifests — is a
"knowledge graph over all of HF" with no download and no `.rete`: the federated,
virtual mirror of rete's materialized file. (Materializing the same thing is also
mechanical: the entity tables are lossless, so reconstruct each → merge, or build
per-dataset `.rete` and `rete federate` across them.)

**Benchmark candidate (TODO):** rete's range-read SPARQL on a `.rete` vs an
Ontop-over-DuckDB VKG on the equivalent Parquet — same queries, same HTTP-range
hosting — measuring bytes fetched, latency, and which graph operations (pyramid /
reachability / SHACL / coherence) the VKG can't answer cheaply.

## A real-world graph: a Wikidata biology slice

For a genuinely large, real dataset, `scripts/fetch_wikidata_bio.py` pulls a
life-sciences slice from the [Wikidata Query Service](https://query.wikidata.org):
genes, the proteins they encode, the diseases they associate with, drugs that
treat those diseases, and a disease subclass hierarchy — one connected graph,
every entity labelled in English. It runs a handful of bounded `CONSTRUCT`
queries (each well under the WDQS timeout) and merges them as N-Triples.

```sh
uv run python scripts/fetch_wikidata_bio.py --limit 4000 -o data/wikidata-bio.nt
rete build data/wikidata-bio.nt -o bio.rete
rete stats bio.rete        # ~40k triples, ~27k terms, hundreds of communities
```

A `--limit 4000` run is roughly 40,000 triples (≈2,800 genes, ≈4,000 proteins,
≈3,600 diseases) — the community pyramid finds hundreds of organism/disease
clusters, and it exercises every surface: typed-class queries, label joins, the
disease hierarchy via `wdt:P279`, and HTTP range queries over a real graph.
`--taxon Q83310` fetches mouse instead of human; `--limit` trades size against
WDQS time. Output lands in `data/` (git-ignored, like all fetched datasets —
the script is tracked, the bytes are regenerated on demand). Be a good WDQS
citizen: it is rate-limited, so fetch a slice, not a firehose.

### Real Wikidata at gigabyte scale (Parquet)

The Query Service is for slices, not bulk. For a genuinely large, real
linked-data graph, `scripts/wikidata_parquet_to_nt.py` reads the full Wikidata
"truthy" dump from the
[`piebro/wikidata-extraction`](https://huggingface.co/datasets/piebro/wikidata-extraction)
Parquet conversion on Hugging Face (~80 partitions, `subject/predicate/object/
language` columns) with DuckDB — `httpfs` streams the remote files, so a bounded
slice needs no full download — and writes N-Triples.

```sh
pip install --break-system-packages duckdb
uv run python scripts/wikidata_parquet_to_nt.py --limit 12000000 -o data/wd.nt  # ~1 GB
rete build data/wd.nt -o wd.rete
```

The source Parquet drops literal datatypes, so the converter **recovers them**
(`--datatypes`, default `auto`): it resolves each property's datatype from a
local cache or one WDQS `wikibase:propertyType` query (~13.5k properties,
cached for reuse) and re-types each literal — dates `xsd:dateTime`, quantities
`xsd:decimal`, coordinates `geo:wktLiteral`; strings stay plain, monolingual
text keeps its language tag, entity values are IRIs. If that map is
unavailable (e.g. WDQS rate-limited), it falls back to an **offline heuristic**
that types the unambiguous values — ISO timestamps and WKT geometries — leaving
numbers plain (a bare number is indistinguishable from a numeric external-id
without the map). `--datatypes heuristic` forces the offline path; `none` emits
plain literals. Once typed, the engine's `DATATYPE(?o)` / `LANG(?o)` filters
can select by datatype.

Measured on the dev container, the full `--limit 12000000` (~1 GB) run:
converting streams in **~24 s** (1.25 GB N-Triples, datatypes recovered) and
builds in **~52 s** to a **110 MB `.rete`** — 5 pyramid levels, ~115k
communities — with typed literals intact (`"1830-10-04T00:00:00Z"^^xsd:dateTime`,
`"Point(5.47 49.50)"^^geo:wktLiteral`) and a selective lookup answering in
under a second. The slice is a real cross-section of *all* of Wikidata (people,
places, works, taxa, …); for a curated biology-only graph use the WDQS fetcher
above. `--parts N` draws from N whole partitions; `--local-dir` reads partitions
you have already downloaded.

## Testing

```sh
cargo test            # full suite (unit, round-trip, robustness, ranged, HTTP)
cargo clippy --workspace --all-targets -- -D warnings
bash scripts/smoke.sh # end-to-end acceptance test of every CLI subcommand
```

CI runs all of this — plus the feature matrix and the wasm build — in containers,
so nothing ever builds on the host.
