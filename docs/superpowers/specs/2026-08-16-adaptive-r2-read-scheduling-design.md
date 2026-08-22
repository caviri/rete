# Adaptive R2 Read Scheduling Design

**Date:** 2026-08-16
**Status:** Approved for implementation
**Branch:** `feat/rust-optimization`

## Summary

The next read optimization will reduce cold Cloudflare R2 query latency for
the existing `.rete` catalog. It will not change the file format or require a
dataset rebuild. A session-local adaptive controller will choose how aggressively
the existing lazy reader batches known-needed ranges. Selective joins and bounded
`LIMIT` queries come first; broad scans and aggregates use the same controller
after the selective path is proven.

The controller is query-aware and network-aware. Query code describes whether a
batch is a selective probe, bounded scan, full scan, or dictionary-resolution
batch. Successful physical reads report elapsed time and returned bytes. The
controller uses those observations to trade bounded overfetch for fewer round
trips. It never invents unknown tile reads, persists a profile, weakens validation,
or introduces `unsafe` code.

Warm/local query throughput is a separate second phase. It starts only after this
remote-read phase has a verified benchmark result, so network scheduling and CPU
kernels are not mixed in one experiment.

## Goals

- Improve cold selective SPARQL joins and bounded `LIMIT` queries over R2.
- Then improve broad scans and aggregates with the same scheduling machinery.
- Benefit native lazy reads, browser remote-lazy reads, named graphs, and shard
  fan-out without changing query results.
- Work with every currently published `.rete` file.
- Learn only from the current physical remote source and current open session.
- Preserve exact framing reads, HTTP validation, lazy failure/retry semantics,
  cache bounds, and safe decoding defaults.
- Produce a reproducible static-versus-adaptive benchmark on pinned R2 objects.

## Non-goals

- No file-format flag, new section, richer on-disk synopsis, rebuild, or republish.
- No persistent per-URL profile, browser storage, cookies, or telemetry upload.
- No speculative fetch of a tile that routing has not already identified as a
  possible input to the current query.
- No change to the native at-or-below-8-MiB one-transfer policy.
- No new unchecked indexing, raw-pointer sharing, `transmute`, `set_len`, or other
  `unsafe` optimization.
- No relaxation of `Content-Range`, body-length, framing, bounds, or decompression
  error handling. Query-triggered zstd output remains subject to the already
  documented uncapped-output limitation; this work does not claim to fix it.
- No warm/local SIMD or decoder work in phase 1.

## Existing Behavior

The current engine already has the pieces an adaptive scheduler should steer:

- `BlockCacheReader` caches fixed aligned blocks and batches missing blocks through
  `RangeReader::read_many`.
- tile and dictionary loaders coalesce nearby known-needed ranges with fixed gaps;
- selective BGP probes gather routed tiles for a batch before loading them;
- unbound scans prefetch four tiles initially and double toward a fixed maximum;
- the native CLI fully transfers objects at or below 8 MiB and opens that owned
  image lazily, while larger native and browser objects remain remotely lazy.

The limitation is that fixed block/coalescing/window choices cannot distinguish a
fast nearby endpoint from a high-RTT endpoint, or a one-tile lookup from a scan
that will consume every following tile.

## Architecture

### One controller per physical source

Each remotely opened physical source owns one `AdaptiveReadController`. The
controller is shared by the source's block cache, dictionary loader, default
graph indexes, and named-graph indexes. A federated graph therefore has one
controller per shard URL; shards tune independently because their latency and
cache state can differ.

The controller is session-local. Closing the graph drops its observations. It has
no global registry and no persistence key.

Local files, slices, and the native small-object owned-memory path use the current
static policy. Precise metadata reads remain exact even when their successful
latency observations are usable as conservative cold-start samples.

### Inputs

`ReadIntent` classifies a batch:

- `SelectiveProbe`: routed tiles for bound lookups or a join-probe batch;
- `BoundedScan`: a scan whose consumer can stop early, including `LIMIT`;
- `FullScan`: a scan or aggregate expected to consume the complete routed span;
- `DictionaryResolve`: dictionary chunks for the current output/probe batch.

`ReadObservation` contains only in-process scheduling data:

- requested and returned bytes;
- physical range count;
- elapsed monotonic time when the backend can supply it;
- success or failure;
- useful bytes or consumed units reported by the caller after a batch is used.

The data is never serialized or exposed as user telemetry.

### Output

For a set of ranges that routing has already declared eligible, the controller
returns a bounded `ReadPlan`:

- maximum byte gap that may be coalesced;
- maximum aligned cache blocks per physical span;
- prefetch-window start and cap;
- maximum in-flight range count, never above `RangeReader::concurrency()`.

The cache retains its stable per-reader logical block size (currently 64 or
128 KiB under `auto_block`). Adaptation changes how many adjacent missing blocks
are fetched in one span, not the cache key size, so a policy change cannot
invalidate resident entries.

## Decision Model

The controller maintains integer/fixed-point exponentially weighted estimates of
successful request latency and transfer throughput. It also tracks the fraction
of a prefetched window actually consumed. Floating-point behavior is not required
for correctness or reproducibility.

For two known-needed ranges separated by `gap_bytes`, the scheduler estimates:

```
extra_transfer_time = gap_bytes / estimated_throughput
saved_request_time  = estimated_round_trip_time
```

It merges the ranges only when the extra-transfer estimate is below the saved
request estimate after a conservative safety margin, and only within both the
relative and absolute overfetch ceilings. Insufficient or invalid timing data
selects the current static constants.

Hysteresis requires multiple consistent observations before moving to a more
aggressive or more conservative tier. One slow response cannot permanently
inflate prefetching.

### Selective probes and bounded limits

- Only tiles already found by index routing or batched join probes are eligible.
- Known-needed scattered ranges may run concurrently up to the reader limit.
- Adjacent known-needed ranges may merge when the cost model says the saved RTT
  outweighs the bounded gap.
- No new prefetch begins once a consumer has satisfied a genuinely streaming
  `LIMIT`. An `ORDER BY`, aggregate, or other operator that must inspect the full
  input is classified as a full scan even when the final syntax contains `LIMIT`.
- Low consumption of a batch moves the next decision toward smaller windows.

### Full scans and aggregates

- A scan begins at the existing four-tile window.
- Consecutive highly consumed windows grow geometrically toward the controller's
  bounded cap.
- Partial consumption or early termination shrinks the next window.
- Contiguous missing cache blocks are grouped into larger spans on high-RTT links,
  while available backend concurrency is used for disjoint spans.

### Dictionary batches

Only dictionary chunks referenced by the current solution/output batch are
eligible. Adaptive coalescing replaces the fixed decision about whether gaps
between those known chunks are cheaper than an additional request.

## Hard Limits and Fallbacks

- The existing 256-MiB cache residency cap remains unchanged.
- A single adaptive physical span may not exceed 2 MiB.
- Bytes fetched solely to bridge gaps may not exceed the smaller of 25% of the
  known-needed batch bytes or 256 KiB.
- Adaptive concurrency is clamped to `1..=RangeReader::concurrency()`.
- Arithmetic is checked or saturating; impossible timing/size observations are
  discarded.
- Fewer than two successful timed physical reads use the current static policy.
- A failed batch is not cached and is not a throughput sample. The retry uses a
  smaller plan and ultimately the existing exact/per-range fallback.
- Policy-state lock poisoning, unavailable monotonic time, or unsupported reader
  capabilities degrades to the current static behavior rather than failing a
  query.
- Precise reads never widen, populate adaptive cache blocks, or cross protected
  metadata/payload boundaries.

These limits are compile-time defaults, not a new public CLI surface. A narrow
internal/static-mode switch is allowed solely for deterministic A/B tests and the
benchmark harness.

## Integration Boundaries

The implementation should keep responsibilities separate:

- a small core policy module owns observations and pure planning decisions;
- range-reader/back-cache code measures physical reads and applies span plans;
- graph-index and dictionary code supply intent and consumption feedback;
- CLI and WASM construct the same default adaptive remote reader;
- local and owned-memory readers keep the static/no-op path.

The policy module must be testable without HTTP, sleeps, WASM, or a real clock.
Backends supply elapsed observations; deterministic tests feed synthetic values.

## Errors and Correctness

Adaptive scheduling changes only which already-needed byte ranges share a
physical request. Parsing, validation, decompression, tile decoding, join order,
solution order, and SPARQL semantics remain unchanged.

All response-size and `Content-Range` checks still apply to every constituent
range. A widened cache span is validated against its own exact requested tuple.
Short, overlong, malformed, or failed responses take the existing clean error
path. No partial batch becomes resident. Existing `index_incomplete` and
`reset_load_failures` behavior is preserved.

## Rollout Order

1. Add the pure controller and deterministic policy tests.
2. Teach the block cache to measure/apply adaptive known-range coalescing while
   preserving precise-read behavior.
3. Label selective tile and dictionary batches; add consumption feedback and
   prove bounded `LIMIT` does not start unnecessary work.
4. Adapt scan window growth and concurrency.
5. Wire the default into native lazy and browser remote-lazy openers, leaving
   native small-object owned-memory reads unchanged.
6. Run static-versus-adaptive synthetic and live R2 gates. Default-on is accepted
   only after correctness, byte, memory, and latency gates pass.

## Testing

### Deterministic unit/property tests

- cold start equals the current static plan;
- high RTT/fast throughput merges more known-needed ranges;
- low RTT/slow throughput conserves bytes;
- hysteresis ignores a single outlier;
- overfetch, span, cache, arithmetic, and concurrency caps always hold;
- failures shrink/fallback and do not train throughput;
- selective plans never include an unrouted tile;
- early `LIMIT` consumption shrinks/stops future prefetch;
- full consumption grows scan windows within the cap;
- named/default graphs and dictionary batches share the source controller;
- precise reads remain physically exact through the block cache;
- static mode reproduces the pre-change request plan.

### Core and surface parity

- lazy adaptive results equal eager/static results across triple-pattern shapes,
  BGP joins, `FILTER`, `ORDER BY`, `LIMIT`, aggregates, named graphs, and `FROM`;
- short/corrupt/failing range readers preserve clean errors and retry semantics;
- default features, no-default-features, all features, CLI, WASM, and shard
  federation remain green;
- no new `unsafe` appears outside the existing research-only feature.

### Live benchmark

Use commit `6562251d` as the static baseline and immutable candidate binaries.
Pin URL, length, ETag, `Accept-Ranges`, executable hash, query text, and output
SHA-256 before and after every run. Alternate modes across fresh processes.

Representative gates must include:

- a native object above 8 MiB with selective and scan/aggregate queries;
- a browser remote-lazy selective/`LIMIT` query on a strict-subset object;
- named-graph coverage;
- all six Wikidata XXL shards, with exact results and no full/unranged GET;
- a repeated browser query proving warm cache adds zero physical reads.

Run at least 15 samples per native query/mode. Report median and nearest-rank p90
wall time, bytes, physical GETs, and peak RSS. Browser evidence records every
physical range response and before/after object pins.

The adaptive default is accepted only when:

- every result hash and row count matches static/eager references;
- at least two selective remote-lazy workloads improve median cold latency by
  at least 15%;
- the scan/aggregate suite improves median latency or physical GET count by at
  least 10% on at least one representative workload;
- no representative workload regresses median latency by more than 5%;
- gap-only overfetch stays within the controller limits and total transferred
  bytes are reported candidly;
- peak RSS does not grow by more than 10% and the 256-MiB cache cap holds;
- malformed-input, default/no-default, workspace, CLI, WASM, smoke, formatting,
  clippy, and browser gates pass.

If the live gates do not justify default-on behavior, retain the controller and
measurements behind the internal experiment switch; do not market an unproven
speedup or silently increase the default read footprint.

## Documentation

Update `docs/BENCHMARK.md` and generated HTML with the exact candidate identity,
R2 pins, queries, result hashes, bytes/GETs, wall-time distribution, RSS, and any
regression or no-effect result. Update browser/CLI prose only if their externally
observable policies change. Keep the native small-object, browser lazy-reader,
shard-federation, named-graph, and uncapped-zstd scope boundaries explicit.

## Phase 2: Warm/Local Throughput

After phase 1 is accepted or explicitly held experimental, create a separate
design and benchmark for warm/local query throughput. Candidate work includes
safe allocation reuse, iterator specialization, SIMD-friendly varint/block
decoding, and join-table layout. It must begin with profiles and preserve safe
defaults; the previous `unsafe-decode-bench` result is evidence that unchecked
indexing alone is not the dominant remote bottleneck.
