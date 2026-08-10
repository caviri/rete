# R2 Reader Hardening Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Harden native HTTP range validation, make named-graph indexes truly lazy on ranged opens, correct the user-facing read-path documentation, and produce reproducible cold-R2 evidence across several catalog datasets and the browser's six-shard path.

**Architecture:** Keep the `.rete` bytes and all public query APIs unchanged. The native HTTP reader will parse one strict semantic `Content-Range` field instead of comparing one canonical spelling. `Rete::open_ranged_lazy` will reuse one internal remote-index constructor for both the default graph and every named graph, so opening reads only framing, dictionary metadata, graph names, tile directories, and optional synopses. The existing benchmark driver will accept a pinned JSON workload while retaining its current Chemotion `--source` interface.

**Tech Stack:** Rust 2021, `ureq`, rete-core `RangeReader`/`GraphIndex`, Python 3 with `unittest`, Docker Compose dev/wasm/gate images, Playwright catalog gates, Cloudflare R2.

## Global Constraints

- Work only in `D:\pro\rete\.claude\worktrees\rust-optimization` on `feat/rust-optimization`.
- Run Rust, Python, docs, WASM, and browser checks through the repository Docker Compose services.
- Do not change file-format bytes, `Header`, `docs/SPEC.md`, or public graph lookup signatures.
- Do not add unchecked indexing, `transmute`, manual `Send`/`Sync`, `set_len`, or other new `unsafe` code. All offsets and lengths remain untrusted and bounds-checked.
- Preserve eager behavior in `Rete::open` and `Rete::open_ranged`; only `Rete::open_ranged_lazy` changes named-graph loading.
- Preserve the current lazy failure contract: a failed tile is not cached, sets `index_incomplete`, and is retried after `reset_load_failures`.
- Do not claim global decompression hardening. Tile laziness removes open-time named-graph decompression, but query-triggered zstd tile decompression retains the existing uncapped behavior.
- Use test-first RED/GREEN/REFACTOR for every production-code change. Commit each task without a `Co-Authored-By` trailer.

---

## Task 1: Parse and validate `Content-Range` semantically

**Files:**

- Modify: `crates/rete-cli/src/http.rs`
- Test: `crates/rete-cli/src/http.rs` (`#[cfg(test)] mod tests`)

- [x] **Step 1: Add failing parser and transport tests**

  Introduce a table-driven unit test for a private parser with this shape:

  ```rust
  fn parse_content_range(value: &str) -> Option<(u64, u64, u64)>;
  ```

  Accepted forms must include canonical text, case-insensitive `bytes`, and ASCII leading zeros:

  ```rust
  assert_eq!(parse_content_range("bytes 100-139/1000"), Some((100, 139, 1000)));
  assert_eq!(parse_content_range("BYTES 0100-0139/01000"), Some((100, 139, 1000)));
  ```

  Rejected forms must cover `bytes */1000`, `bytes 100-139/*`, signed values, Unicode digits, tabs, doubled or trailing spaces, commas, extra `/` or `-`, empty components, and integer overflow. Do not normalize whitespace or accept a multi-range response.

  Extend `ServerMode` and `serve` so transport tests can return:

  - two raw `Content-Range` fields;
  - one case-variant/leading-zero field that is semantically correct;
  - one syntactically valid field whose start, end, or total is wrong.

  Assert duplicate fields fail even if their values match. Assert all protocol failures are `std::io::ErrorKind::InvalidData` and mention URL, requested offset, requested length, expected tuple, and actual raw value/count. Strengthen `rejects_a_truncated_range_response` so the returned kind is `UnexpectedEof` while retaining the same context assertions.

- [x] **Step 2: Run the focused tests and observe RED**

  ```powershell
  docker compose run --rm dev cargo test -p rete-cli http::tests::parse_content_range -- --nocapture
  docker compose run --rm dev cargo test -p rete-cli http::tests::rejects_duplicate_content_range_fields -- --nocapture
  docker compose run --rm dev cargo test -p rete-cli http::tests::rejects_a_truncated_range_response -- --nocapture
  ```

  Expected failures: the parser does not exist, duplicate headers are not counted, and the truncated-body kind is not normalized to `UnexpectedEof`.

- [x] **Step 3: Implement the strict parser and single-field check**

  Add small helpers above `impl RangeReader for HttpRangeReader`:

  ```rust
  fn parse_ascii_u64(value: &str) -> Option<u64> {
      (!value.is_empty() && value.bytes().all(|byte| byte.is_ascii_digit()))
          .then(|| value.parse().ok())
          .flatten()
  }

  fn parse_content_range(value: &str) -> Option<(u64, u64, u64)> {
      let (unit, range_and_total) = value.split_once(' ')?;
      if !unit.eq_ignore_ascii_case("bytes") || range_and_total.contains(' ') {
          return None;
      }
      let (range, total) = range_and_total.split_once('/')?;
      let (first, last) = range.split_once('-')?;
      Some((
          parse_ascii_u64(first)?,
          parse_ascii_u64(last)?,
          parse_ascii_u64(total)?,
      ))
  }
  ```

  Before reading the value, count names from `resp.headers_names()` with `eq_ignore_ascii_case("content-range")`. Require exactly one raw field; do not use an API that combines duplicates into comma-separated text or silently drops unreadable values. Parse `resp.header("content-range")`, then compare the tuple to `(offset, end, self.len)`.

  Build `InvalidData` errors through one contextual helper so missing, duplicate, malformed, and mismatched headers all include:

  ```text
  invalid Content-Range for requested {len} bytes at offset {offset} from {url}: expected bytes {offset}-{end}/{total}; got {actual}
  ```

  For a header-count failure, render `got {count} fields`. For a malformed field, render its debug-quoted raw value. Map premature body EOF—including `ureq`'s body-closed error—to `UnexpectedEof`, while preserving the original body-read error in the message. Do not change other I/O error kinds.

- [x] **Step 4: Run GREEN and package checks**

  ```powershell
  docker compose run --rm dev cargo fmt --all -- --check
  docker compose run --rm dev cargo test -p rete-cli http::tests -- --nocapture
  docker compose run --rm dev cargo test -p rete-cli
  ```

- [x] **Step 5: Review and commit Task 1**

  Verify no acceptance path permits wildcard totals, multi-range syntax, non-ASCII digits, or duplicate raw fields. Verify all range/body failures retain URL and request coordinates.

  ```powershell
  git add crates/rete-cli/src/http.rs
  git commit -m "fix: validate HTTP content ranges semantically"
  ```

---

## Task 2: Make named-graph indexes tile-lazy in ranged lazy opens

**Files:**

- Modify: `crates/rete-core/src/file.rs`
- Modify: `crates/rete-core/src/reader.rs`
- Modify: `crates/rete-core/src/block_cache.rs`
- Modify: `crates/rete-core/tests/ranged.rs`

- [x] **Step 1: Build a multi-tile named-graph fixture and add RED tests**

  In `crates/rete-core/tests/ranged.rs`, import `write_dataset` and add a fixture that builds one default graph plus two named graphs against one shared dictionary. Use `GraphIndexBuilder::with_tile_budget(64)` and enough sorted triples per graph to force multiple tiles in every relevant permutation.

  Add a test-only framing walker that uses the public `Header` offsets and the repository varint format to return the absolute payload ranges of all named-graph tiles. This walker must bounds-check every slice and is only an independent oracle for read-overlap assertions.

  Add these tests:

  ```rust
  #[test]
  fn lazy_open_reads_named_graph_directories_but_no_named_tile_payloads() { /* ... */ }

  #[test]
  fn lazy_named_graph_queries_match_eager_for_graph_from_and_graph_variable() { /* ... */ }

  #[test]
  fn lazy_named_graph_query_fetches_only_the_routed_tiles() { /* ... */ }

  #[test]
  fn lazy_named_graph_tile_failure_sets_incomplete_and_retries_after_reset() { /* ... */ }

  #[test]
  fn corrupt_named_tile_fails_on_first_scan_not_open() { /* ... */ }

  #[test]
  fn malformed_named_graph_framing_or_directory_fails_open_cleanly() { /* ... */ }
  ```

  The correctness test must compare `Rete::open_ranged_lazy` with `Rete::open` for all three forms:

  ```sparql
  SELECT ?s ?o WHERE { GRAPH <http://ex/g1> { ?s <http://ex/p> ?o } } ORDER BY ?s ?o
  SELECT ?g ?s WHERE { GRAPH ?g { ?s <http://ex/p> ?o } } ORDER BY ?g ?s
  SELECT ?s ?o FROM <http://ex/g1> WHERE { ?s <http://ex/p> ?o } ORDER BY ?s ?o
  ```

  The failure/recovery test must open successfully, enable `RecordingReader::fail_from_now`, evaluate a named-graph query, observe `index_incomplete() == true`, call `recover()` and `reset_load_failures()`, re-evaluate, and compare the recovered result to eager output. The corruption test must flip a byte inside a compressed named tile while leaving framing and its directory intact; lazy open succeeds, the first routed scan sets incomplete. Separate mutations of the graph-record framing and one tile directory must remain clean open-time `FileError`s.

- [x] **Step 2: Run focused tests and observe RED**

  ```powershell
  docker compose run --rm dev cargo test -p rete-core --test ranged lazy_open_reads_named_graph -- --nocapture
  docker compose run --rm dev cargo test -p rete-core --test ranged lazy_named_graph -- --nocapture
  docker compose run --rm dev cargo test -p rete-core --test ranged corrupt_named_tile -- --nocapture
  ```

  Expected failure: `open_ranged_lazy` currently reads and decodes the entire `named_graphs` section, overlapping every named tile before a query.

- [x] **Step 3: Extract one internal remote-index constructor**

  In `crates/rete-core/src/file.rs`, introduce a private result carrier and constructor near the existing ranged directory helpers:

  ```rust
  struct RemoteGraphIndex {
      index: GraphIndex,
      section_ranges: [ByteRange; NUM_PERMS],
      tile_ranges: [Vec<(u32, u32, ByteRange)>; NUM_PERMS],
  }

  fn open_remote_graph_index<R: RangeReader + Send + Sync + 'static>(
      reader: std::sync::Arc<R>,
      container: ByteRange,
      codec: u8,
      has_tile_synopsis: bool,
      read_concurrency: usize,
  ) -> Result<RemoteGraphIndex, FileError>;
  ```

  The helper must:

  1. locate six permutation sections with `locate_container_section_ranged`;
  2. read each tile directory and optional synopsis only;
  3. compute absolute tile payload ranges;
  4. install the existing per-tile and coalesced bulk loaders;
  5. set tile lengths and reader concurrency;
  6. return without fetching or decompressing a tile payload.

  Refactor the default-graph block inside `open_ranged_lazy` to call this helper. This should be behavior-preserving and prevents the named-graph implementation from copying loader logic.

- [x] **Step 4: Walk named-graph framing lazily**

  Add a ranged named-graph decoder with this signature:

  ```rust
  fn open_named_graphs_ranged_lazy<R: RangeReader + Send + Sync + 'static>(
      reader: std::sync::Arc<R>,
      section: ByteRange,
      codec: u8,
      has_tile_synopsis: bool,
      read_concurrency: usize,
  ) -> Result<Vec<(String, GraphIndex)>, FileError>;
  ```

  Walk the existing layout without reading the whole section:

  ```text
  graph_count
    repeated graph_count times:
      iri_len | iri_bytes | index_container_len | index_container_bytes
  ```

  Use `read_uvarint_at`, `checked_end`, and explicit comparisons to the named-section end. Fetch only the IRI bytes plus the six directories/synopses inside each index container. Reject graph counts that do not fit `usize` and any field that crosses the section boundary. Preserve the existing `String::from_utf8_lossy` graph-name behavior and the eager decoder's treatment of harmless trailing section bytes. Do not decompress index tiles in this walker.

  Replace only the named-graph block in `open_ranged_lazy`:

  ```rust
  let named_graphs = if header.named_graphs_len > 0 {
      open_named_graphs_ranged_lazy(
          reader.clone(),
          ByteRange {
              offset: header.named_graphs_offset,
              len: header.named_graphs_len,
          },
          header.block_codec,
          header.has_tile_synopsis(),
          read_concurrency,
      )?
  } else {
      Vec::new()
  };
  ```

  Leave `decode_named_graphs` in place for `Rete::open` and `Rete::open_ranged`.

- [x] **Step 5: Run GREEN across feature variants**

  ```powershell
  docker compose run --rm dev cargo fmt --all -- --check
  docker compose run --rm dev cargo test -p rete-core --test ranged -- --nocapture
  docker compose run --rm dev cargo test -p rete-core --no-default-features
  docker compose run --rm dev cargo test -p rete-core
  docker compose run --rm dev cargo build -p rete-core --all-features
  ```

- [x] **Step 6: Review invariants and commit Task 2**

  Confirm from the recording tests that lazy open performs zero reads overlapping named tile payloads; selective queries read only routed payloads; eager APIs still decode named graphs eagerly; failures are uncached and reflected by the pre-existing aggregate `index_incomplete`/reset methods.

  ```powershell
  git add crates/rete-core/src/file.rs crates/rete-core/tests/ranged.rs
  git commit -m "perf: fault named graph tiles lazily"
  ```

---

## Task 3: Correct native, browser, and source documentation

**Files:**

- Modify: `crates/rete-cli/src/commands/url.rs`
- Modify: `crates/rete-cli/src/main.rs`
- Modify: `crates/rete-cli/tests/help_contract.rs`
- Modify: `crates/rete-wasm/src/lib.rs`
- Modify: `scripts/build_playground.py`
- Modify: `docs/cli.md`
- Modify: `docs/browser.md`
- Regenerate: `docs/cli.html`
- Regenerate: `docs/browser.html`

- [x] **Step 1: Replace the stale read-path claims**

  Update the `sparql-url` prose in `docs/cli.md` in both the earlier reasoning section and the command reference. State precisely:

  - native `sparql-url` validates a `HEAD` probe and uses the adaptive opener;
  - files at or below `RETE_EAGER_MAX_MB` (default 8 MiB) are fetched once into owned memory, then parsed/query-executed from that image;
  - larger files remain remote-lazy and use HTTP range reads;
  - `RETE_EAGER_MAX_MB=0` forces remote-lazy behavior;
  - the hidden `--unsafe-decode` flag exists only in non-default `unsafe-decode-bench` builds: HTTP and outer framing validation remain, but triple-block bounds validation is explicitly skipped and malformed/truncated/mutable input can cause undefined behavior;
  - `rete federate` retains its existing ranged opener and is not part of the small-object one-GET optimization.

  In `docs/browser.md`, replace the claim that length discovery uses `Range: bytes=0-0` instead of `HEAD`. Explain that the browser reader prefers `HEAD` and falls back to a `0-0` range probe when needed, and that R2 CORS must expose `Content-Range` for the fallback.

  Update module/command comments in `crates/rete-cli/src/commands/url.rs` so they match the adaptive behavior. Comments must not claim all HTTP queries remain range-lazy.

- [x] **Step 2: Run documentation generation and focused verification**

  ```powershell
  docker compose run --rm dev cargo run -q -p docgen
  docker compose run --rm dev cargo fmt --all -- --check
  docker compose run --rm dev cargo test -p rete-cli
  rg -n "RETE_EAGER_MAX_MB|one GET|remote-lazy|0-0|Content-Range|federate" docs/cli.md docs/browser.md crates/rete-cli/src/commands/url.rs
  ```

  Inspect the generated HTML diff and verify it contains only the intended prose regeneration.

- [x] **Step 3: Review and commit Task 3**

  ```powershell
  git add crates/rete-cli/src/commands/url.rs docs/cli.md docs/browser.md docs/cli.html docs/browser.html
  git commit -m "docs: clarify adaptive remote reads"
  ```

---

## Task 4: Generalize the cold-R2 benchmark with pinned workload files

**Files:**

- Modify: `scripts/bench_cold_r2.py`
- Modify: `scripts/test_bench_cold_r2.py`
- Create: `scripts/cold-r2-workloads/boe.json`
- Create: `scripts/cold-r2-workloads/chebi-full.json`

- [ ] **Step 1: Add failing workload parsing and compatibility tests**

  Add tests for a strict JSON workload loader. The accepted schema is:

  ```json
  {
    "name": "boe",
    "source": "https://data.graphplaza.com/boe/boe.rete",
    "expected_length": 6958628,
    "expected_etag": "\"460709f1f8c26dd15a02e5df5dbfecfa\"",
    "queries": [
      {"name": "bounded-select", "sparql": "SELECT ?s ?p ?o WHERE { ?s ?p ?o } ORDER BY ?s ?p ?o LIMIT 100"}
    ]
  }
  ```

  Assert rejection of missing/unknown fields, empty names/SPARQL/source/ETag, non-positive length, an empty query list, and duplicate query names. Add a subprocess test showing `--workload custom.json` executes exactly `query_count × mode_count × samples` processes, includes the workload name in every JSONL record, pins the custom server's length/ETag before and after, and hashes output consistently. Retain the existing `--source` Chemotion test and its nine-record expectation to prove backward compatibility. Assert `--source` and `--workload` are mutually exclusive and exactly one is required.

- [ ] **Step 2: Run the benchmark tests and observe RED**

  ```powershell
  docker compose run --rm dev uv run python scripts/test_bench_cold_r2.py -v
  ```

  Expected failure: `load_workload`, `Workload`, and `--workload` do not exist.

- [ ] **Step 3: Implement `Workload` and CLI selection**

  Add:

  ```python
  @dataclass(frozen=True)
  class Workload:
      name: str
      source: str
      expected_length: int
      expected_etag: str
      queries: tuple[tuple[str, str], ...]

  CHEMOTION_WORKLOAD = Workload(
      name="chemotion",
      source="",
      expected_length=EXPECTED_LENGTH,
      expected_etag=EXPECTED_ETAG,
      queries=QUERIES,
  )
  ```

  Implement `load_workload(path: pathlib.Path) -> Workload` with exact-key validation and the rules tested above. Change `require_pinned_metadata` to accept a `Workload`, and change the main loop to iterate `workload.queries`. Make the parser group mutually exclusive and required:

  ```python
  source = parser.add_mutually_exclusive_group(required=True)
  source.add_argument("--source")
  source.add_argument("--workload", type=pathlib.Path)
  ```

  With `--source`, construct a copy of `CHEMOTION_WORKLOAD` using that URL. With `--workload`, use its URL, pins, and query list. Add `"workload": workload.name` to SOURCE metadata and every sample record. Preserve threshold modes, rotating order, executable hashes, exclusive output creation, before/after pin checks, physical transfer parsing, and fresh-process semantics.

- [ ] **Step 4: Add BOE and ChEBI Full workload files**

  Pin the observed R2 identities:

  ```text
  boe:        6,958,628 bytes, ETag "460709f1f8c26dd15a02e5df5dbfecfa"
  chebi-full: 164,832,053 bytes, ETag "2954435cf2b9677c9a38f84964b93668-3"
  ```

  Give BOE two deterministic queries (one bound-subject lookup and one aggregate). Give ChEBI Full one deterministic selective query, because its role is to verify the above-threshold path rather than reward a full scan. Every `SELECT ... LIMIT` must use a complete `ORDER BY` before `LIMIT` so eager and lazy modes cannot return different legal subsets. Validate the queries once with the current candidate binary before using them for timed samples.

- [ ] **Step 5: Run GREEN and smoke the CLI help**

  ```powershell
  docker compose run --rm dev uv run python scripts/test_bench_cold_r2.py -v
  docker compose run --rm dev uv run python scripts/bench_cold_r2.py --help
  docker compose run --rm dev uv run python -m json.tool scripts/cold-r2-workloads/boe.json
  docker compose run --rm dev uv run python -m json.tool scripts/cold-r2-workloads/chebi-full.json
  ```

- [ ] **Step 6: Review and commit Task 4**

  Verify workload parsing cannot silently ignore a misspelled pin/query field, legacy Chemotion invocation remains valid, and all sample records carry enough identity to prevent cross-dataset aggregation.

  ```powershell
  git add scripts/bench_cold_r2.py scripts/test_bench_cold_r2.py scripts/cold-r2-workloads/boe.json scripts/cold-r2-workloads/chebi-full.json
  git commit -m "bench: support pinned R2 workloads"
  ```

---

## Task 5: Benchmark the pinned R2 matrix and validate browser/shared paths

**Files:**

- Modify: `docs/BENCHMARK.md`
- Regenerate: `docs/BENCHMARK.html`
- Create benchmark artifacts under ignored `target/bench/` only; do not commit raw samples or binaries.

- [ ] **Step 1: Build the release candidate and record its identity**

  ```powershell
  docker compose run --rm dev cargo build --release -p rete-cli
  docker compose run --rm dev /target/release/rete --version
  docker compose run --rm dev sha256sum /target/release/rete
  git rev-parse HEAD
  ```

  Copy the executable to uniquely named ignored paths before comparing configurations so every sample group identifies immutable bytes. The same executable may be used for threshold 0 and 8; mode separation comes from `RETE_EAGER_MAX_MB`.

- [ ] **Step 2: Verify live R2 object identities before timing**

  Check `HEAD`, `Accept-Ranges`, `Content-Length`, and ETag for Chemotion, BOE, ChEBI Full, and all six Wikidata XXL shards. Required shard pins are:

  ```text
  shard_0000.rete 949270267 "c4b8ad492c00fc88f44cd4bcd505f25f-15"
  shard_0001.rete 554800567 "0674b3b74fd9a14a7754effbfc929c79-9"
  shard_0002.rete 708499856 "8b99a440d2f2e505b93b6953a2d98538-11"
  shard_0003.rete 849908311 "bbcc87cef8be7e23f0929df0c265b41f-13"
  shard_0004.rete 867722504 "27fad16c0ffd0b813f49d8d512f2cb8b-13"
  shard_0005.rete 942757112 "66fc645cb02c604146ec9138f1de1eb6-15"
  ```

  Total observed sharded size is 4,872,958,617 bytes. Stop rather than benchmark if any pin changed; update a workload only after inspecting the replacement dataset and revalidating deterministic result hashes.

- [ ] **Step 3: Re-run Chemotion, then sample BOE and ChEBI Full**

  Use 15 fresh-process samples per query/mode. First re-run the existing pinned Chemotion three-query comparison so the remediation is checked against its accepted 50.5-73.7% median wins. BOE compares thresholds 0 and 8, demonstrating the small-object one-GET path. ChEBI Full compares the same thresholds and must remain lazy in both modes because it is larger than 8 MiB.

  ```powershell
  docker compose run --rm dev uv run python scripts/bench_cold_r2.py --candidate /target/release/rete --thresholds 0,8 --samples 15 --source https://data.graphplaza.com/chemotion/chemotion.rete --out target/bench/chemotion-remediation-r2.jsonl
  docker compose run --rm dev uv run python scripts/bench_cold_r2.py --candidate /target/release/rete --thresholds 0,8 --samples 15 --workload scripts/cold-r2-workloads/boe.json --out target/bench/boe-r2.jsonl
  docker compose run --rm dev uv run python scripts/bench_cold_r2.py --candidate /target/release/rete --thresholds 0,8 --samples 15 --workload scripts/cold-r2-workloads/chebi-full.json --out target/bench/chebi-full-r2.jsonl
  ```

  Preserve output hashes and assert transfer counts are stable within every query/mode group. Report median and nearest-rank p90 wall time, bytes, GET count, and median/p90/max peak RSS. Compute the time win as `(lazy_median - adaptive_median) / lazy_median × 100%`; report negative values as regressions, not wins.

- [ ] **Step 4: Validate the browser/WASM six-shard catalog path**

  First build both browser targets and the playground:

  ```powershell
  docker compose run --rm wasm wasm-pack build crates/rete-wasm --target web --out-dir ../../web/pkg
  docker compose run --rm wasm wasm-pack build crates/rete-wasm --target no-modules --out-dir ../../web/pkg-nomodules
  docker compose run --rm dev uv run python scripts/build_playground.py
  ```

  Run the existing live catalog gate restricted to Wikidata XXL:

  ```powershell
  docker compose run --rm gate-catalog-live bash -lc "npm ci --no-audit --no-fund && node run.mjs --catalog=all --catalog-dataset=wikidata-xxl"
  ```

  Require all catalog queries to complete and the UI to report six federated sources. From the gate's query metadata/network evidence, record the transferred bytes and assert they are strictly below 4,872,958,617 bytes; any full, non-range GET of a shard is a failure. Confirm both an `ASK` query and a deterministic bounded `SELECT ... ORDER BY ... LIMIT` query succeed. This validates compatibility with the shared browser reader and shard fan-out; it does not change native `rete federate`.

  Then run the resident async-WASM harness twice against the pinned 223,233-byte Imaging Plaza object (ETag `"777c82386811e330ab755b7603d062d6"`):

  ```powershell
  docker compose run --rm -e RETE_URL=https://data.graphplaza.com/imaging-plaza/imaging-plaza.rete -e "RETE_Q=SELECT ?s ?p ?o WHERE { ?s ?p ?o } ORDER BY ?s ?p ?o LIMIT 10" gate bash -lc "cd /work && node tests/gate/asyncify_e2e.cjs"
  ```

  Require the first query to transfer a strict subset of 223,233 bytes and the second identical query to add zero requests and zero bytes. This proves the native <=8 MiB one-GET policy did not leak into the shared/resident browser reader.

- [ ] **Step 5: Document measured results and scope**

  Add a dated section to `docs/BENCHMARK.md` containing:

  - exact git revision and executable SHA-256;
  - object URL, length, and ETag for each timed dataset;
  - query text or workload file link;
  - per-mode median/p90, transfer bytes/GETs, peak RSS, and percentage time win;
  - browser six-shard result and transferred-byte evidence;
  - the Imaging Plaza resident-session first-read subset and zero-read second-query cache reuse;
  - the conclusion that the <=8 MiB one-GET path is native `sparql-url` only, while larger native files and browser/sharded graphs remain lazy;
  - the caveat that named-graph tile laziness removes open-time decompression but is not a global compressed-output limit.

  Regenerate HTML:

  ```powershell
  docker compose run --rm dev cargo run -q -p docgen
  ```

- [ ] **Step 6: Run the complete verification matrix**

  ```powershell
  docker compose run --rm dev cargo fmt --all -- --check
  docker compose run --rm dev cargo clippy --workspace --exclude rete-bench --all-targets -- -D warnings
  docker compose run --rm dev cargo test --workspace --exclude rete-bench
  docker compose run --rm dev cargo test -p rete-core --no-default-features
  docker compose run --rm dev cargo build -p rete-core --all-features
  docker compose run --rm dev cargo build -p rete-bench
  docker compose run --rm dev bash scripts/smoke.sh
  docker compose run --rm wasm wasm-pack build crates/rete-wasm --target web --out-dir ../../web/pkg
  docker compose run --rm wasm wasm-pack build crates/rete-wasm --target no-modules --out-dir ../../web/pkg-nomodules
  docker compose run --rm dev uv run python scripts/build_playground.py
  git status --short
  ```

- [ ] **Step 7: Final review and commit Task 5**

  Review the full branch diff against `docs/superpowers/specs/2026-08-10-r2-reader-hardening-design.md`. Confirm there are no new format bytes, public API changes, unbounded open-time named tile decompressions, unpinned benchmark claims, or generated-file drift.

  ```powershell
  git add docs/BENCHMARK.md docs/BENCHMARK.html
  git commit -m "bench: report adaptive reads across R2 datasets"
  ```

---

## Final Branch Gate

- [ ] Ask a fresh reviewer agent to compare the committed branch with the approved design and this plan.
- [ ] Resolve every correctness, safety, compatibility, or reproducibility issue; rerun the affected focused checks.
- [ ] Rerun the complete verification matrix after the last code change.
- [ ] Confirm `git status --short` is clean and report commits, benchmark deltas, browser/shared compatibility, and the explicit remaining zstd limitation to the user.
