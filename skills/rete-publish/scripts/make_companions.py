#!/usr/bin/env python3
"""N-Triples → a relational `triples` companion (Parquet / DuckDB / SQLite).

A companion is the SAME data in a columnar/relational store, so the playground's
Explore tab can compare the rete engine against DuckDB/SQLite over it. This emits
the simple, lossless, generic shape — a flat `triples` table:

    subject TEXT, predicate TEXT, object TEXT (raw N-Triples token),
    otype TEXT ('iri'|'bnode'|'literal'), value TEXT (decoded), datatype TEXT, lang TEXT

`object` keeps the canonical token (lossless); `value`/`datatype`/`lang` are the
decoded convenience columns. For a richer class-partitioned layout (one wide table
per type), use scripts/rdf_to_entity_tables.py instead.

  pip install --break-system-packages duckdb        # only for --parquet / --duckdb
  python make_companions.py foo.nt -o foo --parquet --duckdb --sqlite
    → foo.parquet, foo.duckdb, foo.sqlite

Then upload alongside the .rete and add a CATALOG.companions[key] entry.
"""
import argparse
import re
import sqlite3
import sys

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


def decode(tok):
    if tok[0] == "<":
        return ("iri", tok[1:-1], None, None)
    if tok[0] == "_":
        return ("bnode", tok[2:], None, None)
    m = LIT.match(tok)
    if not m:
        return ("literal", tok, None, None)
    return ("literal", unescape(m.group(1)), m.group(3), m.group(2))


def rows(path):
    fh = sys.stdin if path == "-" else open(path, encoding="utf-8", errors="replace")
    for line in fh:
        s = line.rstrip("\r\n")
        if not s or s.lstrip().startswith("#"):
            continue
        m = TRIPLE.match(s)
        if not m:
            continue
        subj, pred, obj = m.group(1), m.group(2), m.group(3)
        otype, value, dt, lang = decode(obj)
        subj = subj[1:-1] if subj[0] == "<" else subj
        yield (subj, pred[1:-1], obj, otype, value, dt, lang)
    if fh is not sys.stdin:
        fh.close()


DDL = ("CREATE TABLE triples (subject TEXT, predicate TEXT, object TEXT, "
       "otype TEXT, value TEXT, datatype TEXT, lang TEXT)")


def to_sqlite(path, src):
    con = sqlite3.connect(path)
    con.execute("DROP TABLE IF EXISTS triples")
    con.execute(DDL)
    con.executemany("INSERT INTO triples VALUES (?,?,?,?,?,?,?)", rows(src))
    con.execute("CREATE INDEX i_sp ON triples(subject, predicate)")
    con.execute("CREATE INDEX i_p ON triples(predicate)")
    con.commit()
    n = con.execute("SELECT COUNT(*) FROM triples").fetchone()[0]
    con.close()
    sys.stderr.write(f"  sqlite  {path}: {n} rows\n")


def to_duck(parquet, duckdb_file, src):
    try:
        import duckdb
    except ImportError:
        sys.exit("duckdb not installed: pip install --break-system-packages duckdb")
    con = duckdb.connect(duckdb_file or ":memory:")
    con.execute("DROP TABLE IF EXISTS triples")
    con.execute(DDL)
    con.executemany("INSERT INTO triples VALUES (?,?,?,?,?,?,?)", list(rows(src)))
    n = con.execute("SELECT COUNT(*) FROM triples").fetchone()[0]
    if parquet:
        con.execute(f"COPY triples TO '{parquet}' (FORMAT PARQUET)")
        sys.stderr.write(f"  parquet {parquet}: {n} rows\n")
    if duckdb_file:
        sys.stderr.write(f"  duckdb  {duckdb_file}: {n} rows\n")
    con.close()


def main():
    ap = argparse.ArgumentParser(description="N-Triples → relational companion(s)")
    ap.add_argument("input", help="N-Triples file, or - for stdin (NOT - if >1 backend)")
    ap.add_argument("-o", "--out", required=True, help="output basename (no extension)")
    ap.add_argument("--parquet", action="store_true")
    ap.add_argument("--duckdb", action="store_true")
    ap.add_argument("--sqlite", action="store_true")
    args = ap.parse_args()
    if not (args.parquet or args.duckdb or args.sqlite):
        sys.exit("pick at least one of --parquet / --duckdb / --sqlite")
    if args.input == "-" and (int(args.parquet) + int(args.duckdb) + int(args.sqlite)) > 1:
        sys.exit("stdin can only feed one backend (it's consumed once) — use a file for several")
    if args.parquet or args.duckdb:
        to_duck(f"{args.out}.parquet" if args.parquet else None,
                f"{args.out}.duckdb" if args.duckdb else None, args.input)
    if args.sqlite:
        to_sqlite(f"{args.out}.sqlite", args.input)


if __name__ == "__main__":
    main()
