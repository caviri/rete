#!/usr/bin/env python3
"""Verify Parquet files actually DECODE (full row-group scan).

Catches silent intra-file corruption that size + SHA do NOT — e.g. bad snappy
pages written by Docker's Windows bind-mount under heavy concurrent writes. The
file's byte length and checksum still "match" because the corruption is inside
the data pages, not a truncation, so only decoding every row group reveals it.

Exit code 1 if any file is corrupt/unreadable, else 0 — usable in CI / a gate.

Usage (Dockerized, from repo root):
  MSYS_NO_PATHCONV=1 docker run --rm -v "D:/pro/rete:/w" -w //w python:3.12-slim \
    bash -lc 'pip install -q pyarrow && python /w/scripts/verify_parquet.py data/<dataset>'

  # one file:      ... verify_parquet.py data/deps-dev/raw/Projects.parquet
  # a dataset dir: ... verify_parquet.py data/deps-dev
  # EVERYTHING:    ... verify_parquet.py            (scans data/**/*.parquet)

NOTE: this reads every byte of every file, so it is I/O-heavy on large sets —
that is the point; a partial/sampled check can miss corruption anywhere in a file.
"""
import glob
import os
import sys

import pyarrow.parquet as pq

roots = sys.argv[1:] or ["data"]
paths = []
for r in roots:
    if os.path.isfile(r) and r.endswith(".parquet"):
        paths.append(r)
    else:
        paths += glob.glob(os.path.join(r, "**", "*.parquet"), recursive=True)
paths = sorted(set(paths))
if not paths:
    print(f"no .parquet found under {roots}")
    sys.exit(0)

bad_files = 0
for p in paths:
    try:
        pf = pq.ParquetFile(p)
        nrg = pf.num_row_groups
        bad, first = 0, ""
        for i in range(nrg):
            try:
                pf.read_row_group(i)
            except Exception as e:  # noqa: BLE001
                bad += 1
                if bad == 1:
                    first = f"rg {i}: {str(e).splitlines()[0][:56]}"
        if bad:
            bad_files += 1
            print(f"CORRUPT    {p}  ({bad}/{nrg} row groups bad; first {first})", flush=True)
        else:
            print(f"clean      {p}  ({nrg} row groups, {pf.metadata.num_rows:,} rows)", flush=True)
    except Exception as e:  # noqa: BLE001
        bad_files += 1
        print(f"UNREADABLE {p}: {type(e).__name__}: {str(e)[:60]}", flush=True)

print("=" * 60)
print(f"{len(paths)} files checked, {bad_files} corrupt/unreadable")
sys.exit(1 if bad_files else 0)
