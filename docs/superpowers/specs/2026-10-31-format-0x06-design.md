# Format 0x06: design (WS20, 2026-10-31)

Status: **design for review**. No format code is on main. Every number below was measured on 2026-10-31 with
the 0.4.2 CLI, or is derived from such a measurement and says so. The full tables, the method and the
reproduction steps are in `2026-10-31-format-0x06-measurements.md`; the scripts are in `scripts/format06/`.

The user's decisions (2026-10-31) frame it: no backward compatibility, drop the 0x05 compatibility patches,
read and query speed first (remote round trips and bytes, then local), writing second, PMTiles-style (hot data at
the front, two-level directories, data clustered in access order), small datasets first, and readers keep reading
0x05 while the catalog is mixed.

## 1. Summary

- **The open is the cost.** A 0x05 lazy open walks container length prefixes, then reads every dictionary chunk
  directory, every tile directory and every synopsis, each found by a dependent read. Measured cold, before any
  query: 53 round trips (RT) on the 195 MiB OpenCitations sample without the block cache, 9–11 RT with it on
  every catalog file, 59 RT for a 1.9 KB gate fixture, and **131 RT / 84.6 MiB (56 RT / 66.3 MiB with the
  cache) on OpenCitations Meta**. Meta's own build record says the same: each of its starter queries costs
  137–176 requests and 84.6–85.7 MiB.
- **0x06 puts a 128-byte header, a root block, the card, the planner statistics and every leaf directory at the
  front, in that order, and the data after them, clustered.** The first read is **64 KiB**. It holds the header,
  root, card and statistics of every measured file, Meta included (49 KiB). For every small catalog file it also
  holds every leaf directory except the text index's token table, once `convert` has re-keyed the three files
  whose dictionary directories still store whole terms.
- **Expected gain (simulated from the measured traces, 83 runs):** 0x05 without cache 4,651 RT / 83.5 MiB, 0x05
  with cache 947 RT / 166.6 MiB, **0x06 at 64 KiB 503 RT / 78.3 MiB**. Per run (median), that is 2.3× fewer
  round trips than 0x05 with the cache and 8.3× fewer than without it, with 2.7× fewer bytes than with the cache.
  An open becomes 1 read. A card read becomes 1 read. A text search on Meta stops reading the 179 MiB token
  table.
- **One content hash**, a two-level BLAKE3 tree: section digests in the root, the root and card digests in the
  hash. The build record is outside it. `verify` streams each section once. Recard rewrites the card and the
  128-byte header and reads no data.

## 2. What 0x05 costs, and why

Setup: the 0.4.2 CLI image; nginx behind a proxy that logs every request and holds it 50 ms. A **round trip** is a
wave of requests that arrive together. *No cache* is `RETE_BLOCK_KB=0`; *cache* is the default 64/128 KiB block
cache. Each run is a cold process.

| 0x05 open (`ASK { ?s ?p ?o }`) | no cache: RT / KiB | cache: RT / KiB |
|---|--:|--:|
| worldcup2026 (1.9 KB, gate fixture) | 59 / 2.2 | 2 / 1.9 |
| tree-city-inventory (24 MiB) | 53 / 138 | 11 / 960 |
| OpenCitations sample, quad (195 MiB) | 53 / 335 | 9 / 1,920 |
| OpenCitations Meta (59 GiB), from R2 | 131 / 86,622 | 56 / 67,840 |

The dependent reads come from the layout:

1. **Containers.** The dictionary (4 sections) and the index (6 permutations, plus the quad seal) are length-prefixed
   containers. Section *k* is found by reading the count and the *k + 1* prefixes up to it, each a 10-byte read at
   a different place in the file, and the walk restarts for every section: 27 dependent reads to locate the six
   permutations, 14 for the four dictionary sections.
2. **A dictionary section's chunk directory sits behind its restart table.** It is found by a probe, then read in
   growing pieces. The restart tables are never read by the lazy reader: **dead bytes, 0.94% of the OC files**
   (1.9 MB on the sample, 589 MB on Meta).
3. **A tile directory's length is not recorded.** The reader reads 4 KiB at the head of the permutation and doubles
   until it parses: 11 dependent reads for Meta's 3.8 MB SPO directory. The synopsis is a trailer at the other end
   of the permutation.
4. **Section heads hold the roots.** The token table, the label root and the quad preamble each cost a read at the
   head of their section. The schema is at the end of the file. The card is in one of two places.
5. **Big directories are read whole.** A text search reads the whole token table: 3.1 MB on the sample, 179 MiB on
   Meta. A `GRAPH <g>` query reads the quad tile directories in one 233 KB read on the sample (~70 MB at full Meta
   scale with provenance, G4).

Not format costs, but measured, and fixed in the 0x06 reader: the `HEAD` before the first `GET` (1 RT per open);
dictionary chunks faulted one at a time during scans (60 consecutive RT in the journal-ISSN query); result terms
decoded one dictionary section at a time (3 RT on a davidrumsey lookup).

### The hot set, and how much of it a query uses

| file | 0x05 hot: dirs and roots / card / token table (KiB) | dead (KiB) | 0x06 root | card (zstd) | stats+schema | leaves without the token table | token table |
|---|--:|--:|--:|--:|--:|--:|--:|
| OC sample, quad | 776 / 61 / 3,078 | 1,919 | 3.5 | 11 | 1.9 | 537 (quad 443) | 3,078 |
| davidrumsey | 284 / 59 / 1,574 | 492 | 2.4 | 6.4 | – | 113 (≈ 36 re-keyed) | 1,574 |
| swissubase | 516 / 58 / 1,287 | 496 | 2.4 | 7.4 | – | 173 (≈ 29 re-keyed) | 1,287 |
| arxiu | 146 / 34 / 871 | 646 | 1.4 | 4.0 | – | 58 (≈ 37 re-keyed) | 871 |
| tree-city-inventory | 40 / 42 / 1,380 | 152 | 1.8 | 4.8 | – | 12 | 1,380 |
| mtg, rebuilt with labels + schema | 33 / 51 / 232 | 60 | 1.5 | 6.6 | 6.4 | 9.2 | 232 |
| OC Meta (59 GiB) | 88,667 / 51 / 183,526 | 575,647 | 36.9 | 10.1 | 1.9 | 62,097 | 183,526 |

"Re-keyed": davidrumsey, swissubase and arxiu predate separator keys (#198), so their chunk directories store
whole terms. Re-keying them (as `rete convert` will) shrinks those directories 2–10× (measured with `rekey.py`).

A lookup touches the dictionary and index directories: 226 KiB of the sample's 3,915 KiB hot set (6%). A `GRAPH`
query adds the quad directories (460 KiB, 12%). A label search touches 34 KiB. A one-word text search touches
3,116 KiB (80%) for a single token. The card read touches the card (62 KiB). So: the small structures are needed by
nearly every query and belong in the first read; the token table and the quad directories are needed rarely, and
then only one block of them.

## 3. Byte layout

```
0          ┌ HEADER 128 B ───────────────────────────────────────────────┐
128        │ ROOT BLOCK (zstd): section table, directory roots, signals   │  first read: 64 KiB
           │ CARD REGION: card (zstd) | build record (zstd) | slack       │
           │ STATS + SCHEMA (zstd)                                        │
           │ LEAF DIRECTORIES, zstd blocks:                               │
           │   dict (ids, keys) · index SPO…OPS · quad · labels · tokens  │
front_len  ├ DATA, clustered ─────────────────────────────────────────────┤
           │ dict chunks s3 s0 s1 s2 · index tiles SPO POS OSP SOP PSO OPS│
           │ · quad tiles · label blocks · postings · pyramid (optional)  │
file_len-8 └ FOOTER: "RETE" 0x06 0 0 0 ─────────────────────────────────────┘
```

### 3.1 Header (128 bytes, little-endian)

| off | size | field |
|--:|--:|---|
| 0 | 4 | magic `RETE` |
| 4 | 1 | version `0x06` |
| 5 | 1 | flags: b0 named graphs (quad index), b1 RDF-star quoted triples, b2 text index covers named graphs, b3 quad tiles graph-major (§3.5) |
| 6 | 2 | header length (128) |
| 8 | 16 | content hash (§5) |
| 24 | 8 | file length (lets a client open without `Content-Range` or `HEAD`) |
| 32 | 4 | root length (the root starts at 128) |
| 36 | 4 | card region length (starts at 128 + root length) |
| 40 | 8 | front length = offset of the data region |
| 48 | 8 | statement count |
| 56 | 8 | term count |
| 64 | 8 | named-graph count |
| 72 | 4 | permutation mask (byte-50 semantics), dictionary codec, block codec, internal codec (root, leaves, card) |
| 76 | 4 | reserved (zero) |
| 80 | 8 | card: offset (u32, within the card region) and length (u32) |
| 88 | 16 | card digest (BLAKE3, first 16 bytes) |
| 104 | 8 | build record: offset (u32, within the region) and length (u32) |
| 112 | 16 | root digest |

The card's place and digest live in the header, not in the root, so a recard
rewrites 128 bytes at offset 0 and nothing in the root (§6.3).

### 3.2 Root block

One zstd frame (internal codec), at most 40 KiB compressed by the writer's rule (§3.4). It holds:

- **The section table.** Per section: kind, flags, offset, length, digest (16 B). Offsets are relative: a front
  section's to the end of the card region, a data section's to the start of the data region. Kinds: STATS+SCHEMA, and a directory section and a data section for each of dictionary, index,
  quad, labels and text; pyramid (data only). No entry limit. No kind numbering rule beyond "unknown kind: refuse".
- **Directory roots.** For each directory unit (§3.4): its leaf count, the first key of each leaf, and each leaf's
  length, column-wise (PMTiles §4.2): a count, then the delta-coded keys, then the lengths. Leaf offsets are
  implicit (leaves are contiguous in their unit), and so are payload offsets (each leaf records the cumulative
  payload offset of its first entry).
- **Small fixed facts** that today cost a read: dictionary section term and chunk counts; per-permutation tile
  counts; the quad preamble (graph count, quad count, orders, clustering); the label entry count; the text index's
  scope block, token count and posting bytes. The card's `signals` (text index present, token-table bytes, label
  entries, permutations) are read from here, so the card tier is one read.

Measured: 1.4–3.5 KiB on every small file; 36.9 KiB on Meta; 39.9 KiB on Meta with its provenance as quad
index (G4's 70 MB of quad directories; the writer doubles leaves once more).

### 3.3 Front items

- **Card region**: the card (zstd: 4–11 KiB measured, from 35–62 KB of JSON), then the build record (zstd), then
  zero slack. The writer reserves `2 × (card + record) + 8 KiB`, so a recard can write the new pair into the free
  part (§6.3). Readers find the card and the record from the header only.
- **Stats + schema**: the planner's per-predicate `query_stats` and characteristic sets first, then the class
  hierarchy, rollups and coherence axioms (today's kind 9), zstd. 0.1–6.4 KiB measured (2 KiB on Meta). It comes
  right after the card because the planner reads it for joins; at 64 KiB it is in the first read on every measured
  file.

### 3.4 Leaf directories (two levels, PMTiles-style)

Every directory is two levels: root entries (§3.2) and leaf blocks. A leaf is one zstd frame, column-wise (counts,
then each field as its own varint column, deltas against the previous entry, lengths only, offsets implicit
because the data is clustered). The writer starts every unit at 16 KiB leaves and, while the root exceeds 40 KiB,
doubles the leaf size of the unit with the most root bytes (PMTiles writers grow leaves the same way until the root
fits its 16 KiB). The units, in front order:

| unit | entry fields | OC sample: 0x05 → 0x06 bytes, leaves | Meta: 0x05 → 0x06 bytes, leaves × size |
|---|---|--:|--:|
| dict s3, s0, s1, s2 | **id route**: Δfirst run, compressed length; **key route**: shortest separator key (front-coded) | 193,219 → 29,818; 5 | 11.0 MB → 2.8 MB; 172 × 16 KiB |
| index SPO … OPS | Δmin a, span, length, then the synopsis columns min b, Δb, min c, Δc | 37,553 → 29,301; 6 | 50.3 MB → 31.4 MB; 1,918 × 16 KiB |
| quad graphs, G+SPO … G+OPS | graph directory (Δg, quad count); per order: Δfirst g, first a, Δlast g, last a, length\|raw, synopsis | 521,359 → 453,906; 31 | ~70 MB with provenance (G4); not in the published file |
| labels | first key of each data block, offset, entries (today's level-1 node) | 37,094 → 37,094; 3 | 29.4 MB; 898 × 32 KiB |
| text tokens | tokens front-coded, Δposting offset, posting length (today's token table, cut into blocks) | 3,152,312; 193 | 187.9 MB; 717 × 256 KiB |

**Too big for the root at any scale, so leaf level:** the text token table (3.1 MB on the sample, 188 MB on Meta)
and the quad directories (0.5 MB on the sample, ~70 MB at full Meta scale). At Meta scale every directory is leaf
level (dictionary 11 MB, tiles and synopses 50 MB, labels 29 MB); the root keeps one entry per leaf.

The dictionary unit is split in two columns of leaves because the two lookups need different things: `id → term`
needs only the id route (a few bytes per chunk), `term → id` needs only the keys (most of the bytes: 9.6 of Meta's
11.0 MB). One level of leaves is enough at Meta scale with these sizes; PMTiles discourages deeper trees and so
does this design.

### 3.5 Data region

Payload blocks are byte-identical to 0x05 (front-coded dictionary chunks, §6.1 tiles, quad tiles, label blocks,
postings), **without** anything interleaved: no container prefixes, no tile directory at a permutation's head, no
synopsis trailer, no restart tables. Order:

1. Dictionary chunks: predicates (s3), shared (s0), subject-only (s1), object-only (s2).
2. Index tiles: SPO, POS, OSP (the routing three), then SOP, PSO, OPS. A scan of one permutation, or of one
   predicate's run in POS, is one contiguous range.
3. Quad tiles. **Graph-major** when graphs are small: the tiles of all orders for one block of graphs (the
   directory's existing 1,024-graph blocks) are adjacent, so a reader can fetch a graph's slice in every order in
   one request. On the sample, `GRAPH <g>` provenance queries read three order tiles in three dependent waves (a
   POS, a PSO and an SPO tile of ~11 KB each); fetched together on first touch they cost one. A tile that straddles
   a block boundary stays with the block it starts in. Order-major when the mean graph exceeds a tile. Flag b3
   records which; readers route through the directories either way.
4. Label blocks, in key order; text postings, in token order (a prefix search reads one run).
5. The community pyramid (superedges and descriptors), optional and cold: only `summary`/overview reads it.

Not taken from PMTiles: **tile dedup and run lengths.** RDF tiles are not repeated; there is nothing to share.

### 3.6 Footer

8 bytes: `RETE`, `0x06`, three zero bytes. A truncated download is caught by the header's file length; the footer
lets a browser reader that cannot see `Content-Range` confirm the length with one small read off the critical path.

## 4. The open protocol

1. `GET bytes=0-65535`. The version byte decides the reader: 0x05 continues with the 0x05 lazy open, seeded with
   these 64 KiB (for most catalog files they already hold the 1 KiB header, the card and the first container
   prefixes). No `HEAD`: the length comes from `Content-Range`, else from the header (0x06) or the 0x05 path's own
   footer check.
2. 0x06: if `128 + root length` exceeds what was read, read the rest of the root (no measured file needs it). The
   client now has the header, root, card, build record and statistics, plus every leaf that fits in the 64 KiB.
3. A query fetches the leaf blocks it needs, all at once, then payload. **Leaf prediction** removes most leaf round
   trips on big files: a dictionary chunk's id range (known from the id route before the chunk arrives) names the
   tile-directory leaf, so both go in the same wave; a tile's synopsis names the id-route leaves of its result ids.
4. Leaves are cached for the session, keyed by `(content hash, offset)`; the file is immutable, so a persistent cache
   (disk, IndexedDB) is valid forever.
5. **Adaptive growth.** The header gives `front length`. A session client (playground, MCP server, Python) that will
   run several queries fetches the rest of the front at once when it is at most 1 MiB, leaving out the token table
   (OC sample: 553 KiB). The catalog can carry each dataset's front length so the first read covers it.
6. **When the block cache still matters: payload scans.** With exact reads, a class histogram on davidrumsey costs
   48 RT; through 64 KiB blocks, 18. `MIN/MAX` over a predicate on the sample: 75 vs 11. The journal-ISSN path
   query: 72 vs 19. For isolated lookups the block cache only adds bytes (DOI lookup, sample: 312 vs 1,216 KiB at
   the same 5 RT). Recommendation: exact reads for isolated faults, read-ahead within a section on sequential faults,
   a byte-capped LRU. The cache is no longer needed for directories.

Why 64 KiB (from the simulation, 83 runs): 16 KiB is worse in 31 runs (+1 RT each: Meta's root is 36.9 KiB, and
the card plus leaves do not fit beside a small file's root); 256 KiB is better than 64 KiB in 8 runs, by 1 RT, and
only on files whose dictionary directories still have whole-term keys (fixed by re-keying in `convert`) or whose
token table is under 256 KiB (mtg). Over all runs 256 KiB costs 13.5 MiB more for 8 RT fewer.

## 5. Content hash and verify

- Every section-table entry carries `digest = BLAKE3(section bytes)[0..16]`.
- `root digest` (header) = BLAKE3 of the root block; `card digest` (header) = BLAKE3 of the card bytes.
- **`content hash = BLAKE3("rete/0x06" ‖ flags ‖ counts ‖ codecs ‖ permutation mask ‖ card length ‖ card digest ‖
  root length ‖ root digest)[0..16]`.** Through the root digest it covers every section digest, so every byte of
  every section; through the card digest, the card. The build record and the slack are outside it, because two
  builds of the same data differ there by nature.
- **No offsets are hashed.** Root offsets are relative to the front or the data region, so moving a section (a
  copying recard that grows the card region) changes no digest.
- **`verify`**: parse the header and the root with bounds checks; check the root and card digests; stream each
  section once and check its digest; recompute the content hash. One sequential pass, any range reader, memory one
  slice. `verify --section <kind>` checks one section. The build record is not verified, as today.
- Reproducibility stays a requirement: the in-RAM and memory-bounded writers emit identical bytes, so the same input
  and flags give the same hash. New test: `convert(build_0x05(x)) == build_0x06(x)` for the same knobs, since
  convert copies payload blocks verbatim (§7).
- This is not the 0x05 self-digest patch. Those digests sat **outside** the content hash so that old readers' `verify`
  would pass. Here every section is inside the one hash; the digests make it a two-level tree, which is what lets a
  recard skip the data.

## 6. Writers

### 6.1 In-RAM build

It already holds every encoded section before writing. It adds: cut the directories into leaves, apply the
root budget, build the root, write front then data in one sequential pass. No extra copy, no extra memory beyond
the leaves (≤ 4 MB on the sample).

### 6.2 Memory-bounded build

The front depends on the directories, which exist only once the data is written, and the full card is derived from
the finished sections (G20). So: write each data section to its own scratch file (the spill directory), keep the
directory entries (Meta: 279 MB raw, of which the 188 MB token table already streams through the text-index spill),
derive the card by streaming the scratch sections (today's recard derivation), then assemble: front, then each
scratch section in data order, deleting each one once copied.

Cost at Meta scale: **one extra sequential read and write of 62.9 GB.** For scale, Meta's build took 5 h 8 min
(18,465 s; its build record), and its recard's content-hash pass read the whole 62.9 GB in 25.6 s (page cache). At
0.4–1 GB/s the copy is 1–3 min each way: under 2% of the build. Disk peak: the output plus the section in flight
(the largest is SOP, 12.0 GB), below the 237 GB of spill the same build already peaked at. A reserved-gap variant
(write data at a guessed front size) would skip the copy but leave dead bytes; not proposed.

### 6.3 Recard

The card region has slack (§3.3). `rete recard` derives the new card (the existing streaming derivation: 434 s on
Meta), then:

1. writes the new card and build record into the free part of the card region; `fsync`;
2. rewrites the 128-byte header (new card offset, length and digest, new build record, new content hash): one
   sector, the commit point; `fsync`.

No data is read for the hash (the root digest is unchanged), nothing in the root moves, and a crash before step 2
leaves the old file intact. If the new pair does not fit the free part, recard copies: new front, then the data
section by section, byte for byte (digests unchanged, so no hashing; Meta: one 62.9 GB copy). There are no dead
bytes outside the region, and the card is always in one place.

### 6.4 Merge

`rete merge` writes the merged sections to scratch and assembles like §6.2 (it already derives the full card from
the merged sections, G21).

## 7. `rete convert` (0x05 → 0x06)

Reads sections, never text, in one pass over the input, memory bounded by the largest directory (Meta: SPO tile
directory + synopsis, 10.7 MB):

| 0x05 | 0x06 |
|---|---|
| header, kinds 1 and 7 (either place) | card and build record into the card region; the record gains a `converted` entry (engine, source content hash) |
| dictionary chunks | copied verbatim; restart tables dropped; id route copied; **keys re-derived as shortest separators** (decompress each chunk once; the dictionary is 7–30% of the measured files) |
| index tiles, tile directories, synopses | tiles copied verbatim; directories and synopses re-cut into leaves |
| quad index (kind 10) | tiles copied, reordered graph-major when graphs are small; directories into leaves; seal dropped |
| named graphs (kind 5) | **transcoded** to the quad index: decode each graph, encode quad tiles with the existing quad writer (bounded) |
| text index (kind 6 or 11) | token table decompressed and cut into leaves; postings copied; scope block into the root |
| label index (kind 8) | blocks copied; level-1 nodes become leaves; the root moves into the root block |
| legacy labels in pyramid-meta | option `--label-index`: build the label index by streaming the label predicates (repyramid's label pass) |
| schema (kind 9), or legacy schema/`query_stats` in pyramid-meta | the stats + schema item |
| pyramid-meta | the community part as the optional pyramid section; its legacy label and stats blocks are not carried |

Cost: one read and one write of the file. Meta: 62.9 GB each way plus one decompression of its 4.5 GB dictionary.

## 8. Reader plan for the migration

- One entry point, `Rete::open_ranged_lazy`, reads 64 KiB and dispatches on byte 4: 0x05 to today's code (seeded
  with that read, without the `HEAD`), 0x06 to the new path. Both build the same in-memory types (dictionary
  sections with chunk loaders, `GraphIndex` with tile loaders, `QuadNamed`, the text and label indexes, the
  schema), so query evaluation does not change.
- The engine change is the two-level directory: `GraphIndex::from_remote_directories` and the dictionary's
  `partition_point` routing take whole directories at open today; they need a directory that routes through the
  root and faults a leaf.
- The CLI, wasm, Python, R and Node clients share rete-core: one implementation. The browser readers' first read
  grows from 1 KiB to 64 KiB.
- Writers write 0x06 only, from the release that reads it. Old clients (≤ 0.4.2) cannot read 0x06, so a published
  dataset is converted only after the playground, MCPB, the Python/JS packages and the HF Space run that release;
  small datasets first. `web/datasets.lock.json` already records each file's `formatVersion`.
- The format byte 0x06 was used once before, by the unmerged experiment `feat/rete-format-0x06` (August 2026:
  SPO/SOP, POS/PSO and OSP/OPS stored as three "paired families" sharing directories; it reported a lazy open going
  from 89 to 25 index range reads). No published file carries it (survey of all 111 catalog URLs: every one is
  0x05), and its header is 1024 bytes, so it fails the 0x06 header-length check (128). Its pairing could still
  halve the index's root entries; it is not part of this design (open question 13).

## 9. Expected gain

Round trips / KiB transferred, cold. 0x06 is simulated from each measured no-cache trace (§ Method in the
measurements file): hot reads served by the first read, payload reads in their measured waves, one extra RT for
leaf blocks past N, leaf prediction, and the better of exact reads and block reads for the payload.

**OpenCitations sample (195 MiB, quad index, text, labels)**

| query | rows | 0x05 no cache | 0x05 cache | 0x06 N=16K | **0x06 N=64K** | 0x06 N=256K |
|---|--:|--:|--:|--:|--:|--:|
| open + first tile (`ASK { ?s ?p ?o }`) | 1 | 53 / 335 | 9 / 1,920 | 3 / 53 | **2 / 95** | 2 / 287 |
| card | 1 | 5 / 65 | 5 / 65 | 1 / 16 | **1 / 64** | 1 / 256 |
| DOI → work + title (G1) | 1 | 56 / 552 | 13 / 2,560 | 6 / 302 | **5 / 312** | 5 / 504 |
| a work's identifiers (IRI lookup) | 2 | 56 / 536 | 13 / 3,712 | 6 / 283 | **5 / 296** | 5 / 488 |
| ORCID → works | 22 | 63 / 1,448 | 18 / 6,132 | 11 / 4,664 | **10 / 4,672** | 10 / 4,864 |
| journal + ISSN (`frbr:partOf+`) | 2 | 122 / 6,846 | 27 / 10,112 | 20 / 8,635 | **19 / 8,645** | 19 / 8,832 |
| label search "journal of" | 1000 | 7 / 124 | 6 / 756 | 4 / 284 | **4 / 332** | 4 / 524 |
| text search, rare word | 1 | 18 / 3,151 | 9 / 3,968 | 4 / 50 | **4 / 95** | 4 / 287 |
| text search, common word | 3589 | 18 / 4,787 | 9 / 5,760 | 4 / 1,691 | **4 / 1,731** | 4 / 1,923 |
| `GRAPH <g>` provenance history | 2 | 62 / 650 | 20 / 3,828 | 9 / 210 | **8 / 227** | 8 / 419 |
| `GRAPH ?g { ?s ?p ?o } LIMIT 10` | 10 | 62 / 644 | 14 / 3,200 | 8 / 180 | **7 / 199** | 7 / 391 |
| `GRAPH ?g` + REGEX over 50 rows | 50 | 1221 / 32,913 | 160 / 37,120 | 150 / 35,363 | **149 / 35,382** | 149 / 35,574 |
| class histogram | 29 | 68 / 969 | 17 / 3,328 | 13 / 1,841 | **12 / 1,856** | 12 / 2,048 |

The last two rows are payload-bound: 0x06 removes their open, not their scan. Graph-major quad clustering (§3.5)
would take 2 more RT off the `GRAPH <g>` rows; it is not in the simulation.

**Catalog: 5 published files + 2 of them rebuilt with labels and schema, median over the 7**

| query | 0x05 no cache | 0x05 cache | 0x06 N=16K | **0x06 N=64K** | 0x06 N=256K |
|---|--:|--:|--:|--:|--:|
| open + first tile | 52 / 139 | 10 / 896 | 3 / 66 | **2 / 114** | 2 / 306 |
| card | 4 / 53 | 4 / 53 | 1 / 16 | **1 / 64** | 1 / 256 |
| `<iri> ?p ?o` | 56 / 329 | 13 / 1,536 | 6 / 888 | **6 / 896** | 5 / 1,088 |
| `?s ?p <iri> . ?s ?p2 ?o` | 53 / 191 | 10 / 1,152 | 3 / 220 | **3 / 256** | 2 / 448 |
| label prefix search | 12 / 251 | 5 / 512 | 2 / 27 | **2 / 75** | 2 / 267 |
| text search | 14 / 1,381 | 8 / 1,728 | 4 / 61 | **4 / 109** | 4 / 296 |
| class histogram | 66 / 249 | 13 / 1,088 | 7 / 492 | **7 / 512** | 7 / 704 |

Gate fixtures (1.9–20 KB): every query is 1 RT at 64 KiB (the whole file), against 52–63 RT without the cache
for any query that opens the index (2–3 for a card read) and 2–3 RT with it.

**OpenCitations Meta (59 GiB), projected.** 0x05, measured: the open alone is 131 RT / 84.6 MiB (no cache) or
56 RT / 66.3 MiB (cache); a text search then reads the 179 MiB token table on top. 0x06: the sample's query
shapes replayed against Meta's 0x06 front (root 36.9 KiB, card 10.1 KiB, stats 1.9 KiB, 251 MB of leaves).

| query | 0x06 N=16K | **0x06 N=64K** | 0x06 N=256K |
|---|--:|--:|--:|
| open + first tile | 4 / 84 | **3 / 111** | 3 / 303 |
| card | 3 / 47 | **1 / 64** | 1 / 256 |
| DOI → work + title (G1) | 7 / 396 | **6 / 423** | 6 / 599 |
| a work's identifiers | 7 / 444 | **6 / 471** | 6 / 663 |
| ORCID → works | 12 / 5,157 | **11 / 5,182** | 11 / 5,358 |
| label search | 5 / 389 | **4 / 416** | 4 / 608 |
| text search, rare word | 5 / 324 | **4 / 351** | 4 / 543 |
| class histogram | 14 / 1,988 | **12 / 2,015** | 12 / 2,191 |

Local reads gain the same way in work rather than latency: a 0x05 lazy open decodes and keeps every directory
(the fixed heap the code documents: 88 MB on openaire-relation, 224 MB on epfl-infoscience, 61 MB of directory
reads on Meta); a 0x06 open reads 64 KiB and keeps leaves in a bounded cache. In a warm session on a small file the
two formats cost the same per query (both pay only payload); the gain is the cold open, and on big files the
directories that are never loaded.

Size: files shrink by the restart tables (0.4–1.1% of each measured file) and by compressed, re-keyed
directories.

## 10. What 0x06 drops

- Kinds 8, 9 and 11 outside the content hash with self-digests: every section is inside the one hash.
- The quad-index seal inside the index container, and header flag bit 3's refuse-old-readers meaning.
- "New kinds go at the end": sections are placed by the hot/cold order, not by history.
- The card in two places (front or end), and dead bytes after an in-place recard.
- The 1024-byte header with its 40-entry directory: a 128-byte header and an unbounded section table.
- Per-graph named graphs (kind 5): the quad index is the only named-graph layout.
- Length-prefixed containers, the restart tables, tile directories at section heads, synopsis trailers, and
  directory reads by geometric probing.
- The text index's kind 6 / kind 11 split: one text section, its scope in the root.
- The legacy pyramid-meta "combined" layout (labels, `query_stats` and schema inside the pyramid).
- Whole-term chunk keys (pre-#198).
- The synopsis flag (bit 1): synopses are always present.

## 11. Open questions

1. **First read 64 KiB** as the default (recommended; §4). Should the catalog record each dataset's front length
   so the playground's first read covers the whole front?
2. **The card in the hash** (as in 0x05: a new card is a new identity). Keep?
3. **Two-level hash with stored section digests** (§5), so recard and convert never re-hash data. Accept the 16 B
   per section?
4. **Card slack**: `2 × (card + record) + 8 KiB`, copying recard when exceeded. Acceptable, or a fixed minimum?
5. **Bounded build: copy pass** (+1 read and write of the file, under 2% of Meta's build) rather than a
   reserved gap with dead bytes. Agree?
6. **Graph-major quad clustering** when graphs are small (−2 RT on `GRAPH <g>` on the sample, a strided scan of one
   order across all graphs): default on for OC-like data?
7. **Leaf prediction** in the client is the main reader complexity of big files. Needed for the first release, or
   only once files over ~200 MB are converted (below that every leaf is in the first read or one RT away)?
8. **The community pyramid**: keep as an optional cold section, or drop it in 0x06 (only `summary`/overview reads
   it; the large builds already use `--no-pyramid`)?
9. **Convert and legacy labels**: build a label index during convert by default for files that only have the
   pyramid-meta label block?
10. **`GRAPH ?g { <s> … }`** still visits every graph (G4): add an S-leading order with the graph trailing, or a
    subject → graphs posting, in 0x06? It is an index question, not a layout one; it can come later as a new section.
11. **When to drop 0x05 reading**: when the catalog is fully converted, or at 1.0?
12. **Persistent client leaf cache** (IndexedDB / disk, keyed by content hash): in scope of the 0x06 reader work?
13. **Paired permutation families** (the August experiment): share leaf directories between SPO/SOP, POS/PSO and
    OSP/OPS? It saves root and leaf bytes on big files; it changes nothing on small ones, where every leaf is in
    the first read.
