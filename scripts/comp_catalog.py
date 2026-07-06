#!/usr/bin/env python3
"""Emit catalog `companions` entries for the flat-triples companions built under
data/rag/comp/. One entry per <key>.sqlite: a single `triples` table (S,P,O +
decoded value/datatype/lang) in Parquet/DuckDB/SQLite, with two generic SQL
examples. Printed to stdout; spliced into the companions block by comp_splice.py."""
import glob, json, os, sqlite3

RAG = "data/rag/comp"


def sz(p):
    b = os.path.getsize(p)
    return f"{b/1048576:.1f} MB" if b >= 1048576 else f"{max(1,b//1024)} KB"


def entry(key, n, duck_sz, sqlite_sz):
    about = (f"The {key} graph as a flat <code>triples</code> table (subject, predicate, "
             f"object token + decoded value/datatype/lang) in Parquet, DuckDB and SQLite "
             f"— the same {n:,} facts, queryable in SQL to compare against the rete engine.")
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
    obj = {
        "rete": f"{key}/{key}.rete",
        "parquetDir": f"{key}/{key}-tables",
        "duckdb": f"{key}/{key}.duckdb", "duckdbSize": duck_sz,
        "sqlite": f"{key}/{key}.sqlite", "sqliteSize": sqlite_sz,
        "flat": True,
        "about": about,
        "tables": [{"name": "triples", "file": "triples.parquet",
                    "label": "triples — every fact (S, P, O)", "entities": n}],
        "sqlCols": ["subject", "predicate", "object", "otype", "value", "datatype", "lang"],
        "examples": ex,
    }
    return '    "%s": %s,' % (key, json.dumps(obj, ensure_ascii=False))


out = []
for sq in sorted(glob.glob(f"{RAG}/*.sqlite")):
    key = os.path.basename(sq)[:-7]
    if not (os.path.exists(f"{RAG}/{key}.parquet") and os.path.exists(f"{RAG}/{key}.duckdb")):
        continue
    n = sqlite3.connect(sq).execute("SELECT COUNT(*) FROM triples").fetchone()[0]
    out.append(entry(key, n, sz(f"{RAG}/{key}.duckdb"), sz(sq)))

print("\n".join(out))
import sys
print(f"\n// {len(out)} flat companions", file=sys.stderr)
