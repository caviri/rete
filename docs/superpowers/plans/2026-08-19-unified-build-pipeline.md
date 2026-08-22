# Unified Compact Build Pipeline Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace rete's two build implementations and six independent index containers with a single-pass, bounded, deterministic `0x06` pipeline that is at least 1.5x faster and uses at least 25% less peak memory on the approved large workloads.

**Architecture:** Both ordinary and external ingestion produce canonical dictionary and triple-spool artifacts behind a small internal interface. Three paired family builders create SPO/SOP, POS/PSO, and OSP/OPS tile streams with shared routing boundaries; one streaming writer emits `0x06`, while eager and ranged readers reconstruct the existing six logical `GraphIndex` views so query code stays unchanged.

**Tech Stack:** Rust 2021, rayon, zstd, blake3, proptest, clap integration tests, Python 3 benchmark tooling, Docker Compose/devcontainer, wasm-bindgen, repository browser gate.

**Spec:** `docs/superpowers/specs/2026-08-19-unified-build-pipeline-design.md`

## Global Constraints

- Advance the stable format byte exactly from `0x05` to `0x06`; final readers accept only `0x06` and final writers emit only `0x06`.
- Do not add a legacy-output flag or retain the `0x05` decoder after cutover.
- Do not introduce production `unsafe`, unchecked indexing, raw shared pointers, or type transmutation.
- Preserve RDF parsing semantics, HDT-style canonical term ordering, all six logical permutation orders, checked arithmetic, clean malformed-input errors, and lazy range loading.
- Keep ordinary/external output byte-identical when their supported features and metadata payloads are identical.
- External build remains default-graph-only with no pyramid, text index, reasoning, or materialization.
- Primary large-workload gates: median wall time <=66.7% of baseline, peak RSS and phase heap <=75%, output size <=110%, and query median/p90 <=105%.
- Default Louvain is unchanged; Louvain-dominated total wall time has a no-regression gate rather than the 1.5x total gate.
- Use the repository's Docker/devcontainer commands, never host Rust tooling.
- Do not mutate Cloudflare R2. Catalog publication is a separately authorized task.
- Do not change the default permutation/build flags independently of the `0x06` cutover.
- Keep every intermediate commit testable; do not combine the format cutover with unreviewed performance tuning.

## File Structure

New core modules isolate responsibilities:

- `crates/rete-core/src/build_pipeline/mod.rs` — shared types, errors, and orchestration facade.
- `crates/rete-core/src/build_pipeline/timing.rs` — native phase telemetry with a WASM no-op implementation.
- `crates/rete-core/src/build_pipeline/ingest.rs` — provisional IDs, role tracking, memory interning, and canonical remaps.
- `crates/rete-core/src/build_pipeline/spool.rs` — resident and file-backed fixed-width canonical triple spools.
- `crates/rete-core/src/build_pipeline/family.rs` — safe radix helpers, paired tile boundaries, and three family builders.
- `crates/rete-core/src/build_pipeline/writer.rs` — section spools, checked streaming layout, hashing, and destination installation.
- `crates/rete-core/tests/build_pipeline.rs` — public-behavior and ordinary/external equivalence coverage.
- `crates/rete-core/tests/compatibility_v2.rs` and `crates/rete-core/tests/fixtures/v2/` — deliberate break plus stable `0x06` baseline.
- `scripts/bench_build_pipeline.py` and `scripts/test_bench_build_pipeline.py` — strict alternating-process performance evidence.
- `scripts/build-workloads/*.json` — pinned workload definitions without embedding large inputs.

Existing integration boundaries remain focused:

- `dictionary.rs` exposes canonical construction from role-classified terms.
- `index.rs` keeps six logical sections and maps them to three families.
- `file.rs` owns exact `0x06` family encoding/decoding and eager/ranged loading.
- `ingest.rs` and `extbuild.rs` become compatibility facades over the shared pipeline.
- CLI `commands/build.rs` selects memory vs chunked ingestion and installs output.
- `header.rs`, `docs/SPEC.md`, fixtures, WASM, catalog UI, and generated artifacts switch together at the explicit cutover tasks.

---

### Task 1: Common Build-Phase Telemetry

**Files:**
- Create: `crates/rete-core/src/build_pipeline/mod.rs`
- Create: `crates/rete-core/src/build_pipeline/timing.rs`
- Modify: `crates/rete-core/src/lib.rs`
- Modify: `crates/rete-core/src/ingest.rs`
- Modify: `crates/rete-core/src/extbuild.rs`
- Modify: `crates/rete-core/src/file.rs`
- Test: `crates/rete-core/src/build_pipeline/timing.rs`
- Test: `crates/rete-cli/tests/build_timing.rs`

**Interfaces:**
- Produces: `BuildPipelineError`, `BuildPhase`, `BuildCounters`, `BuildTiming::new()`, `BuildTiming::lap(BuildPhase)`, `BuildTiming::set_counters(BuildCounters)`, `BuildTiming::finish()`, `BuildTiming::render_lines()`, plus test-only `BuildTiming::{new_for_test,record_for_test}` helpers.
- Consumes: existing `RETE_BUILD_TIMING` environment switch and current pyramid phase labels.

- [ ] **Step 1: Write failing unit and CLI timing tests**

```rust
#[test]
fn timing_render_is_ordered_and_machine_independent() {
    let mut timing = BuildTiming::new_for_test();
    timing.record_for_test(BuildPhase::ParseIngest, 12);
    timing.record_for_test(BuildPhase::Canonicalize, 7);
    timing.set_counters(BuildCounters {
        statements: 3,
        input_bytes: Some(99),
        spill_bytes: 0,
        output_bytes: 42,
        family_runs: [1, 1, 1],
    });
    assert_eq!(
        timing.render_lines(),
        vec![
            "  [build] parse+ingest: 12 ms",
            "  [build] canonicalize: 7 ms",
            "  [build] statements: 3, input: 99 B, spill: 0 B, output: 42 B",
            "  [build] family runs (S/P/O): 1/1/1",
        ]
    );
}
```

```rust
#[test]
fn ordinary_and_external_builds_report_common_phases() {
    // Run the real rete binary twice with RETE_BUILD_TIMING=1 and assert stderr
    // contains parse+ingest, canonicalize, index families, final write, total.
}
```

- [ ] **Step 2: Run the focused tests and verify RED**

Run:

```sh
docker compose run --rm dev cargo test -p rete-core build_pipeline::timing -- --nocapture
docker compose run --rm dev cargo test -p rete-cli --test build_timing -- --nocapture
```

Expected: compile failure because `build_pipeline` and the timing API do not exist.

- [ ] **Step 3: Implement the telemetry module without exposing a clock to WASM**

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum BuildPhase {
    ParseIngest,
    ChunkSeal,
    Canonicalize,
    Remap,
    Pyramid,
    TextIndex,
    SubjectFamily,
    PredicateFamily,
    ObjectFamily,
    TileEncodeCompress,
    FinalWrite,
    Install,
    Total,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct BuildCounters {
    pub statements: u64,
    pub input_bytes: Option<u64>,
    pub spill_bytes: u64,
    pub output_bytes: u64,
    pub family_runs: [u64; 3],
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum BuildPipelineError {
    #[error(transparent)] Ingest(#[from] crate::ingest::IngestError),
    #[error(transparent)] File(#[from] crate::file::FileError),
    #[error(transparent)] Io(#[from] std::io::Error),
    #[error("term id space exceeds u32")] TooManyTerms,
    #[error("invalid build spool: {0}")] InvalidSpool(&'static str),
    #[error("build arithmetic overflow: {0}")] Overflow(&'static str),
    #[cfg(test)]
    #[error("injected build failure: {0}")] InjectedFailure(&'static str),
}

pub(crate) struct BuildTiming {
    enabled: bool,
    samples: Vec<(BuildPhase, u128)>,
    counters: BuildCounters,
    #[cfg(not(target_arch = "wasm32"))]
    lap_started: Option<std::time::Instant>,
}
```

Implement `label()` as a total match, make the WASM implementation a no-op,
and replace the ad-hoc pyramid clock only when the common timer is enabled. The
final diagnostic lines report statement count, known input bytes (or `unknown`),
spill bytes, output bytes, and the S/P/O family run counts. Add phase/counter
calls to the existing builders without changing their output.

- [ ] **Step 4: Run focused and WASM-safe tests**

```sh
docker compose run --rm dev cargo test -p rete-core build_pipeline::timing -- --nocapture
docker compose run --rm dev cargo test -p rete-cli --test build_timing -- --nocapture
docker compose run --rm dev cargo test -p rete-wasm
```

Expected: all pass; existing build files remain byte-identical.

- [ ] **Step 5: Commit the telemetry boundary**

```sh
git add crates/rete-core/src/build_pipeline crates/rete-core/src/lib.rs crates/rete-core/src/ingest.rs crates/rete-core/src/extbuild.rs crates/rete-core/src/file.rs crates/rete-cli/tests/build_timing.rs
git commit -m "perf(build): add shared phase telemetry"
```

### Task 2: Strict Build Benchmark Harness

**Files:**
- Create: `scripts/bench_build_pipeline.py`
- Create: `scripts/test_bench_build_pipeline.py`
- Create: `scripts/build-workloads/chemotion.json`
- Create: `scripts/build-workloads/small.json`
- Create: `scripts/build-workloads/synthetic-large.json`
- Create: `scripts/build-workloads/synthetic-louvain.json`
- Create: `scripts/build-workloads/synthetic-external.json`

**Interfaces:**
- Produces: `load_workload(path: pathlib.Path) -> Workload`, `run_sample(executable: pathlib.Path, workload: Workload, input_root: pathlib.Path, output_dir: pathlib.Path, implementation: str, repetition: int) -> dict`, `summarize(rows: list[dict]) -> dict`, and JSONL records with `schemaVersion: 1` and kinds `SOURCE`, `SAMPLE`, `SUMMARY`.
- Consumes: exact baseline/candidate executable paths, pinned input path/SHA-256, build arguments, query commands, sample count, and output evidence path.

- [ ] **Step 1: Write failing strict-schema and summary tests**

```python
def test_workload_rejects_unknown_keys_and_duplicate_json_members(self):
    bad = {
        "name": "x", "input": "a", "sha256": "0" * 64,
        "mode": "standard", "args": [], "gateClass": "primary",
        "queries": [], "extra": 1,
    }
    with self.assertRaisesRegex(ValueError, "unknown key"):
        load_workload(write_json(bad))
    with self.assertRaisesRegex(ValueError, "duplicate JSON member"):
        load_workload(write_raw('{"name":"x","name":"y"}'))

def test_summary_uses_median_p90_and_requires_stable_hashes(self):
    rows = sample_rows(times=[100, 90, 110], rss=[50, 45, 55], output_hash="abc")
    got = summarize(rows)
    self.assertEqual(got["wallMsMedian"], 100)
    self.assertEqual(got["outputHashes"], ["abc"])
```

The workload schema is exactly `name`, `input`, `sha256`, `mode`, `args`,
`gateClass`, and `queries`; each query is exactly `name`, `args`, and `sha256`.
Accepted gate classes are `primary`, `small-overhead`, `louvain-no-regression`,
and `external-primary`. Query argument
arrays may contain the reserved literal `{output}` or `{url}`, which the harness
replaces with the just-built file path or its strict local HTTP range URL without
shell interpolation.

- [ ] **Step 2: Run tests and verify RED**

```sh
docker compose run --rm dev uv run python scripts/test_bench_build_pipeline.py -v
```

Expected: import/file failure because the harness does not exist.

- [ ] **Step 3: Implement exclusive evidence and alternating execution**

```python
def open_exclusive(path: pathlib.Path):
    path.parent.mkdir(parents=True, exist_ok=True)
    return path.open("x", encoding="utf-8", newline="\n")

def percentile90(values):
    ordered = sorted(values)
    return ordered[max(0, math.ceil(0.90 * len(ordered)) - 1)]
```

Use fresh processes, alternate baseline/candidate order per repetition, perform two warmups, require at least 15 accepted samples, invoke `/usr/bin/time -v` inside the Linux dev container for peak RSS, hash every output and query result, and abort on any identity drift. Workload JSON pins input hashes but not machine-specific absolute paths; `--input-root` resolves them.
For `{url}` queries, start a loopback-only server that requires `Range`, returns
206 with exact `Content-Range`, records bytes/GETs, and rejects full or malformed
requests; shut it down after each fresh query process.

- [ ] **Step 4: Add pinned workload definitions**

```json
{
  "name": "chemotion-types-card",
  "input": "chemotion/chemotion.nq",
  "sha256": "a60b7da39192fd2a1bef5b302d22d97291222f1a9805cbab9cc709c24b28c950",
  "mode": "standard",
  "args": ["--pyramid-algo", "types", "--card"],
  "gateClass": "primary",
  "queries": []
}
```

Copy the small tracked fixture and generate the shared standard synthetic input
with exactly:

```sh
mkdir -p target/bench/inputs/fixtures target/bench/inputs/synthetic
cp tests/gate/fixtures/worldcup2026.nt target/bench/inputs/fixtures/worldcup2026.nt
python3 scripts/gen_graph.py 400000 5 100 > target/bench/inputs/synthetic/social-400k.nt
```

`small.json` pins SHA-256
`1d71ecfc9a57e287f75b29d927fcf52a9c8e6a583535681062d492d3f881f7ab`
and `gateClass: "small-overhead"`. Both standard synthetic JSON files pin SHA-256
`92cba43189f742f1b441be85cfdb935711a558caac35fce34c7e89665940cc23`.
`synthetic-large.json` uses mode `standard` and args `--no-pyramid`;
its three very high-cardinality predicate groups are also the concrete skewed/
mega-group workload.
`synthetic-louvain.json` uses the default pyramid and gate class
`louvain-no-regression`.

Generate the spill-forcing external source with exactly:

```sh
python3 scripts/gen_graph.py 8000000 5 100 > target/bench/inputs/synthetic/social-8m.nt
```

`synthetic-external.json` pins SHA-256
`8e9a1b8731183a23627a97ef6ec429ebfe8506b148b1d326e38013017ffc4861`,
uses mode `external`, gate class `external-primary`, and no baked-in budget;
the harness's required `--external-budgets 64,256,1024` matrix appends exactly
one `--memory-budget-mb` value per sample. All workload files start with an
empty query list; Task 13 adds accepted query hashes only after differential
correctness discovery.

- [ ] **Step 5: Run harness tests and help contract**

```sh
docker compose run --rm dev uv run python scripts/test_bench_build_pipeline.py -v
docker compose run --rm dev uv run python scripts/bench_build_pipeline.py --help
docker compose run --rm dev python -m json.tool scripts/build-workloads/chemotion.json
docker compose run --rm dev python -m json.tool scripts/build-workloads/synthetic-external.json
```

Expected: all pass; no benchmark starts without explicit executable and evidence paths.

- [ ] **Step 6: Commit the benchmark contract**

```sh
git add scripts/bench_build_pipeline.py scripts/test_bench_build_pipeline.py scripts/build-workloads
git commit -m "bench(build): add pinned pipeline harness"
```

### Task 3: Provisional IDs and Memory Canonicalization

**Files:**
- Create: `crates/rete-core/src/build_pipeline/ingest.rs`
- Modify: `crates/rete-core/src/build_pipeline/mod.rs`
- Modify: `crates/rete-core/src/dictionary.rs`
- Test: `crates/rete-core/src/build_pipeline/ingest.rs`

**Interfaces:**
- Produces: `ProvisionalQuad`, `MemoryIngest::{new,push,finish}`, `CanonicalMemory`, and `Dictionary::from_role_terms`.
- Consumes: `ingest::RawQuad`, `Dictionary`, `BuildStats`, and current lexical sort rules.

- [ ] **Step 1: Write failing canonicalization tests**

```rust
#[test]
fn repeated_terms_allocate_once_and_roles_remap_exactly() {
    let mut ingest = MemoryIngest::new();
    ingest.push(q("<a>", "<p>", "<b>", None)).unwrap();
    ingest.push(q("<b>", "<p>", "<a>", None)).unwrap();
    let built = ingest.finish(|_| Vec::new()).unwrap();
    assert_eq!(built.unique_node_terms(), 2);
    assert_eq!(built.unique_predicate_terms(), 1);
    assert_eq!(built.default_triples, vec![(1, 1, 2), (2, 1, 1)]);
    assert_eq!(built.dictionary.resolve_subject(1), Some("<a>"));
}

#[test]
fn input_order_and_duplicates_do_not_change_sorted_canonical_content() {
    assert_eq!(
        sorted_dictionary_and_deduped_triples(forward()),
        sorted_dictionary_and_deduped_triples(reversed_with_duplicates())
    );
}
```

- [ ] **Step 2: Run tests and verify RED**

```sh
docker compose run --rm dev cargo test -p rete-core build_pipeline::ingest -- --nocapture
```

Expected: compile failure for missing `MemoryIngest`.

- [ ] **Step 3: Implement fixed-width records and borrowed interner probes**

```rust
pub(crate) const DEFAULT_GRAPH_ID: u32 = u32::MAX;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct ProvisionalQuad {
    pub subject: u32,
    pub predicate: u32,
    pub object: u32,
    pub graph: u32,
}

pub(crate) struct CanonicalMemory {
    pub dictionary: Dictionary,
    pub default_triples: Vec<Triple>,
    pub named: BTreeMap<String, Vec<Triple>>,
    pub metadata: Vec<u8>,
    pub stats: BuildStats,
}
```

Use `HashMap<String, u32>` with borrowed `str` lookup, one lexical allocation per unique term, node role bits, a separate predicate map, checked `u32::try_from`, and deterministic lexical sorting. `Dictionary::from_role_terms(shared, subjects, objects, predicates, has_quoted)` consumes already-sorted vectors without cloning them again.

- [ ] **Step 4: Add reference equivalence across RDF term shapes**

Construct IRIs, blank nodes, plain/language/datatype literals, RDF-star terms, named graphs, and duplicates; compare dictionary resolution and encoded triples against the existing `DictionaryBuilder` path.

- [ ] **Step 5: Run focused and core tests**

```sh
docker compose run --rm dev cargo test -p rete-core build_pipeline::ingest -- --nocapture
docker compose run --rm dev cargo test -p rete-core dictionary::tests -- --nocapture
```

Expected: all pass with no production call-site changes.

- [ ] **Step 6: Commit memory canonicalization**

```sh
git add crates/rete-core/src/build_pipeline crates/rete-core/src/dictionary.rs
git commit -m "perf(build): canonicalize provisional ids in one pass"
```

### Task 4: Resident and File-Backed Triple Spools

**Files:**
- Create: `crates/rete-core/src/build_pipeline/spool.rs`
- Modify: `crates/rete-core/src/build_pipeline/mod.rs`
- Modify: `crates/rete-core/src/build_pipeline/ingest.rs`
- Modify: `crates/rete-core/src/extbuild.rs`
- Test: `crates/rete-core/src/build_pipeline/spool.rs`
- Test: `crates/rete-core/tests/build_pipeline.rs`

**Interfaces:**
- Produces: `BuildTemp`, `TripleSpool::{Resident, File}`, `TripleSpool::for_each_block`, `TripleSpool::count`, and `ChunkedIngest::finish`.
- Consumes: `ProvisionalQuad`, `CanonicalMemory` rules, current external chunk term files, and a caller-supplied memory budget.

- [ ] **Step 1: Write failing spool round-trip and cleanup tests**

```rust
#[test]
fn file_spool_replays_exact_fixed_width_records_in_bounded_blocks() {
    let temp = BuildTemp::new(test_dir()).unwrap();
    let spool = TripleSpool::write_file(&temp, "canonical.tri", triples()).unwrap();
    assert_eq!(collect_blocks(&spool, 2).unwrap(), triples());
    assert_eq!(spool.count(), triples().len() as u64);
}

#[test]
fn dropping_build_temp_removes_only_its_owned_directory() {
    // Keep a sibling sentinel, drop BuildTemp, assert sentinel remains and owned dir is gone.
}
```

- [ ] **Step 2: Run tests and verify RED**

```sh
docker compose run --rm dev cargo test -p rete-core build_pipeline::spool -- --nocapture
```

Expected: compile failure because spool types do not exist.

- [ ] **Step 3: Implement exact 12-byte canonical records and checked iteration**

```rust
pub(crate) enum TripleSpool {
    Resident(Vec<Triple>),
    File { path: PathBuf, count: u64 },
}

impl TripleSpool {
    pub(crate) fn for_each_block(
        &self,
        max_records: usize,
        visit: &mut dyn FnMut(&[Triple]) -> Result<(), BuildPipelineError>,
    ) -> Result<(), BuildPipelineError>;
}
```

Reject partial trailing records as `InvalidSpool`, cap block allocation from `max_records`, and keep `BuildTemp`'s resolved owned path for guarded cleanup.

- [ ] **Step 4: Refactor current external chunk merge behind `ChunkedIngest`**

```rust
pub(crate) struct CanonicalSpilled {
    pub dictionary: SpilledDictionary,
    pub triples: TripleSpool,
    pub metadata: Vec<u8>,
    pub stats: BuildStats,
}

pub(crate) struct SpilledDictionary {
    pub section_paths: [PathBuf; 4],
    pub term_count: u64,
    pub has_quoted_triples: bool,
}

impl ChunkedIngest {
    pub(crate) fn new(temp: &BuildTemp, memory_budget: u64) -> Self;
    pub(crate) fn push(&mut self, quad: RawQuad) -> Result<(), BuildPipelineError>;
    pub(crate) fn finish(
        self,
        metadata: impl FnOnce(&BuildStats) -> Vec<u8>,
    ) -> Result<CanonicalSpilled, BuildPipelineError>;
}
```

Move existing chunk sealing, dictionary merge, and remap logic without changing its bytes. `ChunkedIngest` still rejects named graphs immediately and derives every resident buffer from the memory budget.

- [ ] **Step 5: Prove memory/chunked canonical equivalence**

In `tests/build_pipeline.rs`, feed a duplicate-heavy default graph through both backends with 64 MiB and 256 MiB budgets. Assert identical canonical dictionary bytes and sorted canonical triple multisets, plus stable results across chunk boundaries.

- [ ] **Step 6: Run focused and external tests**

```sh
docker compose run --rm dev cargo test -p rete-core --test build_pipeline -- --nocapture
docker compose run --rm dev cargo test -p rete-core extbuild::tests -- --nocapture
```

- [ ] **Step 7: Commit shared spool artifacts**

```sh
git add crates/rete-core/src/build_pipeline crates/rete-core/src/extbuild.rs crates/rete-core/tests/build_pipeline.rs
git commit -m "refactor(build): share canonical triple spools"
```

### Task 5: Safe Paired-Family Tile Builder

**Files:**
- Create: `crates/rete-core/src/build_pipeline/family.rs`
- Modify: `crates/rete-core/src/build_pipeline/mod.rs`
- Modify: `crates/rete-core/src/index.rs`
- Modify: `crates/rete-core/src/triples.rs` only for reusable safe size accounting.
- Test: `crates/rete-core/src/build_pipeline/family.rs`
- Test: `crates/rete-core/tests/properties.rs`

**Interfaces:**
- Produces: `IndexFamily`, `PairedTile`, `FamilyIndex`, `FamilyView`, `build_family`, `GraphIndex::from_families`, `GraphIndex::family_view`, and `IndexPermutation::family_slot`.
- Consumes: `TripleSpool`, `INDEX_TILE_BUDGET`, `GroupSizer`, `encode_sorted_unique`, and the existing six `IndexPermutation` mappings.

- [ ] **Step 1: Write failing family-order and shared-boundary tests**

```rust
#[test]
fn subject_family_produces_spo_and_sop_with_shared_ranges() {
    let family = build_family(&TripleSpool::Resident(fixture()), IndexFamily::Subject, 64).unwrap();
    assert_eq!(decode(&family.first), sorted_spo());
    assert_eq!(decode(&family.second), sorted_sop());
    assert_eq!(ranges(&family.first), ranges(&family.second));
}

#[test]
fn mega_group_continuations_are_bounded_and_complete_in_both_orders() {
    let family = build_family(&TripleSpool::Resident(hot_subject(20_000)), IndexFamily::Subject, 256).unwrap();
    assert!(family.first.len() > 1);
    assert_eq!(decode(&family.first), sorted_spo_hot());
    assert_eq!(decode(&family.second), sorted_sop_hot());
}
```

- [ ] **Step 2: Run tests and verify RED**

```sh
docker compose run --rm dev cargo test -p rete-core build_pipeline::family -- --nocapture
```

Expected: missing `IndexFamily` and `build_family`.

- [ ] **Step 3: Implement safe deterministic radix primitives**

```rust
fn radix_pass(input: &mut Vec<Triple>, scratch: &mut Vec<Triple>, byte: impl Fn(Triple) -> u8) {
    let mut counts = [0usize; 256];
    for &triple in input.iter() { counts[byte(triple) as usize] += 1; }
    let mut offsets = prefix_offsets(counts);
    scratch.resize(input.len(), (0, 0, 0));
    for &triple in input.iter() {
        let bucket = byte(triple) as usize;
        scratch[offsets[bucket]] = triple;
        offsets[bucket] += 1;
    }
    std::mem::swap(input, scratch);
}
```

Compose stable little-endian byte passes from least- to most-significant key. Test against `sort_unstable` for random triples and every family/order. Do not use `MaybeUninit` or an unsafe scatter.

- [ ] **Step 4: Implement common paired boundaries**

```rust
pub(crate) type Synopsis = (u32, u32, u32, u32);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum IndexFamily {
    Subject,
    Predicate,
    Object,
}

pub(crate) struct PairedTile {
    pub min_a: u32,
    pub max_a: u32,
    pub first: Vec<u8>,
    pub second: Vec<u8>,
    pub first_synopsis: Synopsis,
    pub second_synopsis: Synopsis,
}

pub(crate) struct FamilyIndex {
    pub family: IndexFamily,
    pub tiles: Vec<PairedTile>,
}

pub(crate) struct FamilyView<'a> {
    pub family: IndexFamily,
    pub first: &'a [Tile],
    pub second: &'a [Tile],
}

pub(crate) fn build_family(
    spool: &TripleSpool,
    family: IndexFamily,
    tile_budget: usize,
) -> Result<FamilyIndex, BuildPipelineError>;
```

Normalize one family to `(lead, x, y)`, sort by leading ID, produce both tail
orders, and pack the same complete leading groups into each paired tile. When
either order exceeds the tile budget inside one leading group, calculate
independent byte-bounded slices, choose the larger slice count, and further split
the other order's largest non-singleton slices until the counts match. Splitting
may only reduce encoded size, and every slice remains non-empty. Assert every
encoded tile parses and stays within the budget before accepting it.

- [ ] **Step 5: Adapt `GraphIndex` without changing query call sites**

```rust
impl GraphIndex {
    pub(crate) fn from_families(families: [FamilyIndex; 3]) -> GraphIndex {
        // Expand pairs into ALL_PERMS order: SPO, POS, OSP, SOP, PSO, OPS.
    }

    pub(crate) fn family_view(&self, family: IndexFamily) -> FamilyView<'_>;
}
```

Keep `IndexPermutation::section_index`, loaders, planner metrics, and scan methods
stable. Add `IndexPermutation::family_slot() -> (usize, bool)` where the boolean
selects the family's second order, plus a builder-only
`GraphIndexBuilder::build_families()`; do not switch production callers yet.

- [ ] **Step 6: Run unit, property, and no-default tests**

```sh
docker compose run --rm dev cargo test -p rete-core build_pipeline::family -- --nocapture
docker compose run --rm dev cargo test -p rete-core --test properties -- --nocapture
docker compose run --rm dev cargo test -p rete-core --no-default-features build_pipeline::family -- --nocapture
```

- [ ] **Step 7: Commit paired family construction**

```sh
git add crates/rete-core/src/build_pipeline crates/rete-core/src/index.rs crates/rete-core/src/triples.rs crates/rete-core/tests/properties.rs
git commit -m "perf(index): build paired permutation families"
```

### Task 6: Pin the Exact `0x06` Family Encoding

**Files:**
- Modify: `docs/SPEC.md`
- Modify: `crates/rete-core/src/header.rs`
- Modify: `crates/rete-core/src/file.rs`
- Modify: `crates/rete-core/src/index.rs`
- Modify: `crates/rete-core/src/triples.rs`
- Test: `crates/rete-core/src/header.rs`
- Test: `crates/rete-core/src/file.rs`

**Interfaces:**
- Produces: `encode_family_container`, `decode_family_container`, `DecodedFamily`, `FamilyDirectory`, `Prefix2Meta`, `NEXT_FORMAT_VERSION`, and exact `0x06` family bytes without switching the production header yet.
- Consumes: `FamilyIndex`, current container varints, codecs, tile synopses, and fixed 1024-byte header.

- [ ] **Step 1: Write the byte layout into `docs/SPEC.md`**

Specify the root index as exactly three length-framed family payloads in Subject, Predicate, Object order. Specify each family as:

```text
uvarint tile_pair_count
tile_pair_count * (uvarint min_a_delta, uvarint max_a_span)
tile_pair_count * (uvarint first_flags, uvarint first_compressed_len, uvarint first_prefix2_len)
tile_pair_count * (uvarint second_flags, uvarint second_compressed_len, uvarint second_prefix2_len)
first tile records in order: prefix-2 blob, then compressed payload
second tile records in order: prefix-2 blob, then compressed payload
first synopsis trailer: 4 uvarints per tile
second synopsis trailer: 4 uvarints per tile
```

Flags bit 0 means the tile continues the previous tile's leading group and bit 1
means the leading group continues into the next tile; all other bits are
reserved and rejected. A non-empty prefix-2 blob encodes
`uvarint a_group_count`, then delta-coded `(a, a_body_offset, b_count)` entries,
each followed by delta-coded `(b, c_body_offset, c_count)` entries. A zero
prefix-2 length means the existing bounded a-only fallback. Emit a blob only
when the complete compact encoding stays within the format's fixed per-tile
prefix-2 budget; never serialize a partial prefix-2 directory.

State that both orders have the same tile count and leading range per tile pair,
continuation flags must agree with repeated shared ranges, payload lengths
exclude trailers, offsets are cumulative checked `u64`, and empty graphs encode
three zero-count families.

- [ ] **Step 2: Write failing header and literal-byte tests**

```rust
#[test]
fn family_container_matches_literal_bytes() {
    assert_eq!(NEXT_FORMAT_VERSION, 0x06);
    let index = tiny_index();
    assert_eq!(
        encode_family_container(index.family_view(IndexFamily::Subject), CODEC_NONE).unwrap(),
        TINY_V2_BYTES
    );
}
```

- [ ] **Step 3: Run tests and verify RED**

```sh
docker compose run --rm dev cargo test -p rete-core family_container_matches_literal_bytes -- --nocapture
```

Expected: missing `NEXT_FORMAT_VERSION` and family codec fail.

- [ ] **Step 4: Implement checked family encoding and eager decoding**

```rust
pub(crate) struct FamilyDirectory {
    pub ranges: Vec<(u32, u32)>,
    pub first_flags: Vec<u8>,
    pub second_flags: Vec<u8>,
    pub first_lengths: Vec<u64>,
    pub second_lengths: Vec<u64>,
    pub first_prefix2_lengths: Vec<u64>,
    pub second_prefix2_lengths: Vec<u64>,
    pub first_synopses: Vec<Synopsis>,
    pub second_synopses: Vec<Synopsis>,
    pub first_records_offset: u64,
    pub second_records_offset: u64,
}

pub(crate) struct Prefix2Meta {
    pub groups: Vec<Prefix2Group>,
}

pub(crate) struct Prefix2Group {
    pub a: u32,
    pub a_body_offset: u32,
    pub b_entries: Vec<(u32, u32, u32)>, // (b, c_body_offset, c_count)
}

pub(crate) struct DecodedFamily {
    pub first: Vec<Tile>,
    pub second: Vec<Tile>,
}

pub(crate) fn encode_family_container(
    family: FamilyView<'_>,
    codec: u8,
) -> Result<Vec<u8>, FileError>;

pub(crate) fn decode_family_container(
    bytes: &[u8],
    codec: u8,
) -> Result<DecodedFamily, FileError>;
```

Use `checked_add`, `checked_sub`, `usize::try_from`, bounded initial capacities,
exact compressed-slice validation, and clean `FileError` variants for count
mismatch, reserved flags, impossible continuation, offset overflow, short
payload, malformed prefix-2 metadata, and malformed trailers. Expose safe
conversion between complete `GroupDirectory` data and `Prefix2Meta`; a decoded
prefix-2 offset must lie within the decompressed block before it is accepted.
Define
`pub(crate) const LEGACY_FORMAT_VERSION: u8 = 0x05` and
`pub(crate) const NEXT_FORMAT_VERSION: u8 = 0x06` beside the internal codec;
leave `CURRENT_FORMAT_VERSION` and `MIN_STABLE_READ_VERSION` at `0x05` in this
commit.

- [ ] **Step 5: Prove the codec commit remains workspace-green**

```sh
docker compose run --rm dev cargo test -p rete-core
```

Expected: all existing `0x05` production behavior still passes while internal
`0x06` codec tests pin the next layout.

- [ ] **Step 6: Commit the exact format contract**

```sh
git add docs/SPEC.md crates/rete-core/src/header.rs crates/rete-core/src/file.rs crates/rete-core/src/index.rs crates/rete-core/src/triples.rs
git commit -m "feat(format): specify paired index generation 0x06"
```

### Task 7: Eager and Ranged `0x06` Reader Integration

**Files:**
- Modify: `crates/rete-core/src/file.rs`
- Modify: `crates/rete-core/src/index.rs`
- Modify: `crates/rete-core/src/ingest.rs`
- Test: `crates/rete-core/src/file.rs`
- Test: `crates/rete-core/tests/roundtrip.rs`
- Test: `crates/rete-core/tests/properties.rs`
- Test: `crates/rete-core/tests/ranged.rs`
- Test: `crates/rete-core/tests/robustness.rs`
- Modify: `crates/rete-core/tests/compatibility_v1.rs`

**Interfaces:**
- Produces: `write_v2_dataset_from_parts`, `CURRENT_FORMAT_VERSION == 0x06`, transitional `MIN_STABLE_READ_VERSION == 0x05`, eager/ranged dispatch for both layouts, `read_family_directory_ranged`, and sibling-order payload isolation.
- Consumes: `GraphIndexBuilder::build_families`, family codecs, `RangeReader`, existing tile loaders, and adaptive cache controls.

- [ ] **Step 1: Write failing eager, ranged, and transition tests**

```rust
#[test]
fn v2_eager_roundtrip_covers_every_pattern_and_named_graph() {
    let bytes = build_v2_fixture();
    let rete = Rete::open(&bytes).unwrap();
    assert_eq!(rete.header().version, 0x06);
    assert_every_pattern_matches_reference(&rete);
    assert_named_queries_match_reference(&rete);
}

#[test]
fn routed_spo_query_fetches_no_sop_payload_bytes() {
    let fixture = multi_tile_v2();
    let reader = Arc::new(RecordingReader::new(fixture.bytes));
    let rete = Rete::open_ranged_lazy(reader.clone()).unwrap();
    rete.query(Some("<s>"), None, None);
    assert_no_overlap(reader.reads(), fixture.subject_second_payload_range);
}

#[test]
fn transitional_reader_keeps_v1_until_the_explicit_break_task() {
    assert_eq!(CURRENT_FORMAT_VERSION, 0x06);
    assert_eq!(MIN_STABLE_READ_VERSION, 0x05);
    Rete::open(include_bytes!("fixtures/v1/minimal.rete")).unwrap();
}
```

Also corrupt an SOP payload and assert an SPO-only query succeeds while an
SOP-routed query reports an incomplete load.

- [ ] **Step 2: Run focused tests and verify RED**

```sh
docker compose run --rm dev cargo test -p rete-core v2_eager_roundtrip -- --nocapture
docker compose run --rm dev cargo test -p rete-core --test ranged v2_ -- --nocapture
```

Expected: old six-section readers cannot interpret family bytes.

- [ ] **Step 3: Implement eager family dispatch**

```rust
fn encode_index_families(index: &GraphIndex, codec: u8) -> Result<Vec<u8>, FileError> {
    let payloads = [IndexFamily::Subject, IndexFamily::Predicate, IndexFamily::Object]
        .map(|family| encode_family_container(index.family_view(family), codec))
        .into_iter()
        .collect::<Result<Vec<_>, _>>()?;
    let refs = payloads.iter().map(Vec::as_slice).collect::<Vec<_>>();
    Ok(encode_container(&refs, CODEC_NONE))
}
```

Set `CURRENT_FORMAT_VERSION` to `0x06`, retain
`MIN_STABLE_READ_VERSION == 0x05`, and dispatch by header version. `0x05` uses
the existing six-section decoder; `0x06` decodes three families into the
existing six logical section slots. Pin remaining legacy writers to
`LEGACY_FORMAT_VERSION` rather than the current-version constant.

- [ ] **Step 4: Implement exact ranged metadata and loader mapping**

```rust
fn family_order(section: usize) -> (usize, bool) {
    ALL_PERMS
        .get(section)
        .copied()
        .expect("logical section index was checked")
        .family_slot()
}

let (family, second) = family_order(section);
let directory = &directories[family];
let base = if second {
    directory.second_records_offset
} else {
    directory.first_records_offset
};
let ranges = directory.payload_ranges(second, tile_indices)?;
reader.read_many_with_intent(&ranges, intent)
```

Read only counts, shared ranges, both flag/length arrays, derived tile-record
ranges, and synopses at open; do not fetch prefix-2 blobs or tile payloads. When
a scan selects a tile, fetch that order's adjacent prefix-2-plus-compressed tile
record in one range request, validate offsets against the decompressed block,
and seed its `GroupDirectory`.
Coalesce only within the selected order; preserve named-graph precise metadata,
adaptive accounting, failed-tile retry, and incomplete-result semantics.

- [ ] **Step 5: Switch test/public in-memory writers but keep external reference output legacy**

Switch `write_dataset` and non-streaming `assemble_dataset*` wrappers to family
output so integration fixtures exercise `0x06`. Keep
`assemble_dataset_streaming_algo` pinned to legacy only until Task 10 switches
the external/reference pair together. Update `compatibility_v1.rs` to assert the
fixture is `0x05`, current is `0x06`, and the transitional reader still opens it.

- [ ] **Step 6: Run the complete core matrix**

```sh
docker compose run --rm dev cargo test -p rete-core
docker compose run --rm dev cargo test -p rete-core --test ranged -- --nocapture
docker compose run --rm dev cargo test -p rete-core --test robustness -- --nocapture
docker compose run --rm dev cargo test -p rete-core --no-default-features
```

Expected: all green; new in-memory fixtures are `0x06`, while the transitional
reader still opens the committed `0x05` fixture.

- [ ] **Step 7: Commit eager and ranged support together**

```sh
git add crates/rete-core/src/file.rs crates/rete-core/src/index.rs crates/rete-core/src/ingest.rs crates/rete-core/tests/roundtrip.rs crates/rete-core/tests/properties.rs crates/rete-core/tests/ranged.rs crates/rete-core/tests/robustness.rs crates/rete-core/tests/compatibility_v1.rs
git commit -m "feat(format): read paired index families eagerly and lazily"
```

### Task 8: Streaming Sections and Failure-Safe File Installation

**Files:**
- Create: `crates/rete-core/src/build_pipeline/writer.rs`
- Modify: `crates/rete-core/src/build_pipeline/mod.rs`
- Modify: `crates/rete-core/src/file.rs`
- Modify: `crates/rete-core/Cargo.toml`
- Test: `crates/rete-core/src/build_pipeline/writer.rs`
- Test: `crates/rete-cli/tests/build_output_atomic.rs`

**Interfaces:**
- Produces: `SectionSpool`, `StreamingDatasetWriter`, `OutputInstaller`, and `write_dataset_to_path`.
- Consumes: encoded dictionary artifact, `FamilyIndex` streams, metadata/pyramid/text sections, `Header`, blake3 hashing, and destination path.

- [ ] **Step 1: Write failing layout and failure-injection tests**

```rust
#[test]
fn streaming_and_vec_writers_produce_identical_v2_bytes() {
    let parts = fixture_parts();
    assert_eq!(write_to_vec(&parts).unwrap(), write_to_temp_path(&parts).unwrap());
}

#[test]
fn failed_install_preserves_existing_destination() {
    let old = valid_fixture_bytes();
    write(&dest, &old);
    let err = writer_with_failure(FailAt::HeaderPatch).write(&dest, fixture_parts()).unwrap_err();
    assert!(matches!(err, BuildPipelineError::InjectedFailure(_)));
    assert_eq!(read(&dest), old);
}
```

- [ ] **Step 2: Run tests and verify RED**

```sh
docker compose run --rm dev cargo test -p rete-core build_pipeline::writer -- --nocapture
docker compose run --rm dev cargo test -p rete-cli --test build_output_atomic -- --nocapture
```

- [ ] **Step 3: Implement section spooling and bounded compression**

```rust
pub(crate) struct SectionSpool {
    path: PathBuf,
    len: u64,
}

pub(crate) struct StreamingDatasetWriter {
    output: PathBuf,
    temp: BuildTemp,
    header: Header,
    hasher: blake3::Hasher,
}
```

Compress paired tiles through a budget-derived bounded batch, record only directories/synopses in memory, spill payloads in sequence, reserve `HEADER_LEN`, stream/copy each section while hashing, write footer, patch header, flush, reopen with `Rete::open_ranged_lazy`, and verify content hash before installation.

- [ ] **Step 4: Implement one safe cross-platform installation contract**

Add `tempfile = "3.20"` under a non-WASM target dependency and create the
destination-side image with `tempfile::NamedTempFile::new_in`. Install through
the crate's safe `persist` API after flush/validation. The Windows and Unix test
matrix must cover both absent and existing destinations. Installation must leave
either the old complete file or new complete file after every injected failure;
do not add platform FFI or any local unsafe block.

```toml
[target.'cfg(not(target_arch = "wasm32"))'.dependencies]
tempfile = "3.20"
```

```rust
let mut named = tempfile::NamedTempFile::new_in(destination_parent)?;
write_complete_image(named.as_file_mut(), parts)?;
named.as_file_mut().sync_all()?;
validate_completed_image(named.path())?;
OutputInstaller::persist(named, destination)?;
```

- [ ] **Step 5: Run writer, CLI atomicity, and diff tests**

```sh
docker compose run --rm dev cargo test -p rete-core build_pipeline::writer -- --nocapture
docker compose run --rm dev cargo test -p rete-cli --test build_output_atomic -- --nocapture
docker compose run --rm dev cargo test -p rete-core file::tests::content_hash_is_set_and_verifies -- --nocapture
```

- [ ] **Step 6: Commit streaming output**

```sh
git add crates/rete-core/src/build_pipeline crates/rete-core/src/file.rs crates/rete-core/Cargo.toml Cargo.lock crates/rete-cli/tests/build_output_atomic.rs
git commit -m "perf(build): stream validated dataset output"
```

### Task 9: Switch the Standard CLI Builder to Single-Pass Ingestion

**Files:**
- Modify: `crates/rete-core/src/build_pipeline/mod.rs`
- Modify: `crates/rete-core/src/ingest.rs`
- Modify: `crates/rete-cli/src/commands/build.rs`
- Test: `crates/rete-core/tests/build_pipeline.rs`
- Test: `crates/rete-cli/tests/build_pipeline_cli.rs`
- Modify: `scripts/smoke.sh`

**Interfaces:**
- Produces: `build_memory_to_path(stream, output, BuildOptions)`, CLI-private `stream_inputs_once`, `derive_requested_metadata`, and single-pass standard CLI behavior.
- Consumes: `MemoryIngest`, optional pyramid/text/card callbacks, family builder, streaming writer, and existing CLI options.

- [ ] **Step 1: Write failing single-pass and CLI correctness tests**

```rust
#[test]
fn replayable_file_is_opened_and_parsed_once() {
    let source = CountingSource::new(fixture_nq());
    build_memory_to_path(source.stream(), &out, options()).unwrap();
    assert_eq!(source.open_count(), 1);
    assert_query_corpus(&out);
}
```

The CLI integration test builds from file and stdin, with and without named graphs/card/types pyramid, then verifies header `0x06`, counts, exports, and deterministic hashes.

- [ ] **Step 2: Run focused tests and verify RED**

```sh
docker compose run --rm dev cargo test -p rete-core --test build_pipeline standard_ -- --nocapture
docker compose run --rm dev cargo test -p rete-cli --test build_pipeline_cli -- --nocapture
```

- [ ] **Step 3: Implement the shared standard orchestrator**

```rust
pub(crate) struct BuildOptions {
    pub with_pyramid: bool,
    pub with_text_index: bool,
    pub type_override: Option<String>,
    pub pyramid_algo: PyramidAlgo,
    pub tile_budget: usize,
}

pub(crate) fn build_memory_to_path<S>(
    mut stream: S,
    output: &Path,
    options: BuildOptions,
    metadata: impl FnOnce(&BuildStats, &Dictionary, CanonicalDatasetView<'_>) -> Vec<u8>,
) -> Result<BuildStats, BuildPipelineError>
where
    S: FnMut(&mut dyn FnMut(RawQuad) -> Result<(), BuildPipelineError>) -> Result<(), BuildPipelineError>;
```

The metadata callback receives the finalized dictionary and canonical dataset
view (default plus named graphs), not raw owned lexical quads, so card/text/
pyramid derivation does not force a second parse or reconstruct duplicate
strings:

```rust
pub(crate) struct CanonicalDatasetView<'a> {
    pub default_triples: &'a [Triple],
    pub named: &'a BTreeMap<String, Vec<Triple>>,
}

fn stream_inputs_once(
    inputs: &[(&str, &'static str)],
    visit: &mut dyn FnMut(RawQuad) -> Result<(), BuildPipelineError>,
) -> Result<(), BuildPipelineError>;
```

For file-backed N-Triples/N-Quads, adapt the existing callback-only
`stream_reader` and stop at the first visitor error. For Turtle/RDF/XML and
stdin, use the existing whole-text parser once, move each parsed quad into the
visitor, then drop the temporary vector. Preserve the current OWL conversion
hint and source-path error context.

Parse once into `MemoryIngest`, derive metadata before dropping required structures, build optional pyramid/text sections, construct families, stream output, then release all temporary state.

- [ ] **Step 4: Route CLI builds through the new orchestrator**

Keep reasoning/materialization semantics intact: compute inferred resident quads first, feed those once to the memory backend, and preserve coherence/card behavior. Remove the replay closure from the normal N-Triples/N-Quads path.

```rust
let stats = build_memory_to_path(
    |visit| stream_inputs_once(&inputs_fmt, visit),
    Path::new(output),
    options,
    move |stats, dict, triples| derive_requested_metadata(stats, dict, triples, curated),
)?;
let byte_len = usize::try_from(std::fs::metadata(output)?.len())?;
print_build_summary(output, &stats, byte_len);
```

- [ ] **Step 5: Update smoke coverage and run standard suites**

```sh
docker compose run --rm dev cargo test -p rete-cli --test build_pipeline_cli -- --nocapture
docker compose run --rm dev cargo test -p rete-core --test build_pipeline -- --nocapture
docker compose run --rm dev bash scripts/smoke.sh
```

- [ ] **Step 6: Commit standard builder cutover**

```sh
git add crates/rete-core/src/build_pipeline crates/rete-core/src/ingest.rs crates/rete-cli/src/commands/build.rs crates/rete-cli/tests/build_pipeline_cli.rs scripts/smoke.sh
git commit -m "perf(build): parse standard inputs once"
```

### Task 10: Switch the External Builder to Three Family Scans

**Files:**
- Modify: `crates/rete-core/src/extbuild.rs`
- Modify: `crates/rete-core/src/ingest.rs`
- Modify: `crates/rete-core/src/build_pipeline/family.rs`
- Modify: `crates/rete-core/src/build_pipeline/writer.rs`
- Modify: `crates/rete-cli/src/commands/build.rs`
- Modify: `crates/rete-cli/src/commands/merge.rs`
- Test: `crates/rete-core/src/extbuild.rs`
- Test: `crates/rete-core/tests/build_pipeline.rs`

**Interfaces:**
- Produces: `build_external_to_path(stream, output, ExternalBuildOptions)`, CLI-private `stream_external_inputs_once`, using one canonical spool, three scans, paired runs, and streaming output.
- Consumes: `ChunkedIngest`, file-backed `TripleSpool`, `IndexFamily`, memory budget, and existing external metadata callback.

- [ ] **Step 1: Write failing scan-count, budget, and equivalence tests**

```rust
#[test]
fn external_builder_replays_canonical_spool_three_times() {
    let probe = CountingSpool::new(skewed_triples());
    build_external_from_spool(&probe, &out, options_64m()).unwrap();
    assert_eq!(probe.full_scans(), 3);
}

#[test]
fn external_and_standard_v2_bytes_match_without_metadata() {
    assert_eq!(build_standard_no_pyramid(input()), build_external_64m(input()));
}
```

- [ ] **Step 2: Run focused tests and verify RED**

```sh
docker compose run --rm dev cargo test -p rete-core extbuild::tests::external_builder_replays -- --nocapture
docker compose run --rm dev cargo test -p rete-core --test build_pipeline external_ -- --nocapture
```

- [ ] **Step 3: Generate paired runs in one scan per family**

For each canonical spool block, normalize each triple into both family orders, fill two run buffers whose combined reserved bytes stay within the sort allocation, and spill both when either reaches its cap. Sort the pair with `rayon::join`, merge/deduplicate each order separately, and feed a paired streaming tiler that enforces common leading boundaries.

```rust
let (first_runs, second_runs) = spool_family_runs(
    spool,
    family,
    run_records_per_order(memory_budget),
    temp,
)?;
let paired = merge_family_runs(first_runs, second_runs, tile_budget, codec, temp)?;
```

- [ ] **Step 4: Remove six-permutation external loops and duplicate final assembly**

Delete the old `for perm in ALL_PERMS` path only after the three-family tests pass. Route CLI external build and `merge` through `build_external_to_path`; preserve stdin, temp-dir, error, card-count, and cleanup behavior.

```rust
let stats = build_external_to_path(
    |visit| stream_external_inputs_once(&inputs_fmt, visit),
    Path::new(output),
    ExternalBuildOptions {
        memory_budget: memory_budget_mb.saturating_mul(1 << 20),
        tmp_dir: tmp_dir.map(PathBuf::from),
        metadata,
    },
)?;
```

`stream_external_inputs_once` retains the current stdin exclusivity and explicit
format checks, streams every accepted file with the existing 1 MiB buffered
reader, and translates parser/I/O failures to `BuildPipelineError` with the
source path intact.

Switch `assemble_dataset_streaming_algo` from its pinned legacy writer to the
same `0x06` family writer in this step, so external byte-equivalence tests compare
two new-format paths. After this point every core assembly wrapper emits `0x06`.

- [ ] **Step 5: Run all external and CLI suites at three budgets**

```sh
docker compose run --rm dev cargo test -p rete-core extbuild::tests -- --nocapture
docker compose run --rm dev cargo test -p rete-core --test build_pipeline -- --nocapture
docker compose run --rm dev cargo test -p rete-cli --test build_pipeline_cli -- --nocapture
```

Add explicit 64, 256, and 1024 MiB test configurations where fixture size permits multiple chunks/runs; tests may scale record counts down while asserting calculated caps.

- [ ] **Step 6: Commit external cutover**

```sh
git add crates/rete-core/src/extbuild.rs crates/rete-core/src/ingest.rs crates/rete-core/src/build_pipeline crates/rete-cli/src/commands/build.rs crates/rete-cli/src/commands/merge.rs crates/rete-core/tests/build_pipeline.rs
git commit -m "perf(build): construct external indexes in three scans"
```

### Task 11: Complete the Deliberate Compatibility Break

**Files:**
- Delete: `crates/rete-core/tests/compatibility_v1.rs`
- Create: `crates/rete-core/tests/compatibility_v2.rs`
- Keep: `crates/rete-core/tests/fixtures/v1/minimal.rete` as a rejection fixture.
- Create: `crates/rete-core/tests/fixtures/v2/source.nq`
- Create: `crates/rete-core/tests/fixtures/v2/minimal.rete`
- Modify: `crates/rete-core/src/header.rs`
- Modify: `crates/rete-core/src/file.rs`
- Modify: `crates/rete-core/tests/public_api.rs`
- Modify: `scripts/check_format_versions.py`
- Modify: `scripts/refresh_local_retes.py`
- Test: `crates/rete-core/tests/compatibility_v2.rs`

**Interfaces:**
- Produces: `MIN_STABLE_READ_VERSION == 0x06`, committed stable `0x06` fixture, removal of the transitional `0x05` decode branch, and permanent `0x05` rejection contract.
- Consumes: final `0x06` CLI writer and `rete_core::format` facade.

- [ ] **Step 1: Write the replacement compatibility test**

```rust
#[test]
fn stable_reader_opens_v2_and_rejects_v1() {
    let v2 = include_bytes!("fixtures/v2/minimal.rete");
    assert_eq!(Header::from_bytes(v2).unwrap().version, 0x06);
    Rete::open(v2).unwrap();
    let v1 = include_bytes!("fixtures/v1/minimal.rete");
    assert!(matches!(Rete::open(v1), Err(FileError::Header(HeaderError::UnsupportedVersion { found: 0x05, min: 0x06, max: 0x06 }))));
}
```

- [ ] **Step 2: Generate the `0x06` fixture reproducibly**

```sh
docker compose run --rm dev cargo run -q -p rete-cli -- build crates/rete-core/tests/fixtures/v2/source.nq -o crates/rete-core/tests/fixtures/v2/minimal.rete --no-pyramid
```

Record source SHA-256 and fixture SHA-256 in comments in `compatibility_v2.rs`; assert the fixture header, export hash, counts, and representative query.

- [ ] **Step 3: Update version audit scripts**

`check_format_versions.py` reports `0x06` as readable and `0x05` as `LEGACY — rebuild required`. `refresh_local_retes.py` must not download `0x05` into a `0x06` worktree; it reports unavailable until a catalog entry declares a `0x06` object.

- [ ] **Step 4: Remove the transitional decoder and raise the minimum**

Set `MIN_STABLE_READ_VERSION` to `0x06`, delete the `0x05` branch from eager and
ranged openers, and retain the old fixture only as rejection evidence. Search
production Rust for `decode_index_container` and remove it when no non-test
caller remains.

```rust
pub const CURRENT_FORMAT_VERSION: u8 = 0x06;
pub const MIN_STABLE_READ_VERSION: u8 = 0x06;

fn decode_root_index(bytes: &[u8], codec: u8) -> Result<GraphIndex, FileError> {
    decode_index_families(bytes, codec)
}
```

- [ ] **Step 5: Run compatibility, ranged, and public API tests**

```sh
docker compose run --rm dev cargo test -p rete-core --test compatibility_v2 -- --nocapture
docker compose run --rm dev cargo test -p rete-core --test ranged -- --nocapture
docker compose run --rm dev cargo test -p rete-core --test public_api -- --nocapture
docker compose run --rm dev python scripts/check_format_versions.py --help
```

- [ ] **Step 6: Commit the stable baseline switch**

```sh
git add crates/rete-core/tests crates/rete-core/src/header.rs crates/rete-core/src/file.rs scripts/check_format_versions.py scripts/refresh_local_retes.py
git commit -m "feat(format): make 0x06 the sole stable generation"
```

### Task 12: WASM and Owned-Memory Build/Read Parity

**Files:**
- Modify: `crates/rete-wasm/src/lib.rs`
- Modify: `crates/rete-core/src/ingest.rs`
- Test: `crates/rete-wasm/src/lib.rs`
- Test: `crates/rete-wasm/tests/web_api.rs`

**Interfaces:**
- Produces: narrow `rete_core::ingest::MemoryBuild` and `stream_text` facades, WASM `build()` returning `0x06`, `Graph::new` opening `0x06`, and explicit `0x05` rejection.
- Consumes: byte-returning `Vec` writer sink, `MemoryIngest`, paired families, and `OwnedMemoryRangeReader`.

- [ ] **Step 1: Write failing WASM/native binding tests**

```rust
#[test]
fn wasm_build_emits_v2_and_owned_graph_queries_it() {
    let bytes = build(FIXTURE_NT, "nt").unwrap();
    assert_eq!(&bytes[0..5], b"RETE\x06");
    let graph = Graph::new(&bytes).unwrap();
    let json = graph.query("ASK { <a> <p> <b> }", "json").unwrap();
    assert!(json.contains("\"boolean\":true"));
}
```

- [ ] **Step 2: Run rete-wasm tests and verify RED**

```sh
docker compose run --rm dev cargo test -p rete-wasm -- --nocapture
```

- [ ] **Step 3: Route WASM build through the memory pipeline's Vec sink**

Keep all clocks no-op on `wasm32-unknown-unknown`, avoid native-only dependencies in default features, preserve JS error translation, and update comments that mention old eager/two-pass behavior.

```rust
#[wasm_bindgen]
pub fn build(text: &str, format: &str) -> Result<Vec<u8>, JsValue> {
    let mut ingest = rete_core::ingest::MemoryBuild::new();
    rete_core::ingest::stream_text(text, format, &mut |quad| ingest.push(quad))
        .map_err(err)?;
    if ingest.is_empty() {
        return Err(js_error("no statements parsed (empty input or only comments)"));
    }
    ingest.finish_to_vec(Default::default(), |_| Vec::new()).map_err(err)
}
```

Expose the narrow `MemoryBuild` facade from `rete_core::ingest`; keep the
lower-level `build_pipeline` module crate-private.

- [ ] **Step 4: Add a native binding regression for old-format rejection**

Clone the committed `0x05` fixture, call `Graph::new`, and assert the returned
error contains `unsupported .rete format 0x05` and `0x06`.

- [ ] **Step 5: Run native WASM tests**

```sh
docker compose run --rm dev cargo test -p rete-wasm
```

- [ ] **Step 6: Commit WASM parity**

```sh
git add crates/rete-wasm/src/lib.rs crates/rete-wasm/tests crates/rete-core/src/ingest.rs
git commit -m "feat(wasm): build and open format 0x06"
```

### Task 13: Measure, Tune, and Enforce the Acceptance Gates

**Files:**
- Modify: `crates/rete-core/src/build_pipeline/family.rs`
- Modify: `crates/rete-core/src/build_pipeline/spool.rs`
- Modify: `crates/rete-core/src/build_pipeline/writer.rs`
- Modify: `crates/rete-core/src/build_pipeline/timing.rs`
- Modify: `crates/bench/src/buildmem.rs`
- Modify: `scripts/bench_build_pipeline.py`
- Modify: `scripts/test_bench_build_pipeline.py`
- Modify: `docs/BENCHMARK.md`
- Regenerate: `docs/BENCHMARK.html`

**Interfaces:**
- Produces: immutable raw benchmark evidence, exact counting-allocator phase-heap profiles, accepted queue/run thresholds, and documented results.
- Consumes: baseline commit `483c431cc6f0df38c42d9d0b7a215d29187d56b1`, baseline/candidate CLI and `rete-bench` executables, pinned workloads, and query corpus.

- [ ] **Step 1: Build immutable baseline and candidate executables**

Build the baseline in a separate temporary worktree or archive checkout and copy
its CLI and profiler to `target/bench/rete-build-baseline-483c431c` and
`target/bench/rete-buildmem-baseline-483c431c`. Build the candidate once and copy
the corresponding executables to `target/bench/rete-build-candidate` and
`target/bench/rete-buildmem-candidate`. The harness records the full git SHA and
all executable SHA-256 values before sampling; never benchmark a mutable
`target/release` binary while recompiling it.

- [ ] **Step 2: Run correctness discovery before timing**

For every workload, build once with both executables, verify input SHA, canonical
export hash, counts, card fields, and query hashes, and verify candidate output
byte stability with `RAYON_NUM_THREADS` set to 1, 2, and the exact integer
returned by `std::thread::available_parallelism`, plus external budgets
64/256/1024 MiB.

Promote the accepted deterministic query corpus and hashes from discovery into
every workload JSON before timing. The strict loader must then reject an empty
`queries` array for non-overhead workloads, so no timed large-build record can
omit the read-path correctness gate.

The accepted corpus contains all eight bound/unbound triple-pattern shapes,
one merge join, aggregate, property path, named-graph query where supported,
SHACL validation, and reachability. Each SPARQL case runs locally through
`sparql` and range-backed through `sparql-url` against the strict loopback
server. Hash canonical result bytes; queries without semantic ordering include
an explicit complete `ORDER BY` or use canonical row sorting before hashing.

Extend `rete-bench --build-mem` on the candidate to drive the same
`build_memory_to_path` stages while retaining the benchmark crate's existing
counting global allocator. Emit a `BUILD_HEAP` JSON record per phase with live
and high-water bytes. The harness parses the immutable baseline profiler's
existing fixed table and the candidate JSON records; fixture tests pin both
parsers. Run both profiler binaries on the same pinned no-pyramid and typed
inputs and require the candidate's maximum phase heap to be at most 75% of the
baseline. External configurations use process peak RSS as their bounded-memory
gate because the baseline profiler has no external-builder phase driver.

- [ ] **Step 3: Run the complete alternating benchmark matrix**

```sh
docker compose run --rm dev uv run python scripts/bench_build_pipeline.py \
  --baseline /work/target/bench/rete-build-baseline-483c431c \
  --candidate /work/target/bench/rete-build-candidate \
  --workload scripts/build-workloads/chemotion.json \
  --input-root /work/target/bench/inputs \
  --samples 15 \
  --output /work/target/bench/build-pipeline-chemotion-v2.jsonl
```

Run the same command shape for the remaining workload files and write exactly
`build-pipeline-small-v2.jsonl`, `build-pipeline-synthetic-large-v2.jsonl`,
`build-pipeline-synthetic-louvain-v2.jsonl`, and
`build-pipeline-synthetic-external-v2.jsonl`; the external invocation includes
`--external-budgets 64,256,1024`. Preserve these exact evidence filenames and
their SHA-256 values in `BENCHMARK.md`.

- [ ] **Step 4: Tune only measured constants behind existing interfaces**

Candidate constants include radix comparison cutoff, spool block records, paired run records, compression queue bytes, and ordinary/external family concurrency. For each change, rerun the affected focused workload and retain it only when output/query hashes remain stable and the relevant median/RSS improves. Do not weaken bounds or add unsafe code.

- [ ] **Step 5: Add an automated result-gate test**

```python
def assert_primary_gates(summary):
    assert summary["candidate"]["wallMsMedian"] <= summary["baseline"]["wallMsMedian"] * 0.667
    assert summary["candidate"]["peakRssKiBMedian"] <= summary["baseline"]["peakRssKiBMedian"] * 0.75
    assert summary["candidate"]["phaseHeapBytesMax"] <= summary["baseline"]["phaseHeapBytesMax"] * 0.75
    assert summary["candidate"]["outputBytes"] <= summary["baseline"]["outputBytes"] * 1.10
    assert summary["candidate"]["localQueryMedianMs"] <= summary["baseline"]["localQueryMedianMs"] * 1.05
    assert summary["candidate"]["localQueryP90Ms"] <= summary["baseline"]["localQueryP90Ms"] * 1.05
    assert summary["candidate"]["rangeQueryMedianMs"] <= summary["baseline"]["rangeQueryMedianMs"] * 1.05
    assert summary["candidate"]["rangeQueryP90Ms"] <= summary["baseline"]["rangeQueryP90Ms"] * 1.05

def assert_small_overhead_gates(summary):
    assert summary["candidate"]["wallMsMedian"] <= summary["baseline"]["wallMsMedian"] * 1.20
    assert summary["candidate"]["peakRssKiBMedian"] <= summary["baseline"]["peakRssKiBMedian"] * 1.25

def assert_louvain_no_regression(summary):
    assert summary["candidate"]["wallMsMedian"] <= summary["baseline"]["wallMsMedian"] * 1.05
    for phase in summary["baseline"]["nonLouvainPhases"]:
        assert summary["candidate"]["phaseMs"][phase] <= summary["baseline"]["phaseMs"][phase] * 1.05
```

Louvain records carry `gateClass: "louvain-no-regression"`; small records carry
`gateClass: "small-overhead"`; the strict dispatcher rejects unknown gate
classes. The external production-budget summary uses the primary time/RSS gate
without a phase-heap field, while every tested external budget must retain
stable correctness/output hashes and remain within its calculated buffer caps.
Every accepted external sample must also report `spillBytes > 0` and each S/P/O
family run count greater than one; otherwise the supposedly spill-forcing
workload is invalid and the benchmark stops instead of publishing a vacuous
budget comparison.

- [ ] **Step 6: Regenerate benchmark HTML reproducibly**

```sh
docker compose run --rm dev cargo run -q -p docgen
docker compose run --rm dev cargo run -q -p docgen
git diff --exit-code -- docs/BENCHMARK.html
```

- [ ] **Step 7: Commit measured tuning and evidence documentation**

```sh
git add crates/rete-core/src/build_pipeline crates/bench/src/buildmem.rs scripts/bench_build_pipeline.py scripts/test_bench_build_pipeline.py docs/BENCHMARK.md docs/BENCHMARK.html
git commit -m "perf(build): meet unified pipeline gates"
```

### Task 14: Catalog Availability, Documentation, and Generated Artifacts

**Files:**
- Modify: `web/playground-src/catalog.js`
- Modify: `web/playground-src/app.js`
- Modify: `tests/gate/checks/catalog_matrix.mjs`
- Modify: `tests/gate/checks/test_catalog_matrix.mjs`
- Modify: `tests/gate/checks/check_builder.mjs`
- Modify: `web/datasets.lock.json` only through the existing single-key/merge-safe tooling.
- Modify: `docs/cli.md`
- Modify: `docs/browser.md`
- Modify: `docs/SPEC.md`
- Modify: `README.md`
- Regenerate: corresponding `docs/*.html`
- Regenerate: tracked fixtures and browser artifacts through canonical scripts.

**Interfaces:**
- Produces: explicit catalog `formatVersion`/availability state, disabled incompatible choices, accurate migration documentation, and all tracked `0x06` artifacts.
- Consumes: `CURRENT_FORMAT_VERSION == 0x06`, catalog entries, build scripts, docgen, preview generator, and browser gate.

- [ ] **Step 1: Write failing catalog availability tests**

```javascript
assert.equal(datasetAvailability({ formatVersion: 6 }, 6).available, true);
assert.equal(datasetAvailability({ formatVersion: 5 }, 6).reason, "requires a format 0x06 rebuild");
assert.equal(datasetAvailability({ embedded: true, formatVersion: 6 }, 6).available, true);
```

Assert unavailable datasets cannot start a fetch, their examples are disabled, and the UI shows the exact required generation instead of a network error.
Extend `check_builder.mjs` to build the minimal tracked N-Triples source in the
fresh playground, assert byte 4 is `0x06`, open it, run ASK plus ordered SELECT,
and verify a copied header with byte 4 changed to `0x05` rejects cleanly.

- [ ] **Step 2: Implement explicit version state without probing old payloads**

Add `formatVersion: 6` to rebuilt embedded/tracked entries. Mark current unrepublished R2 objects as `formatVersion: 5`; derive availability against the engine constant injected by the build. Do not remove URLs or mutate R2.

```javascript
export function datasetAvailability(dataset, engineFormatVersion) {
  const found = Number(dataset.formatVersion || 0);
  return found === engineFormatVersion
    ? { available: true, reason: "" }
    : {
        available: false,
        reason: `requires a format 0x${engineFormatVersion.toString(16).padStart(2, "0")} rebuild`,
      };
}
```

- [ ] **Step 3: Update source documentation and compatibility claims**

Document single-pass standard builds, three-scan external builds, streamed installation, `0x06`-only readers/writers, benchmark caveats, unavailable legacy catalog objects, and the separate migration authorization. Remove promises that future readers open `0x05`.

- [ ] **Step 4: Rebuild only tracked/local datasets from tracked recipes**

Use `tests/gate/fixtures.sh` for gate fixtures and `scripts/build_wasm.sh` for all browser artifacts. Do not hand-run `wasm-pack`, build ad-hoc fixtures, or download `0x05` R2 objects as replacements.

```sh
docker compose run --rm dev bash tests/gate/fixtures.sh
docker compose run --rm -e RETE_SOURCE_REVISION=$(git rev-parse HEAD) wasm
```

- [ ] **Step 5: Regenerate docs and previews**

```sh
docker compose run --rm dev cargo run -q -p docgen
docker compose run --rm dev bash scripts/preview/run.sh build
```

Run the preview capture command only for entries whose tracked query answers changed; unavailable status cards must not claim a live answer.

- [ ] **Step 6: Run focused catalog and generated-file gates**

```sh
docker compose run --rm gate node checks/test_catalog_matrix.mjs
docker compose run --rm gate node checks/check_social_previews.mjs
git diff --check
```

- [ ] **Step 7: Commit documentation and generated artifacts separately**

```sh
git add web docs README.md tests/gate scripts/check_format_versions.py scripts/refresh_local_retes.py
git commit -m "docs(format): publish the 0x06 compatibility boundary"
```

### Task 15: Final Verification and Independent Review

**Files:**
- Modify only files required by concrete review findings.
- Do not modify the approved spec or performance evidence to hide a missed gate.

**Interfaces:**
- Produces: clean branch, complete verification transcript, reviewed diff, and no remote mutations.
- Consumes: every prior task and repository gate command.

- [ ] **Step 1: Run formatting and static analysis**

```sh
docker compose run --rm dev cargo fmt --all -- --check
docker compose run --rm dev cargo clippy --workspace --exclude rete-bench --all-targets -- -D warnings
git diff --check
```

- [ ] **Step 2: Run the complete Rust matrix**

```sh
docker compose run --rm dev cargo test --workspace --exclude rete-bench
docker compose run --rm dev cargo test -p rete-core --no-default-features
docker compose run --rm dev cargo build -p rete-core --all-features
docker compose run --rm dev cargo build -p rete-bench
docker compose run --rm dev bash scripts/smoke.sh
```

- [ ] **Step 3: Run canonical WASM and browser gates**

```sh
docker compose run --rm -e RETE_SOURCE_REVISION=$(git rev-parse HEAD) wasm
bash tests/gate/gate.sh
```

Expected: every supported embedded/tracked `0x06` dataset passes; catalog `0x05` entries are visibly unavailable and generate no data GET.

- [ ] **Step 4: Re-run the accepted benchmark gate against immutable binaries**

Use the exact evidence commands and workload pins from Task 13. Verify every evidence SHA, input SHA, executable SHA, output hash, query hash, and threshold. Do not substitute cached summaries for raw JSONL recomputation.

- [ ] **Step 5: Audit safety and format scope**

```sh
git diff 483c431cc6f0df38c42d9d0b7a215d29187d56b1...HEAD -- '*.rs' | rg '^\+.*\bunsafe\b' || true
rg -n '0x05|MIN_STABLE_READ_VERSION|CURRENT_FORMAT_VERSION' crates docs scripts web tests
```

Expected: no new production unsafe; remaining `0x05` references describe deliberate rejection, legacy fixtures, or migration.

- [ ] **Step 6: Request independent code and specification review**

Reviewers must separately check: untrusted-byte bounds; atomic replacement; memory-budget arithmetic; family routing/query parity; `0x05` rejection; WASM/native parity; benchmark reproducibility; catalog unavailability behavior; and absence of R2 mutations. Reproduce every Important or Critical finding with a focused failing test before remediation.

- [ ] **Step 7: Remediate findings through TDD and rerun affected/full gates**

For each accepted finding: add the smallest failing test, capture RED, implement the minimal fix, capture GREEN, run the owning package, then rerun the complete matrix after the final fix.

- [ ] **Step 8: Commit final remediations and record branch state**

Stage only the exact paths named by accepted review findings, never `git add -A`
or an unrelated dirty file, then commit with message
`fix(build): remediate unified pipeline review`. Finish with:

```sh
git status --short --branch
git log --oneline --decorate -15
```

If review finds nothing to change, do not create an empty commit. Report exact final commit, benchmark evidence hashes, test totals, catalog availability counts, and the explicit fact that R2 was not mutated.
