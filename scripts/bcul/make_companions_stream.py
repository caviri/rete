#!/usr/bin/env python3
"""Streaming N-Triples → Parquet / DuckDB / SQLite flat-`triples` companion.

Unlike skills/rete-publish/scripts/make_companions.py (which materializes ALL rows
in a Python list → OOM at 100M+), this streams the .nt into a ZSTD Parquet in
row-group batches (bounded RAM), then builds DuckDB and SQLite FROM the Parquet via
DuckDB (its parquet reader + sqlite extension stream too). Suitable for the 117M-triple
BCUL twin.

  python make_companions_stream.py data/bcul/bcul.nt -o data/bcul/bcul --parquet --duckdb --sqlite
    → bcul.parquet, bcul.duckdb, bcul.sqlite
"""
from __future__ import annotations

import argparse
import re
import sqlite3
import sys
import time
from pathlib import Path

IRI = r'<[^\x00-\x20<>"{}|^`\\]*>'
BNODE = r'_:[A-Za-z0-9_][A-Za-z0-9_.-]*'
STRING = r'"(?:[^"\\]|\\.)*"'
LITERAL = rf'{STRING}(?:@[A-Za-z][A-Za-z0-9-]*|\^\^{IRI})?'
TRIPLE = re.compile(rf'^({IRI}|{BNODE})\s+({IRI})\s+({IRI}|{BNODE}|{LITERAL})\s*\.\s*$')
LIT = re.compile(r'^"((?:[^"\\]|\\.)*)"(?:@([A-Za-z][A-Za-z0-9-]*)|\^\^<([^>]+)>)?$')
_UNI = re.compile(r'\\u([0-9A-Fa-f]{4})|\\U([0-9A-Fa-f]{8})')


def unescape(s):
    s = _UNI.sub(lambda m: chr(int(m.group(1) or m.group(2), 16)), s)
    return (s.replace('\\"', '"').replace("\\n", "\n").replace("\\r", "\r")
             .replace("\\t", "\t").replace("\\\\", "\\"))


def decode(obj):
    if obj[0] == "<":
        return ("iri", obj[1:-1], None, None)
    if obj[0] == "_":
        return ("bnode", obj, None, None)
    m = LIT.match(obj)
    if not m:
        return ("literal", obj, None, None)
    return ("literal", unescape(m.group(1)), m.group(3), m.group(2))


COLS = ["subject", "predicate", "object", "otype", "value", "datatype", "lang"]


def build_parquet(nt, out, batch=1_000_000):
    import pyarrow as pa
    import pyarrow.parquet as pq
    schema = pa.schema([(c, pa.string()) for c in COLS])
    writer = pq.ParquetWriter(out, schema, compression="zstd")
    buf = [[] for _ in COLS]
    n = 0
    t0 = time.time()
    with open(nt, encoding="utf-8", errors="replace") as fh:
        for line in fh:
            s = line.rstrip("\r\n")
            if not s or s[0] == "#":
                continue
            m = TRIPLE.match(s)
            if not m:
                continue
            subj, pred, obj = m.group(1), m.group(2), m.group(3)
            otype, value, dt, lang = decode(obj)
            row = (subj[1:-1] if subj[0] == "<" else subj, pred[1:-1], obj, otype, value, dt, lang)
            for i in range(7):
                buf[i].append(row[i])
            n += 1
            if len(buf[0]) >= batch:
                writer.write_table(pa.table({c: buf[i] for i, c in enumerate(COLS)}, schema=schema))
                buf = [[] for _ in COLS]
                print(f"  parquet {n:,} rows ({n/(time.time()-t0):.0f}/s)", flush=True)
    if buf[0]:
        writer.write_table(pa.table({c: buf[i] for i, c in enumerate(COLS)}, schema=schema))
    writer.close()
    print(f"parquet done: {n:,} rows -> {out}", flush=True)
    return n


def build_duckdb(parquet, dbfile):
    import duckdb
    Path(dbfile).unlink(missing_ok=True)
    con = duckdb.connect(dbfile)
    con.execute(f"CREATE TABLE triples AS SELECT * FROM read_parquet('{parquet}')")
    con.execute("CREATE INDEX i_sp ON triples(subject, predicate)")
    con.execute("CREATE INDEX i_p ON triples(predicate)")
    con.close()
    print(f"duckdb done -> {dbfile}", flush=True)


def build_sqlite(parquet, dbfile):
    import duckdb
    Path(dbfile).unlink(missing_ok=True)
    con = duckdb.connect()
    con.execute("INSTALL sqlite; LOAD sqlite")
    con.execute(f"ATTACH '{dbfile}' AS s (TYPE sqlite)")
    con.execute(f"CREATE TABLE s.triples AS SELECT * FROM read_parquet('{parquet}')")
    con.close()
    print("  sqlite rows loaded; building indexes …", flush=True)
    c = sqlite3.connect(dbfile)
    c.execute("CREATE INDEX i_sp ON triples(subject, predicate)")
    c.execute("CREATE INDEX i_p ON triples(predicate)")
    c.commit()
    c.close()
    print(f"sqlite done -> {dbfile}", flush=True)


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("input")
    ap.add_argument("-o", "--out", required=True, help="output basename (no extension)")
    ap.add_argument("--parquet", action="store_true")
    ap.add_argument("--duckdb", action="store_true")
    ap.add_argument("--sqlite", action="store_true")
    args = ap.parse_args()
    parquet = args.out + ".parquet"
    if args.parquet or args.duckdb or args.sqlite:
        if not Path(parquet).exists():
            build_parquet(args.input, parquet)
        else:
            print(f"parquet already exists, reusing {parquet}", flush=True)
    if args.duckdb:
        build_duckdb(parquet, args.out + ".duckdb")
    if args.sqlite:
        build_sqlite(parquet, args.out + ".sqlite")
    for ext in ("parquet", "duckdb", "sqlite"):
        f = Path(args.out + "." + ext)
        if f.exists():
            print(f"  {f.name}: {f.stat().st_size/1e9:.2f} GB", flush=True)


if __name__ == "__main__":
    main()
