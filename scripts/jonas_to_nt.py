#!/usr/bin/env python3
"""Convert the LostMa-ERC Heurist DuckDB (derived from Jonas) to N-Triples.

Source: https://github.com/LostMa-ERC/data-pipeline-output (databases/heurist.duckdb),
the published export of the LostMa Heurist database built from Jonas (IRHT-CNRS,
https://jonas.irht.cnrs.fr/). CC0 1.0.

Model: each Heurist record (its `H-ID`) becomes an entity; columns ending in
`H-ID` are object relations (possibly multi-valued), columns ending in `TRM-ID`
link to the controlled vocabulary (`trm`), Heurist temporal objects become
xsd:date begin/end, and `described_at_URL` links each record back to its live
Jonas page. The `trm` table is emitted as a SKOS vocabulary and `rty` as classes.

Usage (Docker python, or `uv run --with duckdb --with pandas`):
    python scripts/jonas_to_nt.py [data/jonas/heurist.duckdb] [data/jonas/jonas.nt]
Then: rete build data/jonas/jonas.nt -o data/jonas/jonas.rete --pyramid-algo types --card
"""
import re
import sys
import duckdb
import numpy as np
import pandas as pd

DB = sys.argv[1] if len(sys.argv) > 1 else "data/jonas/heurist.duckdb"
OUT = sys.argv[2] if len(sys.argv) > 2 else "data/jonas/jonas.nt"

ENT = "https://lostma-erc.github.io/jonas/id/"
PROP = "https://lostma-erc.github.io/jonas/prop/"
TYPE = "https://lostma-erc.github.io/jonas/type/"
TERM = "https://lostma-erc.github.io/jonas/term/"
RDF = "http://www.w3.org/1999/02/22-rdf-syntax-ns#"
RDFS = "http://www.w3.org/2000/01/rdf-schema#"
SKOS = "http://www.w3.org/2004/02/skos/core#"
OWL = "http://www.w3.org/2002/07/owl#"
XSD = "http://www.w3.org/2001/XMLSchema#"

con = duckdb.connect(DB, read_only=True)
RTY = {r[0]: r[1] for r in con.execute("SELECT rty_ID, rty_Name FROM rty").fetchall()}
TRM = {r[0]: r[1] for r in con.execute("SELECT trm_ID, trm_Label FROM trm").fetchall()}
DOMAIN = ["Digitization", "DocumentTable", "Footnote", "Genre", "Images", "Part",
          "PhysDesc", "Place", "Repository", "Scripta", "Stemma", "Story",
          "Storyverse", "TextTable", "Witness"]
TITLE = ["preferred_name", "preferred_siglum", "current_shelfmark", "place_name",
         "label_name", "invented_label"]


def esc(s):
    return (str(s).replace("\\", "\\\\").replace('"', '\\"')
            .replace("\n", "\\n").replace("\r", "\\r").replace("\t", "\\t"))


def I(u):
    return "<" + u + ">"


def L(v, dt=None):
    s = '"' + esc(v) + '"'
    return s + "^^<" + dt + ">" if dt else s


def empty(v):
    if v is None:
        return True
    if isinstance(v, float) and pd.isna(v):
        return True
    if isinstance(v, np.ndarray):
        return v.size == 0
    if isinstance(v, (list, tuple)):
        return len(v) == 0
    if isinstance(v, str) and v.strip() == "":
        return True
    return False


def aslist(v):
    if isinstance(v, (np.ndarray, list, tuple)):
        return [x for x in list(v) if not empty(x)]
    return [v]


def clean(col):
    b = re.sub(r" (H-ID|TRM-ID)$", "", col).replace("_COLUMN", "")
    return re.sub(r"[^A-Za-z0-9_]", "_", b.strip())


def date_isos(d):
    out = {}
    try:
        if d.get("value") and d["value"].get("iso"):
            out["d"] = d["value"]["iso"]
        for k, src in (("min", "estMinDate"), ("max", "estMaxDate")):
            if d.get(src) and d[src].get("iso"):
                out[k] = d[src]["iso"]
        st = d.get("start") or {}
        en = d.get("end") or {}
        if st.get("earliest") and st["earliest"].get("iso"):
            out.setdefault("min", st["earliest"]["iso"])
        if en.get("latest") and en["latest"].get("iso"):
            out.setdefault("max", en["latest"]["iso"])
    except Exception:
        pass
    return out


n = [0]
f = open(OUT, "w", encoding="utf-8")


def emit(s, p, o):
    f.write(s + " " + p + " " + o + " .\n")
    n[0] += 1


for t in DOMAIN:
    df = con.execute('SELECT * FROM "%s"' % t).df()
    for rec in df.to_dict("records"):
        if empty(rec.get("H-ID")):
            continue
        s = I(ENT + str(int(rec["H-ID"])))
        tn = (RTY.get(int(rec["type_id"]), t).strip().replace(" ", "_")
              if not empty(rec.get("type_id")) else t)
        emit(s, I(RDF + "type"), I(TYPE + tn))
        for tf in TITLE:
            if tf in rec and not empty(rec[tf]):
                vs = aslist(rec[tf])
                if vs:
                    emit(s, I(RDFS + "label"), L(vs[0]))
                    break
        for col, v in rec.items():
            if col in ("H-ID", "type_id") or empty(v):
                continue
            if col.endswith("H-ID"):
                p = I(PROP + clean(col))
                for x in aslist(v):
                    try:
                        emit(s, p, I(ENT + str(int(x))))
                    except Exception:
                        pass
            elif col.endswith("TRM-ID"):
                p = I(PROP + clean(col) + "_term")
                for x in aslist(v):
                    try:
                        emit(s, p, I(TERM + str(int(x))))
                    except Exception:
                        pass
            elif isinstance(v, dict):
                iso = date_isos(v)
                base = clean(col)
                if "d" in iso:
                    emit(s, I(PROP + base), L(iso["d"][:10], XSD + "date"))
                if "min" in iso:
                    emit(s, I(PROP + base + "_earliest"), L(iso["min"][:10], XSD + "date"))
                if "max" in iso:
                    emit(s, I(PROP + base + "_latest"), L(iso["max"][:10], XSD + "date"))
            else:
                base = clean(col)
                p = I(PROP + base)
                for x in aslist(v):
                    if isinstance(x, dict):
                        iso = date_isos(x)
                        if "d" in iso:
                            emit(s, p, L(iso["d"][:10], XSD + "date"))
                        continue
                    if isinstance(x, (bool, np.bool_)):
                        emit(s, p, L("true" if x else "false", XSD + "boolean"))
                        continue
                    if isinstance(x, (int, np.integer)):
                        emit(s, p, L(str(int(x)), XSD + "integer"))
                        continue
                    if isinstance(x, (float, np.floating)):
                        emit(s, p, L(repr(float(x)), XSD + "decimal"))
                        continue
                    xs = str(x).strip()
                    if not xs or xs.lower() == "nan":
                        continue
                    if (xs.startswith("http://") or xs.startswith("https://")) and " " not in xs:
                        emit(s, p, I(xs))
                    elif xs in ("Yes", "No"):
                        emit(s, p, L("true" if xs == "Yes" else "false", XSD + "boolean"))
                    else:
                        emit(s, p, L(xs))
        for g in (aslist(rec.get("geonames_id")) if not empty(rec.get("geonames_id")) else []):
            try:
                emit(s, I(OWL + "sameAs"), I("https://sws.geonames.org/%d/" % int(g)))
            except Exception:
                pass
        for vf in (aslist(rec.get("VIAF")) if not empty(rec.get("VIAF")) else []):
            vfs = str(vf).strip()
            if vfs.isdigit():
                emit(s, I(OWL + "sameAs"), I("http://viaf.org/viaf/" + vfs))

for tid, label in TRM.items():
    if tid is None:
        continue
    ts = I(TERM + str(int(tid)))
    emit(ts, I(RDF + "type"), I(SKOS + "Concept"))
    if label and str(label).strip():
        emit(ts, I(SKOS + "prefLabel"), L(label))
for r in con.execute("SELECT trm_ID,trm_ParentTermID FROM trm WHERE trm_ParentTermID IS NOT NULL").fetchall():
    try:
        emit(I(TERM + str(int(r[0]))), I(SKOS + "broader"), I(TERM + str(int(r[1]))))
    except Exception:
        pass
for tid, name in RTY.items():
    if not name:
        continue
    cn = I(TYPE + name.strip().replace(" ", "_"))
    emit(cn, I(RDF + "type"), I(RDFS + "Class"))
    emit(cn, I(RDFS + "label"), L(name))

f.close()
print("triples:", n[0])
