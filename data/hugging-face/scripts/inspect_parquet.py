#!/usr/bin/env python3
"""Profile every parquet in raw/hub-stats/ and parquet/: rows, columns, fill, extremes.

Run in Docker:
  MSYS_NO_PATHCONV=1 docker run --rm -v "D:/pro/rete:/w" -w //w python:3.12-slim \
    bash -c "pip -q install duckdb && python data/hugging-face/scripts/inspect_parquet.py"
"""
import glob
import os
import duckdb

BASE = os.path.join(os.path.dirname(os.path.abspath(__file__)), "..")
con = duckdb.connect()

for f in (sorted(glob.glob(os.path.join(BASE, "raw", "hub-stats", "*.parquet")))
          + sorted(glob.glob(os.path.join(BASE, "parquet", "*.parquet")))):
    rel = os.path.relpath(f, BASE)
    n = con.execute(f"SELECT count(*) FROM read_parquet('{f}')").fetchone()[0]
    cols = con.execute(f"DESCRIBE SELECT * FROM read_parquet('{f}')").fetchall()
    print(f"\n=== {rel}  ({n:,} rows, {len(cols)} cols, {os.path.getsize(f)/1e6:.1f} MB)")
    for name, dtype, *_ in cols:
        if n == 0:
            print(f"  {name:32s} {dtype}")
            continue
        nn = con.execute(
            f'SELECT count("{name}") FROM read_parquet(\'{f}\')').fetchone()[0]
        print(f"  {name:32s} {dtype:28s} fill {100*nn/n:5.1f}%")

# a few headline aggregates
hs = os.path.join(BASE, "raw", "hub-stats")
if os.path.exists(os.path.join(hs, "models.parquet")):
    print("\n=== headline aggregates")
    for label, sql in [
        ("distinct model authors", f"SELECT count(DISTINCT author) FROM read_parquet('{hs}/models.parquet')"),
        ("distinct dataset authors", f"SELECT count(DISTINCT author) FROM read_parquet('{hs}/datasets.parquet')"),
        ("distinct space authors", f"SELECT count(DISTINCT author) FROM read_parquet('{hs}/spaces.parquet')"),
        ("models with arxiv tag", f"""SELECT count(DISTINCT id) FROM (SELECT id, unnest(tags) t
             FROM read_parquet('{hs}/models.parquet')) WHERE t LIKE 'arxiv:%'"""),
    ]:
        print(f"  {label:28s} {con.execute(sql).fetchone()[0]:,}")
    pq = os.path.join(BASE, "parquet")
    if os.path.exists(os.path.join(pq, "users.parquet")):
        for label, sql in [
            ("users harvested", f"SELECT count(*) FROM read_parquet('{pq}/users.parquet')"),
            ("orgs harvested", f"SELECT count(*) FROM read_parquet('{pq}/orgs.parquet')"),
            ("sum numBuckets (users)", f"SELECT sum(num_buckets) FROM read_parquet('{pq}/users.parquet')"),
            ("sum numBuckets (orgs)", f"SELECT sum(num_buckets) FROM read_parquet('{pq}/orgs.parquet')"),
        ]:
            try:
                print(f"  {label:28s} {con.execute(sql).fetchone()[0]:,}")
            except Exception as e:
                print(f"  {label:28s} n/a ({e})")
