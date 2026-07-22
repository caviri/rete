"""Flatten the CORDIS triples into one Parquet property-table per class.

Each entity goes to its MOST SPECIFIC type (leaf) so a journal paper (typed
Result + ProjectPublication + JournalPaper) yields one row in JournalPaper, not
three. Columns = the predicates used by that class's subjects; single-valued
predicates become scalar columns, multi-valued ones become JSON-array columns.
An `rdf_types` column keeps the full type set; object-IRI values are the raw
target IRIs (foreign keys into other class tables).

Output: data/cordis/parquet/<Class>.parquet
"""

import os
import re
import duckdb

TRIPLES = "read_parquet('D:/pro/rete/data/cordis/triples/*.parquet')"
OUT = r"D:\pro\rete\data\cordis\parquet"
RDFTYPE = "http://www.w3.org/1999/02/22-rdf-syntax-ns#type"
S66 = "http://data.europa.eu/s66#"

# child -> parent (s66 local names) to pick the most specific type
PARENT = {
    "ForProfitOrganisation": "Organisation", "SME": "ForProfitOrganisation",
    "ResearchOrganisation": "Organisation", "PublicBody": "Organisation",
    "HigherOrSecondaryEducation": "Organisation",
    "ProjectPublication": "Result", "ProjectDeliverable": "Result",
    "ProjectReportSummary": "Result",
    "JournalPaper": "ProjectPublication", "ProceedingsPaper": "ProjectPublication",
    "Book": "ProjectPublication", "BookChapter": "ProjectPublication",
    "ThesisDissertation": "ProjectPublication",
}


def depth(name):
    d = 0
    while name in PARENT:
        name = PARENT[name]
        d += 1
    return d


def local(iri):
    return iri.split("#")[-1].split("/")[-1]


def colname(pred):
    ln = local(pred)
    return re.sub(r"[^0-9A-Za-z]+", "_", ln).strip("_") or "p"


def main():
    os.makedirs(OUT, exist_ok=True)
    d = duckdb.connect()
    d.execute("SET threads=8")
    d.execute("SET preserve_insertion_order=false")
    d.execute(f"CREATE VIEW t AS SELECT * FROM {TRIPLES}")

    # subject -> all types (as list) and the chosen leaf type
    d.execute(f"""CREATE TABLE s2types AS
        SELECT subject, list(object) AS types
        FROM t WHERE predicate='{RDFTYPE}' GROUP BY subject""")
    rows = d.execute("SELECT subject, types FROM s2types").fetchall()
    print(f"typed subjects: {len(rows):,}", flush=True)
    leaf = {}
    for subj, types in rows:
        best = None
        best_d = -1
        for ty in types:
            ln = local(ty)
            dep = depth(ln)
            if dep > best_d:
                best_d = dep
                best = ln
        leaf[subj] = best
    # write subject->leaf to a temp parquet and register
    import pyarrow as pa, pyarrow.parquet as pq
    tmp = os.path.join(OUT, "_subj_leaf.parquet")
    pq.write_table(pa.table({"subject": list(leaf.keys()),
                             "leaf": list(leaf.values())}), tmp)
    d.execute(f"CREATE TABLE sl AS SELECT * FROM read_parquet('{tmp.replace(chr(92),'/')}')")
    d.execute("CREATE TABLE tj AS SELECT t.subject, t.predicate, t.object, t.otype "
              "FROM t JOIN sl USING(subject) WHERE t.predicate <> '" + RDFTYPE + "'")

    classes = [r[0] for r in d.execute(
        "SELECT leaf, count(*) c FROM sl GROUP BY 1 ORDER BY c DESC").fetchall()]

    summary = []
    for cls in classes:
        # predicates + max multiplicity for this class
        pm = d.execute(f"""
            SELECT predicate, max(cnt) mx FROM (
              SELECT subject, predicate, count(*) cnt FROM tj
              WHERE subject IN (SELECT subject FROM sl WHERE leaf=?)
              GROUP BY subject, predicate) GROUP BY predicate
        """, [cls]).fetchall()
        if not pm:
            continue
        used = {}
        sel = ["subject"]
        for pred, mx in pm:
            c = colname(pred)
            while c in used and used[c] != pred:
                c += "_"
            used[c] = pred
            if mx <= 1:
                sel.append(f"any_value(object) FILTER (predicate='{pred}') AS \"{c}\"")
            else:
                sel.append(f"to_json(list(object) FILTER (predicate='{pred}')) AS \"{c}\"")
        # types column
        outp = os.path.join(OUT, f"{cls}.parquet").replace(chr(92), "/")
        d.execute(f"""COPY (
            SELECT tj.subject,
                   any_value(st.types) AS rdf_types,
                   {', '.join(sel[1:])}
            FROM tj JOIN s2types st ON tj.subject=st.subject
            WHERE tj.subject IN (SELECT subject FROM sl WHERE leaf='{cls}')
            GROUP BY tj.subject
        ) TO '{outp}' (FORMAT parquet, COMPRESSION zstd)""")
        n = d.execute(f"SELECT count(*) FROM read_parquet('{outp}')").fetchone()[0]
        summary.append((cls, n, len(pm)))
        print(f"  {cls:26s} {n:>9,} rows  {len(pm)} cols", flush=True)

    os.remove(tmp)
    total = sum(n for _, n, _ in summary)
    print(f"\nDONE: {len(summary)} class tables, {total:,} rows -> {OUT}", flush=True)


if __name__ == "__main__":
    main()
