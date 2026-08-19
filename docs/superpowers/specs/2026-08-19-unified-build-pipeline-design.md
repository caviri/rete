# Unified Compact Build Pipeline Design

**Status:** Approved design, pending implementation plan

**Date:** 2026-08-19

**Branch:** `feat/rust-optimization`

## Decision

Replace rete's separate in-memory and external build implementations with one
compact, deterministic pipeline. The pipeline parses RDF once, interns unique
terms, records fixed-width provisional ID quads, canonicalizes the dictionary,
remaps the records, constructs three dual-order index families, and streams the
finished dataset to its destination.

This work deliberately advances the stable file-format byte from `0x05` to
`0x06`. The `0x06` reader accepts only `0x06`; it does not retain a legacy
`0x05` decoder. This is an intentional compatibility break approved for this
project. Existing engines cannot read `0x06`, and the new engine reports a clear
unsupported-version error for older files.

The primary performance targets are at least 1.5x faster builds and at least 25%
lower peak memory on the representative large standard and external workloads.
The format change must not make representative query median or p90 latency more
than 5% slower or make the resulting file more than 10% larger.

## Goals

- Parse replayable N-Triples and N-Quads inputs once rather than twice.
- Allocate each distinct RDF term once during ordinary ingestion instead of
  retaining duplicate strings per statement.
- Give ordinary and `--memory-budget-mb` builds shared canonicalization, index,
  compression, output, instrumentation, and validation semantics.
- Replace six independently constructed index containers with three families
  that share leading-key routing metadata while preserving all six physical
  triple orders.
- Keep SPARQL planning and evaluation above `GraphIndex` unchanged.
- Stream CLI output instead of assembling the whole final `.rete` image in a
  resident `Vec<u8>`.
- Keep build output deterministic across repeated runs and Rayon thread counts.
- Preserve checked parsing, checked arithmetic, bounded external memory, lazy
  range loading, and clean failures for malformed data.
- Install a completed output without exposing a partial file or destroying a
  previously valid destination after a failed build.
- Measure time, memory, spill traffic, output size, and read performance before
  accepting each optimization.

## Non-goals

- This project does not add unchecked indexing, raw shared pointers, `transmute`,
  or another `unsafe` fast path.
- It does not add named-graph, text-index, reasoning, or pyramid support to the
  external builder. Existing external-build feature restrictions remain.
- It does not redesign SPARQL planning, joins, result serialization, or remote
  cache scheduling.
- It does not change RDF parsing semantics or canonical term ordering.
- It does not replace the existing Louvain algorithm. Louvain-heavy builds are
  measured separately because that optional sequential phase can dominate total
  wall time independently of ingestion and indexing.
- It does not upload, overwrite, or delete Cloudflare R2 objects. Publication is
  a later operation requiring separate explicit authorization.
- It does not promise that every current catalog dataset can be rebuilt. Entries
  without a `0x06` artifact are made visibly unavailable to the new engine.

## Current Pipeline and Bottlenecks

The standard file-based CLI path streams N-Triples/N-Quads twice. Pass one builds
the dictionary and pass two re-parses the source to encode ID triples. The
fallback path for stdin, Turtle, reasoning, and materialization holds raw owned
strings until dictionary encoding. Both paths then build pyramid metadata, an
optional text index, six physical permutations, and an in-memory final file
image before `std::fs::write` copies it to disk.

The external builder already consumes the RDF source once and bounds raw-string
memory by chunks. It then merges chunk dictionaries, writes a global triple
spool, and scans that spool once for each of six external permutation sorts.
Each permutation creates sorted runs, merges them, tiles the result, compresses
the tiles, and writes another temporary section before final assembly.

Existing code already parallelizes dictionary sorting, permutation sorting, and
tile compression. The next gains therefore come from eliminating parse replay,
reducing fixed-width record copies and full-spool scans, sharing leading-order
work, bounding compression queues, and streaming final output. They do not come
from removing bounds checks.

The existing Louvain community pyramid remains a separate cost centre. The
shared pipeline's 1.5x end-to-end gate applies to `--pyramid-algo types` and
`--no-pyramid` production-style workloads. Default Louvain builds have a strict
no-regression gate and report the new pipeline's non-Louvain phase speedup, but
are not required to show a 1.5x total-wall improvement when Louvain itself is the
majority of runtime.

## Architecture

```text
RDF source
  -> parser
  -> ingest backend (memory or bounded chunks)
  -> canonical dictionary + remap artifacts
  -> canonical triple spool
  -> S/P/O dual-order family builders
  -> bounded tile encoder/compressor
  -> streaming dataset writer
  -> validated temporary output
  -> installed .rete file
```

The pipeline is composed of small internal units with explicit artifacts between
them. A unit may use memory or temporary files internally, but later units do not
need to know which ingest backend produced the artifacts.

### `IngestBackend`

The ingest backend consumes parsed `RawQuad` values and produces canonicalization
inputs plus statement and graph counts.

The ordinary `MemoryIngest` backend maintains:

- one node interner for subject/object lexical forms;
- one predicate interner;
- subject/object role bits on every interned node;
- a deterministic named-graph table; and
- a `Vec<ProvisionalQuad>` containing four fixed-width IDs, with a sentinel for
  the default graph.

Only a genuinely new term allocates lexical storage. A repeated term contributes
only its fixed-width provisional ID to a record. The node interner preserves the
information needed to divide terms into shared, subject-only, and object-only
dictionary sections.

The `ChunkedIngest` backend uses the same record meaning but not an unbounded
global hash table. It seals a bounded local interner and local-ID record chunk,
writes sorted role-specific term files and records to the spill directory, then
drops the chunk. A k-way term merge assigns canonical IDs and produces compact
per-chunk remap arrays. This retains the external builder's memory guarantee.

Reasoning/materialization inputs continue through their existing resident path
until inferred triples are known. They then enter `MemoryIngest`; the reasoner is
not redesigned by this project.

### `Canonicalizer`

The canonicalizer owns lexical ordering and role-specific ID assignment. It
emits:

- the four front-coded dictionary sections;
- node-provisional to subject-ID and object-ID remaps;
- predicate-provisional to predicate-ID remaps;
- named-graph identity metadata for the ordinary builder; and
- a canonical `(subject, predicate, object, graph)` record stream.

In memory mode, records are remapped in place when that does not increase peak
memory. In chunked mode, remapping is a sequential read/write operation over the
chunk artifacts. Both modes must feed exactly the same canonical record order to
later stages for the same logical input.

The canonicalizer keeps the current HDT-style ID rules: shared nodes receive the
same subject and object ID; subject-only and object-only ranges follow the shared
range; predicates remain in a separate ID space. RDF-star detection continues to
derive from subject/object terms.

### `TripleSpool`

`TripleSpool` is a replayable source of canonical fixed-width triples. The
ordinary implementation may own a `Vec<Triple>`; the external implementation
owns a seekable temporary file. It exposes bounded sequential block reads and
does not expose its storage representation to family builders.

The spool is replayed three times, once for each leading component family, rather
than six times for six permutations. Family construction may pipeline two tail
orders from the same leading partition, but it must not retain an unbounded hot
group.

### `IndexFamilyBuilder`

There are three physical families:

| Family | First order | Second order |
|---|---|---|
| Subject | SPO | SOP |
| Predicate | POS | PSO |
| Object | OSP | OPS |

For one family, the builder radix-partitions records by the leading canonical ID.
Within a leading group it produces the two tail-key orders using bounded radix
runs. A group larger than the configured memory or tile budget is split into
continuation segments, preserving the current complete-scan rule for mega-groups.

Each family shares a leading-key routing directory. The two orders retain
independent tile directories, compressed payload streams, prefix-2 metadata, and
zone-map synopses. A query therefore selects exactly one order and fetches only
that order's routed tiles; it never downloads or decompresses its sibling order.

Parallel workers encode and compress complete tiles into a bounded queue. The
writer consumes results by monotonically assigned tile sequence, so scheduling
cannot change output bytes. Queue capacity is derived from the build memory
budget and has a small fixed default for ordinary builds.

### `DatasetWriter`

The CLI writer targets a temporary file in the destination directory. It writes
section bodies sequentially, records their checked offsets and lengths, computes
content hashes incrementally, writes the footer, validates the completed layout,
patches the reserved header, flushes, and installs the file through a
platform-specific atomic-replacement abstraction.

A failure before installation leaves the prior destination untouched. An
installation failure leaves either the prior complete file or the new complete
file, never the temporary partial image. Temporary spill and output files are
removed on ordinary errors and unwinding. The exact Windows and Unix replacement
mechanisms are implementation details behind one tested abstraction.

Library and WASM APIs that are required to return bytes use the same writer with
a `Vec<u8>` sink. They gain the new ingestion and family construction but cannot
avoid retaining their requested final byte result. The CLI file path is the main
beneficiary of streamed output.

## File Format `0x06`

The existing magic and fixed-header framing remain recognizable, but the format
byte becomes `0x06`. Section offsets, lengths, additions, and checksums remain
checked before allocation or range reads.

The default graph and every ordinary-build named graph contain three family
containers instead of six standalone permutation sections. A family container
has this logical layout:

```text
family header
shared leading-key directory
order A tile directory
order B tile directory
order A compressed tile payloads
order B compressed tile payloads
order A synopsis trailer
order B synopsis trailer
```

The leading directory maps a leading-ID range to the relevant tile-directory
ranges in both orders. Order-specific directories keep exact payload offsets,
continuation information, prefix-2 metadata, and tile lengths. This is a logical
layout requirement; the implementation plan may choose compact varint framing as
long as the specification, overflow limits, and range-request tests pin the exact
bytes before the writer is enabled.

`GraphIndex` constructs six lightweight `IndexPermutation` views over the three
families. Query planning, BGP evaluation, property paths, provenance, SHACL, and
reachability continue to request SPO/POS/OSP/SOP/PSO/OPS by name. The storage
layer maps the name to a family plus order. Lazy local, HTTP, owned-memory, and
WASM readers use the same mapping and loader contract.

The `0x06` reader rejects `0x05` and all earlier generations before attempting
to interpret their sections. The error identifies the encountered and required
format generation. Compatibility tests that currently require every future
reader to open the `0x05` baseline are replaced with tests pinning the deliberate
break and the `0x06` baseline.

`docs/SPEC.md`, compatibility notes, CLI documentation, browser documentation,
and generated HTML are updated with the new layout and migration consequences.

## Public Behavior

The primary commands and flags remain:

```text
rete build INPUT... -o OUTPUT
rete build INPUT... -o OUTPUT --memory-budget-mb N [--tmp-dir DIR]
```

There is no legacy-output flag. Both commands emit `0x06` once the format switch
lands. Existing feature compatibility checks remain in place for the external
path.

`RETE_BUILD_TIMING=1` expands from pyramid-only detail to a common phase report:

- parse and ingest;
- chunk sealing, when applicable;
- dictionary canonicalization;
- ID remapping;
- optional pyramid and text-index work;
- subject, predicate, and object family construction;
- tile encoding and compression;
- final section write and installation; and
- total wall time, statement count, input bytes when known, spill bytes, and
  output bytes.

Human-readable timing output remains diagnostic stderr, not a stable machine
protocol. Benchmark scripts record their own versioned JSONL evidence.

## Errors, Limits, and Cleanup

- Parser errors retain input path and line context.
- ID spaces fail clearly before exceeding `u32::MAX`.
- Every multiplication, addition, offset conversion, allocation length, record
  count, tile count, and compressed length uses checked arithmetic.
- A short read, overlong read, malformed directory, impossible continuation, or
  checksum mismatch is a clean error and never a panic.
- External memory budgets retain the existing floor and are working-set targets,
  not cgroup guarantees. All new queues and radix buffers derive explicit caps
  from that target.
- A hot leading group cannot bypass the memory bound; it is segmented and marked
  for complete routed continuation scans.
- Spill files use collision-resistant per-build directories under the selected
  temporary parent. Cleanup never recursively targets a path that was not
  created and retained by that build instance.
- Existing destinations are not truncated until a complete validated replacement
  is ready to install.
- No production `unsafe` is introduced.

## Determinism and Correctness

The same logical RDF dataset and build options must produce identical `0x06`
bytes through ordinary and external builds when their supported feature sets and
metadata payloads overlap. The direct equivalence gate therefore runs without a
card or supplies the same precomputed metadata payload to both paths; it does not
equate the ordinary builder's enriched derived card with the external builder's
deliberately counts-only card. Input order, duplicate statements, chunk
boundaries, memory budget, and Rayon thread count must not change the final
index, dictionary, or file bytes under that shared configuration.

The canonicalizer deduplicates only where current semantics deduplicate. Named
graph membership, RDF-star terms, metadata counts, pyramid inputs, text postings,
and all six sorted triple orders remain semantically equivalent to the existing
builder.

Differential correctness uses an immutable executable built from commit
`483c431cc6f0df38c42d9d0b7a215d29187d56b1` as the `0x05` reference. Because
the new reader intentionally cannot open the reference output, comparisons use:

- canonical N-Triples/N-Quads export hashes;
- statement, term, named-graph, and metadata counts;
- a pinned SPARQL result corpus covering every triple-pattern shape, merge joins,
  aggregates, paths, named graphs, SHACL, and reachability; and
- deterministic result ordering or canonical result hashing where SPARQL order
  is not specified.

## Performance Measurement

Benchmarks run in the repository's containerized toolchain. They use immutable
baseline and candidate executables, pinned RDF input hashes, fresh processes,
alternating execution order, warmups, and at least 15 accepted samples per
configuration. Raw JSONL evidence is exclusive-created under `target/bench/` and
records git SHA, executable SHA-256, toolchain, CPU/thread count, input identity,
options, output identity, phase times, wall time, peak RSS/heap, spill bytes,
output bytes, and result hashes.

The matrix contains:

- a small deterministic fixture for overhead and byte tests;
- the pinned Chemotion source used by the existing sorted-encoder benchmark;
- a typed multi-million-statement real or pinned synthetic source built with
  `--pyramid-algo types --card`;
- the same large source with `--no-pyramid` to isolate the shared pipeline;
- a skewed source with mega-groups; and
- a source large enough to force multiple external chunks and runs at 64, 256,
  and 1024 MiB budgets.

Primary acceptance gates on the representative large typed and no-pyramid
workloads are:

- candidate median wall time no more than 66.7% of baseline (at least 1.5x);
- candidate peak RSS and phase-profiler peak heap no more than 75% of baseline;
- new file size no more than 110% of baseline;
- local and range-backed query median and p90 no more than 105% of baseline;
- identical canonical exports and query-result hashes; and
- stable output hashes across runs and thread counts.

The external path must meet the time and memory gates at one production-sized
budget and show no correctness or unbounded-memory regression at every tested
budget. Small builds may carry fixed setup overhead, but no small workload may
be more than 20% slower or use more than 25% additional peak memory.

Default Louvain workloads must not regress more than 5% in any non-Louvain phase
or in total wall time. Their pipeline-only speedup is reported separately; the
1.5x total-wall gate does not apply when unchanged Louvain work is more than half
of baseline wall time.

## Test Strategy

### Unit and property tests

- A reference canonicalizer checks shared/subject/object/predicate IDs and every
  RDF lexical form.
- Memory and chunked ingest produce identical canonical artifacts.
- Every one of the eight triple-pattern shapes matches a decoded triple-set
  reference through every permutation view.
- Paired-family routing matches the current six-order reference on randomized,
  duplicate-heavy, and skewed graphs.
- Mega-groups split into bounded continuation tiles without missed or duplicate
  results.
- Bounded compression preserves tile order under varied Rayon thread counts.
- Offset, count, and allocation boundary cases fail cleanly.
- Arbitrary and mutated `0x06` bytes never panic.
- Writer failure injection covers parse, canonicalization, spill, compression,
  write, flush, header patch, validation, and install failures.

### Integration tests

- Ordinary and external builds produce identical `0x06` bytes for their shared
  supported feature set when given identical metadata payloads.
- CLI stdin and non-replayable inputs parse once and build successfully.
- The CLI never exposes a partial destination and preserves an existing valid
  destination on failure.
- Eager and ranged readers return identical results for local, HTTP, owned
  memory, and WASM sources.
- Range tests assert that choosing one family order fetches no sibling-order tile
  payload.
- The `0x06` reader rejects a valid `0x05` fixture with the documented version
  error.
- Differential export and query suites compare the immutable `0x05` reference
  executable with the candidate.

### Repository gates

Before completion, run at minimum:

```sh
docker compose run --rm dev cargo fmt --all -- --check
docker compose run --rm dev cargo clippy --workspace --exclude rete-bench --all-targets -- -D warnings
docker compose run --rm dev cargo test --workspace --exclude rete-bench
docker compose run --rm dev cargo test -p rete-core --no-default-features
docker compose run --rm dev cargo build -p rete-core --all-features
docker compose run --rm dev cargo build -p rete-bench
docker compose run --rm dev bash scripts/smoke.sh
docker compose run --rm -e RETE_SOURCE_REVISION=$(git rev-parse HEAD) wasm
bash tests/gate/gate.sh
```

Format-byte tests, `docs/SPEC.md`, generated HTML, tracked fixtures, WASM
artifacts, playground artifacts, and social previews must agree with `0x06`.

## Delivery Sequence

1. Pin the immutable `0x05` baseline and add common phase instrumentation plus
   benchmark evidence capture without changing output bytes.
2. Introduce provisional IDs, the ordinary interner, chunked ingest artifacts,
   canonicalizer, and memory/disk spool equivalence tests.
3. Add the streaming writer and failure-safe destination installation while the
   old six-section format is still produced internally.
4. Specify exact family bytes in `docs/SPEC.md`; add `0x06` parser, routing,
   corruption, range, property, and differential tests before enabling its
   writer.
5. Implement the three family builders and bounded compression scheduler; prove
   all six logical views against the reference.
6. Switch every builder and reader to `0x06`, remove the `0x05` reader/writer,
   replace compatibility fixtures, and update public documentation.
7. Run the complete performance matrix. Revise or remove any optimization that
   misses correctness, memory, query, or size gates.
8. Rebuild tracked fixtures and canonical browser artifacts, then run the full
   repository and browser gates.
9. Prepare a separate catalog migration inventory. Rebuildable datasets get
   proposed `0x06` artifacts; unrebuildable entries get a visible unavailable
   state. No remote publication occurs in this implementation task.

Each step lands at a testable boundary. The final format switch is not considered
complete while tracked code or artifacts silently assume `0x05`.

## Catalog and Release Consequences

The current R2 catalog consists of `0x05` objects and is intentionally
incompatible with the new reader until separately migrated. The repository must
not leave those entries looking selectable and then fail at query time. Catalog
metadata gains an explicit availability/version state, and the browser explains
that a dataset requires a `0x06` rebuild.

Tracked small datasets and gate fixtures are rebuilt locally. Catalog sources
that are reproducible from tracked recipes can be queued for a later publish.
Datasets without recoverable source remain unavailable to `0x06`; their old R2
objects are not deleted. An R2 publication plan must pin before/after length,
ETag, content hash, output correctness, recovery objects, and client rollout,
and requires separate user approval.

## Risks and Mitigations

- **The family design saves less time than expected.** Phase benchmarks isolate
  leading partition, tail ordering, tiling, and compression. The writer is not
  switched until the family builder passes the 1.5x target on the isolated
  no-pyramid workload.
- **Shared metadata adds a read-path lookup.** Range and resident query gates cap
  the regression at 5%; directories may be duplicated selectively if needed,
  provided the file-size gate still passes.
- **A hot key defeats bounded memory.** Continuation segmentation is exercised by
  synthetic billion-scale cardinality models and concrete bounded tests before
  external benchmarks.
- **Single-pass interning grows with unique vocabulary.** Ordinary builds account
  for one lexical allocation per unique term; external builds retain chunk-local
  interners and merged disk artifacts rather than a global resident hash table.
- **The deliberate format break strands data.** The UI exposes compatibility
  state, old objects are retained, and publication is a separately approved
  migration rather than an implicit side effect of code changes.
- **Streaming output complicates failure recovery.** A single installation
  abstraction and systematic failure injection pin the cross-platform contract.
- **Compression parallelism inflates memory.** Queue capacity is explicit,
  budget-derived, and measured; workers never accumulate an unbounded completed
  batch.

## Completion Criteria

The project is complete only when:

- all builders emit only `0x06` and all readers accept only valid `0x06`;
- ordinary and external builds share the approved pipeline and deterministic
  output contract;
- the primary build-time, memory, file-size, and query gates pass with preserved
  raw evidence;
- malformed input and failure-injection suites remain panic-free and preserve
  existing destinations;
- the full native, no-default, all-feature, WASM, smoke, documentation, and
  browser matrices pass;
- tracked fixtures and generated artifacts contain `0x06`;
- incompatible catalog entries are visibly unavailable rather than silently
  broken; and
- no production `unsafe` or unauthorized R2 mutation has been introduced.
