# Resident Prepared Queries Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (- [ ]) syntax for tracking.

**Goal:** Add dataset-bound prepared SPARQL queries and a bounded resident session so repeated queries avoid immutable parse, lowering, planning, constant-resolution, path-resolution, regex, and reasoning work while retaining byte-identical results and fresh execution state.

**Architecture:** QuerySession owns one immutable Rete and a weighted LRU keyed by exact query text plus QueryOptions. Opaque PreparedQuery handles own immutable compiled work and a dataset identity; each execution creates fresh rows, resolver scratch, aggregates, EXISTS/path adjacency state, nondeterministic values, SERVICE calls, and failure state.

**Tech Stack:** Rust 2021, rete-core SPARQL engine, wasm-bindgen, rete serve, rete-bench, Docker/devcontainer, docgen.

## Global Constraints

- Execute this plan after 2026-08-07-safe-local-wasm-paths.md so preparation reuses its resolved-path boundary instead of creating a competing representation.
- Do not change .rete bytes, header layout, one-shot CLI semantics, or mutation semantics.
- PreparedQuery is opaque; internal plans, IDs, slots, matchers, and dataset keys remain private.
- Preserve SparqlError parse/unsupported/service classes inside QuerySessionError.
- Do not promise concurrent execution; session mutation/execution methods take &mut self.
- A useful content hash contains at least one nonzero byte. Version, hash, quad count, and term count identify modern content. Counts alone never identify a dataset.
- Zero cache limits disable residency. Oversized plans execute but are not retained.
- Cache bounds cover cache-owned references; an explicit PreparedQuery clone may outlive eviction.
- Never cache SERVICE results, nondeterministic values, rows, aggregates, EXISTS answers, adjacency frontiers, or failure verdicts.
- Defaults are 128 entries and 4 MiB. Run every command through Docker.

## File Map

- Create crates/rete-core/src/prepared.rs: public API, dataset binding, compilation, execution guard, weight accounting, LRU.
- Create crates/rete-core/tests/prepared_query.rs: public equivalence, identity, freshness, failure, and cache tests.
- Modify crates/rete-core/src/lib.rs and sparql.rs: export API and split preparation from execution.
- Modify crates/rete-core/src/sparql/eval.rs, crates/rete-core/src/row.rs, crates/rete-core/src/bgp.rs, crates/rete-core/src/sparql/path.rs, crates/rete-core/src/sparql/ql.rs, crates/rete-core/src/service.rs: immutable resources and fresh execution.
- Modify crates/rete-wasm/src/lib.rs and tests: resident RemoteGraph session.
- Modify crates/rete-cli/src/commands/serve.rs and tests: replace session on graph rebuild.
- Create crates/bench/src/resident.rs; modify crates/bench/src/main.rs: correctness/performance acceptance.
- Modify docs/sparql.md, docs/browser.md, docs/BENCHMARK.md and generated HTML.

## Public Interface

~~~rust
pub const DEFAULT_PREPARED_QUERY_ENTRIES: usize = 128;
pub const DEFAULT_PREPARED_QUERY_BYTES: usize = 4 * 1024 * 1024;

#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub struct QueryOptions { reasoning: bool }

impl QueryOptions {
    pub const fn new() -> Self;
    pub const fn with_reasoning(self, reasoning: bool) -> Self;
    pub const fn reasoning(self) -> bool;
}

#[derive(Clone)]
pub struct PreparedQuery { inner: Arc<PreparedInner> }

impl PreparedQuery {
    pub fn estimated_size_bytes(&self) -> usize;
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct QueryCacheStats {
    pub entries: usize,
    pub weight_bytes: usize,
    pub hits: u64,
    pub misses: u64,
    pub evictions: u64,
}

#[derive(Debug, thiserror::Error)]
pub enum QuerySessionError {
    #[error(transparent)]
    Sparql(#[from] SparqlError),
    #[error("prepared query belongs to a different dataset or legacy query session")]
    DatasetMismatch,
    #[error("a range fetch failed while evaluating the query; results would be incomplete")]
    IncompleteRead,
}

pub struct QuerySession {
    rete: Arc<Rete>,
    /* private bounded-cache state */
}

impl QuerySession {
    pub fn new(rete: Rete) -> Self;
    pub fn with_cache_limits(rete: Rete, max_entries: usize, max_weight_bytes: usize) -> Self;
    pub fn rete(&self) -> &Rete;
    pub fn prepare(&mut self, query: &str, options: QueryOptions)
        -> Result<PreparedQuery, QuerySessionError>;
    pub fn execute_prepared(&mut self, prepared: &PreparedQuery)
        -> Result<QueryOutput, QuerySessionError>;
    pub fn execute(&mut self, query: &str, options: QueryOptions)
        -> Result<QueryOutput, QuerySessionError>;
    pub fn cache_stats(&self) -> QueryCacheStats;
    pub(crate) fn rete_arc(&self) -> Arc<Rete>;
}
~~~

Re-export only these public types. Keep PreparedInner, DatasetKey, PlanCache, Slots, Val, prepared BGP/path structures, and Matcher private or pub(crate).

---

### Task 1: Add the Dataset-Bound Prepared Shell

**Files:**

- Create crates/rete-core/src/prepared.rs
- Create crates/rete-core/tests/prepared_query.rs
- Modify crates/rete-core/src/lib.rs:94-107,188-193
- Modify crates/rete-core/src/sparql.rs:785-904

**Interfaces:** Defines the public interface above and private DatasetKey/PreparedForm. At this stopping point preparation owns the lowered query form and slots; later tasks add resources.

- [ ] **Step 1: Write failing public equivalence/identity tests**

~~~rust
#[test]
fn prepared_query_matches_ordinary_query_bytes() {
    let bytes = fixture_bytes();
    let ordinary_rete = open_bytes(bytes.clone());
    let mut session = QuerySession::new(open_bytes(bytes));
    let query = "SELECT ?s ?p ?o WHERE { ?s ?p ?o } ORDER BY ?s ?p ?o LIMIT 10";
    let ordinary = query_json(eval_query(&ordinary_rete, query).unwrap());
    let prepared = session.prepare(query, QueryOptions::new()).unwrap();
    let resident = query_json(session.execute_prepared(&prepared).unwrap());
    assert_eq!(resident, ordinary);
}

#[test]
fn prepared_query_rejects_different_content_with_equal_counts() {
    let mut first = QuerySession::new(open_bytes(equal_count_fixture_a()));
    let mut second = QuerySession::new(open_bytes(equal_count_fixture_b()));
    let prepared = first.prepare("ASK { ?s ?p ?o }", QueryOptions::new()).unwrap();
    assert!(matches!(
        second.execute_prepared(&prepared),
        Err(QuerySessionError::DatasetMismatch)
    ));
}

#[test]
fn identical_modern_content_accepts_the_same_prepared_handle() {
    let bytes = fixture_bytes();
    let mut first = QuerySession::new(open_bytes(bytes.clone()));
    let mut second = QuerySession::new(open_bytes(bytes));
    let prepared = first.prepare("ASK { ?s ?p ?o }", QueryOptions::new()).unwrap();
    second.execute_prepared(&prepared).unwrap();
}

#[test]
fn zero_hash_prepared_query_is_session_bound() {
    let mut bytes = fixture_bytes();
    zero_content_hash(&mut bytes);
    let mut owner = QuerySession::new(open_bytes(bytes.clone()));
    let mut other = QuerySession::new(open_bytes(bytes));
    let prepared = owner.prepare("ASK { ?s ?p ?o }", QueryOptions::new()).unwrap();
    owner.execute_prepared(&prepared).unwrap();
    assert!(matches!(
        other.execute_prepared(&prepared),
        Err(QuerySessionError::DatasetMismatch)
    ));
}
~~~

Use test-local fixture/output helpers; add no fixture-only production API.

- [ ] **Step 2: Run red tests**

~~~sh
docker compose run --rm dev cargo test -p rete-core --test prepared_query
~~~

Expected: unresolved QuerySession, QueryOptions, QuerySessionError, and PreparedQuery.

- [ ] **Step 3: Implement dataset identity**

~~~rust
#[derive(Clone, Debug, Eq, PartialEq)]
enum DatasetKey {
    Content {
        version: u8,
        content_hash: [u8; 16],
        quad_count: u64,
        term_count: u64,
    },
    LegacySession(u64),
}
~~~

Allocate legacy nonces from AtomicU64 starting at 1. Never substitute length, URL, path, ETag, or counts for a zero hash.

- [ ] **Step 4: Extract query-form preparation/execution**

~~~rust
enum PreparedForm {
    Select(Box<Select>),
    Ask(Box<Select>),
    Construct { template: Vec<TriplePattern>, select: Box<Select> },
    Describe(Box<Select>),
}

struct PreparedInner {
    dataset: DatasetKey,
    query_text: Arc<str>,
    options: QueryOptions,
    form: PreparedForm,
    weight_bytes: usize,
}
~~~

Refactor eval_query_inner so ordinary and prepared paths share form-specific execution for SELECT, ASK, CONSTRUCT, and DESCRIBE. Ordinary eval_query must still parse/lower each call and must not enter the session cache.

- [ ] **Step 5: Add guarded execution**

Before each operation call rete.reset_load_failures(). After execution, let a SPARQL/SERVICE error win; otherwise return IncompleteRead when rete.index_incomplete() is true. Consume/clear the existing service error exactly once as ordinary evaluation does.

- [ ] **Step 6: Verify and commit**

~~~sh
docker compose run --rm dev cargo test -p rete-core --test prepared_query
git add crates/rete-core/src/prepared.rs crates/rete-core/src/lib.rs   crates/rete-core/src/sparql.rs crates/rete-core/tests/prepared_query.rs
git commit -m "feat(core): add dataset-bound prepared queries"
~~~

Expected: all four tests PASS.

---

### Task 2: Cache Slots, Constants, BGPs, Paths, Regexes, and Reasoning

**Files:**

- Modify crates/rete-core/src/prepared.rs
- Modify crates/rete-core/src/sparql/eval.rs
- Modify crates/rete-core/src/row.rs
- Modify crates/rete-core/src/bgp.rs
- Modify crates/rete-core/src/sparql/path.rs
- Modify crates/rete-core/src/sparql/ql.rs
- Modify crates/rete-core/src/sparql.rs

**Interfaces:**

~~~rust
pub(crate) struct PreparedResources {
    slots: Arc<Slots>,
    resolver: PreparedResolverSeeds,
    bgps: HashMap<usize, PreparedBgp>,
    paths: HashMap<usize, PreparedPath>,
}

pub(crate) struct PreparedPath {
    ast: ResolvedPath,
    // None means variable; Some(None) means a missing constant dictionary ID.
    subject_constant: Option<Option<u32>>,
    object_constant: Option<Option<u32>>,
}

pub(crate) fn collect_slots(select: &Select) -> Arc<Slots>;
pub(crate) fn query_ctx(
    rete: &Rete,
    slots: Arc<Slots>,
    seeds: PreparedResolverSeeds,
) -> Ctx<'_>;
~~~

Plan-node keys are stable boxed-plan addresses used only as integers; no integer is dereferenced as a pointer.

- [ ] **Step 1: Write failing internal resource tests**

~~~rust
#[test]
fn compilation_records_immutable_resources_once() {
    let rete = test_rete();
    let query = r#"
      SELECT ?s ?o WHERE {
        ?s <urn:p> ?mid .
        ?mid (<urn:q>|^<urn:r>)+ ?o .
        FILTER regex(str(?o), "^prefix", "i")
      }"#;
    let prepared = compile_for_test(&rete, query, QueryOptions::new());
    assert_eq!(prepared.bgp_count(), 1);
    assert_eq!(prepared.path_count(), 1);
    assert_eq!(prepared.compiled_regex_count(), 1);
    assert!(prepared.slot_count() >= 3);
    assert!(prepared.resolved_predicate_count() >= 3);
}
~~~

Add reasoning_rewrite_is_owned_by_prepared_plan and require estimated_size_bytes() > 0. Test-only inspectors stay under cfg(test).

- [ ] **Step 2: Run red tests**

~~~sh
docker compose run --rm dev cargo test -p rete-core prepared::tests::
~~~

Expected: missing prepared resource types/inspectors.

- [ ] **Step 3: Share immutable slots and resolver seeds**

Store Arc<Slots> in Ctx. Ordinary run_select calls collect_slots transiently; prepared execution reuses it. Split resolver data:

~~~rust
#[derive(Clone, Default)]
pub(crate) struct PreparedResolverSeeds {
    canonical_terms: Arc<HashMap<String, Val>>,
    regexes: Arc<HashMap<(String, String), Arc<Matcher>>>,
}
~~~

A new Resolver still owns fresh decoded-term, canonical, numeric, and dynamic-regex maps. Constant canonical values and literal regex pattern/flags check prepared seeds first. Row-dependent regex patterns stay dynamic.

- [ ] **Step 4: Prepare base BGP work**

~~~rust
pub(crate) struct PreparedBgp {
    lowered: Arc<[(SlotTerm, SlotTerm, SlotTerm)]>,
    estimates: Arc<[u64]>,
    order: Arc<[usize]>,
}
~~~

Make SlotTerm pub(crate). Ordinary BGP evaluation performs transient preparation; prepared evaluation reuses constant lowering, predicate/stat maps, estimates, and base order. Correlated seeds may build a fresh probe plan but may not resolve constants or recompute base order.

- [ ] **Step 5: Reuse Track-3 resolved paths**

Make Track 3's ResolvedPath and constructor pub(crate). Store one PreparedPath per stable path-plan address: its ResolvedPath retains predicate/exclusion IDs, while subject_constant and object_constant retain constant endpoint lookups. The outer option distinguishes a variable from a syntactic constant; the inner option records whether that constant exists in the dictionary. Adjacency/frontier/visited sets remain execution-local. Do not introduce PreparedPathAst in parallel.

- [ ] **Step 6: Compile literal regexes and reason once**

Walk boxed plan nodes, nested SELECT/EXISTS/SERVICE plans, expressions, VALUES, GRAPH, templates, and DESCRIBE terms. Compile regex only when pattern and flags are literals. For QueryOptions.reasoning, perform ql::reason_rewrite during prepare before collecting resources; ordinary reasoned queries remain unchanged.

- [ ] **Step 7: Verify and commit**

~~~sh
docker compose run --rm dev cargo test -p rete-core prepared::tests::
docker compose run --rm dev cargo test -p rete-core --test prepared_query
docker compose run --rm dev cargo test -p rete-core sparql::path::tests
git add crates/rete-core/src/prepared.rs crates/rete-core/src/sparql.rs   crates/rete-core/src/sparql/eval.rs crates/rete-core/src/row.rs   crates/rete-core/src/bgp.rs crates/rete-core/src/sparql/path.rs   crates/rete-core/src/sparql/ql.rs
git commit -m "feat(core): reuse immutable prepared query work"
~~~

Expected: all PASS.

---

### Task 3: Prove Execution State and Failures Stay Fresh

**Files:**

- Modify crates/rete-core/tests/prepared_query.rs
- Modify crates/rete-core/src/prepared.rs and sparql/eval.rs
- Reuse mock patterns from service.rs tests, service_federation.rs, sparql_nondeterministic.rs, and ranged.rs

**Interfaces:** Only immutable plan resources cross executions. All mutable values are recreated.

- [ ] **Step 1: Add nondeterminism and SERVICE tests**

~~~rust
#[test]
fn prepared_struuid_is_recomputed_for_every_execution() {
    let mut session = QuerySession::new(test_rete());
    let prepared = session.prepare(
        "SELECT (STRUUID() AS ?r) WHERE { ?s ?p ?o } LIMIT 1",
        QueryOptions::new(),
    ).unwrap();
    let first = first_binding(session.execute_prepared(&prepared).unwrap(), "r");
    let second = first_binding(session.execute_prepared(&prepared).unwrap(), "r");
    assert_ne!(first, second);
}

#[test]
fn prepared_service_is_called_once_per_execution() {
    let calls = Arc::new(Mutex::new(0));
    let mut session = QuerySession::new(test_rete_with_service(calls.clone()));
    let prepared = session.prepare(SERVICE_QUERY, QueryOptions::new()).unwrap();
    session.execute_prepared(&prepared).unwrap();
    session.execute_prepared(&prepared).unwrap();
    assert_eq!(*calls.lock().unwrap(), 2);
}
~~~

Add fail-once SERVICE and fail-once lazy RangeReader tests; the second execution must retry and succeed. Add parse and runtime error-class assertions.

- [ ] **Step 2: Run tests before fixing leaks**

~~~sh
docker compose run --rm dev cargo test -p rete-core --test prepared_query   prepared_struuid_is_recomputed_for_every_execution -- --exact
docker compose run --rm dev cargo test -p rete-core --test prepared_query   prepared_service_is_called_once_per_execution -- --exact
docker compose run --rm dev cargo test -p rete-core --test prepared_query   transient_lazy_read_failure_is_cleared_before_retry -- --exact
~~~

Expected: any leaked execution state produces equal UUIDs, one SERVICE call, or sticky failure.

- [ ] **Step 3: Enforce fresh execution construction**

Every execute_prepared call must construct new rows, resolver scratch maps, aggregates, EXISTS cache, path adjacency/frontier/visited sets, and nondeterministic values. Reset Rete load failures first and consume the service error last. Keep the compiled plan after a transient execution error.

- [ ] **Step 4: Run regression suites and commit**

~~~sh
docker compose run --rm dev cargo test -p rete-core --test prepared_query
docker compose run --rm dev cargo test -p rete-core --test sparql_nondeterministic
docker compose run --rm dev cargo test -p rete-core --test service_federation
docker compose run --rm dev cargo test -p rete-core --test ranged
git add crates/rete-core/src/prepared.rs crates/rete-core/src/sparql/eval.rs   crates/rete-core/tests/prepared_query.rs
git commit -m "test(core): preserve fresh prepared execution state"
~~~

Expected: all PASS.

---

### Task 4: Add the Weighted Resident LRU

**Files:**

- Modify crates/rete-core/src/prepared.rs
- Modify crates/rete-core/tests/prepared_query.rs

**Interfaces:**

~~~rust
#[derive(Clone, Eq, Hash, PartialEq)]
struct CacheKey { query: Arc<str>, options: QueryOptions }

struct CacheEntry {
    prepared: PreparedQuery,
    weight_bytes: usize,
    last_used: u64,
}
~~~

PlanCache tracks HashMap entries, maximums, total weight, monotonic clock, hits, misses, and evictions.

- [ ] **Step 1: Write failing key/cap/disabled tests**

~~~rust
#[test]
fn resident_cache_keys_exact_text_and_options() {
    let mut session = QuerySession::new(test_rete());
    let query = "ASK { ?s ?p ?o }";
    session.execute(query, QueryOptions::new()).unwrap();
    session.execute(query, QueryOptions::new()).unwrap();
    session.execute(query, QueryOptions::new().with_reasoning(true)).unwrap();
    session.execute("ASK  { ?s ?p ?o }", QueryOptions::new()).unwrap();
    let stats = session.cache_stats();
    assert_eq!((stats.entries, stats.hits, stats.misses), (3, 1, 3));
}

#[test]
fn resident_cache_obeys_entry_and_weight_caps() {
    let mut session = QuerySession::with_cache_limits(test_rete(), 2, 16 * 1024);
    for limit in 1..=12 {
        let query = format!("SELECT ?s WHERE {{ ?s ?p ?o }} LIMIT {limit}");
        session.execute(&query, QueryOptions::new()).unwrap();
        assert!(session.cache_stats().entries <= 2);
        assert!(session.cache_stats().weight_bytes <= 16 * 1024);
    }
    assert!(session.cache_stats().evictions >= 10);
}
~~~

Also test (0,0) executes with no hits/entries and a 1-byte budget executes but retains nothing.

- [ ] **Step 2: Run red tests**

~~~sh
docker compose run --rm dev cargo test -p rete-core --test prepared_query   resident_cache_ -- --nocapture
~~~

Expected: execute/cache accounting is missing.

- [ ] **Step 3: Implement lookup, insertion, and eviction**

On a hit, increment clock/hits and clone the handle before execution. On a miss, increment misses, compile once, execute, and insert only if both limits are nonzero and the plan fits. Do not cache preparation errors. Retain a compiled cached plan after transient execution failure.

Evict the minimum last_used until entries <= max_entries and weight <= max_weight_bytes. Use checked/saturating accounting. If the new entry alone exceeds budget, keep existing entries.

- [ ] **Step 4: Implement conservative weight accounting**

Count query bytes; plan/template/expression/value nodes and vector capacities; slot strings/maps; prepared BGP/path arrays; canonical values; regex keys, Matcher size plus 1024 bytes per compiled regex; reasoning-created nodes/strings; and map bucket estimates. Count at least one byte. Exclude Rete data, per-execution scratch, and cache bookkeeping.

- [ ] **Step 5: Verify and commit**

~~~sh
docker compose run --rm dev cargo test -p rete-core --test prepared_query
git add crates/rete-core/src/prepared.rs crates/rete-core/tests/prepared_query.rs
git commit -m "feat(core): add bounded resident query cache"
~~~

---

### Task 5: Integrate RemoteGraph

**Files:**

- Modify crates/rete-wasm/src/lib.rs:228-242,622-705,1611-1626
- Modify/test crates/rete-wasm/tests/web_api.rs

**Interfaces:** RemoteGraph retains one RefCell<QuerySession>; query and query_reasoned call session.execute. Reopening a graph creates a new session.

- [ ] **Step 1: Write a failing wrapper-equivalence/hit test**

Create a private session_query_json helper and test two identical calls against the existing one-shot JSON:

~~~rust
let mut session = QuerySession::new(open_bytes(rete_bytes));
let first = session_query_json(&mut session, QUERY, false);
let second = session_query_json(&mut session, QUERY, false);
assert_eq!(first, expected);
assert_eq!(second, expected);
assert_eq!(session.cache_stats().hits, 1);
~~~

- [ ] **Step 2: Run red test**

~~~sh
docker compose run --rm dev cargo test -p rete-wasm   remote_query_helper_reuses_a_session_without_changing_json -- --exact
~~~

Expected: missing session helper/integration.

- [ ] **Step 3: Store and use the session**

RemoteGraph stores its CountingReader plus RefCell<QuerySession>. Map reasoning through QueryOptions. Convert QuerySessionError to the same JavaScript error messages/classes where applicable. Read-only methods use session.borrow().rete(). Quads call QuerySession::rete_arc to obtain an owned graph handle; use Arc ownership without claiming browser threads.

- [ ] **Step 4: Verify both WASM targets and commit**

~~~sh
docker compose run --rm dev cargo test -p rete-wasm
docker compose run --rm wasm wasm-pack build crates/rete-wasm   --target web --out-dir ../../web/pkg
docker compose run --rm wasm wasm-pack build crates/rete-wasm   --target no-modules --out-dir ../../web/pkg-nomodules
git add crates/rete-wasm/src/lib.rs crates/rete-wasm/tests/web_api.rs
git commit -m "feat(wasm): reuse prepared queries in remote graphs"
~~~

Expected: PASS; API remains single-threaded.

---

### Task 6: Integrate rete serve

**Files:**

- Modify/test crates/rete-cli/src/commands/serve.rs

**Interfaces:** The served Store owns QuerySession instead of bare Rete. Every successful rebuild replaces the whole session; prepared plans never cross a graph mutation.

- [ ] **Step 1: Write a failing rebuild-invalidation test**

~~~rust
#[test]
fn served_store_reuses_queries_and_rebuild_replaces_the_session() {
    let mut store = test_store();
    let query = "ASK { ?s ?p ?o }";
    store.session.execute(query, QueryOptions::new()).unwrap();
    store.session.execute(query, QueryOptions::new()).unwrap();
    assert_eq!(store.session.cache_stats().hits, 1);
    store.rebuild(test_image_bytes_changed()).unwrap();
    assert_eq!(store.session.cache_stats(), QueryCacheStats::default());
}
~~~

- [ ] **Step 2: Run red test**

~~~sh
docker compose run --rm dev cargo test -p rete-cli   served_store_reuses_queries_and_rebuild_replaces_the_session -- --exact
~~~

Expected: Store owns only Rete.

- [ ] **Step 3: Replace and route**

Build/configure the Rete, including SERVICE client, before QuerySession::new. Route served read queries through session.execute. Keep update evaluation on session.rete() unless already using the read path. Every rebuild/reload creates a new QuerySession; never compare counts or transfer plans.

- [ ] **Step 4: Verify and commit**

~~~sh
docker compose run --rm dev cargo test -p rete-cli
git add crates/rete-cli/src/commands/serve.rs
git commit -m "feat(cli): reuse prepared queries in rete serve"
~~~

---

### Task 7: Add Resident Acceptance Benchmark

**Files:**

- Create crates/bench/src/resident.rs
- Modify crates/bench/src/main.rs

**Interfaces:** Adds rete-bench --resident FILE and --resident-check FILE.

- [ ] **Step 1: Add workloads and measurement phases**

Use ASK any, bound point LIMIT, LIMIT 1, full ordered scan, COUNT aggregate, property path, regex/filter, and reasoned class query. For each: 50 ordinary executions, separate preparation time, first prepared execution, 50 warm explicit executions, and 50 session cache hits. Repeat five batches and use median of batch medians. Serialize all outputs through one function and assert byte equality.

- [ ] **Step 2: Add cache and lazy-read stress**

Generate 256 exact query texts by varying LIMIT 1 through 256. Assert <=128 entries, <=4,194,304 weight bytes, and evictions >0. Open a separate lazy session through CountingReader plus BlockCacheReader; record preparation, first execution, and second execution deltas. The second identical execution must add zero underlying requests and bytes.

- [ ] **Step 3: Enforce acceptance**

~~~text
ordinary_json == explicit_prepared_json == session_json
tiny warm median <= ordinary median * 0.80
scan/serialization warm median <= ordinary median * 1.03
second lazy request delta == 0
second lazy byte delta == 0
entries <= 128
weight bytes <= 4194304
rotation evictions > 0
~~~

Only ASK, point, and LIMIT-shaped tiny queries use the 20% gate.

- [ ] **Step 4: Build, run, and commit**

~~~sh
docker compose run --rm dev cargo build -p rete-bench
docker compose run --rm dev cargo run -p rete-bench --release --   --resident /target/rete-rust-opt-bench/chemotion.rete
docker compose run --rm dev cargo run -p rete-bench --release --   --resident-check /target/rete-rust-opt-bench/chemotion.rete
git add crates/bench/src/resident.rs crates/bench/src/main.rs
git commit -m "bench: add resident prepared-query coverage"
~~~

Expected: byte_equal=true, warm lazy deltas zero, and resident_acceptance=pass. If timing fails, retain correctness but profile remaining work before weakening gates.

---

### Task 8: Document and Fully Verify Track 2

**Files:**

- Modify docs/sparql.md, docs/browser.md, docs/BENCHMARK.md
- Regenerate docs/sparql.html, docs/browser.html, docs/BENCHMARK.html

- [ ] **Step 1: Document the exact API and semantics**

Include a QuerySession/PreparedQuery Rust example. State modern content binding, legacy nonce binding, clone/eviction behavior, fresh mutable execution state, serial semantics, 128-entry/4 MiB defaults, zero-limit behavior, error mapping, retry behavior, RemoteGraph automatic reuse, and rebuild invalidation.

- [ ] **Step 2: Record benchmark evidence**

Record commands, dataset hash, workloads, iteration/batch counts, preparation separately, ordinary/prepared/session median and p95, serialization, cache stats, lazy request deltas, heap, and acceptance verdict. Do not generalize a 20% gain beyond tiny query shapes.

- [ ] **Step 3: Regenerate docs and run all gates**

~~~sh
docker compose run --rm dev cargo run -q -p docgen
docker compose run --rm dev cargo fmt --all -- --check
docker compose run --rm dev cargo clippy --workspace --exclude rete-bench --all-targets -- -D warnings
docker compose run --rm dev cargo test --workspace --exclude rete-bench
docker compose run --rm dev cargo test -p rete-core --no-default-features
docker compose run --rm dev cargo build -p rete-core --all-features
docker compose run --rm dev cargo build -p rete-bench
docker compose run --rm dev cargo run -p rete-bench --release --   --resident-check /target/rete-rust-opt-bench/chemotion.rete
docker compose run --rm dev bash scripts/smoke.sh
docker compose run --rm wasm wasm-pack build crates/rete-wasm   --target web --out-dir ../../web/pkg
docker compose run --rm wasm wasm-pack build crates/rete-wasm   --target no-modules --out-dir ../../web/pkg-nomodules
git diff --check
~~~

Expected: all commands PASS and docgen introduces no unexpected drift.

- [ ] **Step 4: Commit documentation and review final scope**

~~~sh
git add docs/sparql.md docs/sparql.html docs/browser.md docs/browser.html   docs/BENCHMARK.md docs/BENCHMARK.html
git commit -m "docs: document resident prepared queries"
git status --short
git diff HEAD~8 --stat
~~~

Final review must confirm byte identity; correct modern/legacy rejection; fresh RAND/UUID/SERVICE/failure state; zero warm lazy reads; bounded LRU; tiny-query >=20%; scan <=3% regression; RemoteGraph/serve reuse; unchanged one-shot CLI; and no concurrent-session claim.
