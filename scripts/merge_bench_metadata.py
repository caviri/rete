#!/usr/bin/env python3
"""Merge the curated `metadata` block from an existing benchmark JSON into a fresh
`rete-bench --json` report.

`rete-bench --json` emits only measured data; the prose details the doc renderer
uses (the `dataset` note, run date, oxigraph version) live in a `metadata` block
that a re-run would otherwise drop. This copies that block over so re-benchmarking
doesn't lose it.

Usage:
  merge_bench_metadata.py <fresh.json> <existing.json> [--date YYYY-MM-DD]

Writes the merged report back to <existing.json>.
"""

from __future__ import annotations

import argparse
import json
from pathlib import Path


def main() -> None:
    ap = argparse.ArgumentParser()
    ap.add_argument("fresh", type=Path, help="a fresh `rete-bench --json` report")
    ap.add_argument("existing", type=Path, help="the doc's JSON (its metadata is preserved)")
    ap.add_argument("--date", default=None, help="set metadata.latest_run")
    args = ap.parse_args()

    new = json.loads(args.fresh.read_text(encoding="utf-8"))
    try:
        old = json.loads(args.existing.read_text(encoding="utf-8"))
    except FileNotFoundError:
        old = {}
    meta = old.get("metadata", {})
    if not isinstance(meta, dict):
        meta = {}
    if args.date:
        meta["latest_run"] = args.date
    new["metadata"] = meta
    args.existing.write_text(json.dumps(new, indent=2) + "\n", encoding="utf-8")
    print(f"merged: {len(new.get('queries', []))} queries · metadata keys {sorted(meta.keys())}")


if __name__ == "__main__":
    main()
