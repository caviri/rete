# Changelog

Notable changes to the **rete** crates. The crate version is tracked separately
from the on-disk **format** version (currently `v0.2`, readers still accept
`v0.1`); the format version lives in the header and is documented in
[`docs/SPEC.md`](docs/SPEC.md).

The project is experimental: the format is not yet stable across versions.

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
