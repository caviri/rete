# Read-Path Acceleration Design

## Context

The first optimization pass made native Cloudflare R2 queries 20–22% faster by
reusing HTTP connections and sharing coalesced cache allocations. It also showed
that unchecked triple-block decoding is the wrong primary lever for remote
queries: it improves a path-heavy local query from 145 ms to 86 ms, but adds only
3–6% on the same R2 path and no measurable benefit on selective or aggregate R2
queries.

Three remaining workloads have different dominant costs:

- A cold native R2 query performs 16–36 range GETs and spends 1.8–4.1 seconds
  mostly waiting for transport.
- Repeated queries over one resident graph reuse fetched blocks, decompressed
  tiles, dictionary chunks, and group directories, but rebuild SPARQL parsing,
  lowering, constant resolution, planning, and per-query memoization.
- Local and WASM property paths repeatedly decode and skip adjacency groups,
  allocate complete triples, and sort results even when traversal needs only the
  neighboring term IDs.

This design addresses all three with safe, format-compatible changes. Each track
is independently benchmarked and independently revertible.

## Goals

- Reduce cold native R2 latency by collapsing serialized request waves and using
  a bounded eager read when the entire object is cheaper than many small reads.
- Make repeated queries reuse immutable query-language and planning work while
  keeping every execution's mutable state isolated.
- Recover most of the path-heavy unchecked-decoder advantage in safe Rust by
  decoding u32 varints efficiently and jumping directly to two-bound adjacency
  lists.
- Preserve query answers, malformed-input behavior, the current `.rete` file
  bytes, default WASM compatibility, and safe decoding for untrusted files.
- Measure wall time, requests, bytes, and memory rather than keeping a change on
  the strength of a microbenchmark alone.

## Non-goals

- No file-format revision, routing-manifest section, or on-disk prefix-2 index in
  this phase.
- No decoded-whole-tile cache or unbounded session cache.
- No async rewrite of `RangeReader`, Tokio dependency, or browser main-thread
  synchronous networking.
- No new production `unsafe`; the research-only unchecked decoder remains
  isolated behind `unsafe-decode-bench`.
- No caching of `SERVICE` results or nondeterministic SPARQL function results.
- No promise that large result materialization, serialization, or full scans
  become proportionally faster when those costs dominate execution.

## Architecture

The work is delivered as three tracks with a shared measurement harness:

1. **Cold native R2:** restore range-reader capability delegation, then choose
   eager or lazy opening from the probed object length.
2. **Resident repeated queries:** introduce dataset-bound prepared plans and a
   bounded session cache while retaining fresh execution state.
3. **Safe local/WASM paths:** specialize checked u32 decoding, add a bounded
   two-level group directory, and expose a neighbor-only scan to property paths.

The tracks do not share cache ownership. The cold path chooses how bytes reach a
`Rete`; the resident path reuses immutable query work above `Rete`; the local
path reduces work inside an already-loaded tile. This separation keeps memory
accounting and failure behavior understandable.

## Track 1: Cold Native R2

### 1. Preserve transport capabilities through `RangedSourceReader`

`HttpRangeReader` supports batched reads with a concurrency of 16, but
`RangedSourceReader` currently forwards only `len` and `read_at`. It therefore
inherits `RangeReader`'s sequential `read_many` and `concurrency = 1` defaults.

`RangedSourceReader` will delegate `read_many` and `concurrency` to its active
backend. File-backed sources keep their backend's current behavior. HTTP sources
therefore retain parallel/coalesced range behavior through every wrapper, and
the existing remote-aware join planner sees the real fan-out.

This is an API-capability correction, not a heuristic. Tests will use an
instrumented reader to prove that delegation preserves result order, errors,
and reported concurrency.

### 2. Adaptively eager-open small native objects

The native `sparql-url` command will use an eager-read threshold of 8 MiB by
default. `RETE_EAGER_MAX_MB` overrides the threshold; `0` disables eager mode.
An invalid or overflowing value is a configuration error reported before any
object bytes are fetched.
If the probed length is nonzero and no larger than the threshold, the command
will request exactly `[0, len)`, require an exact-length response, and open the
returned bytes through the ordinary validated eager reader. Larger objects keep
the lazy block-cache path.

This policy is native CLI-only in this phase. Browser/WASM keeps lazy range
reading because its 32-bit address space and synchronous worker transport need a
different memory threshold. The choice changes neither `.rete` bytes nor query
semantics.

Chemotion is 7,566,404 bytes, so eager mode replaces 16–36 range GETs with one
full-object GET after the existing length probe. It downloads roughly 3–7 times
more bytes than the current three example queries, but avoids several seconds of
serialized latency. Allocation is bounded before the request, offset/length
arithmetic remains checked, and a short or overlong response is an error.

The initial implementation will not invent a new ETag protocol. It preserves
the reader's current consistency model; a future conditional request can be
added independently if object mutation between the length probe and GET becomes
a demonstrated issue.

### 3. Cold-path acceptance

The pinned Chemotion object and the three catalog queries remain the primary R2
workload. Fifteen alternating fresh-process samples compare lazy, delegated
lazy, and adaptive eager modes. Runs record median and p90 wall time, GET count,
bytes, peak RSS, output hash, length, and ETag.

The adaptive policy is retained when:

- every output is byte-identical;
- an eligible object uses one data GET after the probe;
- at least two of the three Chemotion queries improve by 25% or more in median
  wall time and none regresses in median or p90; and
- peak RSS remains within the configured eager threshold plus ordinary eager
  decode overhead.

Threshold sweeps at 0, 4, 8, and 16 MiB will be reported. Oversized lengths,
short responses, ignored ranges, malformed files, and ordinary lazy opens remain
clean errors or correct fallbacks.

## Track 2: Resident Prepared Queries

### 1. Prepared query boundary

The core will gain an opaque `PreparedQuery` created from a `Rete`, query text,
and explicit options such as reasoning. Preparation owns only immutable work:

- parsing and lowering;
- variable-slot layout;
- constant term-to-ID resolution;
- BGP estimates and join order;
- constant property-path predicate IDs;
- compiled regular expressions; and
- the reasoning rewrite and TBox-derived structures when requested.

A prepared query is bound to the source dataset. It records a dataset key from
the file version, content hash, and stable header counts. Execution against a
different key fails before evaluation. For legacy images without a useful
content hash, the owning `QuerySession` assigns an in-memory nonce when it opens
the `Rete`; the prepared plan records that nonce and cannot be executed outside
that session. Weak header metadata is never treated as content identity.

Each execution creates fresh rows, aggregates, resolver scratch state,
`EXISTS` memoization, random/UUID state, lazy-load failure verdict, and service
error state. `SERVICE` calls execute every time. Prepared plans are immutable;
mutable evaluation state is never shared between calls.

### 2. Bounded `QuerySession`

`QuerySession` owns one resident `Rete` and a weighted LRU of prepared plans
keyed by query text plus options. The initial defaults are 128 entries and a
4 MiB plan-weight limit; callers can lower the limit or disable the plan cache.
Weight includes owned query strings, plan nodes, slot tables, compiled regexes,
and reasoning data to the extent measurable without an allocator-specific API.

The existing WASM `RemoteGraph` already reuses network blocks, decompressed
tiles, dictionary chunks, and group directories. It will use the same prepared
plan cache internally rather than creating another graph-data cache. Native
resident surfaces such as `rete serve` can adopt `QuerySession`; one-shot CLI
commands remain one-shot unless a later interactive command is designed.

Concurrent execution is not advertised until lazy-load failure and `SERVICE`
error state are demonstrably per execution. The first API supports serial calls
on a resident session. This avoids changing current error semantics merely to
make the prepared type `Sync`.

### 3. Warm-path acceptance

`rete-bench` will separate preparation, execution, and serialization. It will
open one eager and one lazy graph, run a cold query once, then measure 30–100
steady-state executions for ASK, bound point SELECT/LIMIT, a three-way join,
regex/filter, aggregate, property path, large projection, and reasoned query.
It will test identical reruns and a rotating family of related queries.

The plan cache is retained when:

- prepared and ordinary APIs return byte-identical results;
- tiny ASK/point/LIMIT queries improve by at least 20% at the median;
- scan- or serialization-dominated queries regress by no more than 3%;
- a fully warm lazy execution performs zero additional range reads; and
- cache eviction stays within its configured entry and weight limits.

Tests also require a different-dataset rejection, fresh nondeterministic values,
one mock `SERVICE` call per execution, retry after a transient range failure, and
unchanged parse/runtime error classes.

## Track 3: Safe Local and WASM Path Traversal

### 1. Checked u32 LEB128 fast path

Triple-block fields are u32 values and u32-bounded counts. Their cursor will use
an inlined checked decoder specialized for that contract: a one-byte fast path
followed by at most four continuation bytes. It returns failure for truncation,
continuation beyond five bytes, or invalid high bits. It does not read outside
the slice and does not weaken `TripleBlock::parse` or corruption handling.

The generic u64 varint decoder remains available for format fields that require
it. Literal boundary tests cover one through five bytes, truncation at every
position, overflow encodings, and equivalence with the existing decoder.

### 2. Bounded prefix-2 group directory

The existing per-tile `GroupDirectory` jumps from a bound leading component
`a` to its encoded group. It will additionally record compact entries for each
`b` group: the `b` value, tile-relative start of its c-list, and c-count. Each
`a` entry identifies its contiguous b-entry range, allowing binary search for a
bound `(a,b)` prefix.

Directory construction stays fully checked and is cached in the tile's existing
`OnceLock`. A prefix-2 directory may consume at most 64 KiB per tile. If the
entry count would exceed that budget, construction keeps the existing a-only
directory and scans b-groups as today. This makes pathological mega-groups a
performance fallback, never a correctness or allocation failure.

Offsets and counts are validated against the same immutable tile allocation.
Malformed bytes produce the existing empty/error behavior; no directory entry
can authorize an unchecked slice.

### 3. Neighbor-only scan for paths

`GraphIndex` will expose a crate-private iterator for a two-bound permutation
prefix that yields only the free third-component IDs. It does not allocate
canonical triples and does not sort them. The SPARQL path evaluator will resolve
constant predicates once per prepared/evaluation plan, select the forward or
reverse permutation, and feed neighboring IDs directly into its visited/frontier
sets.

General triple-pattern evaluation remains unchanged. If a path shape cannot use
a two-bound prefix, it falls back to the existing iterator. Set semantics and
zero-length path behavior remain authoritative.

### 4. Local/WASM acceptance

The pinned local Chemotion property-path workload runs 15 alternating release
samples, with separate counters for directory construction, decoded varints,
skipped c-values, path probes, predicate resolutions, touched tiles, and live
heap. The same workload runs in a release WASM Worker.

The path fast path is retained when:

- the local safe median is at most 100 ms or at least 30% faster than 145 ms;
- full-scan, selective, and aggregate controls regress by no more than 3%;
- native and WASM answers and touched byte ranges are identical; and
- per-tile and total observed directory memory remain bounded.

Correctness gates cover all eight bound/unbound pattern shapes, forward and
reverse paths, sequence/alternative/negated/repeated paths, Oxigraph differential
fixtures, and the truncation, byte-flip, fuzz, and malformed-file suites.

## Error Handling and Safety

- Remote lengths are checked before allocation and exact response length is
  required before parsing.
- `read_many` preserves input ordering and propagates any child error without
  returning a partial batch.
- Prepared plans never cache mutable evaluation results, network failures,
  `SERVICE` responses, or nondeterministic values.
- Dataset mismatch is a typed preparation/execution error, not a wrong answer.
- Every new decoder and directory access remains bounds-checked; malformed and
  truncated remote bytes cannot trigger undefined behavior.
- Directory-budget exhaustion and unsupported path shapes use the existing safe
  scan rather than failing the query.
- Lazy tile or dictionary failures continue to invalidate the whole query result
  rather than silently returning fewer rows.

## Delivery Order

1. Add benchmark instrumentation and preserve `RangedSourceReader` capabilities.
2. Add and measure the native adaptive eager policy.
3. Add the safe u32 decoder, then the prefix-2 directory and neighbor iterator,
   measuring after each so their contributions remain visible.
4. Introduce `PreparedQuery`, benchmark explicit preparation, then add the
   bounded `QuerySession` cache and resident-surface integration.
5. Record accepted and rejected results in `docs/BENCHMARK.md` and regenerate
   tracked HTML.

This order starts with the smallest high-confidence cold-path correction,
isolates the local decoder improvements, and leaves the largest API change until
the execution boundary and benchmarks are understood.

## Verification

Each track uses focused red-green tests before implementation. Final verification
runs the repository's Docker gates: formatting, workspace clippy, workspace
tests, core no-default-feature tests, core all-feature build, benchmark-crate
build, CLI smoke tests, feature-gated unsafe decoder tests, generated-doc checks,
and the real/local benchmark identity checks described above.

No optimization is accepted solely because it makes one workload faster. Each
must satisfy its correctness, memory, and control-workload gates independently.
