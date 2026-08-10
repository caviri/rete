# R2 Reader Hardening and Shard Validation Design

## Context

The cold native R2 optimization on `feat/rust-optimization` gives
`rete sparql-url` an adaptive transfer policy: a non-empty HTTP object no larger
than 8 MiB is fetched with one exact full-file range request, while larger
objects retain lazy block-cached range reads. The accepted Chemotion benchmark
shows 50.5-73.7% median wall-time reductions with identical output and bounded
memory.

A post-implementation review found three correctness and documentation gaps:

1. `HttpRangeReader` compares `Content-Range` as a literal string. It therefore
   rejects semantically valid case variants and leading-zero numerals, while
   reading only the first matching field and accepting a valid first field plus
   a conflicting duplicate.
2. `Rete::open_ranged_lazy` still fetches the complete named-graphs section and
   eagerly decompresses every named graph's six index permutations. An
   otherwise eligible small untrusted file can therefore cause large automatic
   decompression during open.
3. CLI, generated HTML, module, and browser documentation contain stale or
   over-broad claims about eager parsing, byte-touch behavior, and the initial
   HTTP probe.

The user also requested validation against additional Cloudflare R2 datasets
and the catalog's sharded datasets. Investigation confirms an important scope
boundary: the 8 MiB policy belongs to native `sparql-url`. Browser sharding uses
one WASM `RemoteGraph` per shard and remains range-lazy; CLI `rete federate`
uses its older opener. Sharding is semantically compatible, but neither
federation surface currently receives the native one-GET policy.

This design remediates the review findings, validates the real R2 and sharded
paths, and preserves that scope boundary.

## Goals

- Accept every `Content-Range` representation that is semantically exact for
  the requested known-length byte range and reject duplicate, ambiguous,
  malformed, overflowing, or mismatched fields.
- Ensure `Rete::open_ranged_lazy` fetches and decompresses zero named-graph tile
  payloads during open.
- Preserve current `.rete` bytes, stable public APIs, query answers, local
  readers, native HTTP readers, owned-memory readers, and default WASM builds.
- Preserve clean failure behavior for untrusted remote files and incomplete
  lazy reads.
- Correct all stale documentation in the reviewed scope and regenerate tracked
  HTML.
- Re-run the pinned Chemotion comparison and add independent small, large, and
  sharded R2 validation with exact source pins and output hashes.

## Non-goals

- No `.rete` format revision or new section metadata.
- No production unchecked indexing, pointer sharing, `transmute`, `set_len`, or
  other new `unsafe`. The existing unchecked decoder remains research-only
  behind `unsafe-decode-bench`.
- No fixed global zstd expansion cap in this change. The format does not record
  uncompressed tile lengths, and valid writers may use custom tile budgets.
- No claim that query-triggered decompression is globally bounded. This change
  removes automatic named-tile decompression from lazy open; a later query that
  selects a tile still invokes the existing decoder.
- No adaptive-transfer refactor of `rete federate`. That is a separate
  follow-up after this remediation passes review.
- No change to browser/WASM's always-lazy remote policy.
- No change to existing UNION-federation limitations: per-source aggregates and
  limits, and no CLI cross-shard joins.

## 1. Semantic and Unambiguous `Content-Range`

### Field cardinality

`ureq` 2.12.1 stores duplicate response fields separately. Its
`Response::header` returns the first readable value, while `Response::all`
omits values that cannot be represented as UTF-8. Duplicate detection will
therefore count case-insensitive `content-range` occurrences in
`Response::headers_names`, not rely on `header` or `all`.

The reader requires exactly one occurrence:

- zero fields is a missing-header `InvalidData` error;
- more than one field is an ambiguous-response `InvalidData` error, even when
  all values are identical;
- one occurrence whose value cannot be read is an invalid-field `InvalidData`
  error rather than being misreported as missing.

`Content-Range` is not list-valued. A comma-combined value in one field is
rejected by the parser.

### Semantic parser

A private helper parses only the strict response form this reader can accept:

```text
range-unit SP first-last/complete-length
```

The range unit is compared case-insensitively with `bytes`. Each numeric
component must be non-empty ASCII digits and must parse as `u64`. Leading zeros
are accepted. Unknown totals (`*`), unsatisfied-range forms (`*/length`), signs,
extra delimiters, extra internal whitespace, commas, and overflow are rejected.

The parsed tuple must equal `(requested_offset, requested_end, reader_len)`.
Comparison is numeric, so `ByTeS 00100-00139/001000` is equivalent to
`bytes 100-139/1000`. The canonical expected field and actual response remain
in the contextual error.

### Body and error contract

Header validation remains before body consumption. The existing bounded body
read (`take(requested_len + 1)`), exact body-length check, URL/offset/length
context, and underlying I/O error kind remain unchanged. The truncated-body
test is strengthened to require `UnexpectedEof` as well as context.

## 2. Tile-Lazy Named Graphs

### Root cause

The current lazy opener reads `header.named_graphs_len` bytes and calls
`decode_named_graphs`. That calls `decode_index_container` for every graph,
which decodes every compressed tile through an uncapped zstd `read_to_end`.
Consequently the default graph is tile-lazy but named graphs are fully resident.

Disabling the small-file transfer policy would not fix this: both the HTTP lazy
branch and the owned-memory full-transfer branch call the same lazy opener.
A fixed decompression cap would also be format-incompatible because the public
builder permits custom tile budgets and the format records compressed, not raw,
tile sizes.

### Ranged named-graph construction

The existing named-graphs format already provides the required framing. Each
record contains:

```text
IRI length | IRI bytes | index-container length | index-container bytes
```

`open_ranged_lazy` will walk this framing through the supplied `RangeReader`,
bounded by the named-graphs section range. It reads graph names and container
lengths, skips tile payload bytes, and locates the six permutation sections in
each graph's index container.

For every graph it reads only each permutation's tile directory and optional
synopsis. It then constructs a remote `GraphIndex` whose tiles contain loaders
backed by the same owned `Arc<RangeReader>`. The loader fetches and decompresses
one selected named-graph tile; the bulk loader coalesces selected adjacent tiles
for scans. Read concurrency and encoded tile lengths are propagated exactly as
for the default graph.

The default-graph and named-graph construction paths will share a focused
internal helper where that removes duplication without changing a public API.
The helper returns the `GraphIndex` plus any directory/range metadata its caller
needs. It does not introduce a second cache or a new ownership model.

### API and format compatibility

`Rete` continues storing `Vec<(String, GraphIndex)>`. Each named `GraphIndex`
exists when open returns; only its tile `OnceLock`s are empty. Therefore these
public signatures remain unchanged:

- `named_graphs() -> &[(String, GraphIndex)]`;
- `graph_names() -> Vec<&str>`;
- `graph_index(&str) -> Option<&GraphIndex>`.

`Rete::open` and `Rete::open_ranged` retain their documented eager behavior.
Only `Rete::open_ranged_lazy` changes. No bytes, header fields, codecs, feature
flags, or dependencies change.

The same implementation works for:

- native HTTP `HttpRangeReader` and its wrappers;
- the small-object `OwnedMemoryRangeReader` after one full transfer;
- lazy local readers;
- WASM `XhrRangeReader` / `RemoteGraph`;
- builds without native compression features.

### Failure contract

Malformed named-graph framing or a malformed tile directory remains an
open-time `FileError`, because the structure cannot safely be represented.
A corrupt or unavailable tile payload is a lazy-load failure, matching the
default graph: the tile is not accepted as valid, the graph marks its load
incomplete, and callers must reject the query through `Rete::index_incomplete`.
Resetting the failure state permits a later retry.

The scoped safety invariant is:

> After `Rete::open_ranged_lazy` returns, it has parsed only bounded on-disk
> framing, graph-name bytes, tile directories, and synopses. It has fetched and
> decompressed zero named-graph tile payloads.

Open-time named-graph memory becomes O(graph names + directory records), not
O(all uncompressed named indexes).

## 3. Documentation Corrections

The remediation updates source Markdown and code comments to distinguish:

- eager transfer from eager parsing: eligible native `sparql-url` objects are
  transferred once but still opened through the tile-lazy reader;
- default-graph and named-graph behavior after the lazy named-index change;
- `--entail` byte behavior: a small eligible object is transferred in full,
  while a larger object fetches the ranges touched by rewritten evaluation;
- browser/WASM's always-lazy policy versus native CLI's adaptive transfer;
- browser length probing: prefer HEAD, with a `Range: bytes=0-0` fallback;
- catalog sharding versus native `sparql-url`, and the separate CLI
  `rete federate` path.

At minimum this covers the reviewed stale paragraph in `docs/cli.md`, the
module and command comments in `crates/rete-cli/src/commands/url.rs`, and the
probe wording in `docs/browser.md`. `docgen` regenerates the tracked HTML; no
generated HTML is edited by hand.

## 4. Test Strategy

### HTTP protocol tests

The existing local socket server gains focused response modes and parser table
tests for:

- canonical success;
- mixed-case `bytes` plus leading-zero numeric fields;
- identical and conflicting duplicate field lines;
- one comma-combined field;
- wrong unit, unknown total, 416 form, signs, empty fields, extra whitespace,
  extra delimiters, and `u64` overflow;
- exact numeric mismatch;
- truncated response body preserving `UnexpectedEof` and request context.

### Core named-graph tests

Instrumented `RangeReader` tests build a multi-tile named-graph fixture and
record every physical range. They require:

- lazy open and `graph_names` read no named tile payload;
- eager and lazy results match for `GRAPH <g>`, `GRAPH ?g`, and `FROM <g>`;
- a bound named-graph query reads only routed tile payloads;
- a corrupt named tile does not fail open but makes the first touching query
  incomplete;
- a transient post-open read failure sets `index_incomplete`, and reset plus
  retry succeeds;
- malformed framing and directories remain clean open errors.

The tests run with default features and `rete-core --no-default-features`.
WASM compilation and browser-facing tests ensure the generic reader remains
portable.

### CLI and documentation tests

Focused CLI tests and `scripts/smoke.sh` retain the exact one-request boundary,
forced-lazy behavior, local-source environment isolation, and output equality.
Generated HTML must match its Markdown source.

## 5. Real R2 and Shard Validation

Every benchmark pins `Content-Length`, ETag, executable SHA-256, query text,
sample order, output SHA-256, bytes, request count, wall time, and peak RSS.
Modes rotate cyclically to reduce warm-order bias. Source metadata is checked
before and after the run.

The existing benchmark harness remains backward-compatible with its pinned
Chemotion workload and is generalized only as needed to accept a checked,
explicit workload definition for other datasets. It must never silently apply
Chemotion's expected metadata or queries to another `--source`.

### Pinned matrix

1. **Chemotion, 7,566,404 bytes.** Re-run the existing three-query comparison
   for delegated lazy versus the 8 MiB policy. Output and one-GET accounting
   must remain identical to the accepted behavior.
2. **BOE, 6,958,628 bytes.** Run deterministic bound-subject and aggregate
   queries at thresholds 0 and 8. Both modes must have identical output hashes;
   threshold 8 must fetch exactly the complete object in one counted data GET.
   Report median and p90 wall time rather than assuming a win.
3. **Chebi Full, 164,832,053 bytes.** Run a deterministic selective query at
   thresholds 0 and 8. Both modes must remain lazy, return identical output,
   and avoid a full-object transfer.
4. **Wikidata XXL, six catalog shards totaling 4,872,958,617 bytes at the
   observed pin.** Run a browser/WASM ASK fan-out and a bounded ordered SELECT.
   All six `RemoteGraph` sources must remain lazy; ASK is OR-reduced and SELECT
   is unioned/deduplicated in fixed source order. No shard may be fully
   downloaded.
5. **Browser small-object smoke.** A small catalog object is queried twice
   through one resident `RemoteGraph`; it remains range-lazy and the second
   query reuses its session cache. Native threshold behavior must not leak into
   WASM.

`rete federate` may receive a correctness-only smoke with the same ASK query.
Its timing is not attributed to the `sparql-url` optimization because it uses a
different opener. A future design can unify those paths after this branch is
safe and reviewed.

### Acceptance

- Every compared mode returns the same canonical output hash for its workload.
- Eligible native objects use exactly one full-object data GET; objects above
  the threshold stay lazy.
- The accepted Chemotion medians, p90s, transfer counts, and peak RSS do not
  materially regress after remediation.
- BOE and Chebi results are reported with sample count and network-variance
  caveats; they validate generality rather than retroactively tuning the
  threshold to one favorable dataset.
- The six-shard browser smoke produces correct merged semantics without any
  full-shard transfer.
- Raw benchmark records and source pins remain reproducible and are not
  committed if repository policy treats them as local artifacts.

## Delivery Order

1. Add failing HTTP parser/cardinality tests, implement semantic validation,
   and run the focused CLI tests.
2. Add failing named-graph range-recording tests, extract the lazy graph-index
   helper, and make named graph tiles lazy.
3. Correct source documentation and regenerate tracked HTML.
4. Generalize the benchmark input boundary without changing the Chemotion
   default, then run the small/large native R2 matrix.
5. Run WASM/browser small-object and six-shard compatibility tests.
6. Run independent code review, remediation review, and the complete Docker
   verification gates.

The HTTP and core changes are independent and can be implemented and reviewed
by separate agents. Documentation follows verified behavior. Real network
benchmarks run only after the candidate passes focused tests.

## Verification Gates

Before completion, run in the repository Docker/devcontainer toolchain:

```sh
docker compose run --rm dev cargo fmt --all -- --check
docker compose run --rm dev cargo clippy --workspace --exclude rete-bench --all-targets -- -D warnings
docker compose run --rm dev cargo test --workspace --exclude rete-bench
docker compose run --rm dev cargo test -p rete-core --no-default-features
docker compose run --rm dev cargo build -p rete-core --all-features
docker compose run --rm dev cargo build -p rete-bench
docker compose run --rm dev bash scripts/smoke.sh
docker compose run --rm wasm wasm-pack build crates/rete-wasm --target web --out-dir ../../web/pkg
docker compose run --rm wasm wasm-pack build crates/rete-wasm --target no-modules --out-dir ../../web/pkg-nomodules
```

Focused tests, benchmark harness tests, generated-document checks, and browser
shard assertions run in addition to these broad gates. Completion claims must
quote fresh command results and the final R2 measurements.
