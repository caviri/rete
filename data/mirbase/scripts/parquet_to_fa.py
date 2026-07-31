#!/usr/bin/env python3
"""Parquet -> miRBase FASTA (the exact inverse of fa_to_parquet.py).

Rebuilds the header as `>{name} {accession} {description}` and re-wraps the
sequence at the width recorded when the Parquet was written, so the output is
byte-identical to the shipped .fa.

    bash data/mirbase/scripts/py.sh parquet_to_fa.py [outdir] [parquetdir]

`parquetdir` defaults to data/mirbase/parquet. It is an argument so that a test
can read a DIFFERENT set of tables (e.g. ones derived back out of the .rete)
without moving the real directory out of the way.
"""
from __future__ import annotations

import sys
from pathlib import Path

import pyarrow.parquet as pq

BASE = Path(__file__).resolve().parent.parent
DEFAULT_OUT = BASE / "roundtrip"

NAMES = {
    "hairpin": "hairpin.fa",
    "hairpin_high_conf": "hairpin_high_conf.fa",
    "mature": "mature.fa",
    "mature_high_conf": "mature_high_conf.fa",
}


def wrap_seq(seq: str, width: int) -> list[str]:
    if width <= 0:
        return [seq]
    return [seq[i:i + width] for i in range(0, len(seq), width)] or [""]


def main() -> None:
    out = Path(sys.argv[1]) if len(sys.argv) > 1 else DEFAULT_OUT
    parq = Path(sys.argv[2]) if len(sys.argv) > 2 else BASE / "parquet"
    out.mkdir(parents=True, exist_ok=True)

    for key, fname in NAMES.items():
        src = parq / f"fasta_{key}.parquet"
        if not src.exists():
            print(f"!! missing {src}", file=sys.stderr)
            continue
        tbl = pq.read_table(src).sort_by("ordinal")
        cols = {c: tbl.column(c).to_pylist() for c in
                ("name", "accession", "description", "sequence", "wrap")}

        lines: list[str] = []
        for i in range(tbl.num_rows):
            head = f">{cols['name'][i]}"
            if cols["accession"][i]:
                head += f" {cols['accession'][i]}"
            if cols["description"][i]:
                head += f" {cols['description'][i]}"
            lines.append(head)
            lines.extend(wrap_seq(cols["sequence"][i], cols["wrap"][i]))

        dst = out / fname
        dst.write_text("\n".join(lines) + "\n", encoding="utf-8", newline="\n")
        print(f"  {src.name:<32} -> {dst} ({tbl.num_rows:,} records)")


if __name__ == "__main__":
    main()
