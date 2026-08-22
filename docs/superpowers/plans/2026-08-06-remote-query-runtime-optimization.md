# Remote Query Runtime Optimization Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Reuse native HTTP connections and remove per-block copies from coalesced cache fetches while preserving exact remote-query behavior and cache limits.

**Architecture:** `HttpRangeReader` retains one shared `ureq::Agent`. `BlockCacheReader` stores one immutable backing allocation per coalesced fetch and maps block numbers to subranges, with backing-aware LRU accounting. Existing `RangeReader -> Vec<u8>` output and HTTP concurrency remain unchanged.

**Tech Stack:** Rust 2021, ureq 2, rete-core `RangeReader`, Docker Compose dev image.

## Global Constraints

- Run builds and tests through `docker compose run --rm dev`.
- Preserve HTTP range validation, short-read errors, query results, and public APIs.
- Preserve the 16-request concurrency limit and request ordering.
- Keep production code safe Rust.
- Do not change `.rete` format bytes.

---

### Task 1: Retain and reuse the CLI HTTP agent

**Files:**
- Modify: `crates/rete-cli/src/http.rs:12-116`
- Test: `crates/rete-cli/src/http.rs:119-286`

**Interfaces:**
- Consumes: `ureq::builder() -> AgentBuilder`, `Agent::{head,get}`.
- Produces: `HttpRangeReader { agent: ureq::Agent, url: String, len: u64 }` with unchanged `RangeReader` behavior.

- [ ] **Step 1: Add a failing keep-alive reuse test**

Add a localhost HTTP/1.1 fixture that counts accepted TCP connections, handles the HEAD probe plus two GET ranges, sends exact `Content-Length`, and keeps each connection alive. Add this test:

```rust
#[test]
fn reuses_the_open_agent_for_sequential_ranges() {
    let data: Vec<u8> = (0..=255u8).cycle().take(1000).collect();
    let (url, accepted) = serve_keep_alive(data.clone(), 3);
    let reader = HttpRangeReader::open(&url).unwrap();

    assert_eq!(reader.read_at(10, 8).unwrap(), &data[10..18]);
    assert_eq!(reader.read_at(500, 16).unwrap(), &data[500..516]);
    assert_eq!(accepted.load(std::sync::atomic::Ordering::SeqCst), 1);
}
```

The fixture must read request heads until `\r\n\r\n`, return no body for HEAD, return `206` plus the requested slice for GET, and use `Connection: keep-alive`.

Add a second test that calls `read_at(u64::MAX, 2)` and expects
`std::io::ErrorKind::InvalidInput` without issuing a GET. This guards the
inclusive HTTP end calculation against integer overflow.

- [ ] **Step 2: Run the focused test and verify RED**

Run:

```sh
docker compose run --rm dev cargo test -p rete-cli http::tests::reuses_the_open_agent_for_sequential_ranges -- --exact
```

Expected: FAIL because the current top-level `ureq::head` and `ureq::get` calls use separate one-shot agents and the server accepts more than one connection.

- [ ] **Step 3: Store and use one agent**

Implement the production change:

```rust
pub struct HttpRangeReader {
    agent: ureq::Agent,
    url: String,
    len: u64,
}

pub fn open(url: &str) -> anyhow::Result<Self> {
    let agent = ureq::builder()
        .timeout(std::time::Duration::from_secs(120))
        .build();
    let resp = agent.head(url).call()?;
    // existing Content-Length parsing
    Ok(Self { agent, url: url.to_string(), len })
}
```

Change `read_at` to calculate the inclusive end with
`offset.checked_add(len - 1)` and return `InvalidInput` on overflow, then start
the request with `self.agent.get(&self.url)`. Do not change status, body-length,
or concurrency handling.

- [ ] **Step 4: Verify GREEN and regression coverage**

Run:

```sh
docker compose run --rm dev cargo test -p rete-cli http::tests -- --nocapture
docker compose run --rm dev cargo test -p rete-cli --test remote_commands
```

Expected: all focused HTTP and remote CLI tests pass.

- [ ] **Step 5: Commit the HTTP change**

```sh
git add crates/rete-cli/src/http.rs
git commit -m "perf(cli): reuse HTTP range connections"
```

---

### Task 2: Share cache backing allocations

**Files:**
- Modify: `crates/rete-core/src/block_cache.rs:66-299`
- Test: `crates/rete-core/src/block_cache.rs:302-428`

**Interfaces:**
- Consumes: coalesced `Vec<u8>` blobs returned by `RangeReader::read_many`.
- Produces: private `Backing`, `CacheEntry`, and `ResidentSlice` types; unchanged `BlockCacheReader<R>: RangeReader` API.

- [ ] **Step 1: Add failing physical-accounting and short-read tests**

Add a test proving one oversized shared span cannot pretend to release memory one logical block at a time:

```rust
#[test]
fn shared_span_is_counted_until_its_last_block_is_evicted() {
    let data: Vec<u8> = (0..32 * 1024u32).map(|i| i as u8).collect();
    let reader = BlockCacheReader::new(SliceReader::new(&data), 4096)
        .with_cache_cap(8 * 1024);

    assert_eq!(reader.read_at(0, 12 * 1024).unwrap(), &data[..12 * 1024]);
    assert_eq!(reader.cached_bytes(), 0,
        "one 12 KiB backing cannot be partly retained under an 8 KiB cap");
}
```

Add a private `ShortReader` whose `read_at` and `read_many` return one byte fewer than requested, then assert `BlockCacheReader::read_at` returns `UnexpectedEof` rather than a short successful vector.

- [ ] **Step 2: Run the focused tests and verify RED**

Run:

```sh
docker compose run --rm dev cargo test -p rete-core block_cache::tests::shared_span_is_counted_until_its_last_block_is_evicted -- --exact
docker compose run --rm dev cargo test -p rete-core block_cache::tests::short_inner_read_is_an_error -- --exact
```

Expected: the accounting test reports 8 KiB retained by separately copied blocks, and the short-read test receives a short successful vector.

- [ ] **Step 3: Replace per-block allocations with backing subranges**

Use these private shapes:

```rust
struct Backing {
    data: Arc<[u8]>,
    resident_blocks: usize,
}

struct CacheEntry {
    backing: u64,
    range: std::ops::Range<usize>,
    stamp: u64,
}

struct ResidentSlice {
    data: Arc<[u8]>,
    range: std::ops::Range<usize>,
}

struct CacheState {
    map: HashMap<u64, CacheEntry>,
    backings: HashMap<u64, Backing>,
    next_backing: u64,
    used: u64,
    tick: u64,
}
```

For each fetched blob, first require `blob.len() as u64 == span.len`; otherwise return `UnexpectedEof`. Convert the blob once with `Arc::<[u8]>::from(blob)`, add its full length to `used`, and insert block entries containing only ranges. When replacing or evicting an entry, decrement its backing's `resident_blocks`; remove the backing and subtract its full length when the count reaches zero.

`assemble` clones `ResidentSlice` values under the mutex and copies `&data[range.clone()]` into the final output. Preserve the direct-reader fallback for a concurrently evicted block and require the final output length to equal the requested length.

- [ ] **Step 4: Verify GREEN and all cache invariants**

Run:

```sh
docker compose run --rm dev cargo test -p rete-core block_cache::tests -- --nocapture
docker compose run --rm dev cargo test -p rete-core --test ranged
```

Expected: all cache, lazy range, cap, and retry tests pass.

- [ ] **Step 5: Commit the cache change**

```sh
git add crates/rete-core/src/block_cache.rs
git commit -m "perf(core): share coalesced cache storage"
```

---

### Task 3: Measure the real R2 workload

**Files:**
- Modify after reproducible results: `docs/BENCHMARK.md`
- Regenerate after Markdown change: `docs/BENCHMARK.html`

**Interfaces:**
- Consumes: pinned `/target/rete-rust-optimization-baseline`, optimized `/target/release/rete`, and the three Chemotion catalog SPARQL queries in `web/playground-src/catalog.js`.
- Produces: byte-identical result hashes and alternating baseline/optimized timing samples.

- [ ] **Step 1: Build and hash the optimized release binary**

```sh
docker compose run --rm dev cargo build --release -p rete-cli
docker compose run --rm dev sha256sum /target/release/rete /target/rete-rust-optimization-baseline
```

- [ ] **Step 2: Verify outputs before timing**

For each catalog query, run both executables with `sparql-url --json`, save stdout in a temporary directory, and compare `sha256sum`. Any mismatch fails the benchmark.

- [ ] **Step 3: Alternate old and new timings**

In one container session, warm each executable once, then alternate execution order for seven measured samples per binary and query. Capture wall-clock milliseconds, stderr byte/request counts, median, minimum, and maximum. Do not combine results from different R2 object ETags.

- [ ] **Step 4: Record only reproducible findings**

If repeated alternating runs agree on direction outside ordinary network variance, add the command, object length/ETag, every sample, medians, and percentage deltas to `docs/BENCHMARK.md`, then regenerate HTML:

```sh
docker compose run --rm dev cargo run -q -p docgen
```

- [ ] **Step 5: Commit reproducible benchmark documentation**

```sh
git add docs/BENCHMARK.md docs/BENCHMARK.html
git commit -m "docs: record R2 range optimization benchmark"
```
