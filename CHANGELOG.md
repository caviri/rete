# Changelog

All notable user-facing changes are recorded here. Rete follows semantic
versioning for its Rust, CLI, and WASM APIs from 1.0.0 onward.

## [Unreleased]

## [0.3.2] - 2026-08-01

No engine change from 0.3.1 — the same code, released again because the 0.3.1
release run could not finish. Two workflow bugs stopped it after the packages
were already on PyPI and npm, so those registries carry 0.3.1 while GitHub
Releases has nothing.

### Fixed

- **The Blender add-on packed and then could not be moved.** The extension built
  correctly — 13,956,498 bytes with all four wheels bundled — and the job died on
  the next line, because `clients/blender/dist` is created by a container running
  as root and the runner user cannot unlink from it. (#129)
- **The browser WASM bundle copied paths that had moved.** It still named
  `docs/rete_wasm.js` and `docs/rete_wasm_bg.wasm` at the top level; the ESM pair
  lives in `docs/engine/`. Those are the same two paths #100 corrected in
  `ci.yml`'s parity list — `release.yml` kept the old spelling, and since it only
  runs on a `v*` tag, nothing exercised it in between. (#130)

Both are tag-only code paths, which is what let them rot unnoticed. `release.yml`
now has every path it copies audited.

## [0.3.1] - 2026-07-31

A correctness release. The headline is a SPARQL bug that returned **wrong
answers rather than an error**, so upgrading is not optional for anyone running
sub-queries. It also carries the browser fix for graphs large enough to push the
wasm heap past 2 GiB, and two additions that were already on main.

### Fixed

- **A sub-SELECT's `LIMIT` / `OFFSET` / `DISTINCT` no longer leaks to the outer
  query.** The planner peeled solution modifiers while walking down to the
  projection and did not stop at the sub-query boundary, so
  `SELECT … WHERE { { SELECT … LIMIT 10 } … }` applied that `LIMIT 10` to the
  **outer** result set. The query still succeeded — it just answered a different
  question than the one asked, which is the worst failure mode a query engine
  has. The peel now stops at a `Slice` whenever it is inside a `WHERE` or the
  projection is already bound, and lowers it as a nested plan instead. (#120)
- **A wasm pointer above 2 GiB no longer bricks the async reader.** wasm32
  pointers cross into JS through `i32` imports, so anything allocated past 2 GiB
  arrives sign-extended — a negative number — and `mem.set(bytes, negative)`
  throws `RangeError: offset is out of bounds`. Because wasm memory never
  shrinks, every later read in that worker failed identically: one query ended
  the page session. A remote scan of wikidata-1GB reached a 2050 MB heap and
  produced `dstPtr = -2145787624`. Every pointer the glue dereferences now goes
  through a `>>> 0` normalizer, and a G0 gate check asserts that in both the
  generator and the generated file so regenerating cannot silently drop it.
  (#121)
- **The Claude Code plugin exposed no skills.** Its marketplace source resolved
  only for one of the ways a plugin can be added, so `skills/` was never
  discovered from the plugin root; the manifest is versioned now too. (#119)

### Added

- **`rete estimate`** — project a build's output size, wall time and temporary
  spill *before* committing to it. Cardinality comes from a HyperLogLog sketch
  (2^14 registers, 16 KiB) over a line-aligned sample, so the estimate costs a
  read of the head of the input rather than a full pass. Reported as bands, not
  false precision. (#114)
- **The Python client streams every quad out in bounded memory.**
  `dump_iter()` / `dump_each()` walk a graph without materializing it, so a
  multi-gigabyte `.rete` can be piped into another store — Oxigraph, a triple
  store load, an N-Quads file — from a small resident footprint, and with no
  `unsafe` in the binding. (#118)

### Fixed (build & tooling)

- **The CI wasm-parity gate guards real files again.** Its diff listed
  gitignored directories and two paths that never existed (silent no-ops, which
  is how five different engine builds came to coexist across the shipped
  pages); `docs/engine/` gained a producer (`build_wasm.sh`) and the parity
  list now names exactly the tracked artifacts it regenerates. Workspace and
  python-client fmt/clippy debt cleared; the python lint job's toolchain is
  pinned to the repo's. (#100)

### Changed

- **Every client pin sits on the 0.3.0 engine line and is enforced.**
  `sync_versions.py --check` now also guards the Blender wheel pin and
  test-image floor and the HF Space wheel floor (eight lockstep targets);
  the docs' jsDelivr snippets load `rete-graph@0.3.0`. (#99)
- **Every browser surface rebuilt on the 0.3.0 engine** — playground,
  explore-100mb, the Asyncify pair, explorer, yasgui, lombardi, atlas, the
  plaza engine pair and `docs/engine` — from CI's canonical wasm bytes; the
  deployed playground stamps the exact commit it was built from. (#100)

### Added

- **Shared links now preview.** The playground keeps its state in the URL
  fragment, which no unfurler or search crawler can see, so every deep link used
  to preview as the same anonymous card. Each catalog example now has a page of
  its own at `q/<dataset>-<n>.html` (each dataset at `d/<dataset>.html`) carrying
  Open Graph / Twitter tags and a pre-rendered 1200×630 card — the question, the
  dataset, and **the answer that query really returns** — which then forwards to
  the playground deep link it describes. 🔗 and **Share** hand out those URLs;
  ad-hoc queries still share the deep link. Browse them at `shared.html`.
  The card numbers are measured, not written: `scripts/preview/capture.mjs` runs
  all 637 examples over the 91 published graphs in a real browser and records
  each result, its timing and its range-read cost.
- **Every documentation and application page carries social tags.** `docgen`
  derives each page's description from its own opening paragraph and emits the
  tags plus a rendered card; `scripts/preview/inject_og.mjs` does the same for
  the pre-built apps (playground, yasgui, atlas, the 3D viewers …) and patches
  their `web/*.template.html`, so a rebuild keeps them. Dataset pages also carry
  schema.org `Dataset` JSON-LD. A new G0 gate check fails if any share page,
  card image or tag goes missing.

## [0.3.0] - 2026-07-22

The 0.3.0 engine line: `rete-core`, `rete-cli`, and `rete-wasm` staged for
crates.io (the registry bootstrap is still pending — nothing is on crates.io
yet; `rete-graph` 0.3.0 *is* published to PyPI and npm). It carries the same
code the 1.0 line will ship, but goes out as a 0.x on purpose: it proves the
packaging, the docs builds, and the release automation end to end before any
version has to honour a compatibility promise. The on-disk format is already
stable generation 1; the Rust, CLI, and WASM APIs carry no semver guarantee
until 1.0.0.

### Added

- **Client versions now track the engine.** `rete-graph` on PyPI and npm and the
  R package all carry the engine's `MAJOR.MINOR` (0.3.x), so "same minor" means
  "same engine"; each client keeps its patch component for binding-only fixes.
  `scripts/sync_versions.py` propagates the workspace version and gates drift in
  CI. Every client also exposes the engine build it embeds — `rete_graph
  .__engine_version__` in Python, backed by the new `rete_core::VERSION`.
- **Claude Desktop extension** (`clients/mcpb`): the engine packaged as an
  [MCP Bundle](https://github.com/modelcontextprotocol/mcpb) — nine tools over
  local and published `.rete` graphs, plus offline `build_rete`. Ships as a
  plain `node` bundle (one JS file + the wasm engine), so a single artifact
  covers macOS, Windows and Linux.
- **Lazy `file://` opens in the JavaScript client**: a local `.rete` is read by
  byte range like a remote one, so a multi-gigabyte file is queryable without
  loading it into memory.
- WASM `card` / `card_url` (the index-free Dataset Card tier, two small range
  reads at any file size) and `RemoteGraph::{card, schema, info, graph_names,
  shacl}` — the resident remote handle now covers the full read surface.
- `header_ranges` additionally reports the metadata section's byte range.
- JavaScript client `card()`, `examples()`, `shacl()`, and a `wasm` escape
  hatch to the raw engine exports (client 0.3.0).

- Stable format version 1 with compatibility fixtures and defensive ranged-file readers.
- Publishable `rete-core`, `rete-cli`, and `rete-wasm` crates with Rust 1.87 MSRV.
- Native CLI builds for Linux, macOS, and Windows on x86-64 and ARM64.
- Browser WASM APIs for eager bytes, synchronous range reads, and asynchronous range reads.
- RDF/XML ingestion, named-graph N-Quads, SPARQL, SHACL, reasoning, federation, GeoSPARQL, and Dataset Cards.
- Reproducible playground generation, R2 catalog validation, coverage floors, fuzz targets, and release-gate browser tests.

### Fixed

- **Aggregation streams.** `GROUP BY` / `COUNT` / `SUM` / … fold solutions
  through per-group accumulators instead of buffering every row, so resident
  memory is **O(groups), not O(rows)** — a bare `COUNT(*)` is a single counter,
  and a `GROUP BY` over the 1.38 B-row type slice of the 9.83 B-triple DataCite
  graph completes inside a 4 GiB container (measurements on the benchmark
  page). (#96)
- **`rete info` / `rete card` no longer read the whole file.** Both use the
  CARD tier — the same two small range reads `card-url` performs over HTTP —
  so a 52 GB graph answers in ~1 s under a 1 GiB cap. A single-graph
  `FROM <g>` now borrows that graph's index instead of copying every triple
  into a fresh one. (#97)
- **The PyPI publish job could ship the R2-only Pyodide-legacy wheel**, whose
  platform tag PyPI rejects — the artifact now lives outside the publish job's
  `wheel-*` download glob. (#98)

### Compatibility

- Pre-1.0 `.rete` files are not guaranteed to open. Rebuild source RDF with the matching CLI.
- Files produced by 0.x may still require rebuilding before final 1.0.0. The compatibility promise begins with that release.

### Known limitations

- Browser bindings are single-threaded by default; threaded WASM remains opt-in and requires cross-origin isolation.
- SPARQL results are evaluated eagerly after lazy range reads.
- File federation unions per-file results; it does not perform arbitrary cross-file joins.
- The upstream RDF/XML dependency still resolves `quick-xml 0.37.5`, because every published `oxrdfxml` requires `quick-xml ^0.37`. RUSTSEC-2026-0194 and RUSTSEC-2026-0195 (both availability-only DoS on untrusted RDF/XML input) are therefore carried as documented exceptions in `deny.toml` and the publish preflight, and will be dropped as soon as Oxigraph ships a `quick-xml >= 0.41` bump.

[0.3.0]: https://github.com/caviri/rete/releases/tag/v0.3.0

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
