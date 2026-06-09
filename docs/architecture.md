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
2. `rete-core` lowers the supported algebra subset into an internal plan.
3. BGPs, paths, filters, joins, aggregates, and graph targets evaluate against
   integer IDs where possible.
4. Final bindings resolve dictionary IDs back into RDF terms for text, JSON, or
   WASM output.

The engine has fast paths, but it is not a full streaming triplestore planner.
Simple BGP/FILTER shapes under `LIMIT` can stop early, `ASK` has
non-materializing paths, and several aggregate shapes operate in integer space.
`ORDER BY`, broad compound algebra, and many expression-heavy queries still
materialize intermediate rows.

Unsupported SPARQL constructs are rejected with clear errors. Known gaps include
nested `SELECT` subqueries and `SERVICE` federation.

## Progressive And Cost Paths

The summary path is the first layer of progressive querying. These query shapes
are exact from the summary and skip the full index:

- `SELECT (COUNT(*) AS ?n) WHERE { ?s <p> ?o }`
- `SELECT ?p (COUNT(*) AS ?n) WHERE { ?s ?p ?o } GROUP BY ?p`
- `SELECT ?p WHERE { ?s ?p ?o }`
- `SELECT (COUNT(DISTINCT ?p) AS ?n) WHERE { ?s ?p ?o }`
- `ASK { ?s <p> ?o }`

`rete cost --json --explain` reports whether a query is summary-only or needs
the full index. `rete progressive` exposes the summary-safe path directly and
returns metadata such as byte counts, request counts, and whether the index was
read.

The next architectural step is tile-routed exact refinement: use the pyramid to
fetch only relevant community/index ranges instead of opening the whole index.

## Range Reads

Range readers use the same file layout as local readers. A ranged open typically
does:

1. Fetch the header.
2. Fetch dictionary metadata and the term sections needed to resolve constants.
3. Fetch summary or index ranges depending on the query shape.
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

The playground is generated from `web/playground.template.html` and the WASM
package by `scripts/build_playground.py`. Do not hand-edit
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
| Tile-routed query refinement | Use the pyramid to fetch only relevant exact ranges |
| Result provenance | Attach contributing triples/blocks/ranges to query answers |
| Query engine rows | Replace wide `BTreeMap<String, String>` bindings with integer slot rows |
| Benchmark docs | Refresh JSON snapshots with `rete-bench --json` and regenerate the benchmark section |
| SHACL | Add `sh:qualifiedValueShapesDisjoint` if full Core coverage becomes necessary |
