#!/usr/bin/env python3
"""Emit catalog `companions` entries for the flat-triples companions built under
data/rag/comp/. Discovers datasets by <key>.parquet; includes the duckdb/sqlite
fields only when those files exist and are a sane size (a big graph ships
Parquet+DuckDB but skips a multi-GB SQLite). Two generic SQL examples over the
`triples` table. Printed to stdout; spliced by comp_splice.py."""
import glob, json, os, sqlite3

RAG = "data/rag/comp"
SQLITE_MAX = 1.5 * 1024**3   # skip a SQLite bigger than this (too heavy to range-read)


def sz(p):
    b = os.path.getsize(p)
    return f"{b/1048576:.1f} MB" if b >= 1048576 else f"{max(1,b//1024)} KB"


def rowcount(key):
    sq = f"{RAG}/{key}.sqlite"
    if os.path.exists(sq):
        return sqlite3.connect(sq).execute("SELECT COUNT(*) FROM triples").fetchone()[0]
    import duckdb
    return duckdb.sql(f"SELECT COUNT(*) FROM read_parquet('{RAG}/{key}.parquet')").fetchone()[0]


def entry(key, n):
    ex = [
        {"label": "Predicate totals", "table": {"file": "triples.parquet", "name": "triples"},
         "sparql": "SELECT ?p (COUNT(*) AS ?n) WHERE { ?s ?p ?o } GROUP BY ?p ORDER BY DESC(?n)",
         "duck": "SELECT predicate, COUNT(*) AS n\nFROM {T}\nGROUP BY predicate\nORDER BY n DESC;",
         "sqlite": "SELECT predicate, COUNT(*) AS n\nFROM \"triples\"\nGROUP BY predicate\nORDER BY n DESC;",
         "note": "How often each predicate is used — the shape of the graph, as a GROUP BY over the flat triples table."},
        {"label": "Busiest subjects", "table": {"file": "triples.parquet", "name": "triples"},
         "sparql": "SELECT ?s (COUNT(*) AS ?n) WHERE { ?s ?p ?o } GROUP BY ?s ORDER BY DESC(?n) LIMIT 50",
         "duck": "SELECT subject, COUNT(*) AS n\nFROM {T}\nGROUP BY subject\nORDER BY n DESC\nLIMIT 50;",
         "sqlite": "SELECT subject, COUNT(*) AS n\nFROM \"triples\"\nGROUP BY subject\nORDER BY n DESC\nLIMIT 50;",
         "note": "The 50 subjects with the most outgoing facts — the hubs of the graph."},
    ]
    obj = {"rete": f"{key}/{key}.rete", "parquetDir": f"{key}/{key}-tables"}
    dk = f"{RAG}/{key}.duckdb"
    if os.path.exists(dk):
        obj["duckdb"] = f"{key}/{key}.duckdb"; obj["duckdbSize"] = sz(dk)
    sq = f"{RAG}/{key}.sqlite"
    if os.path.exists(sq) and os.path.getsize(sq) <= SQLITE_MAX:
        obj["sqlite"] = f"{key}/{key}.sqlite"; obj["sqliteSize"] = sz(sq)
    obj["flat"] = True
    obj["about"] = (f"The {key} graph as a flat <code>triples</code> table (subject, predicate, "
                    f"object token + decoded value/datatype/lang) — the same {n:,} facts in "
                    f"{'Parquet, DuckDB and SQLite' if 'sqlite' in obj else 'Parquet and DuckDB'}, "
                    f"queryable in SQL to compare against the rete engine.")
    obj["tables"] = [{"name": "triples", "file": "triples.parquet",
                      "label": "triples — every fact (S, P, O)", "entities": n}]
    obj["sqlCols"] = ["subject", "predicate", "object", "otype", "value", "datatype", "lang"]
    obj["examples"] = ex
    return '    "%s": %s,' % (key, json.dumps(obj, ensure_ascii=False))


out = []
for pq in sorted(glob.glob(f"{RAG}/*.parquet")):
    key = os.path.basename(pq)[:-8]
    out.append(entry(key, rowcount(key)))
print("\n".join(out))
