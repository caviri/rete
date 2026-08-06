# Sorted Tile Encoding Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Encode already-sorted unique tile slices without copying, re-sorting, nested grouping vectors, or output-vector growth.

**Architecture:** A crate-private `encode_sorted_unique` function validates and sizes a triple slice, then writes the existing grouped-delta format in a second pass. The general builder sorts/deduplicates before delegation; both in-memory and external tilers call the direct function.

**Tech Stack:** Rust 2021, rete-core varint encoding, Rayon-gated builders, Docker Compose dev image.

## Global Constraints

- Preserve byte-identical `.rete` output and all public APIs.
- Keep untrusted decoding bounds-safe; this change affects encoding only.
- Support `rete-core` with default, no-default, and all features.
- Run all commands through the repository Docker toolchain.

---

### Task 1: Introduce the direct sorted-unique encoder

**Files:**
- Modify: `crates/rete-core/src/varint.rs:1-42`
- Modify: `crates/rete-core/src/triples.rs:43-140`
- Modify: `crates/rete-core/src/index.rs:18,354-423`
- Test: `crates/rete-core/src/triples.rs:447-585`

**Interfaces:**
- Consumes: a lexicographically sorted, duplicate-free `&[Triple]`.
- Produces: `pub(crate) fn uvarint_len(u64) -> usize` and
  `pub(crate) fn encode_sorted_unique(triples: &[Triple]) -> Vec<u8>`.

- [ ] **Step 1: Add failing literal-byte and precondition tests**

Add tests that call the not-yet-existing function:

```rust
#[test]
fn sorted_unique_encoder_matches_literal_format_bytes() {
    assert_eq!(encode_sorted_unique(&[]), vec![0, 0, 0, 0, 0, 0, 0, 0]);
    assert_eq!(
        encode_sorted_unique(&[(1, 2, 3)]),
        vec![1, 1, 2, 2, 3, 3, 1, 1, 1, 1, 2, 1, 3]
    );
    assert_eq!(
        encode_sorted_unique(&[(1, 2, 3), (1, 2, 5), (1, 4, 1), (3, 1, 2)]),
        vec![1, 3, 1, 4, 1, 5, 4, 2, 1, 2, 2, 2, 3, 2, 2, 1, 1, 2, 1, 1, 1, 2]
    );
}

#[test]
#[should_panic(expected = "sorted and unique")]
fn sorted_unique_encoder_rejects_duplicates() {
    encode_sorted_unique(&[(1, 2, 3), (1, 2, 3)]);
}
```

Add a second panic test for descending input.

Add literal length cases in `varint.rs`:

```rust
#[test]
fn encoded_lengths_match_leb128_boundaries() {
    assert_eq!(uvarint_len(0), 1);
    assert_eq!(uvarint_len(127), 1);
    assert_eq!(uvarint_len(128), 2);
    assert_eq!(uvarint_len(16_383), 2);
    assert_eq!(uvarint_len(16_384), 3);
    assert_eq!(uvarint_len(u32::MAX as u64), 5);
}
```

- [ ] **Step 2: Run the focused tests and verify RED**

```sh
docker compose run --rm dev cargo test -p rete-core triples::tests::sorted_unique_encoder -- --nocapture
docker compose run --rm dev cargo test -p rete-core varint::tests::encoded_lengths_match_leb128_boundaries -- --exact
```

Expected: compilation fails because `encode_sorted_unique` and `uvarint_len` do
not exist.

- [ ] **Step 3: Implement exact sizing and direct encoding**

Implement `encode_sorted_unique` with these rules:

```rust
pub(crate) fn encode_sorted_unique(t: &[Triple]) -> Vec<u8> {
    assert!(
        t.windows(2).all(|w| w[0] < w[1]),
        "encode_sorted_unique requires triples sorted and unique"
    );
    if t.is_empty() {
        return vec![0; 8];
    }

    // Pass 1: zone min/max, exact a/b/c group counts, and exact sum of
    // varint_len(value) for every header and delta to be emitted.
    let encoded_len = encoded_len_sorted_unique(t);
    let mut out = Vec::with_capacity(encoded_len);

    // Write seven zone-map values, num_a, then scan each contiguous a range.
    // Count b transitions in the a range before writing num_b. For each b
    // range, write its c count followed by c deltas. No nested Vec is built.
    encode_sorted_unique_into(t, &mut out);
    debug_assert_eq!(out.len(), encoded_len);
    out
}
```

Move `index.rs`'s existing `varint_len` implementation to
`varint::uvarint_len`, make it crate-private, and update `GroupSizer` to import
it. Use `uvarint_len` and `write_uvarint` in the new encoder. Use checked
conversions for `t.len() -> u32`; the existing format cannot represent a block
with more than `u32::MAX` triples.

- [ ] **Step 4: Delegate the general builder and verify GREEN**

Retain the existing arbitrary-input contract:

```rust
pub fn build(mut self) -> Vec<u8> {
    self.triples.sort_unstable();
    self.triples.dedup();
    encode_sorted_unique(&self.triples)
}
```

Run:

```sh
docker compose run --rm dev cargo test -p rete-core triples::tests -- --nocapture
docker compose run --rm dev cargo test -p rete-core --test roundtrip
```

Expected: literal bytes, round trips, malformed-input robustness, and scans pass.

- [ ] **Step 5: Commit the encoder primitive**

```sh
git add crates/rete-core/src/varint.rs crates/rete-core/src/triples.rs crates/rete-core/src/index.rs
git commit -m "perf(core): encode sorted triple slices directly"
```

---

### Task 2: Route tile builders through the direct encoder

**Files:**
- Modify: `crates/rete-core/src/index.rs:18,437-456`
- Modify: `crates/rete-core/src/extbuild.rs:47,1184-1200`
- Test: `crates/rete-core/src/index.rs:951-1117`
- Test: existing external-build tests in `crates/rete-core/src/extbuild.rs`

**Interfaces:**
- Consumes: `encode_sorted_unique(&[Triple])` from Task 1.
- Produces: byte-identical `Tile` payloads without an intermediate `TripleBlockBuilder`.

- [ ] **Step 1: Establish byte-identity characterization**

Before changing call sites, run the existing deterministic tiling and compatibility tests and retain their output as the characterization gate:

```sh
docker compose run --rm dev cargo test -p rete-core index::tests -- --nocapture
docker compose run --rm dev cargo test -p rete-core --test compatibility_v1
```

- [ ] **Step 2: Replace the in-memory tile copy**

Import `encode_sorted_unique` and change `make_tile` to:

```rust
let make_tile = |run: &[Triple]| -> Tile {
    Tile::local(
        run[0].0,
        run[run.len() - 1].0,
        encode_sorted_unique(run),
    )
};
```

- [ ] **Step 3: Replace the external-build tile copy**

Change the external `encode_one` closure to:

```rust
let bytes = encode_sorted_unique(run);
let blk = TripleBlock::parse(&bytes)
    .expect("the builder's own encoded tile must parse");
let z = blk.zone();
let syn = (z.min_b, z.max_b, z.min_c, z.max_c);
```

Keep parallel ordering and compression unchanged.

- [ ] **Step 4: Run all build-path gates**

```sh
docker compose run --rm dev cargo test -p rete-core index::tests -- --nocapture
docker compose run --rm dev cargo test -p rete-core extbuild::tests -- --nocapture
docker compose run --rm dev cargo test -p rete-core --test compatibility_v1
docker compose run --rm dev cargo test -p rete-core --no-default-features
docker compose run --rm dev cargo build -p rete-core --all-features
```

- [ ] **Step 5: Commit the call-site optimization**

```sh
git add crates/rete-core/src/index.rs crates/rete-core/src/extbuild.rs
git commit -m "perf(core): avoid rebuilding sorted tiles"
```

---

### Task 3: Measure build time and peak heap

**Files:**
- Modify after reproducible results: `docs/BENCHMARK.md`
- Regenerate after Markdown change: `docs/BENCHMARK.html`

**Interfaces:**
- Consumes: the pinned Chemotion R2 object exported once to deterministic
  N-Triples, plus `rete-bench --build-mem`.
- Produces: baseline/optimized phase time, peak live heap, file size, and content hash.

- [ ] **Step 1: Select and record the deterministic source**

Download `https://data.graphplaza.com/chemotion/chemotion.rete` once to
`/target/rete-rust-opt-bench/chemotion.rete`, verify the expected object length
and ETag, and export it once with the pinned baseline binary to
`/target/rete-rust-opt-bench/chemotion.nt`. Record both files' byte lengths,
triple count, and SHA-256 before running either build. Reuse this exact local
N-Triples file for every build sample; do not re-download between samples.

- [ ] **Step 2: Alternate baseline and optimized builds**

Run at least five release builds per binary, alternating order. Use identical flags and output locations. Record total time and verify the resulting `.rete` files have identical SHA-256 hashes.

- [ ] **Step 3: Compare peak heap by phase**

Run `rete-bench --build-mem` with each code revision or pinned executable environment and record the `build index (3 perms)` time and peak MiB.

- [ ] **Step 4: Document only stable results**

If direction and magnitude reproduce, update `docs/BENCHMARK.md`, regenerate `docs/BENCHMARK.html`, and commit both files.
