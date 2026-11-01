# Architecture

Rete turns RDF into an indexed file that clients query locally or through HTTP
byte ranges. One Rust engine supplies the behavior behind commands, bindings
and browser interfaces. Follow a build and query below to find where your
change belongs.

## Find the code you need {#workspace-map}

| Location | Responsibility |
|---|---|
| `crates/rete-core` | Storage, ingestion, querying, range reads, reasoning and validation |
| `crates/rete-graph` | Public Rust package name; re-exports the core rather than implementing another engine |
| `crates/rete-cli` | Commands, local/remote file handling and terminal output |
| `crates/rete-wasm` | Browser bindings around the same core |
| `clients/` | Python, JavaScript, R, Java, Blender, MCP bundle, relay and desktop integrations |
| `crates/docgen` and `docs/` | Documentation renderer, authored Markdown and generated site |
| `crates/bench`, `tests/` and `fuzz/` | Measurements, cross-system checks, browser tests and fuzzing |
| `web/` and `scripts/` | Editable application sources and build, publishing and verification tools |
| `.github/workflows/` | CI and release automation |

The [workspace manifest](https://github.com/caviri/rete/blob/main/Cargo.toml)
defines six Rust crates. Several clients and the fuzz harness build separately
with their own toolchains. Clients depend on the engine; the engine does not
depend on clients. Dataset harvesters and converters live in the separate
`rete-datasets` repository, not in the public product tooling.

For a Rust entry point, start with the `format`, `query`, `range`, `card`,
`validation` and `reasoning` namespaces in
[`rete-core/src/lib.rs`](https://github.com/caviri/rete/blob/main/crates/rete-core/src/lib.rs).
They group the supported API without exposing every internal module.

## Follow a build into a file {#build-pipeline}

`rete build` starts in the CLI's `commands/build.rs`. The core's `ingest.rs`
parses RDF, the dictionary assigns integer IDs to terms, and `index.rs` sorts
triples into permutation indexes. Different sort orders make different bound
query patterns efficient. Independently compressed tiles let readers retrieve
part of an index.

For larger inputs, `--memory-budget-mb` selects the external builder in
[`extbuild.rs`](https://github.com/caviri/rete/blob/main/crates/rete-core/src/extbuild.rs).
It spills chunks and sorted runs to disk, merges dictionaries, remaps IDs and
writes the final sections incrementally. The budget sizes the working buffers;
it is not an exact process-RSS ceiling, and tile directories still grow with
the dataset. This path has different feature limits from an in-memory build;
check the [CLI reference](cli.md) before adding an option to it.

<span id="file-layout"></span>
`header.rs` describes the section directory; `file.rs` assembles and opens the
file. Besides dictionaries and indexes, files can contain community summaries,
schema, label/text indexes and a Dataset Card. Their presence and placement
depend on build options and file features. The [format specification](SPEC.md)
owns their byte-level contracts.

A snapshot stays immutable. Changes to a logical graph can instead use
[`manifest.rs`](https://github.com/caviri/rete/blob/main/crates/rete-core/src/manifest.rs),
which combines immutable addition/deletion segments. `rete serve` can also keep
updates in a journal. See [Writable graphs](manifest.md) for the lifecycle and
memory implications.

## Follow a query back to an answer {#query-pipeline}

The SPARQL entry points in `sparql.rs` parse a query, lower its algebra in
`sparql/lower.rs`, and evaluate it through `sparql/eval.rs`. Integer slot rows
carry bindings between operators; dictionary IDs become RDF terms when needed
for expressions or output. Selective scans and streaming operators can stop
early for `LIMIT` or `ASK`, while sorting, aggregation and broad traversals can
still require substantial work. `results.rs` serializes the answer for clients.

<span id="range-reads"></span>
Opening strategy matters: `Rete::open` consumes resident bytes;
`open_ranged` fetches whole relevant sections; `open_ranged_lazy` defers
dictionary chunks and tile payloads until used. `reader.rs` supplies the
`RangeReader` interface, with caches and an optional native HTTP implementation
around it. Native and browser HTTP readers allow a bounded whole-body fallback
when a host ignores ranges, normally up to 256 MiB; that download still counts
as transferred data. See [Hosting](hosting.md).

<span id="progressive-and-cost-paths"></span>
Exact lazy queries use permutation tiles, sometimes several tiles for one
leading ID. Community summaries support overviews and documented summary query
shapes; they are not implemented physical community partitions for exact reads.
[Semantic zoom](semantic-zoom.md) explains that distinction.

<span id="result-provenance"></span>
`rete why` exposes physical routing and byte ranges, rather than a narrative
explanation of an answer. Lazy readers must also surface incomplete-fetch
errors: missing data must never appear to be a successful empty result.

## Extend the engine without breaking its readers {#safety-invariants}

<span id="reasoning-and-validation"></span>
Keep shared behavior in the core. `reason.rs` handles materialization,
`sparql/ql.rs` supports query-time reasoning, and `shacl.rs` validates shapes;
their supported subsets remain explicit in the [reasoning](reasoning.md) and
[validation](shacl.md) references. Changes to encoded bytes also need format,
compatibility and corruption-handling checks.

<span id="browser-and-wasm"></span>
The default browser engine is single-threaded; native-only dependencies must
stay behind their feature/target boundaries. Edit application sources under
`web/`, then use `docker compose run --rm wasm`, the canonical producer of the
WASM and browser artifacts. In a worktree, pass the source revision as described
in the [contributor guide](https://github.com/caviri/rete/blob/main/CONTRIBUTING.md).

<span id="extension-points"></span>
Use [Testing and CI](testing.md) to choose checks for the part you changed.
Use [Benchmarks](benchmarks.md) to compare equivalent workloads and recorded
measurements. Check correctness and reading costs before trusting faster timings.
