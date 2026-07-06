#!/usr/bin/env python
# -*- coding: utf-8 -*-
"""
Build the Explore-tab companions (Parquet + DuckDB + SQLite) for the ramon_llull
dataset: ONE wide `ProvidedCHO` entity table (one row per work), named columns for
each Dublin Core / EDM / bvpb property. The ~52 MB bvpb:fulltext is EXCLUDED (a
3 MB text cell is useless in a SQL browse — full-text search stays a rete feature).

Multi-valued properties become LIST columns; `label` is the first dc:title (plain).
Emits companions/ramon_llull.duckdb, ramon_llull.sqlite, and
ramon_llull-tables/{ProvidedCHO_.parquet,_manifest.parquet}.
"""
import os, re, json, sqlite3, tempfile
import duckdb

ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), "..",
                                    "data", "bvpb", "ramon_llull"))
OUT = os.path.join(ROOT, "companions"); TBL = os.path.join(OUT, "ramon_llull-tables")
os.makedirs(TBL, exist_ok=True)

DC = "http://purl.org/dc/elements/1.1/"; DCT = "http://purl.org/dc/terms/"
EDM = "http://www.europeana.eu/schemas/edm/"; FOAF = "http://xmlns.com/foaf/0.1/"
BVPB = "https://bvpb.mcu.es/ns#"; RDFT = "http://www.w3.org/1999/02/22-rdf-syntax-ns#type"
FULLTEXT = BVPB + "fulltext"

# predicate IRI -> column name (order defines the table columns)
COLS = [
    (DC + "title", "title"), (DC + "creator", "creator"), (DC + "contributor", "contributor"),
    (DC + "date", "date"), (DC + "publisher", "publisher"), (DC + "subject", "subject"),
    (DC + "language", "language"), (DC + "type", "type"), (DC + "coverage", "coverage"),
    (DC + "format", "format"), (DC + "description", "description"), (DC + "identifier", "control"),
    (DC + "rights", "rights"), (DCT + "isPartOf", "collection"),
    (EDM + "isShownBy", "pdf"), (EDM + "isShownAt", "landing"), (FOAF + "page", "viewer"),
    (BVPB + "iiifManifest", "iiifManifest"), (BVPB + "pageCount", "pageCount"),
    (BVPB + "ocrEngine", "ocrEngine"),
]
PRED2COL = dict(COLS)
COLNAMES = [c for _, c in COLS]

TRIPLE = re.compile(r'^(<[^>]+>|_:\S+)\s+<([^>]+)>\s+(.+)\s\.\s*$')


def unesc(s):
    return (s.replace("\\n", "\n").replace("\\r", "\r").replace("\\t", "\t")
             .replace('\\"', '"').replace("\\\\", "\\"))


def obj_value(o):
    o = o.strip()
    if o.startswith("<") and o.endswith(">"):
        return o[1:-1]                                   # IRI -> plain
    m = re.match(r'^"(.*)"(?:@[\w-]+|\^\^<[^>]+>)?$', o, re.S)
    return unesc(m.group(1)) if m else o                 # literal lexical -> plain


def parse():
    rows = {}
    for fn in ("ramon_llull.nt", "ramon_llull_ocr.nt"):
        for line in open(os.path.join(ROOT, fn), encoding="utf-8"):
            m = TRIPLE.match(line)
            if not m:
                continue
            s, p, o = m.group(1), m.group(2), m.group(3)
            if p == FULLTEXT:
                continue
            if not s.startswith("<"):
                continue
            subj = s[1:-1]
            r = rows.setdefault(subj, {"entity": subj, "types": [], "_named": {}})
            if p == RDFT:
                r["types"].append(obj_value(o))
            elif p in PRED2COL:
                r["_named"].setdefault(PRED2COL[p], []).append(obj_value(o))
    return rows


def main():
    rows = parse()
    # NDJSON with typed lists for DuckDB read_json
    recs = []
    for subj, r in sorted(rows.items()):
        rec = {"entity": subj,
               "label": (r["_named"].get("title") or [""])[0]}
        for c in COLNAMES:
            rec[c] = r["_named"].get(c, [])
        rec["types"] = r["types"]
        recs.append(rec)
    n = len(recs)
    tmp = os.path.join(OUT, "_rows.ndjson")
    with open(tmp, "w", encoding="utf-8") as f:
        for rec in recs:
            f.write(json.dumps(rec, ensure_ascii=False) + "\n")

    # ---- DuckDB (file) + Parquet ----
    ddb = os.path.join(OUT, "ramon_llull.duckdb")
    if os.path.exists(ddb):
        os.remove(ddb)
    con = duckdb.connect(ddb)
    cols_sql = ", ".join(["entity VARCHAR", "label VARCHAR"] +
                         [f"{c} VARCHAR[]" for c in COLNAMES] + ["types VARCHAR[]"])
    con.execute(f"CREATE TABLE ProvidedCHO ({cols_sql})")
    con.execute(f"INSERT INTO ProvidedCHO SELECT entity, label, "
                + ", ".join(COLNAMES) + ", types FROM read_json_auto('{}', "
                "columns={{{}}})".format(
                    tmp.replace("\\", "/"),
                    ", ".join(["entity:'VARCHAR'", "label:'VARCHAR'"]
                              + [f"{c}:'VARCHAR[]'" for c in COLNAMES]
                              + ["types:'VARCHAR[]'"])))
    con.execute(f"COPY ProvidedCHO TO '{os.path.join(TBL,'ProvidedCHO_.parquet').replace(chr(92),'/')}' (FORMAT parquet)")
    # manifest: column -> predicate map
    con.execute("CREATE TABLE _manifest (tbl VARCHAR, col VARCHAR, predicate VARCHAR)")
    con.executemany("INSERT INTO _manifest VALUES ('ProvidedCHO', ?, ?)",
                    [(c, p) for p, c in COLS])
    con.execute(f"COPY _manifest TO '{os.path.join(TBL,'_manifest.parquet').replace(chr(92),'/')}' (FORMAT parquet)")
    cnt = con.execute("SELECT COUNT(*) FROM ProvidedCHO").fetchone()[0]
    con.close()

    # ---- SQLite (lists as JSON text) ----
    sq = os.path.join(OUT, "ramon_llull.sqlite")
    if os.path.exists(sq):
        os.remove(sq)
    s = sqlite3.connect(sq)
    coldefs = ", ".join(['"entity" TEXT', '"label" TEXT'] +
                        [f'"{c}" TEXT' for c in COLNAMES] + ['"types" TEXT'])
    s.execute(f'CREATE TABLE "ProvidedCHO" ({coldefs})')
    ins = f'INSERT INTO "ProvidedCHO" VALUES ({",".join("?"*(len(COLNAMES)+3))})'
    for rec in recs:
        vals = [rec["entity"], rec["label"]] + \
               [json.dumps(rec[c], ensure_ascii=False) for c in COLNAMES] + \
               [json.dumps(rec["types"], ensure_ascii=False)]
        s.execute(ins, vals)
    s.commit(); s.close()
    os.remove(tmp)

    def mb(p): return f"{os.path.getsize(p)/1e6:.2f} MB"
    print(f"companions: {cnt} ProvidedCHO rows ({n} parsed)")
    print(f"  duckdb  {mb(ddb)}   sqlite {mb(sq)}")
    print(f"  parquet {mb(os.path.join(TBL,'ProvidedCHO_.parquet'))} + _manifest")


if __name__ == "__main__":
    main()
