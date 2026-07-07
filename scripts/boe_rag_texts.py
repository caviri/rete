#!/usr/bin/env python3
"""BOE N-Triples -> data/rag/boe_texts.json ([{iri,title,text}]) for semantic search.

Only the 12,330 in-force NORMS become documents (identified by eli:date_document —
external references and the SKOS vocab concepts are excluded). Each doc's text is
the norm title enriched with its rango and subject (materia) labels, so a query
like 'renewable energy support' matches a law whose title is generic but whose
materias name the topic.
"""
import json
import os
import re

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
NT = os.path.join(ROOT, "data", "boe", "boe.nt")
OUT = os.path.join(ROOT, "data", "rag", "boe_texts.json")
os.makedirs(os.path.dirname(OUT), exist_ok=True)

ELI = "http://data.europa.eu/eli/ontology#"
SKOS = "http://www.w3.org/2004/02/skos/core#"
LINE = re.compile(r'^<([^>]+)>\s+<([^>]+)>\s+(.+?)\s*\.\s*$')


def lit(o):
    m = re.match(r'^"((?:[^"\\]|\\.)*)"', o)
    return m.group(1).replace('\\"', '"').replace("\\n", " ").replace("\\t", " ").strip() if m else None


def iri(o):
    return o[1:-1] if o.startswith("<") and o.endswith(">") else None


title, dated, rango, subjects, preflabel = {}, set(), {}, {}, {}
for ln in open(NT, encoding="utf-8", errors="replace"):
    m = LINE.match(ln.rstrip("\n"))
    if not m:
        continue
    s, p, o = m.groups()
    if p == ELI + "title":
        v = lit(o)
        if v and s not in title:
            title[s] = v
    elif p == ELI + "date_document":
        dated.add(s)
    elif p == ELI + "type_document":
        u = iri(o)
        if u:
            rango[s] = u
    elif p == ELI + "is_about":
        u = iri(o)
        if u:
            subjects.setdefault(s, []).append(u)
    elif p == SKOS + "prefLabel":
        v = lit(o)
        if v and s not in preflabel:
            preflabel[s] = v

docs = []
for s in dated:
    t = title.get(s)
    if not t:
        continue
    parts = [t]
    r = preflabel.get(rango.get(s, ""))
    if r:
        parts.append(r)
    mats = [preflabel[m] for m in subjects.get(s, []) if m in preflabel]
    if mats:
        parts.append("; ".join(mats[:8]))
    text = " · ".join(parts)[:400]
    docs.append({"iri": s, "title": t[:150], "text": text})

json.dump(docs, open(OUT, "w", encoding="utf-8"), ensure_ascii=False)
print(f"boe: {len(docs)} docs -> {OUT}")
