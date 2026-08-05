#!/usr/bin/env python3
"""Embed a PMTiles vector-tile archive as a SECTION INSIDE a .rete file — geo-LOD
"option C": one file = graph + tiles, both HTTP-range-queryable.

The PMTiles bytes become a new section-directory entry (kind 7) in the .rete's 1 KB
header, placed just before the trailing footer MAGIC. Every existing section keeps its
byte offset, so the graph (SPARQL) opens completely unchanged. The content hash
(computed over the graph's payload sections, not the header) is untouched, so the
graph's identity is unchanged. The browser then range-reads the tiles from the SAME
.rete URL at the section's offset.

**Kind 7 is no longer free.** When this script was written 7 was the next unused
`SectionKind` after `TextIndex(6)`, and the reader ignored it as `Unknown(7)`. Since
the build record landed (crates/rete-core/src/header.rs), **7 is `BuildInfo`** — so a
`.rete` built by a current `rete` with a card ALREADY has a kind-7 section, and
appending the tiles under the same kind writes a second one. Nothing merges them:
`rete card` takes the last and warns `unreadable build-info section`, while the
playground's tile reader (`reteTilesSection` in web/playground-src/app.js) takes the
FIRST and would try to parse the 2 KB build record as a PMTiles archive.

So this script now refuses that collision instead of producing a file whose tiles the
browser cannot find. `--drop-build-info` resolves it the only way that keeps today's
published layout working: the build record's directory entry is removed, leaving the
tiles as the sole kind-7 section. That costs the file its build record (provenance and
measured query costs) — the real fix is to move tiles to a kind of their own and teach
the reader, which is engine + web work, not a flag here.

Usage: python scripts/embed_tiles.py <in.rete> <tiles.pmtiles> <out.rete>
                                     [--drop-build-info]
"""
import struct, sys

MAGIC = b"RETE"
HEADER_LEN = 1024
SECTION_DIR_OFFSET = 64
SECTION_ENTRY_LEN = 24
MAX_SECTIONS = (HEADER_LEN - SECTION_DIR_OFFSET) // SECTION_ENTRY_LEN  # 40
TILES_KIND = 7  # collides with SectionKind::BuildInfo — see the module docstring


def main(in_path, tiles_path, out_path, drop_build_info=False):
    b = bytearray(open(in_path, "rb").read())
    tiles = open(tiles_path, "rb").read()
    if bytes(b[0:4]) != MAGIC:
        sys.exit("not a .rete file (bad magic)")
    sc = struct.unpack_from("<H", b, 44)[0]
    if sc >= MAX_SECTIONS:
        sys.exit("section directory is full")
    if bytes(b[-4:]) != MAGIC:
        sys.exit("expected a trailing MAGIC footer — refusing to guess the layout")

    # An existing kind-7 entry is the build record (nothing else writes that kind
    # into a freshly built file). Two entries of one kind is not a layout any
    # reader here resolves, so it is an error unless the caller says which to keep.
    existing = [i for i in range(sc)
                if struct.unpack_from("<H", b, SECTION_DIR_OFFSET + i * SECTION_ENTRY_LEN)[0]
                == TILES_KIND]
    if existing:
        if not drop_build_info:
            sys.exit(
                f"{in_path} already has {len(existing)} section(s) of kind {TILES_KIND} "
                f"(BuildInfo). Embedding the tiles under the same kind gives the "
                f"playground's tile reader the WRONG one — it takes the first match. "
                f"Re-run with --drop-build-info to remove the build record's entry and "
                f"leave the tiles as the only kind-7 section (see the module docstring)."
            )
        # Compact the directory in place: drop those entries, keep the rest in order.
        kept = [bytes(b[SECTION_DIR_OFFSET + i * SECTION_ENTRY_LEN:
                        SECTION_DIR_OFFSET + (i + 1) * SECTION_ENTRY_LEN])
                for i in range(sc) if i not in set(existing)]
        for i, entry in enumerate(kept):
            b[SECTION_DIR_OFFSET + i * SECTION_ENTRY_LEN:
              SECTION_DIR_OFFSET + (i + 1) * SECTION_ENTRY_LEN] = entry
        # Blank the vacated slots so a stale entry cannot be read back by index.
        for i in range(len(kept), sc):
            b[SECTION_DIR_OFFSET + i * SECTION_ENTRY_LEN:
              SECTION_DIR_OFFSET + (i + 1) * SECTION_ENTRY_LEN] = bytes(SECTION_ENTRY_LEN)
        sc = len(kept)
        struct.pack_into("<H", b, 44, sc)
        print(f"dropped {len(existing)} build-info entr(y|ies) to free kind {TILES_KIND}")
    body_end = len(b) - 4              # the footer starts here → tiles go here
    footer = bytes(b[body_end:])
    tiles_offset, tiles_len = body_end, len(tiles)

    # write the new 24-byte directory entry at the next free slot
    p = SECTION_DIR_OFFSET + sc * SECTION_ENTRY_LEN
    struct.pack_into("<H", b, p, TILES_KIND)       # [p..p+2)  kind
    struct.pack_into("<H", b, p + 2, 0)            # [p+2..p+4) flags
    struct.pack_into("<I", b, p + 4, 0)            # [p+4..p+8) reserved
    struct.pack_into("<Q", b, p + 8, tiles_offset) # [p+8..p+16) offset
    struct.pack_into("<Q", b, p + 16, tiles_len)   # [p+16..p+24) length
    struct.pack_into("<H", b, 44, sc + 1)          # section_count++

    with open(out_path, "wb") as f:
        f.write(bytes(b[:HEADER_LEN]))             # rewritten header
        f.write(bytes(b[HEADER_LEN:body_end]))     # original graph sections (verbatim)
        f.write(tiles)                             # the embedded PMTiles section
        f.write(footer)                            # footer MAGIC back at the new end
    total = HEADER_LEN + (body_end - HEADER_LEN) + tiles_len + 4
    print("wrote %s: graph %d B + tiles section (kind %d) %d B @ offset %d + footer = %d B total"
          % (out_path, body_end, TILES_KIND, tiles_len, tiles_offset, total))


if __name__ == "__main__":
    args = sys.argv[1:]
    drop = "--drop-build-info" in args
    args = [a for a in args if a != "--drop-build-info"]
    if len(args) != 3:
        sys.exit("usage: embed_tiles.py <in.rete> <tiles.pmtiles> <out.rete> "
                 "[--drop-build-info]")
    main(args[0], args[1], args[2], drop)
