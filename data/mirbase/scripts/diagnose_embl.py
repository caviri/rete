#!/usr/bin/env python3
"""Show why a record failed to reconstruct from structure (dev aid).

    bash data/mirbase/scripts/py.sh diagnose_embl.py
"""
from __future__ import annotations

from pathlib import Path

import pyarrow.parquet as pq

from embl_to_parquet import parse_record, split_records
from parquet_to_embl import serialize_record

BASE = Path(__file__).resolve().parent.parent


def main() -> None:
    recs = pq.read_table(BASE / "parquet" / "embl_records.parquet")
    raw = recs.column("raw_block").to_pylist()
    names = recs.column("name").to_pylist()
    bad = [i for i, r in enumerate(raw) if r]
    print(f"{len(bad)} records did not reconstruct\n")
    print("name lengths of failures:",
          sorted({len(names[i]) for i in bad}))

    shown = 0
    for i in bad:
        block = raw[i]
        rec, f, rf, x = parse_record(block, i)
        rec["raw_block"] = ""          # force structural rebuild
        got = serialize_record(rec, f, rf, x)
        if got == block:
            continue
        a, b = block.split("\n"), got.split("\n")
        print(f"\n=== {names[i]} (record {i}) ===")
        for ln in range(max(len(a), len(b))):
            x1 = a[ln] if ln < len(a) else "<none>"
            x2 = b[ln] if ln < len(b) else "<none>"
            if x1 != x2:
                print(f"  line {ln}\n    orig: {x1!r}\n    got : {x2!r}")
                break
        shown += 1
        if shown >= 6:
            break


if __name__ == "__main__":
    main()
