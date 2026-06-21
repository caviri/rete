#!/usr/bin/env python3
"""Snapshot each plaza dataset's embedded card (and report which files lack one).

The plaza reads cards live, so this is optional. It exists for two reasons:

  1. Freeze a fully-static snapshot (`cards.snapshot.json`) so the site can be
     hosted with *zero* range requests, fully offline.
  2. Tell you which datasets ship no embedded card, i.e. which `.rete` files to
     rebuild with `rete build --card` to light up their full profile + query
     library in the gallery.

Pure stdlib. Reads local paths from disk and remote URLs over two HTTP range
requests (header, then the card bytes) — mirroring js/rete-card.js exactly.
"""
import json
import os
import struct
import sys
import urllib.request

HERE = os.path.dirname(os.path.abspath(__file__))
PLAZA = os.path.normpath(os.path.join(HERE, "..", "plaza.json"))
HEADER_LEN = 1024  # enough for both the legacy 128B header and the v3/v4 section directory


def _range(url, start, end):
    req = urllib.request.Request(url, headers={"Range": f"bytes={start}-{end}"})
    with urllib.request.urlopen(req, timeout=60) as r:
        data = r.read()
        # 200 (range ignored) -> slice; 206 -> already the slice.
        return data[start : end + 1] if r.status == 200 else data


def _read(src, start, end):
    """Read [start, end] inclusive from a local path or URL."""
    if src.startswith("http://") or src.startswith("https://"):
        return _range(src, start, end)
    path = src if os.path.isabs(src) else os.path.normpath(os.path.join(HERE, "..", src))
    with open(path, "rb") as f:
        f.seek(start)
        return f.read(end - start + 1)


def read_card(src):
    head = _read(src, 0, HEADER_LEN - 1)
    if head[0:4] != b"RETE":
        raise ValueError("not a .rete file (bad magic)")
    version = head[4]
    if version >= 3:
        # 1024-byte core (64B) + 24B section-directory entries from offset 64.
        content_hash = head[8:24].hex()
        quad_count = struct.unpack_from("<Q", head, 24)[0]
        term_count = struct.unpack_from("<Q", head, 32)[0]
        section_count = struct.unpack_from("<H", head, 44)[0]
        meta_off = meta_len = 0
        for i in range(section_count):
            p = 64 + i * 24
            kind = struct.unpack_from("<H", head, p)[0]
            if kind == 1:  # Metadata
                meta_off = struct.unpack_from("<Q", head, p + 8)[0]
                meta_len = struct.unpack_from("<Q", head, p + 16)[0]
    else:
        meta_off = struct.unpack_from("<Q", head, 8)[0]
        meta_len = struct.unpack_from("<Q", head, 16)[0]
        quad_count = struct.unpack_from("<Q", head, 76)[0]
        term_count = struct.unpack_from("<Q", head, 84)[0]
        content_hash = head[92:108].hex()
    header = {
        "version": version,
        "quadCount": quad_count,
        "termCount": term_count,
        "contentHash": content_hash,
    }
    card = None
    if meta_len > 0:
        raw = _read(src, meta_off, meta_off + meta_len - 1)
        card = json.loads(raw[:meta_len].decode("utf-8"))
    return header, card


def main():
    with open(PLAZA, encoding="utf-8") as f:
        manifest = json.load(f)

    snapshot, missing = {}, []
    for d in manifest["datasets"]:
        key, src = d["key"], d["rete"]
        try:
            header, card = read_card(src)
        except Exception as e:  # noqa: BLE001
            print(f"  ! {key:20} {e}", file=sys.stderr)
            continue
        snapshot[key] = {"header": header, "card": card}
        tag = "card" if card else "header-only"
        print(f"  · {key:20} v{header['version']}  {header['quadCount']:>10} quads  [{tag}]")
        if not card:
            missing.append(key)

    out = os.path.normpath(os.path.join(HERE, "..", "cards.snapshot.json"))
    with open(out, "w", encoding="utf-8") as f:
        json.dump(snapshot, f, ensure_ascii=False, indent=2)
    print(f"\nwrote {len(snapshot)} entries to {out}")
    if missing:
        print(
            "\nno embedded card (rebuild with `rete build --card` to enrich): "
            + ", ".join(missing)
        )


if __name__ == "__main__":
    main()
