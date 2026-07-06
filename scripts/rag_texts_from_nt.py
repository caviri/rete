#!/usr/bin/env python3
"""N-Triples -> data/rag/<key>_texts.json ([{iri,title,text}]) for RAG. Picks the
best label per subject over a broad predicate set (rdfs:label, dcterms:title,
dc:elements title, foaf:name, schema:name, skos:prefLabel), preferring en/es."""
import json, re, sys

LABELS = {
    "http://www.w3.org/2000/01/rdf-schema#label",
    "http://purl.org/dc/terms/title",
    "http://purl.org/dc/elements/1.1/title",
    "http://xmlns.com/foaf/0.1/name",
    "http://schema.org/name",
    "http://www.w3.org/2004/02/skos/core#prefLabel",
}
nt, key = sys.argv[1], sys.argv[2]
LINE = re.compile(r'^<([^>]+)>\s+<([^>]+)>\s+"((?:[^"\\]|\\.)*)"(?:@(\w[\w-]*)|\^\^<[^>]+>)?\s*\.\s*$')

best = {}
for ln in open(nt, encoding="utf-8", errors="replace"):
    m = LINE.match(ln.strip())
    if not m:
        continue
    s, p, lit, lang = m.groups()
    if p not in LABELS:
        continue
    lit = lit.replace('\\"', '"').replace("\\n", " ").replace("\\t", " ").strip()
    if not lit:
        continue
    pref = (lang or "").lower().startswith(("en", "es"))
    if s not in best or (pref and not best[s][1]):
        best[s] = (lit, pref)

docs = [{"iri": s, "title": l[:150], "text": l[:400]} for s, (l, _) in best.items()]
json.dump(docs, open(f"data/rag/{key}_texts.json", "w", encoding="utf-8"), ensure_ascii=False)
print(f"{key}: {len(docs)} docs")
