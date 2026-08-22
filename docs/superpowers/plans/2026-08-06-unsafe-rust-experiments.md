# Unsafe Rust Experiments Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Measure unchecked triple-block decoding on complete SPARQL queries and evaluate uninitialized FFI output buffers without changing normal builds.

**Architecture:** A non-default Cargo feature adds an alternate unchecked block cursor and a hidden CLI selection flag. Safe and unchecked results are tested and benchmarked side by side. FFI buffer changes remain experimental until an end-to-end host measurement justifies production code.

**Tech Stack:** Rust 2021, Cargo features, Clap, Docker release builds, WASM/Java host bindings.

## Global Constraints

- Default `rete-core`, CLI, and WASM behavior must remain unchanged and bounds-safe.
- The unchecked path compiles only with `unsafe-decode-bench`.
- Every unsafe block documents lifetime, initialization, bounds, aliasing, provenance, and failure-path invariants that apply to its controlled benchmark input.
- Never run the unchecked path against arbitrary user URLs or malformed fixtures.
- Safe and unchecked SPARQL output must be byte-identical before timing is considered.

---

### Task 1: Add an unchecked triple-block cursor behind a feature

**Files:**
- Modify: `crates/rete-core/Cargo.toml`
- Modify: `crates/rete-core/src/triples.rs:324-445`
- Test: `crates/rete-core/src/triples.rs:447-585`

**Interfaces:**
- Consumes: builder-produced, immutable, valid triple-block bytes.
- Produces: feature-gated `unsafe fn TripleBlock::scan_unchecked(...) -> UncheckedBlockCursor<'a>` and `unsafe fn group_directory_unchecked()`.

- [ ] **Step 1: Add the feature and a failing equivalence test**

Declare an empty non-default feature:

```toml
unsafe-decode-bench = []
```

Under `#[cfg(feature = "unsafe-decode-bench")]`, add a test that builds a block, enumerates representative present/absent values for all eight bound/unbound pattern shapes, and compares sorted results from `scan` and `unsafe { scan_unchecked(...) }`. Add a second comparison for safe and unchecked group directories through `scan_from`.

- [ ] **Step 2: Run the feature test and verify RED**

```sh
docker compose run --rm dev cargo test -p rete-core --features unsafe-decode-bench triples::tests::unchecked_cursor_matches_safe_every_pattern -- --exact
```

Expected: compilation fails because the unchecked cursor API does not exist.

- [ ] **Step 3: Implement the isolated unchecked varint reader**

Add a feature-gated helper that reads builder-emitted `u32` LEB128 values:

```rust
#[cfg(feature = "unsafe-decode-bench")]
#[inline(always)]
unsafe fn rd_unchecked(bytes: &[u8], pos: &mut usize) -> u32 {
    let mut value = 0u32;
    let mut shift = 0u32;
    loop {
        // SAFETY: the caller guarantees `bytes` is a complete block produced by
        // rete's encoder, so each requested u32 LEB128 starts below bytes.len(),
        // terminates within five bytes, and `pos` advances within this allocation.
        let byte = unsafe { *bytes.get_unchecked(*pos) };
        *pos += 1;
        value |= u32::from(byte & 0x7f) << shift;
        if byte & 0x80 == 0 {
            return value;
        }
        shift += 7;
    }
}
```

Implement `UncheckedBlockCursor` with the same state machine and wrapping delta arithmetic as `BlockCursor`, replacing only `rd` calls. Implement unchecked directory construction with the same loop structure. Keep safe types untouched.

- [ ] **Step 4: Verify GREEN plus default isolation**

```sh
docker compose run --rm dev cargo test -p rete-core --features unsafe-decode-bench triples::tests -- --nocapture
docker compose run --rm dev cargo test -p rete-core --no-default-features
```

The first command proves equivalence on generated valid blocks. The second proves the default/no-default build does not need the experiment.

---

### Task 2: Propagate the experimental decode mode through SPARQL

**Files:**
- Modify: `crates/rete-cli/Cargo.toml`
- Modify: `crates/rete-core/src/index.rs:502-894`
- Modify: `crates/rete-core/src/file.rs:1658-1684,2560-2583`
- Modify: `crates/rete-cli/src/main.rs:648-661,1077-1082`
- Modify: `crates/rete-cli/src/commands/url.rs:120-175`
- Test: `crates/rete-core/src/index.rs:951-1117`

**Interfaces:**
- Consumes: the feature-gated unchecked cursor from Task 1.
- Produces: unsafe `Rete::assume_valid_index_blocks`, feature-forwarding CLI feature, and hidden `sparql-url --unsafe-decode`.

- [ ] **Step 1: Add a failing index-mode equivalence test**

Under the feature, build two identical indexes. Leave one safe; call `unsafe { unchecked.assume_valid_blocks() }` on the other. Compare `match_pattern` results for every pattern in the existing brute-force matrix and for tiny multi-tile budgets.

- [ ] **Step 2: Run the index test and verify RED**

```sh
docker compose run --rm dev cargo test -p rete-core --features unsafe-decode-bench index::tests::unchecked_index_matches_safe_every_pattern -- --exact
```

Expected: compilation fails because mode propagation does not exist.

- [ ] **Step 3: Add decode mode without affecting default iterator types**

Add a feature-gated boolean to `GraphIndex`, defaulting to `false` in every constructor. Add unsafe setters on `GraphIndex` and `Rete`; the Rete setter updates the default and named-graph indexes.

Create a feature-gated enum that implements `Iterator<Item = Triple>`:

```rust
enum DecodeCursor<'a> {
    Safe(BlockCursor<'a>),
    Unchecked(UncheckedBlockCursor<'a>),
}
```

Dispatch once when a tile is parsed, then delegate `next` through the enum. Do not branch inside each varint read. Safe mode continues using `TripleBlock::parse`, `group_directory`, `scan`, and `scan_from` exactly as before.

- [ ] **Step 4: Add the hidden CLI flag behind the forwarding feature**

In `rete-cli/Cargo.toml`:

```toml
[features]
unsafe-decode-bench = ["rete-core/unsafe-decode-bench"]
```

Compile the Clap field only with this feature:

```rust
#[cfg(feature = "unsafe-decode-bench")]
#[arg(long, hide = true)]
unsafe_decode: bool,
```

When selected, print a warning naming the controlled benchmark assumption, then call `unsafe { rete.assume_valid_index_blocks() }` before evaluation. Default builds neither parse nor display the flag.

- [ ] **Step 5: Verify feature and default CLI behavior**

```sh
docker compose run --rm dev cargo test -p rete-core --features unsafe-decode-bench index::tests -- --nocapture
docker compose run --rm dev cargo test -p rete-cli --features unsafe-decode-bench
docker compose run --rm dev cargo test -p rete-cli
docker compose run --rm dev cargo run -q -p rete-cli -- sparql-url --help
```

Expected: feature tests pass; default help contains no unsafe option.

- [ ] **Step 6: Commit the isolated experiment**

```sh
git add crates/rete-core/Cargo.toml crates/rete-cli/Cargo.toml \
  crates/rete-core/src/triples.rs crates/rete-core/src/index.rs \
  crates/rete-core/src/file.rs crates/rete-cli/src/main.rs \
  crates/rete-cli/src/commands/url.rs
git commit -m "experiment: benchmark unchecked triple decoding"
```

---

### Task 3: Benchmark safe versus unchecked SPARQL

**Files:**
- Modify after reproducible results: `docs/BENCHMARK.md`
- Regenerate after Markdown change: `docs/BENCHMARK.html`

**Interfaces:**
- Consumes: feature-enabled release CLI, pinned Chemotion R2 object, three catalog queries.
- Produces: safe/unchecked output hashes, CPU/local timing, R2 end-to-end timing, and a keep/remove decision.

- [ ] **Step 1: Build the feature-enabled release binary**

```sh
docker compose run --rm dev cargo build --release -p rete-cli --features unsafe-decode-bench
```

- [ ] **Step 2: Prove output identity**

Run every Chemotion query once in safe mode and once with `--unsafe-decode`; compare stdout SHA-256. Abort the experiment on any mismatch or changed R2 ETag.

- [ ] **Step 3: Measure a local decode-heavy workload**

Run safe and unchecked modes alternately against the same local immutable `.rete` file, at least 15 samples per mode after warm-up. Choose a resolution-heavy query without network I/O and report median and spread.

- [ ] **Step 4: Measure the R2 workload**

Alternate safe and unchecked executions for seven samples per catalog query. Record complete process wall time, bytes, requests, and percentage delta. Treat differences inside network variance as no demonstrated end-to-end win.

- [ ] **Step 5: Record the decision**

Document the measurements and whether the experiment is retained on the branch. Explicitly state that default artifacts remain safe regardless of the result. Regenerate `docs/BENCHMARK.html` if Markdown changes.

---

### Task 4: Evaluate uninitialized FFI output buffers

**Files:**
- Inspect and modify only if accepted: `clients/java/ffi/src/lib.rs:129-165`
- Inspect and modify only if accepted: `crates/rete-wasm/src/lib.rs:1235-1266`
- Test: native helper tests colocated with an accepted implementation.

**Interfaces:**
- Consumes: host imports that promise to initialize the returned byte prefix.
- Produces only if accepted: a small helper that owns uninitialized byte capacity and exposes initialized bytes after validating `got <= capacity`.

- [ ] **Step 1: Measure the zero-fill upper bound without production changes**

Use a temporary release benchmark that compares `vec![0u8; len]` followed by a full overwrite against `Vec::with_capacity(len)` followed by the same full overwrite and `set_len`. Test representative 64 KiB, 512 KiB, and multi-range aggregate sizes. Use `black_box` and checksum every output so writes cannot be optimized away.

- [ ] **Step 2: Attempt an end-to-end host measurement**

For Java, build the existing Chicory WASM artifact and run repeated host range reads through the Java client. For WASM Asyncify, use the existing async build pipeline and a local range server. Compare identical queries and outputs. If the host/runtime overhead hides the initialization difference, reject the production change.

- [ ] **Step 3: If accepted, write failing helper-contract tests**

Tests must cover exact fill, zero length, short fill, and a malicious reported length greater than capacity. The latter three must return clean errors without exposing a typed uninitialized byte.

- [ ] **Step 4: If accepted, implement one narrowly documented helper per crate**

Keep vector length zero across the host call, validate the returned length against capacity, and set length only after the host contract guarantees the prefix is initialized. Put the complete safety proof immediately above `set_len`. Do not share target-specific FFI code through rete-core.

The Java helper has this shape (the WASM helper uses `usize` for `got`):

```rust
unsafe fn host_filled_exact(
    len: usize,
    fill: impl FnOnce(*mut u8) -> u32,
) -> std::io::Result<Vec<u8>> {
    let mut bytes = Vec::with_capacity(len);
    let got = fill(bytes.as_mut_ptr()) as usize;
    if got != len {
        return Err(std::io::Error::new(
            std::io::ErrorKind::UnexpectedEof,
            format!("host range read returned {got} of {len} bytes"),
        ));
    }
    // SAFETY: the caller guarantees `fill` writes exactly `got == len`
    // initialized bytes within this allocation and returns before this point.
    unsafe { bytes.set_len(got) };
    Ok(bytes)
}
```

The real safety comment must additionally name the concrete imported function,
its write bound, and the lifetime of the vector allocation passed to it.

- [ ] **Step 5: Record accept or reject evidence**

Document benchmark commands, sizes, samples, and the decision. Remove temporary benchmark code when the experiment is rejected. Commit production code only when the end-to-end gate passes.
