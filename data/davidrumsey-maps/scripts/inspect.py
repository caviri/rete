#!/usr/bin/env python3
"""Profile the flattened metadata (raw/derived/rumsey_items.jsonl.gz):
field fill rates, type/city/date distributions, image dimensions, asset
coverage. Paste the output into README.md and reuse when modelling the graph.

    python inspect.py [--items PATH]

Stdlib only — runs in a plain python:3.12-slim container.
"""
from __future__ import annotations

import gzip
import json
import re
import sys
from collections import Counter
from pathlib import Path

YEAR_RE = re.compile(r"\b(1[4-9]\d\d|20[0-2]\d)\b")


def main() -> None:
    items = Path("data/davidrumsey-maps/raw/derived/rumsey_items.jsonl.gz")
    if len(sys.argv) == 3 and sys.argv[1] == "--items":
        items = Path(sys.argv[2])

    n = 0
    fill: Counter[str] = Counter()
    types: Counter[str] = Counter()
    cities: Counter[str] = Counter()
    authors: Counter[str] = Counter()
    decades: Counter[str] = Counter()
    dims: list[int] = []
    n_jp2 = n_iiif = n_multi = 0

    for line in gzip.open(items, "rt", encoding="utf-8"):
        rec = json.loads(line)
        n += 1
        for k, vs in rec.get("fields", {}).items():
            if any(str(v).strip() for v in vs):
                fill[k] += 1
        f = rec.get("fields", {})
        for v in f.get("Type", []) or f.get("Pub Type", []):
            types[v] += 1
        for v in f.get("City", []):
            cities[v] += 1
        for v in f.get("Authors", []):
            authors[v] += 1
        for v in f.get("Date", []) or f.get("Pub Date", []):
            m = YEAR_RE.search(str(v))
            if m:
                decades[m.group(1)[:3] + "0s"] += 1
        w, h = rec.get("width"), rec.get("height")
        if isinstance(w, int) and isinstance(h, int):
            dims.append(max(w, h))
        n_jp2 += bool(rec.get("jp2_url"))
        n_iiif += bool(rec.get("iiif_image"))
        n_multi += rec.get("canvases", 1) > 1

    def top(c: Counter, k: int = 15) -> str:
        return "\n".join(f"    {v:>7,}  {name[:70]}" for name, v in c.most_common(k))

    print(f"items: {n:,}")
    print(f"with IIIF image service: {n_iiif:,} | with JP2 master URL: {n_jp2:,} | multi-canvas: {n_multi:,}")
    if dims:
        dims.sort()
        pct = lambda p: dims[min(len(dims) - 1, int(p / 100 * len(dims)))]  # noqa: E731
        print(f"max-side px: p10={pct(10):,} p50={pct(50):,} p90={pct(90):,} p99={pct(99):,} max={dims[-1]:,}")
    print(f"\nfield fill rates ({len(fill)} distinct fields):")
    for k, v in fill.most_common():
        print(f"    {v:>7,}  {v * 100 // max(n, 1):>3}%  {k}")
    print("\ntop types:");   print(top(types))
    print("\ntop cities:");  print(top(cities))
    print("\ntop authors:"); print(top(authors))
    print("\nitems per decade:")
    for d in sorted(decades):
        print(f"    {decades[d]:>7,}  {d}")


if __name__ == "__main__":
    main()
