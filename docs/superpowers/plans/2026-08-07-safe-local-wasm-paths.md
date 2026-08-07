# Safe Local/WASM Path Traversal Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (- [ ]) syntax for tracking.

**Goal:** Recover most of the 145 ms to 86 ms unchecked-decoder gap for path-heavy local and WASM SPARQL while retaining safe Rust, current .rete bytes, bounded memory, and malformed-input behavior.

**Architecture:** Specialize checked triple-block decoding for u32 LEB128, enrich each lazy tile directory with a budgeted (a,b)-to-c-list index, stream only neighboring IDs from GraphIndex, and resolve path predicates once before traversal. Unsupported shapes and directories exceeding 64 KiB fall back to the current safe scan.

**Tech Stack:** Rust 2021, rete-core, rete-wasm, rete-bench, Docker/devcontainer, wasm-pack, Bash, docgen.

## Global Constraints

- Add no production unsafe and do not alter .rete bytes or CURRENT_FORMAT_VERSION.
- Keep generic read_uvarint for non-triple format fields.
- Prefix-2 storage is at most 64 KiB per tile; overflow retains the a-only directory.
- All offsets/counts remain checked; truncation and corruption remain panic-free.
- Default WASM stays single-threaded and free of native-only dependencies.
- General pattern/permutation semantics remain unchanged.
- Keep only changes that meet the path and control-workload gates.
- Run commands in Docker and commit without a co-author trailer.

## File Map

- crates/rete-core/src/triples.rs: checked u32 decoder and prefix-2 directory.
- crates/rete-core/src/index.rs: stored-order core and neighbor iterator.
- crates/rete-core/src/sparql/path.rs: resolved predicates and neighbor traversal.
- crates/rete-core/tests/sparql_integration.rs and crates/bench/tests/differential.rs: behavior gates.
- crates/rete-core/src/read_path_metrics.rs, Cargo.toml, lib.rs: benchmark-only counters.
- crates/bench/src/pathread.rs and main.rs: native report.
- scripts/bench_safe_path.sh and browser Worker assets: alternating native/WASM measurements.
- docs/BENCHMARK.md and generated HTML: evidence.

---

### Task 1: Preserve a Local Path Baseline

**Files:** Ignored /target/read-path-baseline artifacts only.

**Interfaces:** Produces the same feature-enabled release executable used by the existing safe/unchecked benchmark.

- [ ] **Step 1: Build the untouched release binary**

~~~sh
docker compose run --rm   -e CARGO_TARGET_DIR=/target/read-path-baseline   dev cargo build --release -p rete-cli --features unsafe-decode-bench
docker compose run --rm dev bash -lc   'sha256sum /target/read-path-baseline/release/rete'
~~~

Expected: PASS; retain the executable and hash without staging them.

---

### Task 2: Specialize Checked u32 LEB128 Decoding

**Files:**

- Modify/test crates/rete-core/src/triples.rs:205-275,500-507,740-944

**Interfaces:**

~~~rust
fn read_u32_at(bytes: &[u8], pos: &mut usize) -> Option<u32>;
~~~

Failure leaves pos unchanged; success advances one through five bytes. Generic read_uvarint and feature-only rd_unchecked remain.

- [ ] **Step 1: Write failing boundary/equivalence tests**

~~~rust
#[test]
fn checked_u32_decoder_covers_one_through_five_bytes() {
    let cases: &[(u32, &[u8])] = &[
        (0, &[0x00]),
        (127, &[0x7f]),
        (128, &[0x80, 0x01]),
        (16_384, &[0x80, 0x80, 0x01]),
        (1 << 21, &[0x80, 0x80, 0x80, 0x01]),
        (u32::MAX, &[0xff, 0xff, 0xff, 0xff, 0x0f]),
    ];
    for &(want, bytes) in cases {
        let mut pos = 0;
        assert_eq!(read_u32_at(bytes, &mut pos), Some(want));
        assert_eq!(pos, bytes.len());
    }
}

#[test]
fn checked_u32_decoder_rejects_truncation_and_overflow_without_consuming() {
    for bytes in [
        &[0x80][..],
        &[0x80, 0x80][..],
        &[0x80, 0x80, 0x80][..],
        &[0x80, 0x80, 0x80, 0x80][..],
        &[0x80, 0x80, 0x80, 0x80, 0x80][..],
        &[0xff, 0xff, 0xff, 0xff, 0x10][..],
    ] {
        let mut pos = 0;
        assert_eq!(read_u32_at(bytes, &mut pos), None);
        assert_eq!(pos, 0);
    }
}
~~~

Add an encoded sample loop comparing successful values and consumed lengths with read_uvarint.

- [ ] **Step 2: Run red tests**

~~~sh
docker compose run --rm dev cargo test -p rete-core   triples::tests::checked_u32_decoder -- --nocapture
~~~

Expected: compile failure because read_u32_at is missing.

- [ ] **Step 3: Implement the safe fixed-width decoder**

~~~rust
#[inline(always)]
fn read_u32_at(bytes: &[u8], pos: &mut usize) -> Option<u32> {
    let start = *pos;
    let first = *bytes.get(start)?;
    if first < 0x80 {
        *pos = start + 1;
        return Some(u32::from(first));
    }
    let mut value = u32::from(first & 0x7f);
    let mut shift = 7;
    for i in 1..5usize {
        let byte = *bytes.get(start.checked_add(i)?)?;
        if i == 4 && byte & 0xf0 != 0 {
            return None;
        }
        value |= u32::from(byte & 0x7f) << shift;
        if byte & 0x80 == 0 {
            *pos = start + i + 1;
            return Some(value);
        }
        shift += 7;
    }
    None
}
~~~

Make rd call this helper and use it for all triple-block u32 fields. Do not change non-triple varint callers.

- [ ] **Step 4: Verify correctness and commit**

~~~sh
docker compose run --rm dev cargo test -p rete-core triples::tests
docker compose run --rm dev cargo test -p rete-core --test robustness
docker compose run --rm dev cargo test -p rete-core --features unsafe-decode-bench   triples::tests::unchecked_cursor_matches_safe_every_pattern -- --exact
docker compose run --rm dev cargo clippy -p rete-core --all-targets -- -D warnings
git add crates/rete-core/src/triples.rs
git commit -m "perf(core): specialize checked triple varint decoding"
~~~

Expected: all commands PASS and safe/unchecked answers remain identical.

---

### Task 3: Add the 64 KiB Prefix-2 Directory

**Files:**

- Modify/test crates/rete-core/src/triples.rs:278-350,535-581,905-943

**Interfaces:**

~~~rust
const PREFIX2_DIRECTORY_BUDGET: usize = 64 * 1024;

#[derive(Clone, Copy)]
struct BDirEntry {
    b: u32,
    c_pos: u32,
    c_count: u32,
}
~~~

GroupDirectory retains a entries and an optional boxed b-entry array. scan_from automatically uses it when pa and pb are bound.

- [ ] **Step 1: Write failing indexed/fallback tests**

~~~rust
#[test]
fn prefix2_directory_indexes_bound_b_groups() {
    let mut builder = TripleBlockBuilder::new();
    for b in 1..=40 {
        for c in [1, 3, 9] {
            builder.push((7, b, c));
        }
    }
    let bytes = builder.build();
    let block = TripleBlock::parse(&bytes).unwrap();
    let dir = block.group_directory();
    assert!(dir.prefix2_bytes() <= PREFIX2_DIRECTORY_BUDGET);
    assert_eq!(
        block.scan_from(&dir, 7, Some(31), None).collect::<Vec<_>>(),
        vec![(7, 31, 1), (7, 31, 3), (7, 31, 9)]
    );
}

#[test]
fn prefix2_budget_overflow_falls_back_to_a_only() {
    let max = PREFIX2_DIRECTORY_BUDGET / std::mem::size_of::<BDirEntry>();
    let mut builder = TripleBlockBuilder::new();
    for b in 1..=(max as u32 + 1) {
        builder.push((1, b, 1));
    }
    let bytes = builder.build();
    let block = TripleBlock::parse(&bytes).unwrap();
    let dir = block.group_directory();
    assert_eq!(dir.prefix2_bytes(), 0);
    assert_eq!(
        block.scan_from(&dir, 1, Some(max as u32), None).collect::<Vec<_>>(),
        vec![(1, max as u32, 1)]
    );
}
~~~

Also exercise scan_from on every corrupted block accepted by TripleBlock::parse.

- [ ] **Step 2: Run red tests**

~~~sh
docker compose run --rm dev cargo test -p rete-core   triples::tests::prefix2_ -- --nocapture
~~~

Expected: compile failure for missing directory fields/helpers.

- [ ] **Step 3: Define bounded storage**

~~~rust
const MAX_B_DIR_ENTRIES: usize =
    PREFIX2_DIRECTORY_BUDGET / std::mem::size_of::<BDirEntry>();

pub struct GroupDirectory {
    entries: Vec<DirEntry>,
    b_entries: Box<[BDirEntry]>,
}

struct DirEntry {
    a: u32,
    pos: usize,
    num_b: u32,
    a_rem_after: u32,
    b_start: u32,
    b_len: u32,
}
~~~

During the existing checked walk, record b, the u32-convertible position immediately after num_c, and c_count. If the cap, conversion, or complete walk fails, clear all b entries and b ranges but keep the valid a-prefix directory.

- [ ] **Step 4: Add lookup and direct c-list arming**

~~~rust
fn find_prefix2(&self, a: u32, b: u32) -> Option<&BDirEntry> {
    let a_entry = &self.entries[
        self.entries.binary_search_by_key(&a, |entry| entry.a).ok()?
    ];
    let start = a_entry.b_start as usize;
    let entries = self.b_entries.get(start..start.checked_add(a_entry.b_len as usize)?)?;
    entries.binary_search_by_key(&b, |entry| entry.b)
        .ok().map(|i| &entries[i])
}
~~~

When found, scan_from sets pos=c_pos, a=pa, b=pb, c=0, c_rem=c_count, and all a/b remaining counts to zero. When indexed a exists but b does not, return the dead cursor. When prefix-2 data is absent, use the original a-only logic. The unsafe research directory remains a-only.

- [ ] **Step 5: Verify fallback, mega-groups, feature build, and commit**

~~~sh
docker compose run --rm dev cargo test -p rete-core triples::tests
docker compose run --rm dev cargo test -p rete-core   index::tests::directory_backed_scans_match_reference_every_shape -- --exact
docker compose run --rm dev cargo test -p rete-core   index::tests::mega_group_splits_across_tiles_and_lookups_stay_complete -- --exact
docker compose run --rm dev cargo test -p rete-core --features unsafe-decode-bench
docker compose run --rm dev cargo test -p rete-core --test robustness
git add crates/rete-core/src/triples.rs
git commit -m "perf(core): add bounded prefix-2 group directory"
~~~

Expected: all PASS; every retained b-entry array is at most 65,536 bytes.

---

### Task 4: Expose a Neighbor-Only Iterator

**Files:**

- Modify/test crates/rete-core/src/index.rs:812-964,1010-1222

**Interfaces:**

~~~rust
pub(crate) fn scan_prefix2(
    &self,
    permutation: IndexPermutation,
    a: u32,
    b: u32,
) -> impl Iterator<Item = u32> + '_;
~~~

It yields stored c IDs, with no canonical Triple allocation or sort, while retaining tile routing, synopsis pruning, lazy faults, failure flags, mega-group spans, and feature-only cursor choice.

- [ ] **Step 1: Write failing permutation and split-group tests**

~~~rust
#[test]
fn prefix2_neighbor_scan_matches_each_permutation() {
    let (idx, data) = graph();
    for perm in ALL_PERMS {
        for (a, b) in [(1, 10), (1, 11), (2, 10), (99, 99)] {
            let want: Vec<u32> = data.iter().copied()
                .map(|triple| perm.forward(triple))
                .filter(|&(x, y, _)| x == a && y == b)
                .map(|(_, _, c)| c).collect();
            assert_eq!(idx.scan_prefix2(perm, a, b).collect::<Vec<_>>(), want);
        }
    }
}
~~~

Add a 40,000-c-value mega-group with a tiny tile budget and require all values from the chained iterator.

- [ ] **Step 2: Run red tests**

~~~sh
docker compose run --rm dev cargo test -p rete-core   index::tests::prefix2_neighbor_scan -- --nocapture
~~~

Expected: compile failure because scan_prefix2 is missing.

- [ ] **Step 3: Extract stored-order scan logic**

Move the body of scan_iter_with into:

~~~rust
fn scan_permuted_with(
    &self,
    permutation: IndexPermutation,
    pa: Option<u32>,
    pb: Option<u32>,
    pc: Option<u32>,
) -> impl Iterator<Item = Triple> + '_;
~~~

It retains the current tile span, prefetch ramp, synopsis check, tile loading, parsing, zone check, directory OnceLock, and safe/feature cursor. scan_iter_with maps the returned stored triples through permutation.back.

- [ ] **Step 4: Implement direct neighbor output**

~~~rust
pub(crate) fn scan_prefix2(
    &self,
    permutation: IndexPermutation,
    a: u32,
    b: u32,
) -> impl Iterator<Item = u32> + '_ {
    self.scan_permuted_with(permutation, Some(a), Some(b), None)
        .map(|(_, _, c)| c)
}
~~~

- [ ] **Step 5: Verify and commit**

~~~sh
docker compose run --rm dev cargo test -p rete-core index::tests
docker compose run --rm dev cargo test -p rete-core --no-default-features   index::tests::prefix2_neighbor_scan_matches_each_permutation -- --exact
docker compose run --rm dev cargo test -p rete-core --features unsafe-decode-bench   index::tests::unchecked_index_matches_safe_every_pattern -- --exact
git add crates/rete-core/src/index.rs
git commit -m "perf(core): stream two-bound neighbor ids"
~~~

---

### Task 5: Use Resolved Neighbor Scans in Property Paths

**Files:**

- Modify/test crates/rete-core/src/sparql/path.rs:13-295,297-553
- Modify/test crates/rete-core/tests/sparql_integration.rs
- Modify/test crates/bench/tests/differential.rs:246

**Interfaces:**

- ResolvedPathAst stores predicate IDs, excluded-ID sets, direction, and integer leaf keys.
- AdjCache becomes HashMap<(u32,u32), Vec<u32>> keyed by leaf and start node.
- Forward uses SPO (subject,predicate,object); reverse uses OPS (object,predicate,subject).
- Negated property sets retain general-pattern fallback.

- [ ] **Step 1: Write failing resolution/reverse-path tests**

~~~rust
#[test]
fn resolved_path_resolves_each_distinct_predicate_once() {
    let rete = fixture();
    let ast = alt(pred("<p>"), seq(pred("<p>"), pred("<q>")));
    let resolved = ResolvedPath::new(rete.dictionary(), &ast);
    assert_eq!(resolved.predicate_resolutions(), 2);
}
~~~

Add an integration graph n1-p-n0, n2-p-n1, n3-p-n1, n3-p-n2 and require SELECT ?x WHERE { ?x <p>+ <n0> } to return n1,n2,n3 once.

- [ ] **Step 2: Run red tests**

~~~sh
docker compose run --rm dev cargo test -p rete-core   sparql::path::tests::resolved_path_resolves_each_distinct_predicate_once -- --exact
~~~

Expected: compile failure because ResolvedPath is missing.

- [ ] **Step 3: Add the resolved representation**

~~~rust
enum ResolvedPathAst {
    Pred { key: u32, predicate: Option<u32>, reversed: bool },
    NegatedSet { key: u32, excluded: HashSet<u32>, reversed: bool },
    Rep(Box<ResolvedPathAst>, Rep),
    Seq(Box<ResolvedPathAst>, Box<ResolvedPathAst>),
    Alt(Box<ResolvedPathAst>, Box<ResolvedPathAst>),
}

struct PathResolver<'a> {
    dict: &'a Dictionary,
    ids: HashMap<String, Option<u32>>,
    next_key: u32,
    predicate_resolutions: u64,
}
~~~

Resolve each lexical predicate only on cache miss. Reverse the resolved structure without dictionary work: toggle leaf directions, reverse sequence order, recurse through alternatives/repetition.

- [ ] **Step 4: Replace predicate successor scans**

~~~rust
fn successors(
    ctx: &Ctx,
    index: &GraphIndex,
    cache: &mut AdjCache,
    key: u32,
    predicate: Option<u32>,
    reversed: bool,
    start: u32,
) -> Vec<u32> {
    if let Some(value) = cache.get(&(key, start)) {
        return value.clone();
    }
    let dict = ctx.rete.dictionary();
    let value = match predicate {
        Some(pid) if !reversed => dict.node_as_subject_id(start)
            .map(|sid| index.scan_prefix2(IndexPermutation::Spo, sid, pid)
                .map(|oid| dict.object_node(oid)).collect())
            .unwrap_or_default(),
        Some(pid) => dict.node_as_object_id(start)
            .map(|oid| index.scan_prefix2(IndexPermutation::Ops, oid, pid)
                .map(|sid| dict.subject_node(sid)).collect())
            .unwrap_or_default(),
        None => Vec::new(),
    };
    cache.insert((key, start), value.clone());
    value
}
~~~

Make reach_from consume ResolvedPathAst. Preserve BTreeSet/DFS/zero-length semantics. Resolve once at eval_path entry; bound-object traversal reverses the resolved tree. Negated sets use their pre-resolved exclusions with the existing match_pattern fallback.

- [ ] **Step 5: Run unit, integration, differential, and robustness gates**

~~~sh
docker compose run --rm dev cargo test -p rete-core sparql::path::tests
docker compose run --rm dev cargo test -p rete-core --test sparql_integration property_path
docker compose run --rm dev cargo test -p rete-bench --test differential   paths_and_aggregates_agree_with_oxigraph -- --exact
docker compose run --rm dev cargo test -p rete-core --test robustness
~~~

Expected: forward, reverse, sequence, alternative, negation, repetition, absent endpoints, and zero-length behavior all PASS.

- [ ] **Step 6: Commit**

~~~sh
git add crates/rete-core/src/sparql/path.rs   crates/rete-core/tests/sparql_integration.rs   crates/bench/tests/differential.rs
git commit -m "perf(sparql): use resolved neighbor scans for paths"
~~~

---

### Task 6: Add Benchmark-Only Metrics and Harnesses

**Files:**

- Create crates/rete-core/src/read_path_metrics.rs
- Modify crates/rete-core/src/lib.rs and Cargo.toml
- Modify triples.rs, index.rs, path.rs with feature/no-op hooks
- Create crates/bench/src/pathread.rs; modify crates/bench/src/main.rs and Cargo.toml
- Create scripts/bench_safe_path.sh, scripts/bench_safe_path_wasm.html, scripts/bench_safe_path_worker.js

**Interfaces:**

~~~rust
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct ReadPathStats {
    pub decoded_varints: u64,
    pub skipped_c_values: u64,
    pub path_probes: u64,
    pub predicate_resolutions: u64,
    pub directory_builds: u64,
    pub directory_bytes_total: u64,
    pub directory_bytes_max: u64,
    pub touched_tiles: u64,
}

pub fn reset_read_path_stats();
pub fn read_path_stats() -> ReadPathStats;
~~~

Feature read-path-metrics is off by default. Counters are thread-local; normal hooks inline to no-ops.

- [ ] **Step 1: Write a failing reset/unique-tile test**

~~~rust
#[test]
fn reset_clears_all_read_path_counters() {
    reset_read_path_stats();
    record_decoded_varint();
    record_path_probe();
    record_directory(123);
    record_tile(2, 7);
    record_tile(2, 7);
    let stats = read_path_stats();
    assert_eq!(stats.decoded_varints, 1);
    assert_eq!(stats.path_probes, 1);
    assert_eq!(stats.directory_bytes_total, 123);
    assert_eq!(stats.touched_tiles, 1);
    reset_read_path_stats();
    assert_eq!(read_path_stats(), ReadPathStats::default());
}
~~~

- [ ] **Step 2: Run red test, implement feature, and instrument**

~~~sh
docker compose run --rm dev cargo test -p rete-core --features read-path-metrics   read_path_metrics::tests::reset_clears_all_read_path_counters -- --exact
~~~

Expected: compile failure for the missing feature/module.

Add read-path-metrics = [] to core features. Record successful u32 decodes, skipped c values, completed directories/bytes, unique admitted tiles, uncached path probes, and predicate cache misses.

- [ ] **Step 3: Add native benchmark mode and alternating shell harness**

Add rete-bench --path-read FILE [samples]. Open once, warm once, run 15 samples, reset heap/counters per sample, require stable output, and report median/p90/heap/counters. scripts/bench_safe_path.sh compares baseline and candidate on the Chemotion path, full count, selective, and aggregate workloads with one warm-up and 15 alternating samples; fail on stdout hash mismatch.

- [ ] **Step 4: Build and run native acceptance**

~~~sh
docker compose run --rm -e CARGO_TARGET_DIR=/target/read-path-candidate   dev cargo build --release -p rete-cli --features unsafe-decode-bench
docker compose run --rm   -e RETE_SOURCE=/target/rete-rust-opt-bench/chemotion.rete   -e RETE_BASE_EXE=/target/read-path-baseline/release/rete   -e RETE_CANDIDATE_EXE=/target/read-path-candidate/release/rete   -e RETE_SAMPLES=15 dev bash scripts/bench_safe_path.sh
docker compose run --rm dev cargo run --release -p rete-bench --   --path-read /target/rete-rust-opt-bench/chemotion.rete 15
~~~

Acceptance: path is at most 100 ms or at least 30% faster than 145 ms; every control regresses at most 3%; max directory is 65,536 bytes; result hashes and touched ranges match.

- [ ] **Step 5: Add and run a release WASM Worker harness**

The Worker constructs one Graph, warms the exact Chemotion path query, times 15 graph.query(query,"json") calls with performance.now(), sorts samples, and reports samples[7] median and samples[13] p90 plus output. scripts/bench_safe_path_wasm.html imports /target/path-bench/pkg/rete_wasm.js and starts /scripts/bench_safe_path_worker.js. Build and serve the repository root so both URLs resolve:

~~~sh
docker compose run --rm wasm wasm-pack build crates/rete-wasm   --release --target no-modules --out-dir ../../target/path-bench/pkg
docker compose run --rm dev uv run python -m http.server 8008   --directory /work
~~~

Open http://localhost:8008/scripts/bench_safe_path_wasm.html. Expected: stable output and identical source bytes for baseline/candidate Worker runs. Record browser/version.

- [ ] **Step 6: Verify default boundaries and commit the harness**

~~~sh
docker compose run --rm dev cargo build -p rete-core --no-default-features
docker compose run --rm dev cargo build -p rete-core --all-features
docker compose run --rm wasm wasm-pack build crates/rete-wasm   --target web --out-dir ../../target/path-default-web
git add crates/rete-core/Cargo.toml crates/rete-core/src/lib.rs   crates/rete-core/src/read_path_metrics.rs crates/rete-core/src/triples.rs   crates/rete-core/src/index.rs crates/rete-core/src/sparql/path.rs   crates/bench/Cargo.toml crates/bench/src/main.rs crates/bench/src/pathread.rs   scripts/bench_safe_path.sh scripts/bench_safe_path_wasm.html   scripts/bench_safe_path_worker.js
git commit -m "bench: measure safe property-path reads"
~~~

Expected: all builds PASS; normal artifacts contain no enabled metrics.

---

### Task 7: Record Results and Verify Track 3

**Files:**

- Modify docs/BENCHMARK.md
- Regenerate docs/BENCHMARK.html

- [ ] **Step 1: Record exact measurements**

Include dataset length/ETag/SHA, executable hashes, all 15 local and WASM samples, median/p90, control workloads, every metric, heap, browser/version, output hashes, touched ranges, directory memory, and explicit acceptance verdict. Do not claim a gain for any component that misses its gate.

- [ ] **Step 2: Regenerate HTML and run focused gates**

~~~sh
docker compose run --rm dev cargo run -q -p docgen
docker compose run --rm dev cargo test -p rete-core triples::tests
docker compose run --rm dev cargo test -p rete-core index::tests
docker compose run --rm dev cargo test -p rete-core sparql::path::tests
docker compose run --rm dev cargo test -p rete-core --test sparql_integration property_path
docker compose run --rm dev cargo test -p rete-core --test robustness
docker compose run --rm dev cargo test -p rete-bench --test differential   paths_and_aggregates_agree_with_oxigraph -- --exact
docker compose run --rm dev cargo test -p rete-core --features unsafe-decode-bench
~~~

Expected: all PASS.

- [ ] **Step 3: Run repository gates and commit evidence**

~~~sh
docker compose run --rm dev cargo fmt --all -- --check
docker compose run --rm dev cargo clippy --workspace --exclude rete-bench --all-targets -- -D warnings
docker compose run --rm dev cargo test --workspace --exclude rete-bench
docker compose run --rm dev cargo test -p rete-core --no-default-features
docker compose run --rm dev cargo build -p rete-core --all-features
docker compose run --rm dev cargo build -p rete-bench
docker compose run --rm dev bash scripts/smoke.sh
git diff --check
git add docs/BENCHMARK.md docs/BENCHMARK.html
git commit -m "docs: record safe path read results"
~~~

Expected: all commands PASS; no format/header, generated package, dataset, or target artifact enters the diff.
