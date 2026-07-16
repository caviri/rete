#!/usr/bin/env python3
"""Assemble the per-class Parquet tables into a single DuckDB database.

Loads every parquet under <out-dir>/parquet/ as a native DuckDB table so the
whole Open-Pulse graph is queryable relationally in one file. Also loads the
flat triples.parquet (if present) as `triples` for graph-style queries.
"""
import argparse
import glob
import os

import duckdb


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--out-dir", required=True)
    args = ap.parse_args()
    pdir = os.path.join(args.out_dir, "parquet")
    db_path = os.path.join(args.out_dir, "open-pulse.duckdb")
    if os.path.exists(db_path):
        os.remove(db_path)

    con = duckdb.connect(db_path)
    for pq_path in sorted(glob.glob(os.path.join(pdir, "*.parquet"))):
        name = os.path.splitext(os.path.basename(pq_path))[0]
        con.execute(
            f'CREATE TABLE "{name}" AS SELECT * FROM read_parquet(?)', [pq_path]
        )
        n = con.execute(f'SELECT count(*) FROM "{name}"').fetchone()[0]
        print(f"  loaded {name}: {n} rows")
    print("\ntables:")
    for (t,) in con.execute("SHOW TABLES").fetchall():
        print("  -", t)
    con.close()
    print(f"\nwrote {db_path}")


if __name__ == "__main__":
    main()
