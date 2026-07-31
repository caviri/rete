#!/usr/bin/env python3
"""miRBase FASTA -> Parquet.

miRBase headers are a fixed 3-part shape::

    >cel-let-7 MI0000001 Caenorhabditis elegans let-7 stem-loop
     ^name     ^accession ^description

Sequences are hard-wrapped (60 cols in the shipped files). To make the reverse
conversion byte-exact we record the observed wrap width per record rather than
assuming one, and keep the record's original ordinal.

    bash data/mirbase/scripts/py.sh fa_to_parquet.py
"""
from __future__ import annotations

import sys
from pathlib import Path

import pyarrow as pa
import pyarrow.parquet as pq

RAW = Path(__file__).resolve().parent.parent / "raw"
OUT = Path(__file__).resolve().parent.parent / "parquet"

FILES = {
    "hairpin": RAW / "hairpin.fa",
    "hairpin_high_conf": RAW / "hairpin_high_conf.fa",
    "mature": RAW / "mature.fa",
    "mature_high_conf": RAW / "mature_high_conf.fa",
}

SCHEMA = pa.schema([
    ("ordinal", pa.int32()),        # position in the file — preserves order
    ("name", pa.string()),          # cel-let-7
    ("accession", pa.string()),     # MI0000001 / MIMAT0000001
    ("description", pa.string()),   # Caenorhabditis elegans let-7 stem-loop
    ("organism", pa.string()),      # cel  (3-4 letter miRBase prefix of `name`)
    ("sequence", pa.string()),      # RNA, unwrapped
    ("seq_length", pa.int32()),
    ("wrap", pa.int32()),           # original hard-wrap width, for exact rebuild
])


def parse_fasta(path: Path) -> list[dict]:
    recs: list[dict] = []
    name = acc = desc = None
    chunks: list[str] = []
    wrap = 0

    def flush() -> None:
        if name is None:
            return
        seq = "".join(chunks)
        recs.append({
            "ordinal": len(recs),
            "name": name,
            "accession": acc,
            "description": desc,
            "organism": name.split("-")[0] if name else "",
            "sequence": seq,
            "seq_length": len(seq),
            "wrap": wrap or len(seq),
        })

    with path.open(encoding="utf-8", errors="replace") as fh:
        for line in fh:
            line = line.rstrip("\n")
            if line.startswith(">"):
                flush()
                parts = line[1:].split(" ", 2)
                name = parts[0]
                acc = parts[1] if len(parts) > 1 else ""
                desc = parts[2] if len(parts) > 2 else ""
                chunks, wrap = [], 0
            elif line:
                # the wrap width is the length of the FIRST full line
                if not chunks:
                    wrap = len(line)
                chunks.append(line)
    flush()
    return recs


def main() -> None:
    OUT.mkdir(parents=True, exist_ok=True)
    for key, path in FILES.items():
        if not path.exists():
            print(f"!! missing {path}", file=sys.stderr)
            continue
        recs = parse_fasta(path)
        table = pa.Table.from_pylist(recs, schema=SCHEMA)
        dst = OUT / f"fasta_{key}.parquet"
        pq.write_table(table, dst, compression="zstd")
        print(f"  {path.name:<24} -> {dst.name:<32} {len(recs):>7,} records "
              f"({dst.stat().st_size:,} bytes)")


if __name__ == "__main__":
    main()
