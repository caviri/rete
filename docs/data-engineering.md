# Tables, VKG & big builds

Data-engineering companions to the `.rete` file: lossless tabular exports of
the same graph, the virtual-knowledge-graph alternative, and recipes for
pulling genuinely large real datasets. This page is development-facing — for
building a first file and querying it, start at
[Getting started](getting-started.md).

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
range-readable R2 objects, a VKG can `ATTACH` them in place: a single Ontop/DuckDB
endpoint over *every* Parquet in the R2 collection — driven by the manifests — is a
virtual knowledge graph over all published companions with no full download and
no `.rete`: the federated,
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

## Real Wikidata at gigabyte scale (Parquet)

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
