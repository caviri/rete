# Rust Optimization Design

## Context

Polars demonstrates that carefully justified `unsafe` code can remove overhead in
hot data-processing loops. Rete has a different trust boundary: its core decodes
untrusted, remotely fetched binary files, and clean bounds errors are part of the
product contract. This design therefore starts with measured safe changes and
uses the Rust Unsafe Code Guidelines as a proof checklist, not as a source of
optimization recipes.

The current workspace baseline passes with:

```sh
docker compose run --rm dev cargo test --workspace --exclude rete-bench
```

## Goals

- Reduce repeated HTTP connection setup in native CLI range reads.
- Reduce sorting, copying, temporary allocation, and vector growth while encoding
  already-sorted index tiles.
- Avoid copying every block out of a coalesced cache fetch while preserving exact
  cache-cap accounting.
- Measure the two FFI output buffers that are currently zero-initialized before a
  host overwrites them, and adopt `MaybeUninit` only if the measured benefit and
  safety proof are both compelling.
- Preserve file-format bytes, query results, error behavior, public APIs, and the
  single-threaded default WASM build.

## Non-goals

- No unchecked indexing in file parsers or query decoders.
- No custom `Send`/`Sync` pointer wrapper, mutex, allocator, vector, or iterator
  trust marker.
- No `transmute`-based format or FFI conversion.
- No Tokio migration, asynchronous core-reader trait, `rete serve` redesign, or
  persistent native worker pool in this work.
- No new production `unsafe` merely because a microbenchmark improves.

## Design Decisions

### 1. Retain one HTTP agent per CLI reader

`HttpRangeReader` will own a cloneable `ureq::Agent`. `open` will create the
agent, use it for the initial length probe, and retain it for every subsequent
range request. `read_at` and the existing scoped parallel `read_many` workers
will share clones of that agent, allowing ureq's synchronized connection pool to
reuse idle TCP/TLS connections.

The existing concurrency limit, request ordering, HTTP status checks, short-body
errors, and zero-length behavior remain unchanged. A localhost HTTP/1.1 test will
serve multiple requests on one keep-alive connection and assert that opening a
reader followed by sequential reads does not require a new accepted connection
per request. Existing range correctness and failure tests remain authoritative.

### 2. Encode sorted unique tile slices directly

`rete-core::triples` will gain one crate-private safe encoder for a sorted,
duplicate-free `&[Triple]`. It will make a sizing pass that validates ordering,
computes zone-map values and group counts, and determines the encoded byte
length. A second pass will write the existing grouped-delta representation into
a pre-sized `Vec<u8>` without constructing nested `a_groups`, `b_groups`, or
`c` vectors.

`TripleBlockBuilder::build` will continue accepting arbitrary input: it sorts and
deduplicates as today, then delegates to the direct encoder. The in-memory index
tiler and external-build tiler already own sorted, deduplicated slices; they will
call the direct encoder instead of copying each slice into a builder and sorting
it again.

Tests will pin hand-checked literal bytes for empty, singleton, and multi-group
fixtures, then require both builder and direct paths to produce those bytes.
Duplicate-rich builder input and deterministic generated direct input will also
be parsed back to independently derived triples and zone maps. Existing file
layout and compatibility tests must remain unchanged and pass.

### 3. Share backing storage for coalesced cache spans

Each coalesced `RangeReader::read_many` result will be converted once to shared
immutable backing storage. Cached block entries will hold a backing identifier
and byte subrange rather than allocating and copying a separate `Arc<[u8]>` per
block. Assembly will clone the backing handle under the mutex and copy only the
requested final output, which the current `RangeReader -> Vec<u8>` API requires.

Cache state will track each backing allocation and the number of resident block
entries referring to it. `cached_bytes` and the eviction cap will count the full
allocation exactly once while any block retains it. Evicting one block may not
release memory; trimming therefore continues evicting least-recently-used blocks
until enough complete backing allocations are released. This preserves the
documented memory cap between reads, including partial eviction and concurrent
double-fetch replacement.

Tests will cover exact output, coalesced fetch counts, LRU behavior, a span shared
by multiple blocks, replacement of overlapping fetches, short underlying reads,
and a read wider than the cap. The production code remains safe Rust.

### 4. Benchmark-gated FFI buffer experiment

The Java host reader and WASM Asyncify reader currently allocate initialized zero
buffers before an external host writes the requested bytes. A throwaway
experiment may compare that behavior with `Vec::with_capacity` plus host writes.
Production adoption requires all of the following:

- The import contract states that success initializes every byte in `0..got` and
  never writes beyond the supplied capacity.
- Rust checks `got <= capacity` before setting a vector length or constructing a
  byte slice.
- Error and short-write paths never expose or drop typed uninitialized elements.
- The `unsafe` operation is isolated in a small helper with a local safety proof
  and direct tests using native synthetic writers where possible.
- A representative end-to-end host read shows a material improvement, not only
  a synthetic allocation-loop improvement.

If an end-to-end measurement cannot be made reproducibly, or the improvement is
lost in host/network overhead, the experiment will be documented and discarded.
The zero-initialized production buffers will remain.

## Measurement

Correctness gates run in the repository's Docker toolchain. Performance runs use
release builds, identical deterministic input, a warm-up, repeated samples, and
median elapsed time. Build work also records peak live heap through the existing
`rete-bench --build-mem` allocator instrumentation. HTTP tests measure connection
accepts deterministically; real-network timing is supporting evidence only.

Each production change is kept only when it removes the intended work and does
not regress representative wall time or peak memory beyond ordinary measurement
noise. Results, commands, dataset size, and accepted or rejected experiments are
recorded in `docs/BENCHMARK.md` only when the measurement is reproducible and
useful to future maintainers.

## Safety and Error Handling

- All untrusted file parsing remains bounds-checked.
- Offset and length arithmetic remains checked before allocation or slicing.
- A short HTTP, cache, Java-host, or WASM-host response remains a clean error.
- The direct encoder validates its sorted-unique internal precondition during its
  sizing pass and fails fast with a clear internal assertion when a caller
  violates it; it cannot silently emit a corrupt block.
- No file-format bytes change. Byte-identity tests guard this explicitly.
- Any retained `unsafe` code must document allocation lifetime, initialization,
  bounds, aliasing, provenance, and failure-path invariants at the unsafe block.

## Delivery Structure

The work is split into independent commits: HTTP agent reuse, direct tile
encoding, shared cache backing, and the FFI experiment decision. Each commit has
focused red-green tests and can be reviewed or reverted without the others. Final
verification includes formatting, clippy, workspace tests, no-default-feature
core tests, all-feature core build, benchmark-crate build, and the smoke script.
