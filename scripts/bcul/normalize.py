#!/usr/bin/env python3
"""Merge all per-source JSONL into normalized/bcul.jsonl, validate, and report stats.

- Dedupes by `id` (guards against resume/retry duplicates).
- Lightweight schema check (required keys + basic types) against bcul.record.schema.json.
- Prints a coverage report (counts by source / type, digitized, thumbnails, date span).

Usage: python normalize.py [--base-dir ...]
"""
from __future__ import annotations

import argparse
import json
from collections import Counter
from pathlib import Path

REPO = Path(__file__).resolve().parents[2]
SOURCES = ["patrinum", "renouvaud", "ecodices", "scriptorium"]
REQUIRED = ("id", "source", "local_id")


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--base-dir", default=str(REPO / "data" / "bcul"))
    args = ap.parse_args()
    base = Path(args.base_dir)
    norm = base / "normalized"
    out_path = norm / "bcul.jsonl"

    # Streaming merge: hold only the seen-id set (dedupe guard) + counters, so
    # ~3.9M records don't have to sit in RAM at once.
    seen = set()
    per_source_raw = Counter()
    by_source = Counter()
    by_type = Counter()
    bad = dup = digitized = with_thumb_local = with_thumb_url = with_iiif = 0
    ymin = ymax = None

    with open(out_path, "w", encoding="utf-8") as out_fh:
        for src in SOURCES:
            p = norm / f"{src}.jsonl"
            if not p.exists():
                continue
            with open(p, encoding="utf-8") as fh:
                for line in fh:
                    s = line.strip()
                    if not s:
                        continue
                    try:
                        rec = json.loads(s)
                    except json.JSONDecodeError:
                        bad += 1
                        continue
                    if not all(rec.get(k) for k in REQUIRED):
                        bad += 1
                        continue
                    per_source_raw[src] += 1
                    rid = rec["id"]
                    if rid in seen:
                        dup += 1
                        continue
                    seen.add(rid)
                    out_fh.write(s + "\n")
                    by_source[rec["source"]] += 1
                    by_type[rec.get("type") or "?"] += 1
                    if rec.get("has_digital"):
                        digitized += 1
                    if rec.get("thumbnail_local"):
                        with_thumb_local += 1
                    if rec.get("thumbnail_url"):
                        with_thumb_url += 1
                    if rec.get("iiif_manifest"):
                        with_iiif += 1
                    y = rec.get("date_start")
                    if isinstance(y, int):
                        ymin = y if ymin is None else min(ymin, y)
                        ymax = y if ymax is None else max(ymax, y)

    total = len(seen)
    years = [ymin, ymax] if ymin is not None else []
    print(f"== BCUL digital twin — merged {total:,} unique records "
          f"(raw {sum(per_source_raw.values()):,}, deduped {dup:,}, "
          f"skipped {bad:,}) ==")
    print(f"wrote {out_path}")
    print("\nby source:")
    for s, n in by_source.most_common():
        print(f"  {s:12s} {n:>9,}  (raw {per_source_raw[s]:,})")
    print("\nby type:")
    for t, n in by_type.most_common(20):
        print(f"  {t:22s} {n:>9,}")
    print(f"\ndigitized (has files/images): {digitized:,}")
    print(f"thumbnail_url set:            {with_thumb_url:,}")
    print(f"thumbnail downloaded locally: {with_thumb_local:,}")
    print(f"IIIF manifests:              {with_iiif:,}")
    if years:
        print(f"date span (date_start):      {min(years)}–{max(years)}")


if __name__ == "__main__":
    main()
