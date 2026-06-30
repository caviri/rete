#!/usr/bin/env python3
"""Embed a PMTiles vector-tile archive as a SECTION INSIDE a .rete file — geo-LOD
"option C": one file = graph + tiles, both HTTP-range-queryable.

The PMTiles bytes become a new section-directory entry (kind 7) in the .rete's 1 KB
header, placed just before the trailing footer MAGIC. Every existing section keeps its
byte offset, so the graph (SPARQL) opens completely unchanged — the .rete reader treats
kind 7 as an unknown section and ignores it (Rete::open reads only the known kinds; the
entry round-trips as Header::extra_sections / Header::section(SectionKind::Unknown(7))).
The content hash (computed over the graph's payload sections, not the header) is
untouched, so the graph's identity is unchanged. The browser then range-reads the tiles
from the SAME .rete URL at the section's offset.

Usage: python scripts/embed_tiles.py <in.rete> <tiles.pmtiles> <out.rete>
"""
import struct, sys

MAGIC = b"RETE"
HEADER_LEN = 1024
SECTION_DIR_OFFSET = 64
SECTION_ENTRY_LEN = 24
MAX_SECTIONS = (HEADER_LEN - SECTION_DIR_OFFSET) // SECTION_ENTRY_LEN  # 40
TILES_KIND = 7  # next free SectionKind after TextIndex(6)


def main(in_path, tiles_path, out_path):
    b = bytearray(open(in_path, "rb").read())
    tiles = open(tiles_path, "rb").read()
    if bytes(b[0:4]) != MAGIC:
        sys.exit("not a .rete file (bad magic)")
    sc = struct.unpack_from("<H", b, 44)[0]
    if sc >= MAX_SECTIONS:
        sys.exit("section directory is full")
    if bytes(b[-4:]) != MAGIC:
        sys.exit("expected a trailing MAGIC footer — refusing to guess the layout")
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
    if len(sys.argv) != 4:
        sys.exit("usage: embed_tiles.py <in.rete> <tiles.pmtiles> <out.rete>")
    main(sys.argv[1], sys.argv[2], sys.argv[3])
