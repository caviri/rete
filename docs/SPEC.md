# Rete — a cloud-native, range-queryable RDF graph file

**Status:** Stable format generation **1**, implemented (header byte `0x05`, frozen 2026-07-14; no compatibility promise before 1.0.0) · **File extension:** `.rete` · **Header magic:** `RETE`

> One file. Put it on S3, GitHub, or any HTTP server that honors `Range`.
> Give a client the URL. Run SPARQL. No database server.
>
> Like **Parquet** for tables and **PMTiles** for maps — but for **RDF graphs**,
> with a **pyramid** of progressively-refined detail.

---

## 1. Goals & non-goals

### Goals
- **Single immutable file**, queryable in place over HTTP `Range` requests.
- **Bounded request count** to answer a query — the client reads a small header
  (which carries every section's byte range), then fetches only the sections a
  query actually touches (≤4 ranges for a full open; 3 for the overview path).
- **SPARQL** evaluation against the file (BGPs, joins, filters, paths, aggregates,
  named graphs, and the supported query forms in §8).
- **Progressive / pyramidal** access: a coarse *summary graph* loads first, and
  the client refines by "zooming into" regions of interest.
- **WASM-friendly**: the same query core runs in a browser and in a CLI.
- **RDF-faithful**: IRIs, blank nodes, literals (with datatype + language tag),
  and named graphs (quads).

### Non-goals
- **Mutation.** The file is build-once / read-many; this buys aggressive
  compression and CDN caching. Updates happen *around* the file, not in it:
  `rete serve` accepts SPARQL Update into an append-only journal beside the
  base and publishes the merged state as a fresh `.rete` snapshot (see
  [cli](cli.md)); an in-file overlay section remains future work.
- **Full SPARQL 1.1** on day one. We stage it (see §8).
- **Inference / reasoning** at query time. Materialize before build if wanted.

---

## 2. Prior art we build on

| Source | What we take |
|---|---|
| **HDT** (Header-Dictionary-Triples) | Dictionary + bitmap-encoded triples; the closest existing queryable single-file RDF format. We extend it toward range/progressive access. |
| **PMTiles** | Header → root directory → leaf directories → data, all byte-range addressable, with a bounded number of requests. This is the spine of our pyramid + index. |
| **Parquet** | Footer metadata, blocks ("row groups"), and per-block min/max "zone maps" enabling pushdown / block-skipping. |
| **FlatGeobuf** | Packed static index (R-tree) co-located in the file for streamable spatial queries; our analog indexes *graph locality*. |
| **Oxigraph** (`oxrdf`, `spargebra`, `oxttl`) | Reuse RDF model, SPARQL parser, and algebra rather than writing our own. |

---

## 3. Conceptual model

An RDF dataset = a set of **quads** `(subject, predicate, object, graph)`.

Three transformations make it range-queryable:

1. **Dictionary encoding** — every IRI / literal / blank node ⇒ a dense integer
   ID. Triples become integer triples; the dictionary is stored once, compressed.
2. **Permutation indexes** — store the integer triples sorted in six orders by
   default (SPO, POS, OSP, SOP, PSO, OPS) so *any* triple pattern resolves to a
   contiguous scan with its bound components leading. Three of them (SPO, POS,
   OSP) carry that routing on their own and are always present; the other three
   are optional (§6) and buy sort-merge joins.
3. **Pyramid (community summarization)** — partition nodes into a hierarchy of
   communities. Level 0 is a *quotient graph* (communities as supernodes, with
   aggregated edges). Each deeper level expands supernodes into their members.

---

## 4. File layout

The stable generation-1 on-disk layout (what `write_file`/`write_dataset`
actually emit). The
header is the directory: it carries the absolute offset+length of every section, so a
client finds everything from the first 1 KB read — no separate directory or
metadata block to chase.

<img src="img/file-layout.svg" alt="The section directory inside a .rete header, drawn to scale on the published davidrumsey.rete. The header is 1024 bytes: a 64-byte core, then up to 40 directory entries of 24 bytes each starting at byte 64, then zero padding — this file uses 6 entries, so most of the header is spare room. One entry is a 2-byte kind, a 2-byte flags field, 4 reserved bytes, an 8-byte offset and an 8-byte length. The seven typed kinds are dataset card, dictionary, index, pyramid, named graphs, text index and build info; this file has all but named graphs and build info. One GET of bytes 0 to 1023 therefore locates every section in the file.">

*The header is the directory, on a real specimen. Entry `i` lives at `64 + i×24`;
a kind the reader does not recognise is kept verbatim, so a new writer cannot
break an old reader.*

<img src="img/rete-anatomy.svg" alt="Anatomy of a .rete file, drawn to scale on the real dblp.rete — 2.27 GB, 179,328,188 triples, 64,276,736 terms. A 1 KiB header carries a 64-byte core and a directory of 24-byte entries; then a 731-byte dataset card, the front-coded dictionary (413 MB, 18.2%), and six permutation indexes cut into roughly 64 KiB tiles with a per-tile min/max synopsis (1.85 GB, 81.8%). The file ends in a 4-byte RETE magic that a truncated download cannot fake. This specimen has no pyramid, named graphs, text index or build info — those section kinds are optional, and the directory simply does not list them.">

*The layout on a real specimen (`dblp.rete`, 179 M triples): the dictionary and the six permutation indexes carry nearly all the bytes; everything a reader needs to route a query — header, directory, card — fits in the first two range requests. This file carries none of the four optional sections, and the figure says so.*

*The header is the directory: a single 1 KB read carries the offset and length of every section. The ASCII below is the precise reference.*

```
┌──────────────────────────────────────────────────────────────┐
│ HEADER            fixed-size, 1024 bytes, read first.          │
│   A typed section directory: the offset+length of every       │
│   section below, plus codec ids, counts, content hash (§4.1). │
├──────────────────────────────────────────────────────────────┤
│ DICTIONARY        front-coded container of four sections:      │
│   ├ shared (terms used as both subject & object)               │
│   ├ subjects-only                                              │
│   ├ objects-only (incl. literals)                              │
│   └ predicates          (graph IRIs live in NAMED GRAPHS)      │
├──────────────────────────────────────────────────────────────┤
│ INDEX             default-graph permutation container:         │
│   SPO/POS/OSP/SOP/PSO/OPS streams, each a zone-mapped,         │
│   delta-coded, (optionally zstd) block. Via `root_dir_offset`. │
├──────────────────────────────────────────────────────────────┤
│ PYRAMID META      summary superedges (community quotient graph)│
│   + optional per-community tiles. Pointed to by pyramid fields.│
├──────────────────────────────────────────────────────────────┤
│ NAMED GRAPHS      optional: (graph IRI, permutation container) │
│   per named graph, sharing the dictionary. Quads only.         │
├──────────────────────────────────────────────────────────────┤
│ FOOTER            4-byte `RETE` magic — a format/integrity     │
│                   sentinel (the directory is the header).      │
└──────────────────────────────────────────────────────────────┘
```

Each permutation section carries its own **tile directory** (format `0x02`,
§6.2): byte ranges for independently-compressed tiles, keyed by leading-id
range — so single-pattern routing fetches the selected permutation section (one
of the six) and decompresses only the matching tile(s). Per-*community* leaf directories
(mapping `(level, tile, perm)` to byte ranges across the pyramid) are part of
the fuller design and remain future work (see `docs/BENCHMARK.md`).

### 4.1 Header (1024 bytes, little-endian)

The header is a fixed **64-byte core** followed by a **typed section directory** —
up to 40 entries of 24 bytes each `(kind, flags, offset, length)` — zero-padded to
1024. A new top-level section is added as a new directory entry, so the header has
room to grow without a layout reshape. Format byte `0x05` is **stable format
generation 1**, frozen on 2026-07-14 and first released in Rete **0.3.0**. It
fixes this 1024-byte section-directory header and the six-wide permutation
addressing — *how many* of those six a given file actually stores is its
**permutation mask** (byte 50, §6), not its generation. Experimental formats
`0x01` through `0x04` are not readable and must be rebuilt from RDF source.

> There is no Rete 1.0.0. The generation number counts *format* generations and
> is independent of the release version (the workspace is 0.3.x); the Rust, CLI
> and WASM APIs are the surfaces waiting on 1.0.0, not the file format. See
> [Compatibility](compatibility.md#stable-rete-file-compatibility).

**Core (bytes 0..64):**

| Offset | Size | Field |
|---|---|---|
| 0 | 4 | magic `RETE` |
| 4 | 1 | format version (`0x05`, stable generation 1) |
| 5 | 1 | flags (bit0: has named graphs/quads; bit1: tile-synopsis trailer; bit2: contains RDF-star quoted triples) |
| 6 | 2 | header length (= 1024) |
| 8 | 16 | content hash (blake3, first 16 bytes) — also an ETag-like id |
| 24 | 8 | total quad count |
| 32 | 8 | total term count |
| 40 | 2 | number of pyramid levels |
| 42 | 1 | dictionary codec id |
| 43 | 1 | block codec id (e.g. zstd) |
| 44 | 2 | section count (entries in the directory) |
| 46 | 4 | schema-pyramid block length (u32, 0 if none) — the trailing schema block within pyramid-meta, fetched at `pyramid_meta_offset + pyramid_meta_len − this` for an index/dictionary/summary-free schema-coherence read |
| 50 | 1 | **index permutation mask** (§6): bit *i* = permutation *i* of `SPO, POS, OSP, SOP, PSO, OPS` is stored. `0` means **all six** — the canonical spelling, so a full build is byte-identical to every file written before this byte was defined. A mask must contain SPO+POS+OSP (`0b000111`); anything else is rejected at header parse |
| 51 | 13 | reserved (zero) |

**Section directory (bytes 64…, `section_count` entries of 24 bytes):**

| Offset | Size | Field |
|---|---|---|
| +0 | 2 | section kind (`1` metadata, `2` dictionary, `3` index, `4` pyramid-meta, `5` named-graphs, `6` text-index, `7` build-info; higher ids reserved for future sections) |
| +2 | 2 | section flags (reserved) |
| +4 | 4 | reserved (zero) |
| +8 | 8 | section offset |
| +16 | 8 | section length |

A reader maps known kinds to its named accessors and **preserves unknown kinds
verbatim** (so a newer writer's sections survive an older reader). A kind absent
from the directory means that section is not present.

> Rationale: a fixed 1024-byte header means **one small range read**
> (`bytes=0-1023`) tells the client where every section lives, with headroom for
> new sections (group directories, geo indexes, …) as new directory entries
> rather than a format break — the optional **text-index** section (kind `6`,
> §6.4) was added exactly this way.

The **metadata section** (offset 8 / length 16) sits between the header and the
dictionary and is `0`-length by default. When present it carries an opaque,
application-defined payload — the CLI stores a JSON [Dataset Card](dataset-cards.md)
there. Its bytes are included in the content hash (so `verify` covers it), and a
range-reading client never fetches it for a query (it reads sections by their
header offsets). A reader that doesn't understand the payload simply skips it: the
dictionary is always located via `dictionary offset`, which already accounts for
the section's length.

The **build-info section** (kind `7`) is an optional opaque payload laid out
**immediately after the metadata section** — adjacent, so a card reader fetches
metadata + build-info in one coalesced range and the CARD tier stays at one
header read plus one range read. The CLI stores build-conditions JSON there:
build timestamp, builder version, the flags in force, and measured starter-query
costs (see [Dataset Cards](dataset-cards.md)). Unlike every other section its
bytes are **deliberately excluded from the content hash**: it records exactly
the facts that differ between two builds of identical data (when, by which
binary, how fast), and the reproducible-hash property — two builds of the same
input hash identically — must survive them. `verify` therefore does not cover
it, on old readers (which see an unknown kind-7 entry and ignore it) and new
ones alike; treat its contents as advisory provenance, not integrity-protected
data.

---

## 5. Dictionary

<img src="img/dictionary-roles.svg" alt="The dictionary is four front-coded sections. Terms used as both subject and object are shared and take IDs 1 to S, and the same ID means the same term in either position; subject-only and object-only terms continue at S+1 inside their own space; predicates get an independent ID space that starts again at 1. So a term's ID range alone tells you its role, with no second lookup. Every section is sorted, deduped and front-coded against the previous term, with a restart point every 16 terms giving O(log n) term-to-ID lookup, and is chunked at about 64 KiB so resolving one term faults in one chunk instead of the whole section.">

*Four front-coded sections with role-ordered IDs: a term's ID range reveals whether it is a subject, object, or predicate.*

- Terms are **sorted within each kind** and **front-coded** (store the shared
  prefix length + the suffix vs. the previous term). Cheap to compress, supports
  binary search for `string → id`, and prefix-friendly for IRI namespaces.
- IDs are assigned so that **shared terms get the lowest IDs**, then subjects,
  then objects — this lets a triple pattern know a term's role from its ID range.

  Concretely (the HDT four-section scheme): let `S` = #shared (terms used as
  *both* subject and object). Subject IDs are `1..=S` (shared) then `S+1..`
  (subject-only); object IDs are `1..=S` (the **same** shared IDs) then `S+1..`
  (object-only). So a shared term has one ID that means the same thing in subject
  and object position. Predicates and graphs occupy their own independent ID
  spaces. This is what `dictionary.rs` builds on top of the §5.1 sections.
- Literals carry datatype IRI (as a dictionary ID) and optional language tag.
- **RDF-star quoted triples** (`<< s p o >>`) are stored as ordinary terms: a
  quoted triple is interned by its canonical N-Triples-star surface, exactly like
  an IRI or literal, so the dictionary, the permutation indexes, and this layout
  need no change for RDF-star — hence no format-version bump. A file that contains
  any quoted triple sets header flag bit 2 (§4.1) so a plain-RDF reader can detect
  it without scanning. See [SPARQL § RDF-star](sparql.html#rdf-star).
- The dictionary is four independently-decompressible **sections** (shared /
  subjects / objects / predicates), each with a restart-indexed table (§5.1) that
  supports `O(log n)` term↔ID lookup once loaded. Each section is additionally
  **chunked** for ranged access (§6.2): resolving one IRI faults in exactly one
  ~64 KiB chunk rather than the whole section.

### 5.1 Section encoding (front-coded, restart-indexed)

Each dictionary *kind* (shared / subjects / objects / predicates / graphs) is one
**section**. Within a section terms are UTF-8, **sorted ascending**, deduped, and
assigned **dense 1-based IDs** in that order (ID `0` is reserved as "absent").

Terms are stored in **runs of `R` entries** (default `R = 16`, the *restart
interval*). Each run begins at a **restart point** storing the full term; the
remaining `R-1` entries are *front-coded* against their predecessor:

```
restart entry:   varint(0)            varint(len)  bytes[len]          # full term
delta entry:     varint(shared_pfx)   varint(suf)  bytes[suf]          # share prefix
```

A small **restart index** (one `(first_term, byte_offset)` per run, the first
terms also front-coded against each other) sits at the head of the section. Both
lookups stay within one run after a binary search over restarts:

- **`id → term`**: run = `(id-1) / R`; jump to its offset; decode forward
  `(id-1) % R` steps.
- **`term → id`**: binary-search the restart index for the candidate run; decode
  that run comparing each materialized term; return its ID or `None`.

Restart points are also the natural **chunk/compression boundaries**: a client
fetches just the run(s) covering the IDs/terms a query touches.

### 5.2 Integer domains and hostile varints

Dictionary terms and all physical triple components are **u32 IDs**. `0` is
the absent sentinel and a single dictionary role space therefore holds at most
`u32::MAX` assigned IDs. The term count, restart interval, run counts when used
as IDs, tile range endpoints, group counts, and prefix-directory offsets are
decoded with checked `u64 → u32` conversions; a larger varint is malformed, not
a truncating cast. This is a physical-ID limit, not a file-size limit.

Aggregate counts and byte coordinates remain **u64**: header quad counts,
section offsets/lengths, compressed payload lengths, cumulative record offsets,
and aggregate graph totals may exceed `u32::MAX`. The staged `0x06` family
decoder bounds its framed count/length conversions and capacities against bytes
already present; dictionary restart reconstruction and chunk-run coordinates use
checked `u64` arithmetic before conversion to their physical `u32`/`usize`
domains. These are format-boundary guarantees, not a claim about every reader
implementation path.

---

## 6. Triples / quads

<img src="img/index-permutations.svg" alt="Three of the six permutation indexes answer every SPARQL triple pattern: SPO serves the four patterns with a bound subject or nothing bound, POS the two with a bound predicate and unbound subject, OSP the two with a bound object and unbound subject and predicate. The other three — SOP, PSO, OPS — route nothing; their only job is to hand a merge join a stream already sorted on the join column. Measured on tree-city-inventory.rete built without a pyramid, the three routing indexes are 7.52 MB and the three optional ones 9.82 MB, so building with rete build --permutations 3 drops 50.5 percent of that file — and 36.8 percent of a literal-heavy one such as davidrumsey.">

*Which pattern reaches which index, and what the other three actually buy. The
routing three are measured on `tree-city-inventory`; the optional three are the
bigger half, because a sort order that does not lead with a subject or predicate
prefix compresses worse.*

- Stored as integer triples in **SPO, POS, OSP** and, by default, also **SOP,
  PSO, OPS** (stable format `0x05`; experimental `0x03` stored only the first
  three). Which orders a file carries is recorded in the header's **permutation
  mask** (byte 50; `0` = all six) and is fixed at build time by
  `rete build --permutations 3|6`. **The default is six.**
  - The first three *match* any of the eight triple-pattern shapes, and they do
    so at the **same longest bound prefix** the full six achieve on every one of
    those eight — enumerated in `index.rs`'s `perm_routing_never_leaves_core`.
    So routing, the tiles fetched, and the rows returned are identical either
    way; the choice is invisible from a query's results.
  - The full six additionally sort the triples on **every** prefix of columns, so
    for any bound prefix and any free column there is a permutation that routes
    on the prefix **and** streams sorted on that column — the precondition a
    **sort-merge join** needs (both inputs co-sorted on the join key). Exactly
    three of the twelve (bound-set, join-column) shapes lose that stream with
    three permutations: subject-bound sorted on object, predicate-bound sorted on
    subject, object-bound sorted on predicate. The planner then declines the
    merge seed and hash-joins, which is a slower plan, never a wrong one.
  - The cost is ~2× the index payload: measured at **36.8%** of a built
    literal-heavy file (`davidrumsey`, 5.00 M triples) and **50.5%** of a
    short-term-heavy one (`tree-city-inventory`, 3.15 M triples) — the two
    builds tabulated in
    [BENCHMARK.md](BENCHMARK.md#the-merge-join-permutations-cost-vs-benefit).
  - **A file with fewer than six permutations is not readable by a Rete that
    predates the mask.** Such a reader passes six to the index container's
    section-count check and gets `malformed container: expected 6 permutation
    sections` (resident) or `unexpected container section count` (ranged), exit
    1, on every command that touches the index — a loud refusal, not a wrong
    answer. Its `info`, `verify` and `card-url` still work, because they read
    only the header and the metadata section.
- Each permutation is encoded as an **adjacency / bitmap-triples** structure
  (HDT-style): for SPO, a sorted list of subjects, each with its predicate
  list, each with its object list, delta-encoded.
- Quads (named graphs) add a `G` dimension; the format stores per-graph triple
  sets plus a default-graph union. (Full GSPO permutations are a later optimization.)
- Triples are **partitioned by pyramid tile** (see §7), and within a tile split
  into fixed-size **blocks**. Every block is prefixed with a **zone map**
  (min ID, max ID, count) so the query planner can skip blocks — Parquet-style.

### 6.1 Triple block encoding (grouped delta)

Within a block, triples for a permutation are sorted ascending on `(a, b, c)`
(e.g. SPO ⇒ a=S, b=P, c=O) and stored as a nested **grouped, delta-coded**
adjacency — the compact, decode-forward form HDT calls bitmap-triples, written
here with explicit group counts:

```
zone map:  varint min_a, max_a, min_b, max_b, min_c, max_c, triple_count
body:      varint num_a
           per a-group:  varint Δa            # Δ from previous a (first = absolute)
                         varint num_b
              per b-group: varint Δb          # Δ from previous b *within this a*
                           varint num_c
                 per c:      varint Δc         # Δ from previous c *within this b*
```

Deltas reset at each group boundary, so values stay small and varint-friendly.
The **zone map** lets the planner skip a block whose `[min,max]` range cannot
contain a bound constant before fetching the body — the §7 routing then bounds
*which* blocks are fetched at all.

### 6.2 Tiled permutation sections (format `0x02`)

Each permutation section is **tiled**: consecutive runs of whole a-groups are
packed to a byte budget (default 64 KiB of encoded triples), and each tile is a
fully self-contained §6.1 block (its own zone map; deltas restart). Tiles are
**compressed individually** with the header's block codec, so a ranged client
can fetch and decompress exactly the tiles a query routes to. The section
payload is stored raw inside the index container (the container-level codec for
index sections is `none`; compression lives at tile granularity):

```
section payload:  varint num_tiles
                  per tile: varint Δmin_a       # Δ from previous tile's min_a
                            varint max_a−min_a  # leading-id span (routing)
                            varint clen          # compressed tile length
                  tiles:    num_tiles × compressed §6.1 blocks, concatenated
                  synopsis trailer (only if header FLAG_TILE_SYNOPSIS, 0b10):
                  per tile: varint min_b, varint max_b−min_b   # non-leading col B
                            varint min_c, varint max_c−min_c   # non-leading col C
```

A bound leading component binary-searches the directory to exactly **one**
tile (a-groups are never split across tiles); an unbound one visits every
tile, zone-map-pruned. The directory is uncompressed so it is readable before
any tile is fetched.

The optional **tile-synopsis trailer** lifts each tile's two non-leading-column
ranges out of its (compressed, must-be-fetched) zone map and into the section, so
a range reader can prune a routed tile by a bound *secondary* component **without
fetching it** — a negative/sparse lookup then costs zero tile reads. It is
appended **after** the tile payloads precisely so a reader predating the flag
locates tiles by `clen` and never reads it (backward-compatible; no version bump).
The values mirror each tile's own zone map exactly, and a reader only ever uses
them to *skip* a proven miss (the in-tile zone map is the backstop), so the prune
can never drop a result. Cost: one extra small tail read per section at open,
amortized across a session's queries.

**Dictionary sections are chunked the same way** (their container-level codec
is also `none`): each of the four §5 sections is stored as

```
section payload:  varint header_len, raw §5.1 header (term count, restart
                  interval, restart-offset table — original encoding, so the
                  offsets stay valid in the section's coordinate space)
                  varint num_chunks
                  per chunk: varint Δfirst_run        # Δ from previous chunk
                             varint key_len, key bytes # routing separator
                             varint clen               # compressed chunk length
                  chunks:    run-aligned body slices, compressed individually
```

Chunks hold whole front-coded runs (~64 KiB of body per chunk), and the
directory carries one **routing key** per chunk — so `term → id`
binary-searches the directory locally and faults exactly **one** chunk, and
`id → term` computes its chunk arithmetically and faults one. A lazily-opened
remote file therefore pays the section headers + directories (KBs) up front and
O(touched chunks) afterwards, instead of the whole dictionary container.

The key is a **separator, not a term**. Its only contract is

```
last_term(chunk i-1)  <  key(i)  <=  first_term(chunk i)
```

with chunk 0's key empty (`b"" <= anything`). A reader routes by
`partition_point(|c| c.key <= term)`, which needs nothing else; a term that
falls in the gap `key(i) <= term < first_term(i)` lands on chunk `i`, finds no
match, and is correctly reported absent. Writers store the **shortest** such
string — `first_term(i)` truncated one byte past where it diverges from
`last_term(i-1)` — which on a graph of long literals is a few bytes where the
term is kilobytes. Files written before 2026-08 store the chunk's first term
verbatim; that is the degenerate separator, so **both vintages route
identically and no version check is involved.** An existing file keeps its
larger directory until it is rebuilt.

Two consequences a future reader must respect: the key may not be reconstructed
into a term, compared for equality with one, or reported as one; and a key that
is *not* a separator (a truncation, a fixed-size hash) mis-routes **silently** —
`id → term`, `dump` and `export` route by `Δfirst_run` and stay byte-perfect
while `term → id` returns wrong answers.

### 6.3 Staged paired-family container (generation `0x06`)

The following is the exact internal contract for the next file generation. It
is deliberately staged: this repository still writes and dispatches stable
header generation `0x05` at this point. `0x06` is not emitted by production
writers or selected by public readers. `CURRENT_FORMAT_VERSION` and the minimum
stable read version both remain `0x05`. Task 11 will deliberately make the
eventual `0x06`-only break and remove the `0x05` reader; that incompatibility is
not part of this staged codec implementation.

Its index root is exactly three uncompressed length-framed family payloads in
**Subject, Predicate, Object** order. An empty graph is therefore
`varint(3), varint(1), 0, varint(1), 0, varint(1), 0`: three zero-pair family
payloads. A subject family pairs SPO/SOP, predicate pairs POS/PSO, and object
pairs OSP/OPS.

Each family payload is exactly:

```text
uvarint tile_pair_count
tile_pair_count × (uvarint min_a_delta, uvarint max_a_span)
tile_pair_count × (uvarint first_flags, uvarint first_compressed_len,
                   uvarint first_prefix2_len)
tile_pair_count × (uvarint second_flags, uvarint second_compressed_len,
                   uvarint second_prefix2_len)
first records in order:  prefix-2 blob, then compressed §6.1 tile payload
second records in order: prefix-2 blob, then compressed §6.1 tile payload
first synopsis trailer:  tile_pair_count × 4 uvarints
second synopsis trailer: tile_pair_count × 4 uvarints
```

`min_a_delta` is from the prior pair's `min_a` (first is absolute) and
`max_a_span = max_a - min_a`. Both orders must have exactly the same pair count
and leading range at every pair. Compressed lengths name only their compressed
tile payload: they exclude the prefix-2 blob and both trailers. Record offsets
are cumulative checked `u64` values.

Flags bit 0 means this tile continues the previous tile's leading group; bit 1
means that leading group continues into the next tile. All other bits are
reserved and rejected. A continuation repeats the same **singleton** leading
range in adjacent pairs, and both sibling orders carry identical continuation
flags. Non-continuing ranges are strictly ascending and disjoint.

A non-empty prefix-2 blob is:

```text
uvarint a_group_count
for each a group: uvarint a_delta, uvarint a_body_offset, uvarint b_count
  for each b group: uvarint b_delta, uvarint c_body_offset, uvarint c_count
```

`a_delta` is from the preceding `a`; `b_delta` resets for each a-group. The
body offsets are byte offsets in the **decompressed** §6.1 block and must be
inside that block; entries must agree exactly with its complete grouped body.
The blob is emitted only when the complete compact `(a,b)` directory fits the
fixed 64 KiB per-tile prefix-2 budget. Otherwise its length is zero and the
reader uses the existing bounded a-only directory; partial prefix-2 metadata is
never serialized.

Each synopsis trailer record is `min_b, max_b-min_b, min_c, max_c-min_c` and
must equal its decompressed tile's zone map. Counts, flags, varint domains,
ranges, continuation links, compressed slices, prefix-2 metadata, trailers,
and trailing bytes are all exact framing checks. Family varints are canonical
(at most ten bytes); prefix-2 blobs and decompressed tiles are each capped at
64 KiB. A zstd record is exactly one fully consumed frame, and the staged
decoder bounds count-dependent capacity by bytes already framed in the payload.
Its frame header is checked before decoder construction: both declared window
and any nonzero declared content size are at most 64 KiB; the staged encoder
uses the same 64 KiB window limit.

### 6.4 Full-text index (TEXT_INDEX section, optional)

An **opt-in** section (kind `6`, built with `rete build --text-index`) that maps
each **word** appearing in a string literal to the **subjects** that carry it, so
a reader can answer "which entities mention `glucose`?" without scanning the
literals — and a *remote* reader fetches only the posting lists it queries, never
the whole index. Absent by default: a build without `--text-index` writes no kind
`6` entry and is byte-identical to one that never had the feature.

**What is indexed.** Every triple whose object is a **string literal** is
tokenized: the literal's lexical value is split on non-alphanumeric (Unicode)
boundaries, each run lowercased and kept if ≥ 2 characters. The build and query
sides share one tokenizer, so a query word matches how it was stored. The result
is `token → sorted distinct subject ids`.

**On-disk layout** (the section payload):

```
varint token_table_len
token table (compressed with the header's block codec):
  varint num_tokens
  per token (ascending): varint shared_prefix_len   # front-coded vs previous token
                         varint suffix_len, suffix bytes
                         varint posting_off, varint posting_len   # into the postings blob
postings blob (uncompressed, so one posting range-reads directly):
  per token (same order): varint count, then `count` delta-varint ascending subject ids
```

The **token table** is small (distinct words, front-coded) and read whole; the
**postings blob** is the bulk and is fetched one posting at a time. A remote
search reads the leading `token_table_len` varint + the compressed token table as
one prefix range, binary-searches the (sorted) tokens locally, then range-reads
the single `(posting_off, posting_len)` it needs — `lookup(word)` faults one
posting, `prefix(word)` the contiguous run of postings whose tokens share the
prefix. Multiple query words **AND** by intersecting their sorted posting lists.
SPARQL never touches this section, so a query open neither fetches nor pays for it.

**Discoverability.** Because a file *without* the section answers the same
`FILTER(CONTAINS(…))` with the same rows — by full scan — the capability cannot
be inferred from a query result, and a reader must be able to ask the file. It
can: a kind-`6` entry in the section directory is present or it is not, in the
same `bytes=0-1023` a client already reads. The application layer surfaces that
as the Dataset Card's `signals.text_index`
(`{present, bytes, token_table_bytes}` — see
[Dataset Cards](dataset-cards.md#the-full-text-signal-measured-not-stored)),
**measured by the reader from this directory, never written into the metadata
section**. That is a deliberate exception to how every other card field works,
and it is what keeps the answer true across `repyramid --text-index` (which adds
the section to an existing file) and true for a file carrying no card at all.
`token_table_bytes` — the section's leading `token_table_len` varint plus the
table it measures, i.e. the prefix a first search fetches — costs one ≤10-byte
range read at `text_index offset` and is the honest cost figure; the section
length alone overstates it several-fold, since the postings blob is never read
whole.

**Compatibility:** `0x05` is stable format generation 1. Every stable reader from
Rete **0.3.0** onward reads `0x05` — there is no 1.0.0; the generation was frozen
in the 0.3 line and the release version is a separate number.
Optional sections and flags may extend it without changing its required semantics.
A required layout change uses a new format byte. **No backwards-compatibility
promise is made before 1.0.0:** a later generation may drop `0x05` read support
and force a rebuild from RDF source. The staged paired-family plan makes that
choice explicitly: Task 11 moves to `0x06` only and removes the `0x05` reader.
Experimental formats `0x01` through `0x04` are already such a break and must be
rebuilt from RDF source.

---

## 7. The pyramid — community summarization

The headline feature. "Zoom" = level of graph detail.

<img src="img/pyramid.svg" alt="The pyramid stores a graph at several levels of detail so a client can read an overview before touching the data. Level 0 at the top is the coarsest: a handful of supernodes with aggregated edges. Middle levels split those into finer communities, each tile targeted at about 64 KiB so one zoom is one range read. Level N-1 at the base is the full triple graph, fetched only where a query drills in. On the published davidrumsey.rete the whole pyramid is 1,332,512 bytes — 1.8 percent of the 74.8 MB file — so the overview is cheap and the base is not.">

*Top to bottom: coarse community summary → communities → full triples. Fewer bytes at the top, more detail below; clients fetch the overview first and drill down on demand.*

### 7.1 Build time
1. Run hierarchical community detection (**Louvain**; `--pyramid-algo types`
   swaps in a deterministic `rdf:type` partition instead) on the (optionally
   edge-weighted) graph. This yields a dendrogram of communities.
2. Cut the dendrogram into levels. **Level 0** = coarsest: each top-level
   community becomes a **supernode**. **Level N-1** = the full graph.

   **Cut policy: size-targeted tiles (committed).** We do *not* fix `N` up front.
   Instead each tile targets a byte budget `T` (default ~64 KiB, configurable) so
   that **one zoom ≈ one block ≈ one range read**, exactly like PMTiles. We
   descend the dendrogram and emit a tile boundary whenever a community's encoded
   triple payload would exceed `T`; communities still larger than `T` at the
   finest cut are split into multiple co-located tiles. `N` (the level count)
   therefore falls out of the data and `T`, rather than being chosen by hand.
   Consequences: predictable request sizes, uniform client latency per zoom, and
   a directory whose leaf granularity matches the transfer unit.
3. For each level build the **quotient graph**:
   - supernode = a community; its weight = node/edge count inside.
   - superedge `A→B` for predicate `p` = aggregate of all `p` edges crossing
     from community `A` to community `B`, with a count.
4. Emit, per supernode, the **member map** (which finer supernodes / nodes it
   expands into) so a client can drill down without re-clustering.

### 7.2 Query / browse time
- Client loads **level 0** (small): a graph of supernodes + aggregated relations.
  Good for overview, "shape of the data," and routing.
- To **zoom into** a supernode, the client follows its directory entry to fetch
  only that community's finer level — one (or few) range reads.
- SPARQL over the pyramid: a query can run against a level (approximate / rolled-
  up answers via superedges) or be **routed** — use level 0 to find which
  communities can satisfy the pattern, then descend only into those tiles. This
  is the graph analog of Parquet block-skipping.

### 7.3 The pyramid-meta section (on disk)

The pyramid-meta section (`crates/rete-core/src/meta.rs`) is a flat varint stream.
**v1** carries the chosen round, the summary super-edges, and (reserved) per-tile
triple blocks:

```text
varint round                       # the materialized dendrogram round
varint num_superedges;  per: varint s_comm, predicate, o_comm, count
varint num_tiles;       per: varint community, block_len, block_bytes   # currently empty
```

**v2** appends a **schema pyramid** (§7.4) after the tiles. The v2 block is written
**only when schema content exists**, so a typeless graph encodes byte-for-byte as
v1. A v1 reader stops after the tiles loop and silently ignores the appended bytes,
so v2 files remain readable by older clients — the upgrade is additive both ways.

```text
--- v2 (optional) ---
u8 schema_version (= 2)
varint num_strings;        per: len-prefixed UTF-8 IRI/sentinel     # local table: classes + predicates
varint num_hierarchy;      per: varint class_idx, num_parents, parent_idx…, depth   # non-exclusive DAG
varint num_rollups;        per: varint round, depth, num_entries, (class_idx, count)…
varint num_level_links;    per: varint round, depth, num_links, (s_idx, pred_idx, o_idx, count)…
varint num_descriptors;    per: varint community, dominant_idx+1 (0 = none),
                                num_class_counts, (class_idx, count)…,
                                u8 has_bbox [+ 4×f64 le],
                                u8 has_time [+ from(str) + to(str)]
```

The string table holds both class and predicate IRIs, so the schema pyramid decodes
**without the dictionary** — the ontology travels as self-contained text.

### 7.4 The schema pyramid — semantic zoom (v2)

The community pyramid (§7.1) is **topological**; the schema pyramid is the
**ontology, leveled** — upper-level classes describe coarse zoom, leaf classes
resolve as you zoom in. It is built once, index-free, into pyramid-meta:

- **`class_hierarchy`** — the **non-exclusive** `subClassOf` DAG over the classes
  that actually have instances (plus their ancestors), each with **all** its
  direct parents and a computed **depth** (0 = root). Multiple inheritance is
  preserved (`parents: Vec`); a canonical (lexicographically smallest) parent
  drives the deterministic depth/rollup spanning tree, while the other parents stay
  as navigable cross-links.
- **`level_rollups`** — one type histogram per **semantic level**, the instance
  counts rolled up the `subClassOf` chain to the level's depth. Level 0 is the
  most abstract (`{Agent: 12k, Place: 8k}`); each finer level resolves one step
  (`Agent → {Person: 9k, Organisation: 3k}`, then `Person → {Scientist, Artist}`).
  Counts conserve up the hierarchy. With **no** `subClassOf` in the data every
  class is a depth-0 root and this degrades to a single flat histogram (= the
  Dataset Card's `classes`).
- **`level_links`** — the **lateral** class-relation graph rolled up per level: the
  non-`is-a` connections `(s_class, predicate, o_class, count)` between abstract
  classes, so a level is a leveled *graph* not just a histogram (`Person memberOf
  Organisation` at a fine level becomes `Agent memberOf Agent` at a coarse one).
  `rdf:type` and `rdfs:subClassOf` triples are excluded — they are the hierarchy
  itself, not data relations.
- **`descriptors`** (Phase 4) — a per-community refinement index: the dominant
  class, local type histogram, and optional CRS84 `bbox` / temporal `time_range`,
  for progressive zoom into a region without fetching its triples. (Physical
  per-community triple tiles remain future work; these descriptors ship in the
  index-free pyramid-meta and are ready to attach to those tiles.)

A client reads the leveled legend over the same index-free range fetch as the
summary: `rete summary --level k` (and `summary-url`) render it without touching
the triple index. The `subClassOf` axioms come from the data, or from
`rete build --materialize` (the OWL-RL reasoner).

---

## 8. SPARQL evaluation (staged)

Implemented in `rete-core::sparql` (parses with `spargebra`, lowers to a small
plan algebra — `Bgp`/`Join`/`Union`/`Minus`/`LeftJoin`/`Filter`/`Path`/`Values`/
`Graph`):

- **Stage 1 — Triple patterns & BGPs.** ✅ Integer patterns over the permutation
  index; BGPs stream their least selective pattern against a hash table of the
  joined prefix (`bgp.rs`), and under a small `LIMIT`/`ASK` demand bound switch
  to index-nested-loop probes that jump to their group via a lazily-built block
  directory. Blank nodes in patterns are non-distinguished variables.
- **Stage 2 — FILTER, projection, DISTINCT, ORDER BY, LIMIT/OFFSET.** ✅ The
  whole algebra evaluates as a lazy pull pipeline over integer slot rows, so
  `LIMIT`/`ASK`/`DISTINCT … LIMIT` demand stops the index scans early; only
  aggregation, ORDER BY (bounded top-k when `LIMIT` is present), and hash-join
  build sides block. ORDER BY sorts on variable/constant keys: numeric when
  both are numbers, else lexical; complex key expressions are not yet evaluated
  for ordering.
- **Stage 3 — OPTIONAL, UNION, MINUS, VALUES, FILTER EXISTS/NOT EXISTS,
  property paths, GRAPH.** ✅ Paths (`p+`/`p*`/`p?`, reverse, sequence `a/b`,
  alternative `a|b`) evaluated forward from a bound endpoint in integer node
  space. **Named graphs / quads** (full dataset model): N-Quads input → one
  shared dictionary + a permutation index per graph; `GRAPH <iri>`/`GRAPH ?g`
  switch the active graph, `FROM` builds the default graph as an RDF merge,
  `FROM NAMED` scopes which graphs `GRAPH` sees, and EXISTS honors the enclosing
  graph context. *Future:* let paths exploit the pyramid (coarse reachability).
- **Stage 4 — Aggregation / GROUP BY / HAVING.** ✅ COUNT/SUM/AVG/MIN/MAX,
  BIND, expression functions (arithmetic, CONCAT, SUBSTR, type checks, …). Exact
  per-predicate totals come straight from the summary superedge counts
  (`SummaryView::predicate_totals`) without reading the index.
- **Query forms.** ✅ SELECT, ASK, CONSTRUCT, DESCRIBE (concise bounded
  description = each resource's outgoing triples).
- **Input/output.** ✅ N-Triples, N-Quads, and Turtle input; N-Quads/Turtle/JSON-LD
  export; SPARQL Results JSON output.
- **Supported:** nested `SELECT` **subqueries** — evaluated independently to their
  projected solutions, which then join with the surrounding pattern on shared
  variables.
- **Supported:** SPARQL 1.1 **`SERVICE` federation** — the block is shipped (as
  written) to the remote endpoint through a host-injected `ServiceClient` (the
  engine does no I/O itself; the CLI and browser clients attach HTTP transport)
  and its solutions join on shared variables. `SERVICE SILENT` degrades a failed
  call to one empty solution per the spec.
- **Not supported (rejected with a clear error, never silently mis-evaluated):**
  `SERVICE ?var` (a variable-bound endpoint). Complex ORDER BY key *expressions*
  (beyond a bare variable/constant) are also not yet evaluated for ordering.

The planner's job is to minimize **bytes fetched**, not CPU — the cost model is
dominated by range-request count and block sizes.

---

## 9. Access protocol (the client)

The header *is* the directory — every section offset/length is in it, so no
separate directory/footer round-trip is needed:

```
1. GET bytes=0-1023         → header: all section offsets/lengths + content hash
2a. Overview path  → GET dictionary + pyramid-meta ranges; answer from the
    summary (community quotient graph, per-predicate totals). The index is
    never fetched. (`SummaryView::open_ranged`; `summary_overview` in wasm.)
2b. Full query path → GET dictionary + index (+ named-graphs) ranges, then:
     - resolve constant terms in the dictionary
     - match the pattern in the SPO/POS/OSP/SOP/PSO/OPS permutation blocks
       (zone-map pruned)
     - resolve result IDs back to terms via the dictionary
     - optionally emit result provenance: matched IDs/terms, graph scope, chosen
       permutation, and the dictionary/index/payload/pyramid byte ranges
2c. Routed single-pattern path → GET dictionary, resolve constants, choose the
    best of the file's permutations, then follow the index container's length
    prefixes and fetch only that one permutation payload. Unknown bound terms
    skip the index entirely and return an empty result.
```

<img src="img/remote-open-cost.svg" alt="What a cold remote open costs, measured with rete cost on the published davidrumsey.rete — 74.8 MB, 5,001,983 triples. Each track is the whole file; the solid part is what crosses the wire. Reading the dataset card costs 2 requests and 61 KB, 0.08 percent. Opening lazily for a query costs 65 requests and 407 KB, 0.54 percent, because only tile directories are read. But routing one triple pattern costs 2 requests and 16.1 MB, 21.5 percent, and the overview costs 3 requests and 17.4 MB, 23.3 percent — both because resolving a term to an id pulls the whole dictionary. Reading the file whole costs 74.7 MB, 99.9 percent.">

*The same file, five ways in. The lazy path is 0.54% of the file; the two paths
that must resolve a constant term to an id pay for the whole dictionary. This
file predates separator keys, so its chunk directory also stores every chunk's
first term verbatim — 261,271 B, which a rebuild takes to 48,009 B (#198). On
this graph that is a rounding error inside a 16 MB dictionary; on a graph of
long literals it is most of the open.*

A full open touches ≤4 ranges (header, dict, index, pyramid-meta); the overview
path touches 3 and skips the index entirely. The routed single-pattern path reads
the selected permutation payload instead of the whole index container. These
access invariants are asserted by the `ranged` test suite. Per-tile leaf
directories that would let the query path fetch only the relevant community
tiles (rather than a whole permutation section) are future work. For the same
reason, current provenance identifies the index container and selected
permutation payload, while tile/block-level provenance remains a physical-layout
extension.

- The file is **immutable**; its content hash (header field) acts as a strong
  validator. Clients may cache sections by `(url, hash, range)` forever.
- Range coalescing + a small read-ahead keep the request count low.
- **Malformed input is safe.** Because a `.rete` can be fetched truncated or
  corrupt from an arbitrary URL, every header-derived offset/length and every
  embedded section length is bounds-checked before use. This holds end-to-end:
  `open` and the ranged readers, *and* querying a file that opened but carries
  corrupt block/dictionary internals (term resolution, front-coded delta decode,
  permutation-block iteration, unified node-id mapping) all return empty/`None`
  or an error — never panic, never over-allocate on an untrusted count. (Covered
  by the `robustness` suite: all truncations, header byte-flips, and arbitrary
  garbage, each then exercised through `dump`/`query`/SPARQL incl. a property
  path.)

---

## 10. Language & build

- **Rust core**, compiled to both **native** (CLI, server-side) and **WASM**
  (browser client) — identical query code in both.
- Crates to lean on: `oxrdf` / `oxttl` / `spargebra` (RDF + SPARQL), `ureq`
  (CLI HTTP range reads), `zstd`/`ruzstd`, `blake3`, `regex-lite`, `serde_json`,
  `rayon` behind native-only feature gates, and Louvain-style community
  detection.
- **Everything runs in Docker dev containers.** No host execution. The container
  carries the Rust toolchain, rustfmt, clippy, Python smoke-test tooling,
  `wasm-pack`, and the WASM target.

---

## 11. Glossary

- **Term** — an RDF IRI, blank node, or literal.
- **Quotient graph** — graph whose nodes are communities and whose edges
  aggregate the original edges crossing between them.
- **Supernode** — a community represented as a single node at a pyramid level.
- **Zone map** — per-block min/max/count stats enabling block-skipping.
- **Tile** — the unit of range-addressable data for a (level, community).
