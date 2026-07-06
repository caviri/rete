#!/usr/bin/env python3
"""Parse raw/<key>.txt (CLI label-query output) -> data/rag/<key>_texts.json
([{iri,title,text}]), deduping by IRI and preferring an English label. Datasets
with < 5 labelled entities are skipped (no semantic index)."""
import glob, json, os, re

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
RAG = os.path.join(ROOT, "data", "rag")
LINE = re.compile(r'\?s=<([^>]+)>\s+\?label="(.*)"(?:@([\w-]+)|\^\^<[^>]+>)?\s*$')


def parse(raw):
    best = {}   # iri -> (label, is_en)
    for ln in open(raw, encoding="utf-8", errors="replace"):
        m = LINE.match(ln.strip())
        if not m:
            continue
        iri, label, lang = m.group(1), m.group(2), (m.group(3) or "")
        label = label.replace('\\"', '"').replace("\\n", " ").replace("\\\\", "\\").strip()
        if not label:
            continue
        en = lang.lower().startswith("en")
        if iri not in best or (en and not best[iri][1]):
            best[iri] = (label, en)
    return [{"iri": i, "title": l[:150], "text": l[:400]} for i, (l, _) in best.items()]


def main():
    made = []
    for raw in sorted(glob.glob(os.path.join(RAG, "raw", "*.txt"))):
        key = os.path.splitext(os.path.basename(raw))[0]
        dst = os.path.join(RAG, key + "_texts.json")
        if os.path.exists(dst):
            made.append((key, "exists")); continue
        docs = parse(raw)
        if len(docs) < 5:
            print(f"  {key:24s} skip ({len(docs)} labels)"); continue
        json.dump(docs, open(dst, "w", encoding="utf-8"), ensure_ascii=False)
        made.append((key, len(docs)))
        print(f"  {key:24s} {len(docs)} docs")
    print(f"\n{sum(1 for _,n in made if n!='exists')} new texts.json")


if __name__ == "__main__":
    main()
