"""0x06 sizing model: turn a 0x05 layout map into 0x06 directory units (root entries + leaf blocks), with sizes
measured by compressing the real 0x05 directory bytes (zstd level 9, what a writer would emit).

Each directory unit of 0x06:
  dict.s0..s3   chunk directory of one dictionary section (separator keys + first-run deltas + lengths)
  index.<P>     tile directory + synopsis of one permutation
  quad.graphs   graph directory (quad index P0 + graph-directory blocks)
  quad.G+<P>    quad tile directory of one graph-leading order
  labels        label-index directory nodes (the 0x05 root moves into the 0x06 root block)
  text          text-index token table (already zstd in 0x05: kept as is)
Root block = fixed header + section table + one entry per leaf block of every unit + schema/stats.

usage: as a module (units(), root_and_leaves()); `python hot06.py LAYOUT.json FILE` prints the table.
"""
import json
import math
import sys

import zstandard

HEADER06 = 256          # fixed header (magic, version, flags, counts, hash, offsets of root/card/leaves/data)
SECTION_ENTRY = 24      # one entry of the section table (kind, flags, offset, length)
ROOT_ENTRY = {"dict": 14, "index": 7, "quad": 9, "labels": 14, "text": 12}  # bytes per leaf entry in the root
LEAF_ORDER = ["dict.s3", "dict.s0", "dict.s1", "dict.s2", "index.SPO", "index.POS", "index.OSP", "index.SOP",
              "index.PSO", "index.OPS", "quad.graphs", "quad.G+SPO", "quad.G+POS", "quad.G+OSP", "quad.G+SOP",
              "quad.G+PSO", "quad.G+OPS", "labels", "text"]
ZC = zstandard.ZstdCompressor(level=9)


def unit_of(sec, part):
    """The 0x06 directory unit that locates (or is) a 0x05 region; None for the root block / card / data-less."""
    p = part.split(".")
    if sec == "dict" and part.startswith("s"):
        return f"dict.{p[0]}"
    if sec == "index" and p[0] in ("SPO", "POS", "OSP", "SOP", "PSO", "OPS"):
        return f"index.{p[0]}"
    if sec == "quad":
        if part.startswith("G+"):
            return f"quad.{p[0]}"
        if part in ("graph-dir-blocks", "P0.graph-dir-index"):
            return "quad.graphs"
    if sec == "labels" and part in ("dir-nodes", "data-blocks", "preamble+root"):
        return "labels"
    if sec == "text" and part in ("token-table", "postings"):
        return "text"
    return None


def units(layout, path=None, ratios=None):
    """{unit: {"raw": 0x05 bytes, "z": 0x06 compressed bytes, "data": (start, end) of the data it locates}}."""
    U = {}
    f = open(path, "rb") if path else None
    for a, b, sec, part, cls in layout["regions"]:
        u = unit_of(sec, part)
        if u is None:
            continue
        d = U.setdefault(u, {"raw": 0, "z": 0, "data": [None, None], "kind": u.split(".")[0]})
        if cls == "dir" or (sec == "quad" and cls == "root" and part.startswith("P0")) or (
                sec == "labels" and part == "preamble+root" and layout["stats"].get("labels", {}).get("levels") == 0):
            d["raw"] += b - a
            if sec in ("text", "labels"):
                d["z"] += b - a  # already compressed in 0x05
            elif f is not None:
                f.seek(a)
                d["z"] += len(ZC.compress(f.read(b - a)))
            else:
                d["z"] += int((b - a) * ratios.get(u.split(".")[0], 0.6))
        elif cls == "cold":
            lo, hi = d["data"]
            d["data"] = [a if lo is None else min(lo, a), b if hi is None else max(hi, b)]
    return U


def fixed_root(layout, path=None):
    """Bytes the 0x06 root block carries besides leaf entries: header, section table, planner statistics
    (~12 B per predicate: count, distinct subjects/objects, max fan-out). The schema is a front unit of its own."""
    n = sum(1 for k, o, l in layout["sections"] if l)
    preds = 0
    for s in layout.get("stats", {}).get("dict", {}).get("sections", [])[3:4]:
        preds = s["terms"]
    return HEADER06 + SECTION_ENTRY * (n + 2) + 12 * preds


def front_item(layout, sec_name, path=None, ratio=0.3):
    """Compressed size of a whole-section front item (card, schema)."""
    for a, b, sec, part, cls in layout["regions"]:
        if sec == sec_name:
            if path:
                with open(path, "rb") as f:
                    f.seek(a)
                    return len(ZC.compress(f.read(b - a)))
            return int((b - a) * ratio)
    return 0


def card_z(layout, path=None, ratio=0.25):
    for a, b, sec, part, cls in layout["regions"]:
        if sec == "card":
            if path:
                with open(path, "rb") as f:
                    f.seek(a)
                    return len(ZC.compress(f.read(b - a)))
            return int((b - a) * ratio)
    return 0


def plan(layout, U, B, path=None, card_first=True, root_max=40 << 10, card=None, schema=None):
    """Lay out the 0x06 front: root block, card, schema, leaf blocks in LEAF_ORDER. Every unit starts with leaves of
    B bytes; while the root exceeds root_max, the unit with the most root bytes doubles its leaf size (the writer's
    rule, as PMTiles writers grow leaves until the root fits). Returns sizes and offsets of everything."""
    Bu = {u: B for u in LEAF_ORDER}

    def entries(u):
        return max(1, math.ceil(U[u]["z"] / Bu[u])) * ROOT_ENTRY[U[u]["kind"]]

    live = [u for u in LEAF_ORDER if u in U and U[u]["z"] > 0]
    fixed = fixed_root(layout, path)
    while root_max and fixed + sum(entries(u) for u in live) > root_max:
        big = max(live, key=entries)
        if entries(big) <= ROOT_ENTRY[U[big]["kind"]]:
            break
        Bu[big] *= 2
    root = fixed
    P = {}
    for u in live:
        n = max(1, math.ceil(U[u]["z"] / Bu[u]))
        root += n * ROOT_ENTRY[U[u]["kind"]]
        P[u] = {"leaves": n, "leaf": U[u]["z"] / n, "z": U[u]["z"], "B": Bu[u]}
    if card is None:
        card = card_z(layout, path)
    if schema is None:
        schema = front_item(layout, "schema", path)
    pos = root
    card_off = None
    if card_first:
        card_off, pos = pos, pos + card
    schema_off, pos = pos, pos + schema
    for u in LEAF_ORDER:
        if u in P:
            P[u]["start"] = pos
            pos += P[u]["z"]
    if not card_first:
        card_off = pos
        pos += card
    return {"root": root, "card": card, "card_off": card_off, "schema": schema, "schema_off": schema_off,
            "units": P, "hot_end": pos}


def main():
    L = json.load(open(sys.argv[1]))
    path = sys.argv[2] if len(sys.argv) > 2 else None
    U = units(L, path)
    print(f"{L['source']}  size {L['size']:,}")
    tot_raw = sum(u["raw"] for u in U.values())
    tot_z = sum(u["z"] for u in U.values())
    for k in LEAF_ORDER:
        if k in U:
            print(f"  {k:12} raw {U[k]['raw']:>12,}  z {U[k]['z']:>12,}")
    print(f"  {'all':12} raw {tot_raw:>12,}  z {tot_z:>12,}   fixed root {fixed_root(L, path):,}  card z {card_z(L, path):,}")
    for B in (4096, 16384, 65536):
        p = plan(L, U, B, path)
        print(f"  B={B >> 10:>2} KiB: root {p['root']:>8,}  +card {p['card']:>7,}  +schema {p['schema']:>6,}  hot end {p['hot_end']:>12,}")


if __name__ == "__main__":
    main()
