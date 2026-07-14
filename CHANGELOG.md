# Changelog

All notable user-facing changes are recorded here. Rete follows semantic
versioning for its Rust, CLI, and WASM APIs from 1.0.0 onward.

## [1.0.0-rc.1] - 2026-07-14

First release candidate for the stable Rete format and toolchain.

### Added

- Stable format version 1 with compatibility fixtures and defensive ranged-file readers.
- Publishable `rete-core`, `rete-cli`, and `rete-wasm` crates with Rust 1.87 MSRV.
- Native CLI builds for Linux, macOS, and Windows on x86-64 and ARM64.
- Browser WASM APIs for eager bytes, synchronous range reads, and asynchronous range reads.
- RDF/XML ingestion, named-graph N-Quads, SPARQL, SHACL, reasoning, federation, GeoSPARQL, and Dataset Cards.
- Reproducible playground generation, R2 catalog validation, coverage floors, fuzz targets, and release-gate browser tests.

### Compatibility

- Pre-1.0 `.rete` files are not guaranteed to open. Rebuild source RDF with the matching 1.0 CLI.
- Files produced by an RC may still require rebuilding before final 1.0.0. The compatibility promise begins with the final release.

### Known limitations

- Browser bindings are single-threaded by default; threaded WASM remains opt-in and requires cross-origin isolation.
- SPARQL results are evaluated eagerly after lazy range reads.
- File federation unions per-file results; it does not perform arbitrary cross-file joins.
- The current upstream RDF/XML dependency still resolves `quick-xml 0.37.5`; registry publication is blocked on resolving its two active RustSec advisories.

[1.0.0-rc.1]: https://github.com/caviri/rete/releases/tag/v1.0.0-rc.1

## Pre-1.0 development history

The crate version and the experimental on-disk format version evolved
independently before 1.0. Each format step was a clean break, so those files
must be rebuilt with the 1.0 toolchain.

### Format & storage

- **Format `v0.4`: six permutation indexes** (SPO/POS/OSP + SOP/PSO/OPS, #57) —
  every triple-pattern shape gets a prefix-routed, co-sorted permutation, the
  precondition for sort-merge joins. Roughly doubles the index payload vs the
  three-permutation `v0.3`; a clean break (rebuild older files).
- Format `v0.3`: the 128-byte header became a **1 KB typed section directory**
  (up to 40 sections; new sections are just new directory entries).
- Opt-in **full-text index** (`TEXT_INDEX` section, kind 6, #55): word → sorted
  subject ids, range-readable per word; `rete build --text-index` +
  `rete search --contains <word…>` (~39× a `FILTER(CONTAINS)` literal scan).
- `rete repyramid` — rebuild a file's pyramid / schema pyramid / card / text
  index in place, straight from the existing `.rete` (no export/build round-trip).

### Query & serve

- **SPARQL 1.1 `SERVICE` federation**: a `SERVICE <endpoint> { … }` block is
  shipped to the remote endpoint and joined on shared variables (SILENT
  honored; transport injected by the host — ureq in the CLI, sync XHR in wasm).
- **`rete serve`** — a live SPARQL 1.1 Protocol endpoint (query **and Update**)
  over one `.rete`: the base file is never mutated, updates append to an
  N-Quads journal, `GET /snapshot.rete` publishes the merged state.
- Nested `SELECT` subqueries; correlated property-path evaluation from a bound
  endpoint; SPARQL 1.1 conformance at 232/309 (75.1%) of the W3C
  query-evaluation suite.
- **GeoSPARQL** filter functions (contains/within/intersects/disjoint/equals +
  distance/envelope) over `geo:wktLiteral` geometry.
- `rete shacl-url` — lazy remote SHACL: validation routed as range reads, only
  each shape's targets fetched (#58).
- Engine rework: lazy pull pipeline over integer slot rows, adaptive
  index-nested-loop joins, top-k ORDER BY — wins or ties Oxigraph on 20/24
  benchmark operators.

### Ecosystem

- Datasets are served **directly from Cloudflare R2 / any range+CORS host**
  (Zenodo DOIs included — the length probe tries `HEAD` first); the docs grew
  a [hosting guide](docs/hosting.md).
- The playground grew to **40+ real datasets** with cross-source joins,
  sharded-dataset fan-out, SQL companions (DuckDB/SQLite/Parquet), semantic
  (RAG) search, a local SPARQL-drafting AI, media-aware result cells, and a
  live-endpoint editing mode over `rete serve` — see the
  [playground guide](docs/playground-guide.md).

## 0.1.0

First tagged minor release. Highlights of the capabilities and the most recent
work; PR numbers reference [github.com/caviri/rete](https://github.com/caviri/rete).

### Format & storage

- Single-file, immutable `.rete` image — dictionary, SPO/POS/OSP permutation
  indexes, and a pyramidal community summary — queryable in place over HTTP
  `Range` requests, no server.
- Tiled permutation sections (format `v0.2`): independently-compressed ~64 KiB
  tiles with a byte-range directory and per-tile zone maps, plus **tile
  synopses** — per-tile min/max of the non-leading columns in a backward-
  compatible trailer (header flag `FLAG_TILE_SYNOPSIS`) so a range reader prunes a
  routed tile by a bound secondary component before fetching it (#51).
- Chunked, front-coded dictionary sections for ranged `term ↔ id` resolution.
- Append-only pyramid-meta blocks: the schema pyramid (semantic zoom), planner
  `query_stats`, characteristic sets / entity shapes, and a bounded label index.

### Query

- SPARQL SELECT / ASK / CONSTRUCT / DESCRIBE with BGPs, OPTIONAL, UNION, MINUS,
  FILTER, BIND, property paths, aggregation, and solution modifiers.
- Cost-based BGP join ordering from the pyramid summary and measured
  per-predicate selectivity (`query_stats`); hash + index-nested-loop joins.
- `rete search` — case-insensitive label **prefix search** from a bounded label
  index, ~22× a `FILTER(STRSTARTS(LCASE(…)))` literal scan (#48).
- Progressive / summary-only answers and a range-read cost preview.

### Reasoning, validation, federation

- Prototype OWL RL / RDFS reasoner (coherence checking + optional materialization).
- SHACL Core validation.
- Federated SPARQL across several `.rete` sources (union + dedup, predicate routing).

### Performance

- Build peak RAM cut ~39% on a 3 M-triple build (stream-parse + drop the raw
  string statements before the pyramid) (#49).
- Louvain community-pyramid build ~2.7× faster (dense-scratch local moving,
  byte-identical output) (#50).
- WASM query-result serialization ~13× less peak heap and ~10× faster
  (direct-to-string envelope instead of a `serde_json::Value` tree) (#52).
- Parallel index/dictionary builds and batch reachability (rayon).

### Tooling

- `rete` CLI (build, inspect, query, reason, shacl, federate, search, …).
- WASM browser client + the static playground.
- Playground **Find a term** picker: browse a graph's classes/predicates (from
  the resident schema card) and search entities by label (lazy over HTTP range on
  remote graphs), with a **values ›** faceted drill that lists a predicate's
  distinct objects — IRIs resolved to labels, cached after the first read.
- Playground **Settings** now shows a **per-file breakdown** of the opt-in
  persistent (IndexedDB) range cache — each cached `.rete` with the share of the
  file held and a fill bar, plus per-file Clear; "Clear all" now also wipes ranges.
- Profilers: `rete-bench --build-mem` (build memory) and `--query-mem`
  (query/serialization memory).
