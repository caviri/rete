#!/usr/bin/env python
# -*- coding: utf-8 -*-
"""
Supplement the Ramón Llull graph with the OCR/IIIF layer for the printed editions:
  <record> bvpb:iiifManifest <manifest URL>   (page-turning viewer + text overlay)
  <record> bvpb:fulltext     "<corrected OCR text>"   (feeds the rete text index)
  <record> bvpb:pageCount    <n>
  <record> bvpb:ocrEngine    "Tesseract (lat) + dictionary long-s correction"
Output: data/bvpb/ramon_llull/ramon_llull_ocr.nt  (merged with ramon_llull.nt at build).
"""
import os, json, glob

ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), "..",
                                    "data", "bvpb", "ramon_llull"))
TEXT = os.path.join(ROOT, "text"); PAGES = os.path.join(ROOT, "pages")
OUT  = os.path.join(ROOT, "ramon_llull_ocr.nt")
BASE = "https://data.graphplaza.com/ramon_llull/iiif"
BVPB = "https://bvpb.mcu.es/ns#"


def esc(s):
    return (s.replace("\\", "\\\\").replace('"', '\\"')
             .replace("\n", "\\n").replace("\r", "\\r").replace("\t", "\\t"))


def main():
    recs = {r["control"]: r for r in
            (json.loads(l) for l in open(os.path.join(ROOT, "meta", "records.jsonl"),
             encoding="utf-8") if l.strip())}
    n = tw = 0
    with open(OUT, "w", encoding="utf-8") as w:
        for txtf in sorted(glob.glob(os.path.join(TEXT, "*.txt"))):
            ctrl = os.path.splitext(os.path.basename(txtf))[0]
            rec = recs.get(ctrl)
            if not rec or not rec.get("id"):
                continue
            subj = f"<https://bvpb.mcu.es/es/consulta/registro.do?id={rec['id']}>"
            text = open(txtf, encoding="utf-8").read().strip()
            if not text:
                continue
            pj = os.path.join(PAGES, ctrl, "pages.json")
            npages = len(json.load(open(pj, encoding="utf-8"))["pages"]) if os.path.exists(pj) else 0
            w.write(f'{subj} <{BVPB}iiifManifest> <{BASE}/{ctrl}/manifest.json> .\n')
            w.write(f'{subj} <{BVPB}pageCount> "{npages}"^^<http://www.w3.org/2001/XMLSchema#integer> .\n')
            w.write(f'{subj} <{BVPB}ocrEngine> "Tesseract (lat) + dictionary long-s correction"@en .\n')
            w.write(f'{subj} <{BVPB}fulltext> "{esc(text)}"@la .\n')
            n += 1; tw += len(text.split())
    print(f"wrote {OUT}: {n} printed books, ~{tw:,} OCR words indexed")


if __name__ == "__main__":
    main()
