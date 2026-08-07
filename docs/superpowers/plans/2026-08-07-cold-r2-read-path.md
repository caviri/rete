# Cold Native R2 Read Path Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (- [ ]) syntax for tracking.

**Goal:** Preserve native HTTP batch-read capabilities and eager-open eligible remote objects up to 8 MiB so cold Chemotion queries need one data GET rather than 16–36.

**Architecture:** RangedSourceReader forwards the active backend's complete RangeReader behavior. sparql_url parses a bounded native-only threshold before networking, probes length, then uses one exact full-object range read plus Rete::open or the existing block-cached lazy path.

**Tech Stack:** Rust 2021, ureq, anyhow, Docker/devcontainer, Python, docgen.

## Global Constraints

- Default RETE_EAGER_MAX_MB is 8; 0 disables eager mode.
- Invalid, non-Unicode, non-integer, or overflowing values fail before HEAD or GET.
- Eager mode applies only to HTTP(S) sparql-url; local paths and WASM stay unchanged.
- Zero-length and above-threshold sources retain the lazy path.
- Exact response length, checked allocation, safe parsing, and incomplete-query errors remain mandatory.
- Do not change .rete bytes, add an async runtime, add production unsafe, or commit target artifacts.
- Run every Rust/Python command through the repository Docker toolchain.
- Commit without a co-author trailer.

## File Map

- crates/rete-cli/src/commands/range_source.rs: capability forwarding.
- crates/rete-cli/src/http.rs: exact bounded range bodies.
- crates/rete-cli/src/commands/url.rs: threshold and eager/lazy decision.
- crates/rete-cli/tests/common/mod.rs and remote_commands.rs: observable HTTP integration tests.
- crates/rete-cli/src/main.rs, crates/rete-cli/README.md, docs/cli.md, scripts/smoke.sh: public contract.
- scripts/bench_cold_r2.py: pinned real-R2 harness.
- docs/BENCHMARK.md and generated HTML: measured decision.

---

### Task 1: Preserve the Pre-Change Executable

**Files:** Ignored artifacts only: /target/bench/rete-cold-r2-baseline and its .sha256 file.

**Interfaces:** Produces the executable against which every candidate mode is compared.

- [ ] **Step 1: Verify branch and cleanliness**

~~~sh
git branch --show-current
git status --short
~~~

Expected: feat/rust-optimization; no output from status.

- [ ] **Step 2: Build and copy the baseline before editing production code**

~~~sh
docker compose run --rm dev cargo build --release -p rete-cli
docker compose run --rm dev bash -lc '
  mkdir -p /target/bench &&
  cp /target/release/rete /target/bench/rete-cold-r2-baseline &&
  sha256sum /target/bench/rete-cold-r2-baseline |
    tee /target/bench/rete-cold-r2-baseline.sha256
'
~~~

Expected: the executable and hash exist under ignored /target; git status remains clean.

---

### Task 2: Preserve RangedSourceReader Capabilities

**Files:**

- Modify/test: crates/rete-cli/src/commands/range_source.rs:79-136

**Interfaces:**

- Consumes RangeReader::read_many(&[(u64,u64)]) and RangeReader::concurrency().
- Produces matching overrides on RangedSourceReader, delegating to LocalRangeReader or HttpRangeReader.

- [ ] **Step 1: Write a failing HTTP forwarding test**

Add a threaded test range server which sleeps 40 ms per GET and records peak concurrent handlers:

~~~rust
#[test]
fn http_variant_preserves_batch_order_and_concurrency() {
    let data: Vec<u8> = (0..128u8).collect();
    let (url, max_active) = serve_parallel_ranges(data.clone());
    let reader = RangedSourceReader::open(&url).unwrap();
    assert_eq!(reader.concurrency(), 16);
    assert_eq!(
        reader.read_many(&[(64, 4), (0, 3), (32, 2)]).unwrap(),
        vec![data[64..68].to_vec(), data[0..3].to_vec(), data[32..34].to_vec()]
    );
    assert!(max_active.load(Ordering::SeqCst) >= 2);
}
~~~

Add local_variant_keeps_default_serial_concurrency using a temporary file and expecting 1.

- [ ] **Step 2: Prove the regression**

~~~sh
docker compose run --rm dev cargo test -p rete-cli   commands::range_source::tests::http_variant_preserves_batch_order_and_concurrency   -- --exact --nocapture
~~~

Expected: FAIL because the wrapper reports 1 and serializes the batch.

- [ ] **Step 3: Add minimal delegation**

~~~rust
fn read_many(&self, ranges: &[(u64, u64)]) -> io::Result<Vec<Vec<u8>>> {
    match self {
        Self::Local(reader) => reader.read_many(ranges),
        Self::Http(reader) => reader.read_many(ranges),
    }
}

fn concurrency(&self) -> usize {
    match self {
        Self::Local(reader) => reader.concurrency(),
        Self::Http(reader) => reader.concurrency(),
    }
}
~~~

- [ ] **Step 4: Run focused tests and commit**

~~~sh
docker compose run --rm dev cargo test -p rete-cli commands::range_source::tests
docker compose run --rm dev cargo test -p rete-cli http::tests
git add crates/rete-cli/src/commands/range_source.rs
git commit -m "fix(cli): preserve ranged source capabilities"
~~~

Expected: both suites PASS before the commit.

---

### Task 3: Require Exact HTTP Range Bodies

**Files:**

- Modify/test: crates/rete-cli/src/http.rs:42-81,140-388

**Interfaces:** Keeps RangeReader::read_at; rejects lengths that cannot fit usize and bodies whose declared or observed length differs from the request.

- [ ] **Step 1: Add an overlong response and failing test**

Add ServerMode::OverlongBody, returning 206 with the requested bytes plus one:

~~~rust
#[test]
fn rejects_an_overlong_range_response() {
    let data: Vec<u8> = (0..=255).cycle().take(1000).collect();
    let url = serve(data, ServerMode::OverlongBody);
    let reader = HttpRangeReader::open(&url).unwrap();
    let err = reader.read_at(100, 40).unwrap_err();
    assert!(err.to_string().contains("range response length"), "{err}");
}
~~~

- [ ] **Step 2: Verify the existing take(len) hides the excess**

~~~sh
docker compose run --rm dev cargo test -p rete-cli   http::tests::rejects_an_overlong_range_response -- --exact --nocapture
~~~

Expected: FAIL at unwrap_err().

- [ ] **Step 3: Implement bounded exact reading**

~~~rust
let capacity = usize::try_from(len).map_err(|_| {
    io::Error::new(io::ErrorKind::InvalidInput, "HTTP range does not fit in memory")
})?;
if let Some(declared) = resp.header("content-length").and_then(|v| v.parse().ok()) {
    if declared != len {
        return Err(io::Error::other(format!(
            "range response length mismatch: declared {declared}, expected {len}"
        )));
    }
}
let mut body = Vec::with_capacity(capacity);
resp.into_reader().take(len.saturating_add(1)).read_to_end(&mut body)?;
match (body.len() as u64).cmp(&len) {
    std::cmp::Ordering::Less => Err(io::Error::other("short range response")),
    std::cmp::Ordering::Greater => Err(io::Error::other("overlong range response")),
    std::cmp::Ordering::Equal => Ok(body),
}
~~~

Preserve offset, length, and URL in the real error messages.

- [ ] **Step 4: Verify and commit**

~~~sh
docker compose run --rm dev cargo test -p rete-cli http::tests -- --nocapture
git add crates/rete-cli/src/http.rs
git commit -m "fix(cli): require exact HTTP range bodies"
~~~

Expected: ignored-range, truncation, overflow, overlong, server-error, and reuse tests all PASS.

---

### Task 4: Add the Adaptive Eager Policy

**Files:**

- Modify/test: crates/rete-cli/src/commands/url.rs:11-18,126-185
- Modify: crates/rete-cli/tests/common/mod.rs:78-160
- Modify/test: crates/rete-cli/tests/remote_commands.rs

**Interfaces:**

~~~rust
const DEFAULT_EAGER_MAX_BYTES: u64 = 8 * 1024 * 1024;
fn parse_eager_max_bytes(raw: Option<&OsStr>) -> anyhow::Result<u64>;
fn eager_max_bytes() -> anyhow::Result<u64>;
fn should_eager_open(source: &str, len: u64, max: u64) -> bool;
~~~

- [ ] **Step 1: Write failing pure policy tests**

~~~rust
#[test]
fn eager_threshold_contract() {
    assert_eq!(parse_eager_max_bytes(None).unwrap(), 8 * 1024 * 1024);
    assert_eq!(parse_eager_max_bytes(Some(OsStr::new("0"))).unwrap(), 0);
    assert_eq!(parse_eager_max_bytes(Some(OsStr::new("16"))).unwrap(), 16 << 20);
    for raw in ["-1", "8.5", "eight", "18446744073709551615"] {
        assert!(parse_eager_max_bytes(Some(OsStr::new(raw))).is_err());
    }
}

#[test]
fn eager_policy_is_http_nonempty_bounded_and_inclusive() {
    let max = 8 << 20;
    assert!(should_eager_open("https://host/g.rete", max, max));
    assert!(!should_eager_open("https://host/g.rete", 0, max));
    assert!(!should_eager_open("https://host/g.rete", max + 1, max));
    assert!(!should_eager_open("graph.rete", 1024, max));
    assert!(!should_eager_open("https://host/g.rete", 1024, 0));
}
~~~

- [ ] **Step 2: Run red tests**

~~~sh
docker compose run --rm dev cargo test -p rete-cli commands::url::tests -- --nocapture
~~~

Expected: compile failure for missing helpers.

- [ ] **Step 3: Implement strict parsing and selection**

~~~rust
fn parse_eager_max_bytes(raw: Option<&OsStr>) -> anyhow::Result<u64> {
    let Some(raw) = raw else { return Ok(DEFAULT_EAGER_MAX_BYTES) };
    let text = raw.to_str().ok_or_else(|| anyhow::anyhow!(
        "RETE_EAGER_MAX_MB must be valid UTF-8"
    ))?;
    let mb = text.parse::<u64>().map_err(|_| anyhow::anyhow!(
        "RETE_EAGER_MAX_MB must be a non-negative integer, got {text}"
    ))?;
    let bytes = mb.checked_mul(1024 * 1024).ok_or_else(|| anyhow::anyhow!(
        "RETE_EAGER_MAX_MB value {text} overflows its byte count"
    ))?;
    usize::try_from(bytes).map_err(|_| anyhow::anyhow!(
        "RETE_EAGER_MAX_MB value {text} exceeds this platform"
    ))?;
    Ok(bytes)
}

fn should_eager_open(source: &str, len: u64, max: u64) -> bool {
    crate::commands::range_source::is_url(source) && len != 0 && max != 0 && len <= max
}
~~~

- [ ] **Step 4: Make the integration server observable**

Add this to tests/common/mod.rs, have serve call serve_with_stats(...).0, and add RangeMode::Truncate:

~~~rust
#[derive(Default)]
pub struct RangeStats {
    pub heads: AtomicUsize,
    pub gets: AtomicUsize,
    pub ranges: Mutex<Vec<(usize, usize)>>,
}

pub fn serve_with_stats(data: Vec<u8>, mode: RangeMode) -> (String, Arc<RangeStats>);
~~~

- [ ] **Step 5: Write failing CLI policy tests**

~~~rust
#[test]
fn sparql_url_eager_fetches_an_eligible_object_once() {
    let fixture = common::fixture();
    let bytes = std::fs::read(&fixture.rete).unwrap();
    let len = bytes.len();
    let (url, stats) = common::serve_with_stats(bytes, common::RangeMode::Honor);
    let output = common::rete()
        .env("RETE_EAGER_MAX_MB", "8")
        .args(["sparql-url", &url, SELECT, "--json"])
        .output().unwrap();
    assert!(output.status.success());
    assert_eq!(stats.heads.load(Ordering::SeqCst), 1);
    assert_eq!(stats.gets.load(Ordering::SeqCst), 1);
    assert_eq!(*stats.ranges.lock().unwrap(), vec![(0, len - 1)]);
}

#[test]
fn invalid_eager_configuration_fails_before_networking() {
    let fixture = common::fixture();
    let (url, stats) = common::serve_with_stats(
        std::fs::read(&fixture.rete).unwrap(), common::RangeMode::Honor
    );
    common::rete().env("RETE_EAGER_MAX_MB", "eight")
        .args(["sparql-url", &url, SELECT, "--json"])
        .assert().code(1)
        .stderr(predicate::str::contains("RETE_EAGER_MAX_MB"));
    assert_eq!(stats.heads.load(Ordering::SeqCst), 0);
    assert_eq!(stats.gets.load(Ordering::SeqCst), 0);
}
~~~

Also test forced lazy (threshold 0), eager/lazy stdout identity, truncated response, ignored Range, and malformed eligible bytes.

- [ ] **Step 6: Prove current behavior fails**

~~~sh
docker compose run --rm dev cargo test -p rete-cli --test remote_commands   sparql_url_eager -- --nocapture
docker compose run --rm dev cargo test -p rete-cli --test remote_commands   invalid_eager_configuration -- --nocapture
~~~

Expected: the eager test sees exactly one full-file Range GET after the length probe; invalid configuration performs no network request.

- [ ] **Step 7: Branch after probing length**

Parse the environment before RangedSourceReader::open. Compute block only in the lazy branch:

~~~rust
let eager_max = eager_max_bytes()?;
let reader = Arc::new(CountingReader::new(RangedSourceReader::open(url)?));
let total = reader.len();
let mut rete = if should_eager_open(url, total, eager_max) {
    let image = reader.read_at(0, total)?;
    Rete::open(&image)?
} else {
    let block = match std::env::var("RETE_BLOCK_KB")
        .ok()
        .and_then(|raw| raw.parse::<u64>().ok())
    {
        Some(kb) => kb
            .checked_mul(1024)
            .ok_or_else(|| anyhow::anyhow!("RETE_BLOCK_KB overflows u64"))?,
        None => auto_block(total),
    };
    if block == 0 {
        Rete::open_ranged_lazy(reader.clone())?
    } else {
        Rete::open_ranged_lazy(Arc::new(BlockCacheReader::new(reader.clone(), block)))?
    }
};
~~~

Keep service setup, the feature-gated research flag, evaluation, incomplete guard, rendering, and counters unchanged.

- [ ] **Step 8: Verify and commit**

~~~sh
docker compose run --rm dev cargo test -p rete-cli commands::url::tests
docker compose run --rm dev cargo test -p rete-cli --test remote_commands
docker compose run --rm dev cargo test -p rete-cli http::tests
git add crates/rete-cli/src/commands/url.rs         crates/rete-cli/tests/common/mod.rs         crates/rete-cli/tests/remote_commands.rs
git commit -m "feat(cli): eager-open small remote SPARQL files"
~~~

Expected: all tests PASS; eligible HTTP uses one GET and outputs remain identical.

---

### Task 5: Publish and Smoke-Test the CLI Contract

**Files:**

- Modify crates/rete-cli/src/main.rs:648-665
- Modify crates/rete-cli/README.md:22-31
- Modify docs/cli.md:478-489
- Modify scripts/smoke.sh:263-275
- Regenerate docs/cli.html

**Interfaces:** Documents threshold 8, disable value 0, HTTP-only behavior, and one-GET accounting.

- [ ] **Step 1: Correct command help**

~~~rust
/// Run SPARQL against a remote .rete: files up to 8 MiB are fetched eagerly;
/// larger files use lazy HTTP range reads. RETE_EAGER_MAX_MB=0 forces lazy.
~~~

- [ ] **Step 2: Extend smoke coverage**

~~~sh
check "sparql-url eager" "Bob|solution|1 range request" --   env RETE_EAGER_MAX_MB=8 $B sparql-url   "http://127.0.0.1:8099/web.rete"   "PREFIX e: <http://ex/> SELECT ?y WHERE { e:Alice e:knows ?y }"

check "sparql-url forced lazy" "Bob|solution|range request" --   env RETE_EAGER_MAX_MB=0 RETE_BLOCK_KB=0 $B sparql-url   "http://127.0.0.1:8099/web.rete"   "PREFIX e: <http://ex/> SELECT ?y WHERE { e:Alice e:knows ?y }"
~~~

- [ ] **Step 3: Update Markdown and generate HTML**

Document the HEAD probe, exact eager GET, lazy fallback, invalid configuration, and native-only scope. Replace "without downloading it first" in the CLI README with "directly from its host."

~~~sh
docker compose run --rm dev cargo run -q -p docgen
rg -n "RETE_EAGER_MAX_MB" docs/cli.md docs/cli.html
git diff --check
~~~

Expected: both files contain the setting and have no whitespace errors.

- [ ] **Step 4: Verify and commit**

~~~sh
docker compose run --rm dev cargo test -p rete-cli --test help_contract
docker compose run --rm dev bash scripts/smoke.sh
git add crates/rete-cli/src/main.rs crates/rete-cli/README.md         docs/cli.md docs/cli.html scripts/smoke.sh
git commit -m "docs(cli): explain adaptive remote opening"
~~~

---

### Task 6: Benchmark and Decide

**Files:**

- Create scripts/bench_cold_r2.py
- Modify docs/BENCHMARK.md
- Regenerate docs/BENCHMARK.html

**Interfaces:** The harness compares baseline_lazy, delegated_lazy, and eager_8, plus a 0,4,8,16 MiB threshold sweep.

- [ ] **Step 1: Implement the pinned harness**

Reuse the three queries in scripts/bench_unsafe_decode.sh. Emit one JSONL record per fresh-process run:

~~~python
record = {
    "query": label,
    "mode": mode.name,
    "run": run,
    "wall_ms": wall_ms,
    "bytes": int(match.group(1)),
    "gets": int(match.group(2)),
    "peak_rss_kib": peak_rss_kib,
    "sha256": hashlib.sha256(stdout).hexdigest(),
    "length": length,
    "etag": etag,
}
~~~

HEAD before and after; require length 7566404 and ETag "6cefd111dee3c59c063f0bede9cd60f9"; alternate mode order; measure monotonic wall time; poll /proc/<pid>/status for VmHWM; parse stderr; compute median and nearest-rank p90; fail on hash mismatch.

- [ ] **Step 2: Smoke-test and commit the harness**

~~~sh
docker compose run --rm dev cargo build --release -p rete-cli
docker compose run --rm dev python3 scripts/bench_cold_r2.py   --baseline /target/bench/rete-cold-r2-baseline   --candidate /target/release/rete --samples 1   --source https://data.graphplaza.com/chemotion/chemotion.rete   --out /target/bench/cold-r2-smoke.jsonl
git add scripts/bench_cold_r2.py
git commit -m "bench(cli): measure cold R2 opening modes"
~~~

Expected: stable hash per query; eager reports one GET and 7,566,404 bytes.

- [ ] **Step 3: Run acceptance samples**

~~~sh
docker compose run --rm dev python3 scripts/bench_cold_r2.py   --baseline /target/bench/rete-cold-r2-baseline   --candidate /target/release/rete --samples 15   --source https://data.graphplaza.com/chemotion/chemotion.rete   --out /target/bench/cold-r2-15.jsonl

docker compose run --rm dev python3 scripts/bench_cold_r2.py   --candidate /target/release/rete --thresholds 0,4,8,16 --samples 15   --source https://data.graphplaza.com/chemotion/chemotion.rete   --out /target/bench/cold-r2-thresholds.jsonl
~~~

Acceptance: one eager GET; identical hashes; at least two queries improve median by at least 25%; no median or p90 regression; report RSS. If eager misses, keep capability/exact-body fixes, revert only the eager production commit, and document rejection.

- [ ] **Step 4: Record and commit results**

Add date, URL, length, ETag, executable hashes, all median/p90/bytes/GET/RSS results, threshold sweep, and verdict:

~~~sh
docker compose run --rm dev cargo run -q -p docgen
git diff --check
git add docs/BENCHMARK.md docs/BENCHMARK.html
git commit -m "docs: record cold R2 opening benchmark"
~~~

---

### Task 7: Verify Track 1

**Files:** No new files.

- [ ] **Step 1: Run focused and repository gates**

~~~sh
docker compose run --rm dev cargo fmt --all -- --check
docker compose run --rm dev cargo clippy --workspace --exclude rete-bench --all-targets -- -D warnings
docker compose run --rm dev cargo test --workspace --exclude rete-bench
docker compose run --rm dev cargo test -p rete-core --no-default-features
docker compose run --rm dev cargo build -p rete-core --all-features
docker compose run --rm dev cargo build -p rete-bench
docker compose run --rm dev cargo test -p rete-cli --features unsafe-decode-bench
docker compose run --rm dev bash scripts/smoke.sh
~~~

Expected: every command exits zero.

- [ ] **Step 2: Check generated files and one real-object sample**

~~~sh
docker compose run --rm dev cargo run -q -p docgen
git diff --check
git status --short
docker compose run --rm dev python3 scripts/bench_cold_r2.py   --candidate /target/release/rete --thresholds 0,8 --samples 1   --source https://data.graphplaza.com/chemotion/chemotion.rete   --out /target/bench/cold-r2-final.jsonl
~~~

Expected: no generated drift or unintended artifacts; hashes match and threshold 8 uses one data GET.
