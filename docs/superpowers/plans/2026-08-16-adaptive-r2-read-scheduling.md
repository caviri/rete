# Adaptive R2 Read Scheduling Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add a session-local, query-aware adaptive scheduler that lowers cold R2 SPARQL latency for existing `.rete` files by tuning known-range coalescing, cache-span batching, concurrency, and scan prefetch without changing query semantics or file bytes.

**Architecture:** A pure `AdaptiveReadController` in `rete-core` owns fixed-point network and consumption estimates. `BlockCacheReader` times only physical backing reads and exposes the same controller through `RangeReader`; tile/dictionary loaders ask it for an intent-specific `ReadPlan` and report consumption. Native HTTP and browser remote-lazy openers enable the controller with platform clocks, while local/owned-memory and precise metadata paths retain static behavior.

**Tech Stack:** Rust 2021, `rete-core`, `rete-cli`, `rete-wasm`, `RangeReader`, `BlockCacheReader`, wasm-bindgen/js-sys, Python benchmark harnesses, Node/Playwright browser gates, Docker Compose dev/wasm/gate services.

## Global Constraints

- Existing `.rete` files must work byte-for-byte; do not change `docs/SPEC.md`, the header, section layout, or codec framing.
- The controller is per physical source and per open session; do not persist profiles or upload telemetry.
- Never adapt or widen `read_at_precise`; named-graph open must still transfer/decompress zero named tile payload bytes.
- Do not add `unsafe`; the existing `unsafe-decode-bench` feature remains isolated and non-default.
- Keep the native at-or-below-8-MiB one-transfer policy unchanged; only larger native HTTP objects and browser remote-lazy objects enable adaptation.
- Keep the cache residency cap at 256 MiB.
- Adaptive spans are at most 2 MiB; gap-only overfetch is at most `min(known_bytes / 4, 256 KiB)`; concurrency is at most `RangeReader::concurrency()`.
- Failed/short/overlong/malformed reads never populate caches or train throughput; retries retain existing incomplete/reset behavior.
- Query-triggered zstd output remains uncapped; do not claim global decompression hardening.
- All Rust commands run in Docker from the worktree. Browser production artifacts are built with `docker compose run --rm wasm`, not bare optimized `wasm-pack`.
- Commit without a `Co-Authored-By` trailer.

---

### Task 1: Pure adaptive policy controller

**Files:**
- Create: `crates/rete-core/src/adaptive.rs`
- Modify: `crates/rete-core/src/lib.rs`
- Test: `crates/rete-core/src/adaptive.rs`

**Interfaces:**
- Produces:
  - `pub enum ReadIntent { SelectiveProbe, BoundedScan, FullScan, DictionaryResolve }`
  - `pub struct ReadObservation { pub requested_bytes: u64, pub returned_bytes: u64, pub physical_ranges: usize, pub elapsed_micros: Option<u64>, pub success: bool }`
  - `pub struct ReadPlan { pub coalesce_gap: u64, pub max_span: u64, pub prefetch_start: usize, pub prefetch_cap: usize, pub max_in_flight: usize }`
  - `pub struct AdaptiveReadController`
  - `AdaptiveReadController::new() -> Self`
  - `AdaptiveReadController::plan(&self, intent: ReadIntent, known_bytes: u64, static_gap: u64, concurrency: usize) -> ReadPlan`
  - `AdaptiveReadController::observe(&self, observation: ReadObservation)`
  - `AdaptiveReadController::report_consumption(&self, intent: ReadIntent, consumed: usize, offered: usize)`
  - `AdaptiveReadController::successful_samples(&self) -> u32`
- Consumes: no reader or HTTP types; this module is a pure policy/state unit.

- [ ] **Step 1: Write failing cold-start and network-tier tests**

Add focused tests that express the static fallback and two-sample warm-up:

```rust
#[test]
fn cold_start_is_the_current_static_policy() {
    let c = AdaptiveReadController::new();
    let p = c.plan(ReadIntent::SelectiveProbe, 256 * 1024, 4096, 8);
    assert_eq!(p.coalesce_gap, 4096);
    assert_eq!(p.prefetch_start, 4);
    assert_eq!(p.prefetch_cap, 512);
    assert_eq!(p.max_in_flight, 8);
}

#[test]
fn high_rtt_fast_link_merges_more_after_two_samples() {
    let c = AdaptiveReadController::new();
    for _ in 0..2 {
        c.observe(ReadObservation {
            requested_bytes: 1024 * 1024,
            returned_bytes: 1024 * 1024,
            physical_ranges: 1,
            elapsed_micros: Some(120_000),
            success: true,
        });
    }
    let p = c.plan(ReadIntent::SelectiveProbe, 1024 * 1024, 4096, 8);
    assert!(p.coalesce_gap > 4096);
    assert!(p.coalesce_gap <= 256 * 1024);
}
```

- [ ] **Step 2: Run the focused test and verify RED**

Run:

```sh
docker compose run --rm dev cargo test -p rete-core adaptive::tests -- --nocapture
```

Expected: compile failure because `adaptive` and its types do not exist.

- [ ] **Step 3: Add failing cap, hysteresis, failure, and consumption tests**

Cover these exact invariants:

```rust
fn observe(c: &AdaptiveReadController, bytes: u64, micros: u64, success: bool) {
    c.observe(ReadObservation {
        requested_bytes: bytes,
        returned_bytes: if success { bytes } else { 0 },
        physical_ranges: 1,
        elapsed_micros: Some(micros),
        success,
    });
}

fn trained_high_rtt_controller() -> AdaptiveReadController {
    let c = AdaptiveReadController::new();
    observe(&c, 1024 * 1024, 120_000, true);
    observe(&c, 1024 * 1024, 120_000, true);
    c
}

#[test]
fn plan_never_exceeds_hard_limits() {
    let c = trained_high_rtt_controller();
    let p = c.plan(ReadIntent::FullScan, u64::MAX, u64::MAX, usize::MAX);
    assert!(p.max_span <= 2 * 1024 * 1024);
    assert!(p.coalesce_gap <= 256 * 1024);
    assert!(p.max_in_flight <= 16);
}

#[test]
fn one_outlier_does_not_change_tier() {
    let c = AdaptiveReadController::new();
    observe(&c, 64 * 1024, 20_000, true);
    observe(&c, 64 * 1024, 20_000, true);
    let before = c.plan(ReadIntent::BoundedScan, 512 * 1024, 4096, 8);
    observe(&c, 1024 * 1024, 500_000, true);
    let after = c.plan(ReadIntent::BoundedScan, 512 * 1024, 4096, 8);
    assert_eq!(after.prefetch_start, before.prefetch_start);
    assert_eq!(after.prefetch_cap, before.prefetch_cap);
}

#[test]
fn failed_sample_does_not_train_throughput_and_shrinks_aggression() {
    let c = trained_high_rtt_controller();
    let before = c.plan(ReadIntent::SelectiveProbe, 1024 * 1024, 4096, 8);
    let samples = c.successful_samples();
    observe(&c, 1024 * 1024, 1_000_000, false);
    let after = c.plan(ReadIntent::SelectiveProbe, 1024 * 1024, 4096, 8);
    assert_eq!(c.successful_samples(), samples);
    assert!(after.coalesce_gap <= before.coalesce_gap);
}

#[test]
fn low_bounded_scan_consumption_shrinks_next_window() {
    let c = trained_high_rtt_controller();
    let before = c.plan(ReadIntent::BoundedScan, 512 * 1024, 4096, 8);
    c.report_consumption(ReadIntent::BoundedScan, 1, before.prefetch_start);
    let after = c.plan(ReadIntent::BoundedScan, 512 * 1024, 4096, 8);
    assert!(after.prefetch_start <= before.prefetch_start);
}
```

- [ ] **Step 4: Implement the minimal deterministic controller**

Use a `Mutex<ControllerState>` with integer/fixed-point EWMAs. Two valid timed
samples are required before adaptation. Compute the break-even byte gap from
`throughput_bytes_per_second * latency_micros / 1_000_000`, apply a 3/4 safety
factor, then clamp it to `known_bytes / 4` and 256 KiB. Keep the static gap and
`4..512` scan window during cold start. Use two consecutive tier votes for
hysteresis; invalid/overflowing observations are ignored. Recover a poisoned
mutex with `into_inner()` and return the static plan if state cannot be used.

- [ ] **Step 5: Export the policy types without widening the stable query API**

Add `#[doc(hidden)] pub mod adaptive;` and hidden root re-exports in `lib.rs`.
The `range` facade may export the four policy types because `RangeReader` will
refer to them in Task 2; do not add CLI-facing configuration.

- [ ] **Step 6: Run focused and no-default tests**

Run:

```sh
docker compose run --rm dev cargo test -p rete-core adaptive::tests -- --nocapture
docker compose run --rm dev cargo test -p rete-core --no-default-features adaptive::tests -- --nocapture
docker compose run --rm dev cargo fmt --all -- --check
```

Expected: all adaptive tests pass and both feature modes compile.

- [ ] **Step 7: Commit**

```sh
git add crates/rete-core/src/adaptive.rs crates/rete-core/src/lib.rs
git commit -m "feat(core): add adaptive range policy"
```

### Task 2: Reader seam and physically timed adaptive block cache

**Files:**
- Modify: `crates/rete-core/src/reader.rs`
- Modify: `crates/rete-core/src/block_cache.rs`
- Test: `crates/rete-core/src/reader.rs`
- Test: `crates/rete-core/src/block_cache.rs`

**Interfaces:**
- Consumes: `AdaptiveReadController`, `ReadIntent`, `ReadObservation`, `ReadPlan` from Task 1.
- Produces:
  - `RangeReader::read_many_with_intent(&self, ranges: &[(u64, u64)], intent: ReadIntent) -> io::Result<Vec<Vec<u8>>>` with a default delegate to `read_many`;
  - `RangeReader::adaptive_controller(&self) -> Option<Arc<AdaptiveReadController>>` with default `None`;
  - forwarding implementations for `Arc<R>` and `CountingReader<R>`;
  - `BlockCacheReader::with_adaptive_clock<F>(self, clock: F) -> Self where F: Fn() -> Option<u64> + Send + Sync + 'static`;
  - `BlockCacheReader::adaptive_controller(&self) -> Option<Arc<AdaptiveReadController>>` for deterministic tests and loader sharing.

- [ ] **Step 1: Write failing forwarding tests in `reader.rs`**

Create a distinguishing reader whose intent method increments an atomic and whose
controller is an `Arc`. Assert `Arc<CountingReader<R>>` forwards both the intent
call and the identical controller pointer.

- [ ] **Step 2: Write a failing deterministic-clock block-cache test**

Use a fake physical reader plus an `AtomicU64` clock:

```rust
let now = Arc::new(AtomicU64::new(0));
let tick = now.clone();
let cache = BlockCacheReader::new(physical.clone(), 4096)
    .with_adaptive_clock(move || Some(tick.fetch_add(100_000, SeqCst)));
cache.read_many_with_intent(&[(0, 8), (16 * 4096, 8)], ReadIntent::SelectiveProbe)?;
assert!(cache.adaptive_controller().unwrap().successful_samples() >= 1);
```

Also assert an ordinary `BlockCacheReader::new` exposes no controller and retains
the pre-change request plan.

- [ ] **Step 3: Run focused tests and verify RED**

Run:

```sh
docker compose run --rm dev cargo test -p rete-core reader::tests block_cache::tests -- --nocapture
```

Expected: compile failures for the new trait methods and builder.

- [ ] **Step 4: Add the trait defaults and forwarding implementations**

`read_many_with_intent` must be correctness-equivalent to `read_many` for readers
that do not opt in. `CountingReader` must count constituent returned ranges once,
not count both its intent method and delegated `read_many`. `read_at_precise`
remains unchanged.

- [ ] **Step 5: Add optional controller/clock state to `BlockCacheReader`**

Store:

```rust
controller: Option<Arc<AdaptiveReadController>>,
clock: Option<Arc<dyn Fn() -> Option<u64> + Send + Sync>>,
```

`with_adaptive_clock` creates one controller and records start/end clock values
around each call to the *inner* `read_many`/fallback `read_at`. Report a success
only after exact-length validation; report failure on any transport/count/length
error. Do not time cache hits and do not observe `read_at_precise`.

- [ ] **Step 6: Split physical runs by the controller's `max_span`**

Change `ensure` to accept `ReadIntent`. Split a consecutive missing-block run
before it exceeds `ReadPlan::max_span`; then issue chunks of spans no wider than
`ReadPlan::max_in_flight`. Cache insertion stays all-or-nothing for each returned
batch and keeps shared-backing accounting unchanged.

- [ ] **Step 7: Add failure and precise-boundary regressions**

Assert:

- short/overlong physical reads do not advance successful sample count;
- a failed batch leaves every requested block absent and retries later;
- `read_at_precise` produces exactly one exact underlying read, no cache bytes,
  and no adaptive sample;
- cache bytes remain at or below the configured cap after every completed read;
- no planned physical span exceeds 2 MiB.

- [ ] **Step 8: Run focused, ranged, and clippy gates**

```sh
docker compose run --rm dev cargo test -p rete-core block_cache::tests reader::tests -- --nocapture
docker compose run --rm dev cargo test -p rete-core --test ranged -- --nocapture
docker compose run --rm dev cargo clippy -p rete-core --all-targets -- -D warnings
```

- [ ] **Step 9: Commit**

```sh
git add crates/rete-core/src/reader.rs crates/rete-core/src/block_cache.rs
git commit -m "perf(core): time and batch adaptive cache reads"
```

### Task 3: Intent-aware tile and dictionary coalescing

**Files:**
- Modify: `crates/rete-core/src/file.rs`
- Modify: `crates/rete-core/src/index.rs`
- Modify: `crates/rete-core/src/dict.rs`
- Test: `crates/rete-core/src/file.rs`
- Test: `crates/rete-core/src/index.rs`
- Test: `crates/rete-core/src/dict.rs`

**Interfaces:**
- Consumes: Task 2 `RangeReader::read_many_with_intent` and shared controller.
- Produces:
  - `read_coalesced(reader, ranges, static_gap, intent)` with adaptive gap/span/batch limits;
  - `TileBulkLoader = Box<dyn Fn(usize, &[usize], ReadIntent) -> Option<Vec<Vec<u8>>> + Send + Sync>`;
  - `ChunkBulkLoader = Box<dyn Fn(&[usize], ReadIntent) -> Option<Vec<Vec<u8>>> + Send + Sync>`;
  - `GraphIndex` and `ChunkedSection` calls that label selective, bounded/full scan, and dictionary batches.

- [ ] **Step 1: Write failing coalescing-oracle tests**

Train a controller with synthetic high-RTT samples, pass sorted disjoint ranges
through `read_coalesced`, and assert nearby known-needed ranges merge. Train a
low-RTT/slow controller and assert the same gaps remain separate. In both cases:

```rust
assert!(gap_only_bytes <= (known_bytes / 4).min(256 * 1024));
assert!(physical_spans.iter().all(|(_, len)| *len <= 2 * 1024 * 1024));
```

Retain the existing fixed-gap test using a reader with no controller and assert
its ranges are byte-identical to the pre-change plan.

- [ ] **Step 2: Run focused tests and verify RED**

```sh
docker compose run --rm dev cargo test -p rete-core file::tests::adaptive_coalescing -- --nocapture
```

Expected: compile failure because `read_coalesced` has no intent/controller path.

- [ ] **Step 3: Implement adaptive coalescing over already-eligible ranges**

Compute `known_bytes` with checked/saturating addition, ask the shared controller
for a plan, and merge only while cumulative gap-only bytes and span length remain
within the plan. Fetch span groups through `read_many_with_intent`; reconstruct
the original range outputs exactly as today. Never sort or invent ranges inside
the helper: reject/return `None` for overlap or overflow as the current callers
already provide ascending disjoint input.

- [ ] **Step 4: Thread `ReadIntent` through tile bulk loads**

- `prefetch_probe_tiles` calls `bulk_fault(..., ReadIntent::SelectiveProbe)`.
- scan prefetch calls `ReadIntent::BoundedScan` unless the caller explicitly uses
  the full-scan path.
- `triple_count`, eager `match_pattern`, dump/export sweeps, and explicit whole
  permutation prefetch call `ReadIntent::FullScan`.
- single-tile loader behavior stays exact and retryable.

- [ ] **Step 5: Thread intent through dictionary bulk loads**

- `prefetch_chunks` uses `DictionaryResolve`;
- `prefetch_all` uses `FullScan`;
- individual chunk lookup remains a single exact lazy load;
- update `file.rs` bulk closures to pass the intent into adaptive coalescing.

- [ ] **Step 6: Add default/named/dictionary sharing tests**

Open a multi-tile file with named graphs through one adaptive block cache. Assert
the default index, named indexes, and dictionary bulk paths advance the same
controller's observations, while lazy open still has zero reads overlapping any
named tile payload.

- [ ] **Step 7: Run core matrices**

```sh
docker compose run --rm dev cargo test -p rete-core
docker compose run --rm dev cargo test -p rete-core --no-default-features
docker compose run --rm dev cargo build -p rete-core --all-features
docker compose run --rm dev cargo clippy -p rete-core --all-features --all-targets -- -D warnings
```

- [ ] **Step 8: Commit**

```sh
git add crates/rete-core/src/file.rs crates/rete-core/src/index.rs crates/rete-core/src/dict.rs
git commit -m "perf(core): adapt tile and dictionary range batches"
```

### Task 4: Consumption-aware scan windows and streaming limits

**Files:**
- Modify: `crates/rete-core/src/index.rs`
- Modify: `crates/rete-core/src/bgp.rs`
- Modify: `crates/rete-core/src/sparql/eval.rs`
- Test: `crates/rete-core/src/index.rs`
- Test: `crates/rete-core/src/sparql/eval.rs`
- Test: `crates/rete-core/tests/ranged.rs`

**Interfaces:**
- Consumes: shared `AdaptiveReadController` exposed by the remote reader.
- Produces:
  - an internal scan-feedback guard reporting `(consumed_tiles, offered_tiles)` on iterator drop;
  - explicit bounded/full scan intent selection;
  - adaptive `prefetch_start`/`prefetch_cap` while retaining `4..512` static behavior.

- [ ] **Step 1: Write failing bounded-limit tests**

Build a many-tile remote fixture and compare physical reads for:

```sparql
SELECT ?s ?p ?o WHERE { ?s ?p ?o } LIMIT 1
```

versus a full unbound scan. Assert the bounded query never begins a second
window after producing its row, reports low consumption, and returns the same row
as eager/static evaluation.

- [ ] **Step 2: Write failing full-consumption tests**

Feed two fully consumed scan windows and assert the next plan grows, remains at
or below 512 tiles, and uses `FullScan` for blocking aggregate/`ORDER BY` paths.
Add an `ORDER BY ... LIMIT` regression proving it is not mislabeled streaming.

- [ ] **Step 3: Run focused tests and verify RED**

```sh
docker compose run --rm dev cargo test -p rete-core index::tests::adaptive_scan sparql::eval::tests::adaptive_scan -- --nocapture
```

- [ ] **Step 4: Attach the shared controller to remote `GraphIndex` objects**

During `open_remote_graph_index`, obtain `reader.adaptive_controller()` and store
it in the remote index. Local indexes keep `None`. The controller pointer must be
shared, not copied into per-index state.

- [ ] **Step 5: Replace the fixed scan cells with a feedback guard**

The iterator begins with the plan's `prefetch_start`, doubles only after consuming
the previous offered window, clamps to `prefetch_cap`, and reports its final
consumption when dropped. The guard must not borrow the iterator or reader and
must be safe to drop during error/early-limit unwinding.

- [ ] **Step 6: Label query paths without changing semantics**

Use `Ctx::limit_hint` only for genuinely streaming limits. Aggregation,
`DISTINCT`, `ORDER BY`, and other blocking modifiers remain full consumers.
Do not start background work or add cancellation threads; iterator demand remains
the only trigger for prefetch.

- [ ] **Step 7: Run SPARQL/ranged parity**

```sh
docker compose run --rm dev cargo test -p rete-core --test ranged -- --nocapture
docker compose run --rm dev cargo test -p rete-core sparql -- --nocapture
docker compose run --rm dev cargo test -p rete-core --no-default-features
```

- [ ] **Step 8: Commit**

```sh
git add crates/rete-core/src/index.rs crates/rete-core/src/bgp.rs crates/rete-core/src/sparql/eval.rs crates/rete-core/tests/ranged.rs
git commit -m "perf(core): adapt remote scan prefetch to demand"
```

### Task 5: Enable adaptive reads in native HTTP and browser remote sessions

**Files:**
- Modify: `crates/rete-cli/src/commands/url.rs`
- Modify: `crates/rete-wasm/src/lib.rs`
- Test: `crates/rete-cli/src/commands/url.rs`
- Test: `crates/rete-wasm/tests/web_api.rs`
- Test: `tests/gate/checks/check_wasm_boot.mjs`

**Interfaces:**
- Consumes: `BlockCacheReader::with_adaptive_clock`.
- Produces:
  - native remote clock from a captured `std::time::Instant` origin;
  - browser worker monotonic microseconds from `globalThis.performance.now()`;
  - default adaptive construction for remote-lazy HTTP only.

- [ ] **Step 1: Write a failing native opener contract test**

Factor a small helper that constructs the cache from `(reader, block, is_http)`.
Assert HTTP returns a cache with a controller, while local-path use and block=0
do not. Assert an at-or-below-8-MiB HTTP source still takes the owned-memory
one-transfer branch and never constructs the adaptive cache.

- [ ] **Step 2: Implement the native monotonic clock wiring**

Capture `Instant::now()` only in the HTTP remote-lazy branch:

```rust
let origin = std::time::Instant::now();
let cached = BlockCacheReader::new(reader, block).with_adaptive_clock(move || {
    u64::try_from(origin.elapsed().as_micros()).ok()
});
```

Do not put an unconditional `Instant::now()` in `rete-core` or any wasm-compiled
path. Apply the helper to `sparql-url` and `why-url`; keep local lazy open static.

- [ ] **Step 3: Write a failing WASM clock/controller test**

Add a host-independent test for the helper's missing/invalid performance object
fallback, and a wasm-bindgen/browser test asserting `RemoteGraph::new` creates an
adaptive source when `performance.now()` is available.

- [ ] **Step 4: Implement the browser clock**

Use `js_sys::global()` plus `Reflect`/`Function::call0` to invoke
`globalThis.performance.now()` in both Window and Worker contexts. Convert finite,
non-negative milliseconds to saturated integer microseconds; return `None` on
missing/non-callable/NaN values. Pass it to `with_adaptive_clock` in `open_url`.

- [ ] **Step 5: Verify browser and native surfaces**

```sh
docker compose run --rm dev cargo test -p rete-cli
docker compose run --rm dev cargo test -p rete-wasm
docker compose run --rm wasm wasm-pack build crates/rete-wasm --target web --out-dir ../../target/adaptive-wasm-web -- --no-default-features
docker compose run --rm gate node checks/check_wasm_boot.mjs
```

Expected: CLI tests pass; WASM builds; current generated production packages
still boot. Do not regenerate checked-in packages until Task 7.

- [ ] **Step 6: Commit**

```sh
git add crates/rete-cli/src/commands/url.rs crates/rete-wasm/src/lib.rs crates/rete-wasm/tests tests/gate/checks/check_wasm_boot.mjs
git commit -m "perf: enable adaptive reads for HTTP and WASM"
```

### Task 6: Reproducible static-versus-adaptive benchmarks

**Files:**
- Modify: `scripts/bench_cold_r2.py`
- Modify: `scripts/test_bench_cold_r2.py`
- Modify: `scripts/cold-r2-workloads/chebi-full.json`
- Modify: `tests/gate/checks/check_wikidata_xxl_traffic.mjs`
- Modify: `tests/gate/checks/test_wikidata_xxl_traffic.mjs`
- Modify: the existing Asyncify resident-evidence harness and its focused test
- Create: ignored benchmark/evidence artifacts under `target/bench/` only

**Interfaces:**
- Consumes: immutable baseline commit `6562251d` and the current candidate.
- Produces: exclusive-created JSONL/JSON evidence with executable/artifact hashes,
  before/after R2 pins, exact query/output hashes, timing, bytes, GETs, and RSS.

- [ ] **Step 1: Add a failing workload-coverage test**

Require `chebi-full.json` to contain both its bound-entity selective query and a
deterministic broad aggregate/full-scan query. Keep every limited SELECT's complete
approved ordering in the existing ordering gate.

- [ ] **Step 2: Extend the native harness identity schema**

Record git revision and an explicit `read_policy` label for each executable mode.
Reject duplicate/blank identity fields. Preserve exclusive `open("x")`, rotating
mode order, 15 fresh processes, exact result hashes, and before/after pins.

- [ ] **Step 3: Build immutable baseline and candidate binaries**

Use separate target directories and copy binaries to timestamped, non-overwritten
paths under `target/bench/`. Record SHA-256 and git revision. Both ChEBI modes use
`RETE_EAGER_MAX_MB=0`, so the comparison is static lazy versus adaptive lazy.

- [ ] **Step 4: Run native ChEBI selective and scan samples**

```sh
docker compose run --rm dev uv run python scripts/bench_cold_r2.py \
  --baseline /work/target/bench/rete-baseline-6562251d \
  --candidate /work/target/bench/rete-candidate-adaptive \
  --workload scripts/cold-r2-workloads/chebi-full.json \
  --samples 15 \
  --out target/bench/adaptive-chebi-20260816.jsonl
```

Require stable pins/transfer counts and identical result hashes. Compute median,
nearest-rank p90, bytes, GETs, and median/p90/max RSS.

- [ ] **Step 5: Build baseline and candidate WASM artifacts canonically**

Use the repository's `--no-opt`/canonical scripts in isolated output directories.
Run the boot check against both before any live query. Do not use Debian Binaryen
v108's corrupting optimized wasm-pack path.

- [ ] **Step 6: Run browser strict-subset and six-shard comparisons**

For pinned Jonas (`https://data.graphplaza.com/jonas/jonas.rete`, length
2,163,156, ETag `"afe8ebf6962fc3af9b92eae1327352b1"`), run this exact query
against baseline and candidate fresh sessions, then repeat it in-session and
require +0 bytes/+0 requests:

```sparql
SELECT ?w ?siglum WHERE {
  ?t <http://www.w3.org/2000/01/rdf-schema#label> "Lancelot" .
  ?w <https://lostma-erc.github.io/jonas/prop/is_manifestation_of> ?t ;
     <https://lostma-erc.github.io/jonas/prop/preferred_siglum> ?siglum .
} ORDER BY ?w ?siglum LIMIT 50
```

The result must remain 49 rows with SHA-256
`0657bd63ff1331eebd7f7448b2fb38b327a0293dd1cbd3b078e78f08d8e13aa6`.
For Wikidata XXL, require ASK true, exact 10 ordered rows/output SHA, all six
sources, only 206 ranged GETs, no full GET, and exact before/after
length/ETag/Accept-Ranges pins.

- [ ] **Step 7: Evaluate the acceptance gates rather than assuming a win**

- two selective remote-lazy workloads: candidate median at least 15% faster;
- one scan/aggregate: median latency or physical GET count at least 10% better;
- no representative median regression over 5%;
- RSS growth at most 10%;
- all result hashes identical;
- all adaptive gap/span/cache bounds satisfied.

If these do not pass, keep adaptive construction non-default and use the captured
evidence to tune or reject the policy. Do not describe a failed experiment as an
optimization.

- [ ] **Step 8: Commit only tracked harness/workload changes**

```sh
git add scripts/bench_cold_r2.py scripts/test_bench_cold_r2.py scripts/cold-r2-workloads/chebi-full.json tests/gate/checks
git commit -m "bench: compare adaptive R2 scheduling"
```

### Task 7: Documentation, generated artifacts, and completion audit

**Files:**
- Modify: `docs/BENCHMARK.md`
- Modify: `docs/BENCHMARK.html`
- Modify if observable policy changed: `docs/cli.md`, `docs/cli.html`, `docs/browser.md`, `docs/browser.html`
- Modify: generated `web/pkg*`/playground artifacts only through canonical build scripts
- Modify: `docs/superpowers/plans/2026-08-16-adaptive-r2-read-scheduling.md` checkboxes

**Interfaces:**
- Consumes: verified Task 6 evidence and all prior commits.
- Produces: exact reproducible benchmark prose, generated HTML/WASM, and a clean verified branch.

- [ ] **Step 1: Document measured results with candid scope**

Record candidate/baseline git and binary/WASM hashes, R2 pins, query links/text,
output hashes, all median/p90/bytes/GET/RSS values, browser physical traffic, and
whether each acceptance gate passed. Preserve the native <=8 MiB, browser lazy,
shard, named-graph, safe-default, and uncapped-zstd caveats.

- [ ] **Step 2: Regenerate documentation deterministically**

Use a branch-specific target directory to avoid the shared-worktree stale-docgen
binary problem:

```sh
docker compose run --rm -e CARGO_TARGET_DIR=/target/adaptive-docgen dev cargo run -q -p docgen
```

Run it twice and assert target HTML hashes are identical; restore no unrelated
generated-page drift.

- [ ] **Step 3: Rebuild canonical browser artifacts**

```sh
docker compose run --rm wasm
```

Require fresh web/no-modules boot checks, Asyncify build, playground/explorer
generation, fixtures, and manifest generation to pass.

- [ ] **Step 4: Run the complete Rust/Python/browser verification matrix**

```sh
docker compose run --rm dev cargo fmt --all -- --check
docker compose run --rm dev cargo clippy --workspace --exclude rete-bench --all-targets -- -D warnings
docker compose run --rm dev cargo test --workspace --exclude rete-bench
docker compose run --rm dev cargo test -p rete-core --no-default-features
docker compose run --rm dev cargo build -p rete-core --all-features
docker compose run --rm dev cargo clippy -p rete-core --all-features --all-targets -- -D warnings
docker compose run --rm dev cargo build -p rete-bench
docker compose run --rm dev bash scripts/smoke.sh
docker compose run --rm dev uv run python scripts/test_bench_cold_r2.py -v
docker compose run --rm gate node checks/test_wikidata_xxl_traffic.mjs
docker compose run --rm gate node test_asyncify_e2e_report.cjs
docker compose run --rm gate node checks/check_wasm_boot.mjs
```

- [ ] **Step 5: Audit every design requirement against current evidence**

Check format compatibility, session ownership, exact metadata, controller sharing,
selective/full/dictionary intents, caps, failures/retries, native small objects,
native large objects, browser, named graphs, shards, output parity, live pins,
performance thresholds, RSS, docs, generated artifacts, and absence of new unsafe.
Treat missing evidence as incomplete work.

- [ ] **Step 6: Mark plan checkboxes, inspect the diff, and commit**

```sh
git diff --check
git status --short
git add docs crates/rete-core crates/rete-cli crates/rete-wasm web tests scripts
git commit -m "perf: validate adaptive R2 read scheduling"
```

- [ ] **Step 7: Queue phase 2 rather than mixing it into this commit**

Create the warm/local-throughput design only after phase-1 measurements are final.
Profile safe allocation reuse, iterator specialization, SIMD-friendly decoding,
and join-table layout independently; do not infer a CPU win from remote timing.
