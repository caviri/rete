# Architecture

`rete` is a publish-and-query RDF stack built around one immutable,
range-readable file. The design bias is simple: do expensive graph preparation at
build time, ship one cacheable artifact, and let clients answer useful questions
with bounded reads before they reach for a server.

## Workspace Map

| Path | Responsibility |
|---|---|
| `crates/rete-core` | File format, dictionary, triple indexes, pyramid summary, SPARQL evaluation, reasoning, SHACL, range readers |
| `crates/rete-cli` | The `rete` binary, command routing, JSON/text rendering, URL/range command helpers |
| `crates/rete-wasm` | Browser-facing API compiled with `wasm-bindgen`; no native-only dependencies on the default path |
| `crates/docgen` | Static docs renderer for `docs/*.md` to `docs/*.html`, including each page's social tags |
| `crates/bench` | Dev-only Oxigraph comparison harness and benchmark JSON output |
| `web/` | Playground template, generated WASM packages, embedded datasets, captured example answers (`web/preview/`) |
| `scripts/` | Smoke tests, synthetic data generation, range server, docs/playground build helpers |
| `scripts/preview/` | Social-preview pipeline: measures every catalog example, renders the card images, generates the share pages under `docs/q` and `docs/d` |

The CLI is split by command group under `crates/rete-cli/src/commands/`. Core
query behavior lives in `rete-core`; the CLI and WASM layers should mostly adapt
inputs and outputs rather than reimplement engine logic.

## Build Pipeline

`rete build` turns RDF text into a `.rete` snapshot:

1. Parse RDF input into default-graph triples and optional named-graph quads.
2. Intern terms in the dictionary so query-time matching works on compact integer
   IDs instead of repeated strings.
3. Build the permutation indexes over the default graph — by default six: SPO,
   POS, OSP, SOP, PSO, and OPS. The first three suffice to match any pattern
   shape at the same longest bound prefix, and are always written; the other
   three let any join key be streamed pre-sorted, enabling sort-merge joins.
   `rete build --permutations 3` drops them, and the header's permutation mask
   records which set the file carries ([SPEC §4.1](SPEC.md#41-header-1024-bytes-little-endian)).
4. Compute the pyramid summary: community hierarchy, super-edges, predicate
   totals, class/type summaries, and named-graph metadata.
5. Optionally attach dataset-card metadata into the reserved metadata section.
6. Write the header, dictionary, indexes, summary, metadata, and content hash.

The file is immutable. Updates mean rebuilding the artifact. That keeps readers
simple and lets a `.rete` file sit on GitHub Pages, S3, GCS, or any HTTP server
that honors `Range`.

**Build memory.** On a big graph the build's peak RAM is dominated by the raw
string statements — every term is an owned `String`, heavily duplicated across
triples — which are fully redundant with the dictionary once interned. Two
choices keep the peak low: an N-Triples/N-Quads **file is stream-parsed** line by
line (the whole text is never resident), and the parsed statements are **dropped
right after they are encoded to integer id-triples**, before the memory-heavy
pyramid and index phases. So the dictionary + id-triples (compact) coexist with
the pyramid working set, but the bulky string statements do not. Measured on a
3 M-triple / 189 MB N-Triples build, peak RSS fell **~1371 MiB → ~836 MiB (39%)**,
output byte-identical. The phase-by-phase heap profile is reproducible with
`cargo run --release -p rete-bench -- --build-mem <file.nt>`.

**Build time.** On a big graph the build is dominated by the **Louvain community
pyramid** (`build_dendrogram`) — on the 3 M-triple build it was ~91% of the
wall-clock. The local-moving inner loop runs a reusable dense scratch array
(community → edge-weight, with a touched-list reset) instead of allocating a
`HashMap` per node per sweep; accumulation order and the sorted candidate scan are
unchanged, so the partition — and the file's content hash — are byte-for-byte
identical. That cut `build_dendrogram` **~118 s → ~44 s (2.7×)** and the whole
build ~141 s → 67 s. Set `RETE_BUILD_TIMING=1` on `rete build` for the per-phase
breakdown. (A *parallel* Louvain is not byte-identical — the partition depends on
the sequential move order — so the pyramid stays single-threaded; the index
permutations and the dictionary/permutation sorts already use all cores via
`rayon`.)

## File Layout

The header is a fixed 1 KB block: a small core (magic, version, content hash,
counts, codecs) plus a **typed section directory** of `(kind, offset, length)`
entries that point at every other section. New top-level sections are added as new
directory entries, so the format has headroom without a layout break (stable
generation 1, header byte `0x05`). Every stable reader from 0.3.0 onward accepts
this generation; the experimental generations `0x01`–`0x04` predate it and must be
rebuilt from RDF source. Readers validate the header, then decide how
much of the file to load:

| Section | Purpose |
|---|---|
| Header | Magic/version, content hash, counts, and the section directory |
| Dictionary | Front-coded term strings and role-aware ID spaces |
| Indexes | Compressed triple blocks in the file's permutation orders — six by default (SPO/POS/OSP/SOP/PSO/OPS), three (SPO/POS/OSP) with `build --permutations 3` |
| Summary | Pyramid/community graph, predicate totals, classes, named-graph counts, and append-only profiling blocks (query_stats, entity shapes, label index) |
| Text index | Optional `--text-index` section: word → subjects, for full-text (`--contains`) search |
| Metadata | Optional dataset-card JSON/catalog data |

`Rete::open` reads the full file. `Rete::open_ranged` fetches the sections needed
for exact index queries. `SummaryView::open_ranged` reads only header,
dictionary, and summary, deliberately skipping the index.

The pyramid-meta grows by **length-prefixed, version-tagged blocks** appended
before the (trailing) schema block: `query_stats` (per-predicate cardinality),
characteristic sets (entity shapes), and the **label index** (a bounded,
label-sorted `(label, subject)` table powering `rete search` / `prefix_search` —
case-insensitive prefix lookup by binary search, no literal scan). Each block is
optional and skipped by a reader that predates it, so the format stays
backward-compatible and a typeless, label-free graph encodes byte-identically to
v1.

For **full-text** search — finding entities by a word *anywhere* in their
literals, not just a label prefix — an opt-in **TEXT_INDEX** section (`rete build
--text-index`, kind `6`) maps each literal word to its sorted subject ids. Like
the dictionary, it is range-readable: a small front-coded token table is read
once, then `rete search --contains <word>` (or `prefix_search`'s sibling
`text_search`) faults only the posting list of each queried word — a remote
`--contains` over a multi-GB file fetches kilobytes. It is off by default (a build
without the flag is byte-identical to one without the feature) and lives outside
the pyramid-meta so SPARQL never pays for it. See §6.3 of the SPEC.

## Query Pipeline

SPARQL goes through four stages:

1. `spargebra` parses the query.
2. `rete-core` lowers the supported algebra subset into an internal plan and
   compiles its variables into a per-query slot map.
3. The algebra evaluates as a lazy pull pipeline over integer slot rows:
   joins, `MINUS`, `DISTINCT`, filters, and `GRAPH` stream, so `LIMIT` and
   `ASK` stop the underlying index scans early. Aggregation, `ORDER BY`
   (a bounded top-k when `LIMIT` is present), and hash-join/`MINUS` build
   sides are the only blocking points.
4. Only the surviving rows' projected values resolve dictionary IDs back into
   RDF terms for text, JSON, or WASM output (late materialization, memoized
   per query).

Joins are adaptive: under a small known demand (`LIMIT`/`ASK`), multi-pattern
BGPs and BGP-shaped join sides switch from one-scan-per-pattern hash joins to
index-nested-loop probing — each row jumps to its group through a lazily-built
in-memory block directory, so producing k solutions costs O(k) point lookups
instead of full pattern scans.

Join **order** is cost-based: each pattern's cardinality is estimated from the
pyramid summary's exact per-predicate totals (plus default selectivities for a
bound subject/object), and the cheapest connected pattern joins first — so on
skewed data, leading with a rare predicate avoids a large intermediate. A bound
subject/object's selectivity comes from the `query_stats` block's **measured**
per-predicate `distinct_subjects` / `distinct_objects` (so `<s> <p> ?o` estimates
the average objects-per-subject — exactly one for a functional predicate); files
built before that block, or with no summary at all, fall back to fixed default
selectivities / a most-constants-first heuristic. The estimate is free for an
in-memory file (the summary is resident) and is skipped on the lazy remote path
so it never forces a pyramid fetch. The hash join builds whichever side is
smaller (the join is symmetric on the key), bounding the hash table — and after
this ordering the accumulating left side is usually the small one.

Unsupported SPARQL constructs are rejected with clear errors. Nested `SELECT`
subqueries are supported (lowered to an independent inner plan, then joined on
shared variables), and so is `SERVICE` federation — the block ships to the
remote endpoint through a host-injected `ServiceClient` and its solutions join
like any other operand (see [sparql](sparql.html)).

## Result Provenance

`Rete::query_with_provenance` is the core API behind `rete why`. It runs the same
triple-pattern path as `Rete::query`, then attaches:

- the matched terms and dictionary IDs,
- the graph scope (`default` today for the local triple-pattern command),
- the resolved ID-space pattern,
- the selected permutation and its section index — routing always picks one of
  `SPO`, `POS`, `OSP`, which tie the longest bound prefix on all eight pattern
  shapes; `SOP`, `PSO`, `OPS` are never *routed* to, they only feed a sort-merge
  join a pre-sorted stream,
- the header byte ranges for dictionary, index container, selected permutation
  payload, and pyramid metadata.

This is intentionally physical-file provenance, not a narrative explanation. Since
format v0.2 each permutation section is tiled (independently compressed
~64 KiB tiles with a byte-range directory, SPEC §6.2): routed reads fetch the
directory plus only the matching tile(s), and provenance names the physical
tile (`PERM/index`) with its compressed byte range. For pre-tiling (v0.1)
files `rete why --json` still reports tile provenance as `not_materialized`.

**Tile synopses.** A routed lookup with a bound leading id lands on exactly one
tile, but the *other* bound components are only checked after the tile is fetched
(its in-tile zone map). The optional tile-synopsis trailer (header flag
`FLAG_TILE_SYNOPSIS`) lifts each tile's two non-leading-column min/max into the
section directory, so a range reader prunes that tile by a bound secondary
component **before** fetching it — a negative or sparse remote lookup (e.g. "does
this subject have that predicate?", or a join probe that misses) is answered with
**zero tile fetches**. The prune is conservative — it only skips a tile the
in-tile zone map would also reject — so it can never drop a result; the trailer is
appended past the tiles, so older readers ignore it (backward-compatible). It
costs one extra small tail read per section at open, amortized across a session.

## Progressive And Cost Paths

The summary path is the first layer of progressive querying. These query shapes
are exact from the summary and skip the full index:

- `SELECT (COUNT(*) AS ?n) WHERE { ?s <p> ?o }`
- `SELECT (COUNT(*) AS ?n) WHERE { ?s ?p ?o }`
- `SELECT ?p (COUNT(*) AS ?n) WHERE { ?s ?p ?o } GROUP BY ?p`
- `SELECT DISTINCT ?p WHERE { ?s ?p ?o }`
- `SELECT (COUNT(DISTINCT ?p) AS ?n) WHERE { ?s ?p ?o }`
- `ASK { ?s ?p ?o }`
- `ASK { ?s <p> ?o }`

`rete cost --json --explain` reports whether a query is summary-only, exactly
routable as one triple pattern, or still needs the full index. `rete
progressive` exposes the summary-safe path directly and returns metadata such as
byte counts, request counts, and whether the index was read.

The first exact routed refinement is implemented for single default-graph triple
patterns: the reader resolves constants from the dictionary, chooses the
best-routing permutation (always one of SPO/POS/OSP), follows the container
length prefixes, and fetches only that permutation payload. The next architectural step is physical community-tile
directories: use the pyramid to fetch only relevant community ranges instead of
even a whole permutation section.

## Range Reads

Range readers use the same file layout as local readers. A ranged open typically
does:

1. Fetch the header.
2. Fetch dictionary metadata and the term sections needed to resolve constants.
3. Fetch summary, one selected permutation payload, or the full index depending
   on the query shape.
4. Count bytes and requests with `CountingReader` for cost/progressive reporting.

The product boundary here is strict: a host that ignores `Range` must fail
clearly instead of silently downloading the world and pretending the query was
bounded.

## Reasoning And Validation

`build --materialize` can run the RDFS/OWL-RL subset at build time and write the
inferred triples into the snapshot. SHACL validation reads the snapshot and shape
graph, then validates the asserted or already-materialized data through
`rete-core`'s reusable SHACL API.

Reasoning and SHACL are intentionally snapshot-oriented. A `.rete` file records
the graph state you publish; validation runs against that exact state.

## Browser And WASM

The browser API is compiled from `crates/rete-wasm` and embedded in the static
playground. The default WASM build is single-threaded and avoids native-only
dependencies. The native `parallel` feature uses Rayon for CPU-side workloads
such as batch reachability, but it is not part of the default browser path.

**Result serialization.** A query's result crosses the worker boundary as a JSON
string, so its serialization is on the WASM hot path. The envelope is written
**directly into a `String`** (`rete_core::results_envelope_json`) rather than
built as a `serde_json::Value` tree and then stringified. On a 500k-row `SELECT`
(a 27 MB result) the tree path allocates ~670 MiB — **25× the payload** (a
`BTreeMap` per row plus a key clone and a term clone per cell, then a second pass
to stringify) — and costs more than the query itself; the direct writer is **~13×
less peak heap and ~10× faster** (50 MiB / 41 ms vs 670 MiB / 408 ms), which on
the bounded WASM heap is the difference between answering and OOM-ing on a large
result. Reproduce with `cargo run --release -p rete-bench -- --query-mem
<file.rete> "<sparql>"`, which contrasts the two serializers and checks they
agree as JSON.

The playground is a static console generated by `scripts/build_playground.py`.
The shell lives in `web/playground.template.html`; editable source fragments live
under `web/playground-src/` (`styles.css`, `catalog.js`, and `app.js`). The
generator inlines those fragments, the no-modules WASM glue, the WASM bytes, and
the bundled `.rete` datasets into `docs/playground.html`. Run it with
`uv run python scripts/build_playground.py`. Do not hand-edit
`docs/playground.html`.

## Safety Invariants

- Binary readers handle corrupt or truncated bytes with clean errors, not panics.
- Bounds checks are product behavior because `.rete` files may be fetched from
  arbitrary URLs.
- WASM default builds must not pull in native-only dependencies.
- Generated docs HTML must match the Markdown and templates.
- Benchmarks must assert result parity before trusting timing numbers.
- File-format changes require updates to `docs/SPEC.md`, layout tests, and
  compatibility notes.

## Extension Points

| Area | Next work |
|---|---|
| Tile-routed query refinement | Add physical community-tile directories so exact routing can fetch relevant community ranges, not whole permutation payloads |
| Result provenance | Extend tile-range provenance (done for permutation tiles) to pyramid/community tiles |
| Query engine rows | Replace wide `BTreeMap<String, String>` bindings with integer slot rows |
| Benchmark docs | Refresh JSON snapshots with `rete-bench --json` and regenerate the benchmark section |
| SHACL | Add SHACL-SPARQL constraints only if the CLI needs extension-level coverage |
