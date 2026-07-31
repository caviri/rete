#!/usr/bin/env python3
"""Parquet -> miRBase GFF3 (the exact inverse of gff3_to_parquet.py).

Re-emits the preserved header block then the 9 tab-separated columns, using the
verbatim attribute string so nothing can drift through the round trip.

    bash data/mirbase/scripts/py.sh parquet_to_gff3.py [outdir] [parquetdir]

`parquetdir` defaults to data/mirbase/parquet. It is an argument so that a test
can read a DIFFERENT set of tables (e.g. ones derived back out of the .rete)
without moving the real directory out of the way.
"""
from __future__ import annotations

import sys
from collections import defaultdict
from pathlib import Path

import pyarrow.parquet as pq

BASE = Path(__file__).resolve().parent.parent
DEFAULT_OUT = BASE / "roundtrip" / "genomes"

COLS = ("seqid", "source", "type", "start", "end",
        "score", "strand", "phase", "attributes")


def main() -> None:
    out = Path(sys.argv[1]) if len(sys.argv) > 1 else DEFAULT_OUT
    parq = Path(sys.argv[2]) if len(sys.argv) > 2 else BASE / "parquet"
    out.mkdir(parents=True, exist_ok=True)

    feats = pq.read_table(parq / "gff3_features.parquet")
    # the header comment block is presentation, not graph data, so a
    # rete-derived table set won't have it — fall back to the canonical one
    hpath = parq / "gff3_headers.parquet"
    if not hpath.exists():
        hpath = BASE / "parquet" / "gff3_headers.parquet"
    heads = pq.read_table(hpath)

    hdr = dict(zip(heads.column("organism").to_pylist(),
                   heads.column("header").to_pylist()))

    data = {c: feats.column(c).to_pylist() for c in ("organism", "ordinal", *COLS)}
    by_org: dict[str, list[int]] = defaultdict(list)
    for i, org in enumerate(data["organism"]):
        by_org[org].append(i)

    for org, idxs in sorted(by_org.items()):
        idxs.sort(key=lambda i: data["ordinal"][i])
        lines = [hdr.get(org, "")] if hdr.get(org) else []
        for i in idxs:
            lines.append("\t".join(str(data[c][i]) for c in COLS))
        dst = out / f"{org}.gff3"
        dst.write_text("\n".join(lines) + "\n", encoding="utf-8", newline="\n")
        print(f"  {org}.gff3  {len(idxs):,} features")

    print(f"ok  wrote {len(by_org)} GFF3 files to {out}")


if __name__ == "__main__":
    main()
