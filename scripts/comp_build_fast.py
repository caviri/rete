#!/usr/bin/env python3
"""Fast flat-triples companion builder. SQLite via a single executemany in one
transaction (fast), then DuckDB + Parquet are derived from the SQLite with
DuckDB's native sqlite_scan (no slow row-by-row DuckDB executemany). Loops every
data/rag/comp/<key>.nt, skipping ones already complete (parquet+duckdb+sqlite)."""
import glob, os, re, sqlite3, sys

IRI = r'<[^\x00-\x20<>"{}|^`\\]*>'
BNODE = r'_:[A-Za-z0-9_][A-Za-z0-9_.-]*'
STRING = r'"(?:[^"\\]|\\.)*"'
LITERAL = rf'{STRING}(?:@[A-Za-z][A-Za-z0-9-]*|\^\^{IRI})?'
TRIPLE = re.compile(rf'^({IRI}|{BNODE})\s+({IRI})\s+({IRI}|{BNODE}|{LITERAL})\s*\.\s*$')
LIT = re.compile(r'^"((?:[^"\\]|\\.)*)"(?:@([A-Za-z][A-Za-z0-9-]*)|\^\^<([^>]+)>)?$')
_UNI = re.compile(r'\\u([0-9A-Fa-f]{4})|\\U([0-9A-Fa-f]{8})')


def unescape(s):
    s = _UNI.sub(lambda m: chr(int(m.group(1) or m.group(2), 16)), s)
    return s.replace('\\"', '"').replace("\\n", "\n").replace("\\r", "\r").replace("\\t", "\t").replace("\\\\", "\\")


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
    with open(path, encoding="utf-8", errors="replace") as fh:
        for line in fh:
            s = line.rstrip("\r\n")
            if not s or s.lstrip().startswith("#"):
                continue
            m = TRIPLE.match(s)
            if not m:
                continue
            subj, pred, obj = m.group(1), m.group(2), m.group(3)
            otype, value, dt, lang = decode(obj)
            yield (subj[1:-1] if subj[0] == "<" else subj, pred[1:-1], obj, otype, value, dt, lang)


DDL = ("CREATE TABLE triples (subject TEXT, predicate TEXT, object TEXT, "
       "otype TEXT, value TEXT, datatype TEXT, lang TEXT)")


def build(key):
    base = f"data/rag/comp/{key}"
    if all(os.path.exists(f"{base}.{e}") and os.path.getsize(f"{base}.{e}") for e in ("parquet", "duckdb", "sqlite")):
        return "skip", 0
    for e in ("sqlite", "duckdb", "parquet"):
        try: os.remove(f"{base}.{e}")
        except OSError: pass
    # 1. SQLite (fast bulk insert in one transaction)
    con = sqlite3.connect(f"{base}.sqlite")
    con.execute(DDL)
    con.executemany("INSERT INTO triples VALUES (?,?,?,?,?,?,?)", rows(f"{base}.nt"))
    con.execute("CREATE INDEX i_sp ON triples(subject, predicate)")
    con.execute("CREATE INDEX i_p ON triples(predicate)")
    con.commit()
    n = con.execute("SELECT COUNT(*) FROM triples").fetchone()[0]
    con.close()
    if n == 0:
        os.remove(f"{base}.sqlite"); return "empty", 0
    # 2. DuckDB + Parquet derived from the SQLite via native scan (fast)
    import duckdb
    d = duckdb.connect(f"{base}.duckdb")
    d.execute("INSTALL sqlite; LOAD sqlite;")
    d.execute(f"CREATE TABLE triples AS SELECT * FROM sqlite_scan('{base}.sqlite', 'triples')")
    d.execute(f"COPY triples TO '{base}.parquet' (FORMAT PARQUET)")
    d.close()
    return "ok", n


def main():
    keys = [os.path.basename(f)[:-3] for f in sorted(glob.glob("data/rag/comp/*.nt"))]
    for key in keys:
        status, n = build(key)
        print(f"  {key:24s} {status:6s} {n}", flush=True)
    print("FAST_BUILD_DONE", flush=True)


if __name__ == "__main__":
    main()
