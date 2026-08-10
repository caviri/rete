# Changelog

All notable user-facing changes are recorded here. Rete follows semantic
versioning for its Rust, CLI, and WASM APIs from 1.0.0 onward.

## [Unreleased]

### Added

- **Filtered dumps prune the file, not the rows (#117)** — `rete export
  --graph/--subject/--predicate/--object`, `dump({predicate})` in the JS client,
  `iter_quads(predicate=…)` in Python. A dump
  scoped to one graph, one predicate, one subject or one object now goes through
  the same routed access path a query does: `GraphIndex::scan_iter` picks the
  permutation with the longest bound prefix, binary-searches its tile directory
  down to the tiles that can match, and drops the rest by their recorded
  synopsis **without fetching them**. Previously the only way to get a slice out
  of a dump was to dump the graph and throw away what did not match, in the
  consumer.

  Measured on the published `cordis.rete` (801 MB, a 417 MB dictionary, 26.4M
  quads, six named graphs), opened lazily through a counting range reader, peak
  RSS from `/proc/<pid>/status` `VmHWM`:

  | dump | before | after |
  |---|--:|--:|
  | one predicate (337,811 of 9.97M rows) | 375.8 MB · 1797 req · 2105.8 MB · 12.8 s | **16.0 MB · 17 req · 182.5 MB · 0.42 s** |
  | a rarer predicate (40,761 rows) | 375.8 MB · 1797 req · 2105.7 MB · 12.8 s | **15.5 MB · 16 req · 155.3 MB · 0.22 s** |
  | one subject (15 rows) | 375.8 MB · 1797 req · 2105.8 MB · 12.8 s | **3.0 MB · 21 req · 114.3 MB · 0.02 s** |
  | one object (2 rows) | 375.8 MB · 1797 req · 2105.8 MB · 12.8 s | **2.4 MB · 15 req · 114.9 MB · 0.01 s** |

  **Where it does not help, stated plainly.** The index is pruned; the
  dictionary is not. A predicate whose objects are long abstracts went
  260.7 MB → 213.4 MB — 1.2x, not 23x — because resolving the rows it keeps
  still faults the chunks their literals live in. An **unfiltered** dump is
  unchanged by construction (452.5 MB, 2423 MB peak, both before and after):
  that is the floor, and the resident dictionary sets it, since a faulted
  chunk is a `OnceCell` nothing evicts. Two ceilings sit above all of it —
  that one, and the chunk-directory read a lazy open pays before any dump work
  (up to 32.7% of a literal-heavy file, #198).

  Every one of the eight bound/unbound shapes routes inside `PermSet::CORE`
  (`{SPO, POS, OSP}`), so a file built with `--permutations 3` prunes
  identically — same tiles, same rows, no fallback path. A filtered dump streams
  in the *routed* permutation's order, so it is the same set as the unfiltered
  dump filtered but not the same order.

- **`rete cost --dump` previews what a dump will fetch, before it starts
  (#117).** The dump twin of the query cost preview, in the same report shape.
  The index figure is **computed from the tile directories**, not sampled — it
  names the permutation, how many of the section's tiles the filter admits and
  their exact encoded bytes, without fetching a tile — and the dictionary
  section length is reported as an honest ceiling rather than guessed at. New
  `GraphIndex::scan_plan` / `Rete::dump_plan` expose the same plan to any
  consumer.

- **The three merge-join permutations are optional — `rete build
  --permutations 3`, and the file says which it has.** SOP, PSO and OPS exist
  only to hand a sort-merge join a co-sorted stream; SPO, POS and OSP are what
  decide *routing*, and they tie the longest bound prefix on **all eight**
  triple-pattern shapes (enumerated by `perm_routing_never_leaves_core`). So a
  three-permutation file answers every query with the same rows, from the same
  tiles — measured across two datasets' own catalog example queries, 16 query
  pairs, zero row-count disagreements — while dropping **36.8%** of
  `davidrumsey` (58.7 MB → 37.1 MB) and **50.5%** of `tree-city-inventory`
  (19.4 MB → 9.6 MB).

  **The default is still six.** This ships the mechanism and the measurement,
  not a change of policy.

  What it costs is the merge join, on exactly three of the twelve (bound-set,
  join-column) shapes: subject-bound sorted on object, predicate-bound sorted on
  subject, object-bound sorted on predicate. Over HTTP, cold, that is worth
  **+75.0%** on `tree-city-inventory`'s "commonest trees in Geneva", **+31.5%**
  on its species-diversity query and **+11.7%** on its tallest-trees query —
  while *fetching fewer bytes*, so the loss is join CPU, not I/O. The other
  thirteen query pairs are within ±10%, and `davidrumsey` — whose stars are wide
  rather than deep — shows no loss at all. Build time barely moves on the in-RAM
  path (the permutations are built in parallel), but the memory-bounded external
  builder sorts them one at a time and gets **13.5%** / **11.9%** faster; peak
  RSS falls 7.2% / 14.7%.

  `GraphIndex::best_permutation_in` and `permutation_sorted_on_in` consult the
  file's set instead of assuming six, and the latter now refuses a co-sorted
  stream that would cost *routing* — a no-op with six permutations (the co-sorted
  order always ties the best prefix there) and what stops a lean file from buying
  sort order with a whole-section scan.

  **A three-permutation file is not readable by an older Rete**, and that is the
  point: `decode_index_container` and the ranged
  `locate_container_section_ranged` both check the index container's section
  count, so an older reader fails with `malformed container: expected 6
  permutation sections` / `unexpected container section count` and exit 1 on
  every command that touches the index — never a short answer. (Issue #206
  predicted the opposite, "an old reader would return zero rows"; the receipt is
  in `docs/compatibility.md`.) `rete info`, `rete verify` and `rete card-url`
  still work, because they read only the header and the metadata section.

  The set is recorded in the header's previously-reserved byte 50 as a 6-bit
  mask, where **`0` means all six** — so a default build is byte-identical to
  every file written before the byte was defined, and no format-version bump is
  involved. It flows through the in-RAM builder, the two-pass streaming builder
  and the memory-bounded external builder (byte-identical outputs); `repyramid`
  preserves its input's set and `merge` writes the union of its inputs'.

  Surfaced as `signals.permutations` — `{count, names, merge_join}` — on the
  Dataset Card, plus `rete info` and `rete stats`. Following
  `signals.text_index`, it is **derived at read time and never stored**: which
  permutations a file carries is a fact about its own bytes, so a stored copy
  would be an authored claim about the file's own layout. It costs no range read
  at all — the mask is in the 1 KiB header every card read already fetches.

- **An unbounded scan streams instead of materializing — `SELECT ?s ?p ?o
  LIMIT 1` works on 9.8 billion quads.** The Java client's scans returned a
  `List`, so the engine built the *whole* result inside wasm32 linear memory
  before the first row crossed the boundary. That is not an edge case: RDF4J
  issues exactly `getStatements(null, null, null)` for `SELECT ?s ?p ?o … LIMIT
  1`, because the `LIMIT` is a `Slice` above the triple source and the Sail
  never sees it — verified against RDF4J 5.3.2, which takes one row and closes
  the iteration. So the most trivial exploratory query a user can type read the
  whole graph.

  `Rete.scanCursor(s, p, o)` / `scanCursorInGraph(g, s, p, o)` return a
  `QuadCursor` that pulls bounded batches, and the RDF4J Sail's
  `getStatements` is driven by one over any `Path`/`URI` source. Measured
  (`--memory=12g`, `-Xmx8g`, compiled engine, peak RSS from the kernel's
  `VmHWM`, one fresh JVM per figure) through a real `SailRepository`:

  | file | `SELECT ?s ?p ?o … LIMIT 1` — before | after |
  | --- | --- | --- |
  | `mirbase.rete` 39.2 MiB / 2.70 M quads | 14.9 s · 4151 MB | **2.0 s · 670 MB** |
  | `cordis.rete` 763.9 MiB / 26.4 M quads | wasm trap after 72 s · 2794 MB | **6.3 s · 825 MB** |
  | **`datacite.rete` 48.6 GiB / 9,834,714,813 quads, over HTTP** | impossible | **27.4 s · 1504 MB** |

  A full drain is unchanged in what it returns — all 2,701,457 quads of
  `mirbase.rete`, in 14.2 s inside a **1 GiB** heap. It does not make an
  *unqualified* drain of a multi-GB quads file possible: the engine's
  lazily-faulted dictionary chunks and index tiles stay resident for the life of
  the handle, so `cordis.rete` now reaches 17.0 M of its 26.4 M quads before
  wasm32's 4 GiB address space runs out (against **zero** rows before). That
  ceiling is the handle's, not the cursor's — it lands within 0.2% of the same
  row at batch 32, 2048 and 16384.
  - The engine side is `Rete::query_batch` in `rete-core`, backed by a new
    `GraphIndex::scan_batch` and `TripleBlock::scan_resume`. The state that
    survives between calls is one opaque `u64` — `(tile index, next a-group)` —
    not a suspended iterator, so nothing borrows the `Rete` across the
    boundary. Batches end on a group boundary and nothing is ever rescanned, so
    a drain stays O(n). `Rete::query_iter` is the in-process pull twin, and
    `dump_iter` is now a special case of it.
  - The batch ramps from 32 rows, doubling to `-Drete.scan.batch` (default
    2048). Small first because `LIMIT 1` pays for one batch (on `cordis.rete`:
    2.2 s at 32 rows, 6.4 s at 2048, 11.0 s at 8192); growing because full
    drain throughput is flat at and above 2048 and 6–15% worse below it.
  - A cursor is released by `close()`, by exhaustion, by an exception, by
    `Rete.close()` (which drops every cursor on the file), and — if it is
    abandoned mid-scan and collected — by a `Cleaner` that queues its id for
    the owning thread to reap. `Rete.openCursorCount()` is the leak check.
  - `sizeInternal(context)` counts through the cursor rather than
    materializing the graph.
  - New wasm exports: `rete_handle_scan_open` / `_next` / `_close` and
    `rete_open_cursors`. Purely additive; every existing entry point is
    unchanged, and the in-memory `byte[]` path still buffers (the image is
    already resident there, so a cursor would bound nothing).

- **The Java client opens a `.rete` from disk lazily — the size ceiling is
  gone.** `Rete.openFile(Path)`, `ReteEngine.openFile(Path)` and
  `new ReteSail(Path)` read a local file by *range*, exactly as the existing
  HTTP path does. The `byte[]` entry points copy the whole image into wasm32
  linear memory on every call, so they died at roughly 700 MB with
  `decompression failed: out of memory` — inside wasm, with the JVM heap
  untouched, which is why no amount of `-Xmx` ever helped. Measured
  (`--memory=12g`, `-Xmx8g`, compiled engine, peak RSS from the kernel's
  `VmHWM`, one fresh JVM per figure):

  | file | `byte[]` | `openFile` |
  | --- | --- | --- |
  | `mirbase.rete` 39.2 MiB, `info()` | 14.3 s · 738 MB · 100% read | **1.3 s · 557 MB · 6.4%** |
  | `davidrumsey.rete` 71.3 MiB, `info()` | 30.4 s · 1004 MB · 100% | **1.4 s · 556 MB · 3.9%** |
  | `cordis.rete` 763.9 MiB, `info()` | **OOM** after 79.6 s · 4212 MB | **1.7 s · 582 MB · 6.0%** |
  | `hugging-face-full.rete` 2.52 GiB, `info()` | **OOM** in 4.7 ms (`byte[]` limit) | **2.7 s · 1058 MB · 4.5%** |

  Through the RDF4J Sail, `datacite.rete` — **48.6 GiB, 9.83 billion quads**,
  over HTTP — answers a bounded query from a JVM in 24 s reading 154 MB
  (0.3‰ of the file). No new reader was written for any of this: the wasm
  module's one host import, `rete_host_read_range`, was always
  source-agnostic, so the local path is the remote path with a `FileChannel`
  under it instead of a socket. (Mirrors PR #200, which gave the browser the
  same thing with a `Blob` under it.)
  - Handle operations lost their HTTP-specific names: `info()`,
    `query(String)`, `graphs()`, `scanInGraph(g,s,p,o)`, `scanQuads(s,p,o)`,
    `bytesRead()`. The `…Remote` spellings and `bytesFetched()` remain as
    aliases, and every `byte[]` entry point is untouched — the change is
    purely additive.
  - `rete_remote_open` in the wasm ABI is now `rete_ranged_open`, with the old
    name kept as an alias.
- **`rete build` reads gzipped inputs, and the memory-bounded external build
  accepts Turtle and TriG.** The dumps that actually need
  `--memory-budget-mb` do not ship as plain N-Triples — they ship as
  `dump.ttl.gz` / `dump.trig.gz`. Converting one first is not a formality: at
  the 146.8 N-Quads bytes per triple measured on SemOpenAlex, its 8.5 GiB
  author dump expands to roughly 400 GB of scratch — an order of magnitude
  more disk than the source, spent to avoid spending RAM. Now every accepted
  syntax streams from the reader (`ingest::stream_reader` grew Turtle, TriG and
  RDF/XML arms over `oxttl`/`oxrdfxml`'s reader parsers), and compression is
  detected from the bytes — `1f 8b` — rather than the file name, so a renamed
  `.gz` and a piped stream behave the same. `MultiGzDecoder` reads concatenated
  members through to the end instead of stopping silently after the first.
  RDF/XML remains excluded from the external build.
  - **`--collapse-graphs`** folds every named graph into the default graph.
    Dumps that put all their data inside named graphs — TriG exports like
    SemOpenAlex, most Wikibase and GraphDB dumps — otherwise answer
    `?s ?p ?o` with nothing and build an empty pyramid, because in SPARQL the
    default graph is not the union of the named ones. It is also what makes
    such an input eligible for the default-graph-only external build.
  - `rete estimate` takes the same inputs (previously N-Triples/N-Quads only),
    so "will this fit?" can be asked about the file as downloaded. Its
    `--sample-mb` is now enforced by a counting reader **under** the gzip
    decoder, which both makes sampling work for syntaxes whose statements span
    lines and keeps the extrapolation in the same units as the file's size on
    disk.
  - Verified on the real thing: SemOpenAlex `sources`, read straight from its
    57.5 MB `.trig.gz` under a 256 MiB budget (27 chunks, 14,820,651 triples),
    is **byte-identical** to the in-RAM `--no-pyramid` build of the same graph
    — and finished in 36 s against 56 s.

### Changed

- **The dictionary's chunk directory stores a shortest separator, not the
  chunk's first term (#198).** Writer-side only, on by default, no flag, no
  generation bump. Each directory entry keeps its framing — `Δfirst_run`,
  `key_len`, `key`, `clen` — but `key` is now the shortest byte string `s` with
  `last_term(chunk i-1) < s <= first_term(chunk i)`, i.e. the chunk's first term
  truncated one byte past where it diverges from its predecessor's last term.
  Chunk 0's key is empty. That was already the whole contract:
  `ChunkedSection::id` routes with `partition_point(|c| c.key <= term)` and
  reads the key nowhere else, so **an unmodified older reader routes a
  separator-keyed file correctly** — the verbatim first term is the degenerate
  separator.

  Measured by rebuilding real datasets with **both** writers and parsing the
  directories back out of the two files:

  | dataset | chunks | chunk directory | after | | longest key |
  |---|--:|--:|--:|--:|--:|
  | `proteinbase` (10.8 MB) | 238 | 244,979 B | **26,432 B** | 9.3× | 6,793 → 942 B |
  | `tree-city-inventory` (19.4 MB) | 330 | 26,826 B | **10,426 B** | 2.6× | 80 → 67 B |
  | `dance` (325 KB) | 7 | 345 B | **51 B** | 6.8× | 52 → 7 B |

  Across 147 local generation-`0x05` files ≤400 MB — directories parsed and every
  chunk decompressed to find its real boundary terms — 28,393,024 B of directory
  becomes 4,436,838 B (6.40×). Read the **distribution**, not the aggregate:
  median 6.38×, p10 1.93×, min 1.30× (`tracking`), max 1,175× (`ramon_llull`),
  and two files are 53.5% of all the directory bytes. The saving is proportional
  to how much of a boundary term is *not* shared with its predecessor, so
  free-text and blob literals gain 15–1,000× (`geoadmin` 102.7×, `postscriptum`
  75.3×, `swissubase-demo` 38.4×, `davidrumsey` 261,271 → 48,009 B) while one IRI
  namespace gains almost nothing (`openalex-entities` 1.6×, `bph` 1.4×) and
  kilobyte-prefixed WKT is the weak case (`ohm-full` 4.0×, and still the corpus's
  largest directory afterwards at 3,053,714 B). Build cost is noise. The chunk
  bodies, the ids, the term order and every query answer are unchanged; the
  content hash is not, so a rebuilt file will not match a previously published
  hash.

  Existing published files keep their large directories until they are rebuilt —
  this buys nothing without a rebuild, and it does not oblige one.

  The one way this bites is a future reader treating the key as a term.
  `SectionChunk::first_term` is renamed to `key` and documents the invariant, and
  a test asserts the key is *not* the first term, because that failure is silent:
  `id → term`, `dump` and `export` route by `Δfirst_run` and stay byte-perfect
  while only `term → id` lies. Verified against binaries built from unmodified
  `origin/main`: 78 checks over two graphs × six build variants
  (in-RAM, `--text-index`, `--permutations 3`, `--memory-budget-mb`) × two reader
  paths × 2,854 boundary probes, 0 failures — and a negative control writing
  #198's rejected truncated key gives 398 wrong `term → id` answers at 8 bytes
  with a byte-identical `export`.

- **No backwards-compatibility promise before 1.0.0 — the one made earlier in
  this cycle is withdrawn.** Earlier in this same unreleased cycle the project
  promised that "generation-1 files stay readable, and no flag-day rebuild is
  reserved before 1.0.0" (#217, closing #214). **That promise is withdrawn.**
  rete is pre-1.0 and commits to no `.rete` backwards compatibility yet: the
  format may change, including in ways that require rebuilding a file you have
  already published. `README.md`, `docs/SPEC.md`, `docs/compatibility.md`,
  `docs/release.md`, `docs/index.md` and the `MIN_STABLE_READ_VERSION` doc
  comment now say that and nothing stronger. The durable compatibility promise
  starts at 1.0.0.

  What is *not* withdrawn is the fact underneath it, because it was never the
  promise: stable format generation 1 (`0x05`) froze on 2026-07-14, first shipped
  in **0.3.0** — there is no Rete 1.0.0 — and has not moved since. That is a
  track record, not a guarantee. Nor does it pin reader capability: #68 changed
  writer semantics *inside* `0x05` nine days after the freeze, which is why a
  stale reader can return silently incomplete rows (#124). Read published files
  with a current reader, and keep the RDF source you built from. (#214)

### Fixed

- **`CITATION.cff` recorded a version DOI under a comment promising a concept
  DOI.** The comment said the value "resolves to the latest archived release
  rather than pinning one"; the value, `10.5281/zenodo.21546288`, is v0.3.0's
  own DOI, so every citation copied out of this repository would have pinned
  v0.3.0 forever. Zenodo's API for that record reports `conceptrecid: 21546287`
  and `conceptdoi: 10.5281/zenodo.21546287`, and the concept DOI currently
  resolves to the 0.3.2 archive. Corrected in `CITATION.cff`, `codemeta.json`
  (which mirrors it) and the README's DOI badge. (#213)

- **Format generation 1 was never "introduced by Rete 1.0.0" — there is no
  1.0.0.** `SPEC.md` §4.1, its compatibility statement, `compatibility.md`,
  `README.md` and the `CURRENT_FORMAT_VERSION` doc comment all attributed
  `0x05` to a release that does not exist, and told readers that files
  "produced before Rete 1.0.0" (i.e. every published file) are experimental
  artifacts. `0x05` was frozen on 2026-07-14 and first released in **0.3.0**;
  the experimental generations `0x01`–`0x04` are what predate the freeze. The
  generation number counts *format* generations and is independent of the
  release version — it is the Rust/CLI/WASM APIs that are waiting on 1.0.0.
  `compatibility.md` now also states the corollary that cost #124 a silent
  wrong answer: `0x05` does not pin reader capability, because #68 changed
  writer semantics inside the generation nine days after it froze. (#206)

- **The six permutations are no longer described as fixed.** After
  `--permutations 3`, `architecture.md`, `getting-started.md`, `SPEC.md` §2/§9,
  the skills and the file-explorer pages said the format stores six, and
  `BENCHMARK.md` credited the extra three with something they never did: SPO,
  POS and OSP already resolve *any* triple pattern to a contiguous scan at the
  same longest bound prefix — SOP/PSO/OPS only supply a co-sorted stream for a
  sort-merge join, and routing has never selected one. `cli.md` gains the
  `repyramid` (preserves) and `merge` (union) rules, and a `rete merge` section
  it never had. `SPEC.md` and `dataset-cards.md` also carried a superseded
  `davidrumsey` measurement (36.2%, 4.57 M triples) against the repro'd
  benchmark table (36.8%, 5,001,983 triples).

- **Generated heading anchors keep their underscores, like GitHub's.** `docgen`
  slugs a heading into the id github.com would mint for it — that is the entire
  point of the convention, since an author writes and tests in-page links on
  GitHub. Its slugger dropped `_`, which GitHub keeps (`_` sits in the gap its
  punctuation class jumps over), so `6.3 Full-text index (TEXT_INDEX section,
  optional)` became `…-textindex-…` in the built page against `…-text_index-…`
  on GitHub. A spec full of `TEXT_INDEX`-shaped identifiers made that a live
  trap: the one link already written to the GitHub-correct anchor failed the
  docs link check and was *changed to match the slugger* instead. One anchor and
  one link change; `docs/dataset-cards.md → SPEC §6.3` now resolves in both
  places.

- **The embedded Dataset Card reports the size of the file, not the size of the
  input.** The card was derived from the statements *ingested*, while the header
  records what the index kept after deduplication — and every permutation index
  sorts and dedups. For duplicate-free input the two numbers are equal, which is
  why the gap went unnoticed; for anything paged with overlapping windows (most
  SPARQL harvests) the published card over-stated the graph. `switzerland-fedlex`
  advertised 66,392,663 quads for a 56,321,446-quad file. `rete card` and
  `rete info` now cannot disagree, and neither can the `wrote …` build summary.
  A card that carries counts is now derived in two stages — everything that
  needs the source quads while they are resident, then the counts stamped once
  the indexes exist (`rete_core::ingest::DeferredMetadata` / `FinalCounts`), so
  no build path pays extra memory for the correction. Fixes #128.
  - The distributions (predicate/class histograms, hub degrees) are still
    tallied over the ingested multiset — they describe shape, not size.

- **A build no longer ships a starter query it just measured at zero rows.**
  A carded build already runs every generated starter query against the
  finished file to record its cost — so for those files emptiness is
  *measured*, not inferred, and the measurement is ground truth. The build now
  acts on it: a query that comes back with no rows (or a false `ASK`, or
  nothing constructed) is **removed from the card before the file is written**,
  with the id and the reason printed and kept in the build record's new
  `dropped_queries`. It also catches the shape no row count can — an
  un-grouped aggregate returning its guaranteed one row while binding *no
  variable at all*, which is what `sp-bbox` does on a file where `wgs:lat` and
  `wgs:long` never sit on the same subject, and which that template's own note
  said the card could not do better than ship.
  - **Dropped, not fatal.** Refusing to build is right for *authored* content
    (an oversized `extra` bag is the publisher's text); a generated starter
    query has no author, and failing would make a file unbuildable for a reason
    its publisher cannot fix, at the end of a build that may have taken hours,
    over a metadata nicety. The generator already drops rather than fails when
    its static hook fires; measurement is a better oracle for the same question.
  - **The static machinery from the previous entry stays and now gets
    cross-checked.** It still acts at generation time and still carries every
    card built without a build record. Where the two disagree — a template
    declaring its query cannot be empty, and then it is — the build names it a
    generator defect and flags `contradicts_claim` in the record, because a
    static rule that says "fine" about a query measured at zero is a bug in the
    rule. Templates that admitted they could not decide
    (`top-dangling`, `sp-within`) set no flag.
  - **Free, and no churn.** The run already happened; a healthy build writes
    byte-identical bytes to one that never measured. Dropping does change the
    content hash (the card is inside it, correctly) — so `--no-card-costs`,
    which skips the run and therefore the check, is no longer hash-neutral on a
    dataset that *has* a useless starter query. The build says so on stderr.
  - A vacuous `COUNT` (`cmp-coverage`'s `total = 76990, have = 0`) binds, so no
    rows-based gate can see it; that class is closed by derivation instead —
    see the `{{LABELED_CLASS}}` change below.

- **A generated starter query can no longer be guaranteed-empty by
  construction.** The library instantiated each `{{PLACEHOLDER}}` from its own
  ranking and then conjoined the results, so two substitutions that were each
  certainly *present* could describe parts of the graph that never meet. Three
  templates shipped queries that could not match a statement on **published,
  plain default-graph** files:
  - `lb-labels` joined the most populous class to the most-used label
    predicate. `mtg`'s top class is `mtg:Ruling`, which carries no
    `schema:name`; `hugging-face`'s is `hf:Model`, while `rdfs:label` sits only
    on the embedded ontology terms. Both returned **0 rows**. The class is now
    `LABELED_CLASS` — the most populous class a `class_links` row *proves*
    carries the predicate — with a class-free fallback when the card can prove
    none. Where the top class is labelled (the common case) the emitted SPARQL
    is byte-identical to before.
  - `top-reach` walked the most frequent predicate from the busiest subject,
    two things nothing ties together: **0 rows** on `hugging-face`. It now
    seeds the path from a subject *of* the relation, and picks a relation whose
    objects are proven not to be literals (a `+` over a labelling predicate
    could never walk past one hop anyway).
  - `sp-within` hard-coded `geo:hasGeometry/geo:asWKT` while its gate accepted
    any one of three geometry signals. `geoadmin` hangs `geo:asWKT` straight
    off each District and has no `geo:hasGeometry` at all: **0 rows** on 52,959
    geometries. The path is now read from the data's actual shape.
  - Also: `cmp-coverage` measured labelling completeness of the wrong class
    (`76990 / 0` on mtg, now `34633 / 34633`); `lk-sameas` listed three of the
    four alignment predicates its gate accepts; `lk-external` and `top-in-hubs`
    are now gated on a witness that the filter/grouping keeps something.
  - The rule is enforced rather than remembered: capabilities that meet in a
    body must be **jointly derived**, every template declares *why* it cannot
    return zero rows, and the three that genuinely cannot know say so with a
    reason. Re-carding `lombardi`, `mtg` and `geoadmin` with the fixed
    generator gives 23/23, 23/23 and 22/22 starter queries returning rows.

### Added

- **`rete card-audit --measure` runs the starter queries instead of reasoning
  about them, and can write what they cost back into the file.**
  - The static audit has a ceiling it cannot raise: nothing in a card ties a
    subject to a predicate, and nothing records which objects are also
    subjects, so `top-reach` and `top-dangling` are undecidable **by
    construction** — 79 of 96 audited files were left undecided on `top-reach`
    alone. `--measure` opens the file cold, runs each shipped query, and
    reports rows, bytes and range requests beside the card's verdict. The two
    are never merged: one is what a card can prove, the other is what the file
    did.
  - It is the **same measurement a build records** (`measure_query`, shared
    with `rete build`), not a second copy of it. That is what makes the figures
    comparable: where a file already carries a build record, the command checks
    itself against it and prints `= build record` / `!= build record`. On
    `switzerland-fedlex.rete`, the one published file that has a record, all
    ten queries reproduce it byte-for-byte and request-for-request.
  - **Local or remote, and the output says which.** `bytes`/`requests` are the
    same quantity through a file handle and through HTTP — no block cache is in
    the stack, so the range sequence follows layout and query, not transport —
    but only the remote run pays for them. The transport is printed above the
    table and stored in `measurement.transport`.
  - It is a download, so it has a leash: `--only <ids>` measures a subset and
    `--max-mb N` abandons a query that asks for more, reporting the
    abandonment with the bytes it spent.
  - **`--write-costs`** records the run in the build-info section, so the next
    reader gets the figures from the CARD tier (two range requests) instead of
    re-measuring. The section is outside the content hash, so the file keeps
    its identity — same checksum, `rete verify` still passes, N-Quads
    byte-identical — but it sits right after the card, so the file is rewritten
    end to end to make room. Proved on published files: `tree-city-inventory`
    (25 MB) in 11 s including the measurement, +2,007 bytes, same `079f5d5f…`
    checksum, 569,694,820 bytes of sorted N-Quads identical; `switzerland-fedlex`
    (1.04 GB) in 381 s, +32 bytes, same `b2ddf84b…` checksum. The rewrite is a
    bounded-buffer copy — the RAM goes into *running* the queries (eager
    evaluation: `ng-list`'s 497,905 rows alone peak at 3.2 GiB, the whole fedlex
    run at 14.2 GiB), which is still under a `repyramid` re-card's ≈36 GiB
    prediction for that file and needs no staged N-Quads at all. It refuses when
    a query measured zero rows (that file needs a re-card, which rewrites it
    anyway and fixes the queries too), when a run did not finish, and when
    `--only` measured a subset.
  - `rete_core::plan_build_info` exposes the splice arithmetic the in-memory
    `attach_build_info` already used, so the streaming rewriter derives its
    header from the same rule rather than a second copy of it.
- **A card's `description` is Markdown, and can be as long as a README section.**
  - The 🏷 Card viewer renders **headings, bulleted and numbered lists (nested by
    indentation), block quotes, horizontal rules, fenced code and links** — the
    same small renderer that already draws `text/markdown` result cells, so
    there is one grammar and one escaping path, not two. Headings are shifted
    **under** the modal's own heading, so a published file can never inject an
    `<h1>` into someone else's page outline.
  - **Raw HTML is not a description format, deliberately.** A card is
    third-party data — it arrives inside a file someone else published — so a
    `<script>` (or an `onerror=` on an `<img>`) in a description would be
    remote code execution in every reader's browser, on every open. HTML is
    escaped and shown as text; `javascript:` links degrade to text. Markdown
    buys the formatting with none of that.
  - Every surface that has room for **one line** — the dataset sidebar, the
    picker blurb, the header tagline, the plaza tile and hero, the social/OG
    text — shows the same description with its block markers removed rather
    than leaking `## ` into a paragraph. Only the card viewer renders blocks.
  - **Writing one**: `--card-file` now accepts `"description"` as an **array of
    lines**, joined with newlines, so a Markdown description does not have to be
    hand-escaped into a JSON string. It is input sugar only — the card stores
    one string either way, so `rete card --json` still feeds straight back into
    `--card-file`. `--description "$(cat description.md)"` and the Build panel's
    textarea work too.
  - `description` is now **bounded at 8 KiB**, the same budget as the `extra`
    bag and for the same reason: both ride in the metadata section every
    CARD-tier reader fetches on every open. Over the cap the build fails loudly
    rather than truncating. Readers never validate, so existing files are
    unaffected. `rete card` indents a multi-line description to its value column.
- **The Dataset Card is now interoperable and auditable.** (#153)
  - A new **build-info section** (kind `7`, laid out right after the card so
    both arrive in the CARD tier's one header + one range read) records what no
    card carried before: the build timestamp (`SOURCE_DATE_EPOCH` honored), the
    `rete` that wrote the file, the flags in force (`--no-pyramid`, codec,
    `--memory-budget-mb`, `--materialize`/`--reason`, the card's top-N cap), and
    **measured starter-query costs** — bytes + range requests (portable
    properties of layout + query) paired with a wall-clock `debug_ms` stored
    with its context (engine, transport, one machine) as a reference, not a
    guarantee. The section is **deliberately outside the content hash**: two
    builds of identical data still hash identically, and stripping the section
    yields byte-identical images. Cardless builds are unchanged, byte-for-byte.
    `--no-card-costs` skips the measurements.
  - **Curated identity and provenance fields** on the card (via `--card-file`):
    `version`, `creators` (ORCID IRIs), `publisher` (ROR IRI), `canonical_url`,
    `sparql_endpoint`, `source_date`, `derived_from`, `doi`, `cite_as` — all
    deterministic, all inside the hash, all joinable against the published
    ORCID/ROR graphs.
  - **`rete card --format jsonld`** (and `card-url`) projects the card to
    JSON-LD — VoID for the graph (`void:triples`, partitions, `void:vocabulary`,
    `void:sparqlEndpoint`, `void:dataDump`), schema.org for the descriptive
    header, PROV-O for origin (`prov:wasDerivedFrom`, `prov:wasGeneratedBy` with
    the build activity), and a small `rete:` namespace for what no standard
    covers. The card stays plain JSON at rest; the projection is derived on
    demand, so nothing can drift. **`--format croissant`** emits the
    honestly-mappable Croissant subset — descriptive header, licence, creators,
    the `.rete` as a `cr:FileObject` distribution — with **no `recordSet`**
    (Croissant models tables; an RDF graph has no records) and no fabricated
    `sha256`.
  - A **one-row starter query** (`ov-one-row`) in every generated library: the
    unambiguous "did this file open and answer?" smoke test, graph-scope aware,
    where a `COUNT` honestly answers `0` on a named-graph-only file and reads
    as failure.
  - The card records `top_n`, the cap its profile lists were derived under —
    the number `truncated: true` was hinting at without stating.

### Fixed

- **16 playground dataset descriptions showed their own markup as text.** They
  were written as raw HTML back when the description renderer honoured it;
  since the renderer became escaping-only, `hugging-face` opened with a literal
  `<a href="https://huggingface.co" target="_blank"…` in the picker, the sidebar
  and the header tagline. All sixteen are now Markdown — `<b>`→`**`, `<i>`→`*`,
  `<code>`→backticks, `<a href>`→`[text](url)`, `&amp;`/`&lt;`/`&nbsp;`→the
  characters themselves — so the 137 bold spans, 17 italics, 159 code spans and
  13 links are real elements again. Wording is untouched and the conversion is
  inline-only: every one still renders as exactly one paragraph, so no picker
  row or sidebar `<p>` changes shape.
- **The social-preview reducer no longer eats angle-bracket prose.** `plain()`
  still stripped HTML from a description, a leftover from when the catalog held
  it, and its tag pattern also matched every angle-bracket phrase the authors
  actually write: `<< ?a rdf:predicate ?b >>` (RDF-star), `rete <command>
  --help`, `gbif.org/occurrence/<id>`, `deps.dev/<system>/<name>/<version>`,
  `?node <- edge -> ?node`. Nineteen `og:description` / JSON-LD abstracts were
  silently truncated to nonsense by it. A description is Markdown now, so the
  HTML branch is gone and the brackets survive.

## [0.3.2] - 2026-08-01

No engine change from 0.3.1 — the same code, released again because the 0.3.1
release run could not finish. Two workflow bugs stopped it after the packages
were already on PyPI and npm, so those registries carry 0.3.1 while GitHub
Releases has nothing.

### Fixed

- **The Blender add-on packed and then could not be moved.** The extension built
  correctly — 13,956,498 bytes with all four wheels bundled — and the job died on
  the next line, because `clients/blender/dist` is created by a container running
  as root and the runner user cannot unlink from it. (#129)
- **The browser WASM bundle copied paths that had moved.** It still named
  `docs/rete_wasm.js` and `docs/rete_wasm_bg.wasm` at the top level; the ESM pair
  lives in `docs/engine/`. Those are the same two paths #100 corrected in
  `ci.yml`'s parity list — `release.yml` kept the old spelling, and since it only
  runs on a `v*` tag, nothing exercised it in between. (#130)

Both are tag-only code paths, which is what let them rot unnoticed. `release.yml`
now has every path it copies audited.

## [0.3.1] - 2026-07-31

A correctness release. The headline is a SPARQL bug that returned **wrong
answers rather than an error**, so upgrading is not optional for anyone running
sub-queries. It also carries the browser fix for graphs large enough to push the
wasm heap past 2 GiB, and two additions that were already on main.

### Fixed

- **A sub-SELECT's `LIMIT` / `OFFSET` / `DISTINCT` no longer leaks to the outer
  query.** The planner peeled solution modifiers while walking down to the
  projection and did not stop at the sub-query boundary, so
  `SELECT … WHERE { { SELECT … LIMIT 10 } … }` applied that `LIMIT 10` to the
  **outer** result set. The query still succeeded — it just answered a different
  question than the one asked, which is the worst failure mode a query engine
  has. The peel now stops at a `Slice` whenever it is inside a `WHERE` or the
  projection is already bound, and lowers it as a nested plan instead. (#120)
- **A wasm pointer above 2 GiB no longer bricks the async reader.** wasm32
  pointers cross into JS through `i32` imports, so anything allocated past 2 GiB
  arrives sign-extended — a negative number — and `mem.set(bytes, negative)`
  throws `RangeError: offset is out of bounds`. Because wasm memory never
  shrinks, every later read in that worker failed identically: one query ended
  the page session. A remote scan of wikidata-1GB reached a 2050 MB heap and
  produced `dstPtr = -2145787624`. Every pointer the glue dereferences now goes
  through a `>>> 0` normalizer, and a G0 gate check asserts that in both the
  generator and the generated file so regenerating cannot silently drop it.
  (#121)
- **The Claude Code plugin exposed no skills.** Its marketplace source resolved
  only for one of the ways a plugin can be added, so `skills/` was never
  discovered from the plugin root; the manifest is versioned now too. (#119)

### Added

- **`rete estimate`** — project a build's output size, wall time and temporary
  spill *before* committing to it. Cardinality comes from a HyperLogLog sketch
  (2^14 registers, 16 KiB) over a line-aligned sample, so the estimate costs a
  read of the head of the input rather than a full pass. Reported as bands, not
  false precision. (#114)
- **The Python client streams every quad out in bounded memory.**
  `dump_iter()` / `dump_each()` walk a graph without materializing it, so a
  multi-gigabyte `.rete` can be piped into another store — Oxigraph, a triple
  store load, an N-Quads file — from a small resident footprint, and with no
  `unsafe` in the binding. (#118)

### Fixed (build & tooling)

- **The CI wasm-parity gate guards real files again.** Its diff listed
  gitignored directories and two paths that never existed (silent no-ops, which
  is how five different engine builds came to coexist across the shipped
  pages); `docs/engine/` gained a producer (`build_wasm.sh`) and the parity
  list now names exactly the tracked artifacts it regenerates. Workspace and
  python-client fmt/clippy debt cleared; the python lint job's toolchain is
  pinned to the repo's. (#100)

### Changed

- **Every client pin sits on the 0.3.0 engine line and is enforced.**
  `sync_versions.py --check` now also guards the Blender wheel pin and
  test-image floor and the HF Space wheel floor (eight lockstep targets);
  the docs' jsDelivr snippets load `rete-graph@0.3.0`. (#99)
- **Every browser surface rebuilt on the 0.3.0 engine** — playground,
  explore-100mb, the Asyncify pair, explorer, yasgui, lombardi, atlas, the
  plaza engine pair and `docs/engine` — from CI's canonical wasm bytes; the
  deployed playground stamps the exact commit it was built from. (#100)

### Added

- **Shared links now preview.** The playground keeps its state in the URL
  fragment, which no unfurler or search crawler can see, so every deep link used
  to preview as the same anonymous card. Each catalog example now has a page of
  its own at `q/<dataset>-<n>.html` (each dataset at `d/<dataset>.html`) carrying
  Open Graph / Twitter tags and a pre-rendered 1200×630 card — the question, the
  dataset, and **the answer that query really returns** — which then forwards to
  the playground deep link it describes. 🔗 and **Share** hand out those URLs;
  ad-hoc queries still share the deep link. Browse them at `shared.html`.
  The card numbers are measured, not written: `scripts/preview/capture.mjs` runs
  all 637 examples over the 91 published graphs in a real browser and records
  each result, its timing and its range-read cost.
- **Every documentation and application page carries social tags.** `docgen`
  derives each page's description from its own opening paragraph and emits the
  tags plus a rendered card; `scripts/preview/inject_og.mjs` does the same for
  the pre-built apps (playground, yasgui, atlas, the 3D viewers …) and patches
  their `web/*.template.html`, so a rebuild keeps them. Dataset pages also carry
  schema.org `Dataset` JSON-LD. A new G0 gate check fails if any share page,
  card image or tag goes missing.

## [0.3.0] - 2026-07-22

The 0.3.0 engine line: `rete-core`, `rete-cli`, and `rete-wasm` staged for
crates.io (the registry bootstrap is still pending — nothing is on crates.io
yet; `rete-graph` 0.3.0 *is* published to PyPI and npm). It carries the same
code the 1.0 line will ship, but goes out as a 0.x on purpose: it proves the
packaging, the docs builds, and the release automation end to end before any
version has to honour a compatibility promise. The on-disk format is already
stable generation 1; the Rust, CLI, and WASM APIs carry no semver guarantee
until 1.0.0.

### Added

- **Client versions now track the engine.** `rete-graph` on PyPI and npm and the
  R package all carry the engine's `MAJOR.MINOR` (0.3.x), so "same minor" means
  "same engine"; each client keeps its patch component for binding-only fixes.
  `scripts/sync_versions.py` propagates the workspace version and gates drift in
  CI. Every client also exposes the engine build it embeds — `rete_graph
  .__engine_version__` in Python, backed by the new `rete_core::VERSION`.
- **Claude Desktop extension** (`clients/mcpb`): the engine packaged as an
  [MCP Bundle](https://github.com/modelcontextprotocol/mcpb) — nine tools over
  local and published `.rete` graphs, plus offline `build_rete`. Ships as a
  plain `node` bundle (one JS file + the wasm engine), so a single artifact
  covers macOS, Windows and Linux.
- **Lazy `file://` opens in the JavaScript client**: a local `.rete` is read by
  byte range like a remote one, so a multi-gigabyte file is queryable without
  loading it into memory.
- WASM `card` / `card_url` (the index-free Dataset Card tier, two small range
  reads at any file size) and `RemoteGraph::{card, schema, info, graph_names,
  shacl}` — the resident remote handle now covers the full read surface.
- `header_ranges` additionally reports the metadata section's byte range.
- JavaScript client `card()`, `examples()`, `shacl()`, and a `wasm` escape
  hatch to the raw engine exports (client 0.3.0).

- Stable format version 1 with compatibility fixtures and defensive ranged-file readers.
- Publishable `rete-core`, `rete-cli`, and `rete-wasm` crates with Rust 1.87 MSRV.
- Native CLI builds for Linux, macOS, and Windows on x86-64 and ARM64.
- Browser WASM APIs for eager bytes, synchronous range reads, and asynchronous range reads.
- RDF/XML ingestion, named-graph N-Quads, SPARQL, SHACL, reasoning, federation, GeoSPARQL, and Dataset Cards.
- Reproducible playground generation, R2 catalog validation, coverage floors, fuzz targets, and release-gate browser tests.

### Fixed

- **Aggregation streams.** `GROUP BY` / `COUNT` / `SUM` / … fold solutions
  through per-group accumulators instead of buffering every row, so resident
  memory is **O(groups), not O(rows)** — a bare `COUNT(*)` is a single counter,
  and a `GROUP BY` over the 1.38 B-row type slice of the 9.83 B-triple DataCite
  graph completes inside a 4 GiB container (measurements on the benchmark
  page). (#96)
- **`rete info` / `rete card` no longer read the whole file.** Both use the
  CARD tier — the same two small range reads `card-url` performs over HTTP —
  so a 52 GB graph answers in ~1 s under a 1 GiB cap. A single-graph
  `FROM <g>` now borrows that graph's index instead of copying every triple
  into a fresh one. (#97)
- **The PyPI publish job could ship the R2-only Pyodide-legacy wheel**, whose
  platform tag PyPI rejects — the artifact now lives outside the publish job's
  `wheel-*` download glob. (#98)

### Compatibility

- Pre-1.0 `.rete` files are not guaranteed to open. Rebuild source RDF with the matching CLI.
- Files produced by 0.x may still require rebuilding before final 1.0.0. The compatibility promise begins with that release.

### Known limitations

- Browser bindings are single-threaded by default; threaded WASM remains opt-in and requires cross-origin isolation.
- SPARQL results are evaluated eagerly after lazy range reads.
- File federation unions per-file results; it does not perform arbitrary cross-file joins.
- The upstream RDF/XML dependency still resolves `quick-xml 0.37.5`, because every published `oxrdfxml` requires `quick-xml ^0.37`. RUSTSEC-2026-0194 and RUSTSEC-2026-0195 (both availability-only DoS on untrusted RDF/XML input) are therefore carried as documented exceptions in `deny.toml` and the publish preflight, and will be dropped as soon as Oxigraph ships a `quick-xml >= 0.41` bump.

[0.3.0]: https://github.com/caviri/rete/releases/tag/v0.3.0

## Pre-1.0 development history

The crate version and the experimental on-disk format version evolved
independently before 1.0. Each format step was a clean break, so those files
must be rebuilt with the 1.0 toolchain.

### Format & storage

- **Format `v0.4`: six permutation indexes** (SPO/POS/OSP + SOP/PSO/OPS, #57) —
  every triple-pattern shape gets a prefix-routed, co-sorted permutation, the
  precondition for sort-merge joins. Roughly doubles the index payload vs the
  three-permutation `v0.3`; a clean break (rebuild older files).
- Format `v0.3`: the 128-byte header became a **1 KB typed section directory**
  (up to 40 sections; new sections are just new directory entries).
- Opt-in **full-text index** (`TEXT_INDEX` section, kind 6, #55): word → sorted
  subject ids, range-readable per word; `rete build --text-index` +
  `rete search --contains <word…>` (~39× a `FILTER(CONTAINS)` literal scan).
- `rete repyramid` — rebuild a file's pyramid / schema pyramid / card / text
  index in place, straight from the existing `.rete` (no export/build round-trip).

### Query & serve

- **SPARQL 1.1 `SERVICE` federation**: a `SERVICE <endpoint> { … }` block is
  shipped to the remote endpoint and joined on shared variables (SILENT
  honored; transport injected by the host — ureq in the CLI, sync XHR in wasm).
- **`rete serve`** — a live SPARQL 1.1 Protocol endpoint (query **and Update**)
  over one `.rete`: the base file is never mutated, updates append to an
  N-Quads journal, `GET /snapshot.rete` publishes the merged state.
- Nested `SELECT` subqueries; correlated property-path evaluation from a bound
  endpoint; SPARQL 1.1 conformance at 232/309 (75.1%) of the W3C
  query-evaluation suite.
- **GeoSPARQL** filter functions (contains/within/intersects/disjoint/equals +
  distance/envelope) over `geo:wktLiteral` geometry.
- `rete shacl-url` — lazy remote SHACL: validation routed as range reads, only
  each shape's targets fetched (#58).
- Engine rework: lazy pull pipeline over integer slot rows, adaptive
  index-nested-loop joins, top-k ORDER BY — wins or ties Oxigraph on 20/24
  benchmark operators.

### Ecosystem

- Datasets are served **directly from Cloudflare R2 / any range+CORS host**
  (Zenodo DOIs included — the length probe tries `HEAD` first); the docs grew
  a [hosting guide](docs/hosting.md).
- The playground grew to **40+ real datasets** with cross-source joins,
  sharded-dataset fan-out, SQL companions (DuckDB/SQLite/Parquet), semantic
  (RAG) search, a local SPARQL-drafting AI, media-aware result cells, and a
  live-endpoint editing mode over `rete serve` — see the
  [playground guide](docs/playground-guide.md).

## 0.1.0

First tagged minor release. Highlights of the capabilities and the most recent
work; PR numbers reference [github.com/caviri/rete](https://github.com/caviri/rete).

### Format & storage

- Single-file, immutable `.rete` image — dictionary, SPO/POS/OSP permutation
  indexes, and a pyramidal community summary — queryable in place over HTTP
  `Range` requests, no server.
- Tiled permutation sections (format `v0.2`): independently-compressed ~64 KiB
  tiles with a byte-range directory and per-tile zone maps, plus **tile
  synopses** — per-tile min/max of the non-leading columns in a backward-
  compatible trailer (header flag `FLAG_TILE_SYNOPSIS`) so a range reader prunes a
  routed tile by a bound secondary component before fetching it (#51).
- Chunked, front-coded dictionary sections for ranged `term ↔ id` resolution.
- Append-only pyramid-meta blocks: the schema pyramid (semantic zoom), planner
  `query_stats`, characteristic sets / entity shapes, and a bounded label index.

### Query

- SPARQL SELECT / ASK / CONSTRUCT / DESCRIBE with BGPs, OPTIONAL, UNION, MINUS,
  FILTER, BIND, property paths, aggregation, and solution modifiers.
- Cost-based BGP join ordering from the pyramid summary and measured
  per-predicate selectivity (`query_stats`); hash + index-nested-loop joins.
- `rete search` — case-insensitive label **prefix search** from a bounded label
  index, ~22× a `FILTER(STRSTARTS(LCASE(…)))` literal scan (#48).
- Progressive / summary-only answers and a range-read cost preview.

### Reasoning, validation, federation

- Prototype OWL RL / RDFS reasoner (coherence checking + optional materialization).
- SHACL Core validation.
- Federated SPARQL across several `.rete` sources (union + dedup, predicate routing).

### Performance

- Build peak RAM cut ~39% on a 3 M-triple build (stream-parse + drop the raw
  string statements before the pyramid) (#49).
- Louvain community-pyramid build ~2.7× faster (dense-scratch local moving,
  byte-identical output) (#50).
- WASM query-result serialization ~13× less peak heap and ~10× faster
  (direct-to-string envelope instead of a `serde_json::Value` tree) (#52).
- Parallel index/dictionary builds and batch reachability (rayon).

### Tooling

- `rete` CLI (build, inspect, query, reason, shacl, federate, search, …).
- WASM browser client + the static playground.
- Playground **Find a term** picker: browse a graph's classes/predicates (from
  the resident schema card) and search entities by label (lazy over HTTP range on
  remote graphs), with a **values ›** faceted drill that lists a predicate's
  distinct objects — IRIs resolved to labels, cached after the first read.
- Playground **Settings** now shows a **per-file breakdown** of the opt-in
  persistent (IndexedDB) range cache — each cached `.rete` with the share of the
  file held and a fill bar, plus per-file Clear; "Clear all" now also wipes ranges.
- Profilers: `rete-bench --build-mem` (build memory) and `--query-mem`
  (query/serialization memory).
