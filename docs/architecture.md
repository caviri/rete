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
| `crates/docgen` | Static docs renderer for `docs/*.md` to `docs/*.html` |
| `crates/bench` | Dev-only Oxigraph comparison harness and benchmark JSON output |
| `web/` | Playground template, generated WASM packages, embedded datasets |
| `scripts/` | Smoke tests, synthetic data generation, range server, docs/playground build helpers |

The CLI is split by command group under `crates/rete-cli/src/commands/`. Core
query behavior lives in `rete-core`; the CLI and WASM layers should mostly adapt
inputs and outputs rather than reimplement engine logic.

## Build Pipeline

`rete build` turns RDF text into a `.rete` snapshot:

1. Parse RDF input into default-graph triples and optional named-graph quads.
2. Intern terms in the dictionary so query-time matching works on compact integer
   IDs instead of repeated strings.
3. Build three permutation indexes over the default graph: SPO, POS, and OSP.
4. Compute the pyramid summary: community hierarchy, super-edges, predicate
   totals, class/type summaries, and named-graph metadata.
5. Optionally attach dataset-card metadata into the reserved metadata section.
6. Write the header, dictionary, indexes, summary, metadata, and content hash.

The file is immutable. Updates mean rebuilding the artifact. That keeps readers
simple and lets a `.rete` file sit on GitHub Pages, S3, GCS, or any HTTP server
that honors `Range`.

## File Layout

The header gives fixed offsets and lengths for the remaining sections. Readers
first validate the header, then decide how much of the file to load:

| Section | Purpose |
|---|---|
| Header | Magic/version, section offsets, content hash |
| Dictionary | Front-coded term strings and role-aware ID spaces |
| Indexes | Compressed triple blocks in SPO/POS/OSP order |
| Summary | Pyramid/community graph, predicate totals, classes, named-graph counts |
| Metadata | Optional dataset-card JSON/catalog data |

`Rete::open` reads the full file. `Rete::open_ranged` fetches the sections needed
for exact index queries. `SummaryView::open_ranged` reads only header,
dictionary, and summary, deliberately skipping the index.

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

Unsupported SPARQL constructs are rejected with clear errors. Known gaps include
nested `SELECT` subqueries and `SERVICE` federation.

## Result Provenance

`Rete::query_with_provenance` is the core API behind `rete why`. It runs the same
triple-pattern path as `Rete::query`, then attaches:

- the matched terms and dictionary IDs,
- the graph scope (`default` today for the local triple-pattern command),
- the resolved ID-space pattern,
- the selected permutation (`SPO`, `POS`, or `OSP`) and section index,
- the header byte ranges for dictionary, index container, selected permutation
  payload, and pyramid metadata.

This is intentionally physical-file provenance, not a narrative explanation. In
format v0.2 each permutation section is tiled (independently compressed
~64 KiB tiles with a byte-range directory, SPEC §6.2): routed reads fetch the
directory plus only the matching tile(s), and provenance names the physical
tile (`PERM/index`) with its compressed byte range. For pre-tiling (v0.1)
files `rete why --json` still reports tile provenance as `not_materialized`.

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
patterns: the reader resolves constants from the dictionary, chooses the best
SPO/POS/OSP permutation, follows the container length prefixes, and fetches only
that permutation payload. The next architectural step is physical community-tile
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
