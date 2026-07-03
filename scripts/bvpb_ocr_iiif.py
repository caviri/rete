#!/usr/bin/env python
# -*- coding: utf-8 -*-
"""
Full-corpus OCR + IIIF pipeline for the BVPB Ramón Llull PRINTED editions (Libros;
manuscripts are skipped — Tesseract can't read the hands).

  books    : list the printed editions and their page counts.
  extract  : pull every page as a lossless JPEG -> pages/<control>/p-NNNN.jpg
  ocr      : (run separately in the rete-ocr container, parallel over cores)
  build    : per book -> a IIIF v3 manifest (dict-fixed OCR words inline as
             supplementing annotations) + a plain full-text file for the rete
             text index.  Resumable.

Layout under data/bvpb/ramon_llull/:
  pages/<control>/p-NNNN.jpg (+ .hocr after OCR)
  iiif/<control>/manifest.json
  text/<control>.txt
IIIF image/manifest URLs resolve under  https://data.graphplaza.com/ramon_llull/iiif/
"""
import sys, os, re, json, glob, html
from concurrent.futures import ThreadPoolExecutor

ROOT  = os.path.abspath(os.path.join(os.path.dirname(__file__), "..",
                                     "data", "bvpb", "ramon_llull"))
PAGES = os.path.join(ROOT, "pages"); IIIF = os.path.join(ROOT, "iiif")
TEXT  = os.path.join(ROOT, "text")
for d in (PAGES, IIIF, TEXT): os.makedirs(d, exist_ok=True)
BASE  = os.environ.get("IIIF_BASE", "https://data.graphplaza.com/ramon_llull/iiif")

import importlib.util as _il
_sp = _il.spec_from_file_location("latin_fix", os.path.join(os.path.dirname(__file__), "latin_ocr_fix.py"))
latin_fix = _il.module_from_spec(_sp); _sp.loader.exec_module(latin_fix)

WORD = re.compile(
    r'class=["\']ocrx_word["\'][^>]*title=["\']bbox (\d+) (\d+) (\d+) (\d+)'
    r'(?:;\s*x_wconf (\d+))?["\'][^>]*>(.*?)</span>', re.S)


def records():
    return [json.loads(l) for l in open(os.path.join(ROOT, "meta", "records.jsonl"),
            encoding="utf-8") if l.strip()]


def dc(control, tag):
    p = os.path.join(ROOT, "rdf", f"{control}.rdf.xml")
    if not os.path.exists(p):
        return None
    m = re.search(rf"<dc:{tag}[^>]*>([^<]+)</dc:{tag}>", open(p, encoding="utf-8").read())
    return html.unescape(m.group(1).strip()) if m else None


def printed_books():
    out = []
    for r in records():
        t = dc(r["control"], "type") or ""
        if t.lower().startswith("libro") and r.get("pdf_paths"):
            out.append(r)
    return out


# ---------------------------------------------------------------- books
def books():
    import fitz
    bs = printed_books()
    tot = 0
    for r in bs:
        pdf = os.path.join(ROOT, "pdf", f"{r['control']}__{r['pdf_paths'][0]}.pdf")
        d = fitz.open(pdf); n = d.page_count; d.close(); tot += n
        print(f"  {r['control']}  {n:4d}p  {(dc(r['control'],'title') or '')[:52]}")
    print(f"books: {len(bs)} printed editions, {tot} pages")


# ---------------------------------------------------------------- extract
def _extract_one(r):
    import fitz
    ctrl = r["control"]
    outdir = os.path.join(PAGES, ctrl); os.makedirs(outdir, exist_ok=True)
    done = os.path.join(outdir, ".extracted")
    if os.path.exists(done):
        return ctrl, "skip"
    pdf = os.path.join(ROOT, "pdf", f"{ctrl}__{r['pdf_paths'][0]}.pdf")
    d = fitz.open(pdf); idx = []
    for i in range(d.page_count):
        pg = d[i]; imgs = pg.get_images(full=True)
        if imgs:
            xref = max(imgs, key=lambda im: im[2] * im[3])[0]
            info = d.extract_image(xref)
            ext = "jpg" if info["ext"] == "jpeg" else info["ext"]
            fn = f"p-{i+1:04d}.{ext}"
            open(os.path.join(outdir, fn), "wb").write(info["image"])
            idx.append({"n": i + 1, "file": fn, "w": info["width"], "h": info["height"]})
        else:
            pm = pg.get_pixmap(dpi=150); fn = f"p-{i+1:04d}.jpg"
            pm.save(os.path.join(outdir, fn)); idx.append({"n": i + 1, "file": fn, "w": pm.width, "h": pm.height})
    d.close()
    json.dump({"control": ctrl, "pages": idx},
              open(os.path.join(outdir, "pages.json"), "w", encoding="utf-8"))
    open(done, "w").write(str(len(idx)))
    return ctrl, len(idx)


def extract():
    bs = printed_books()
    print(f"extract: {len(bs)} books")
    with ThreadPoolExecutor(max_workers=8) as ex:
        for i, (ctrl, n) in enumerate(ex.map(_extract_one, bs), 1):
            print(f"[{i}/{len(bs)}] {ctrl}: {n} pages")
    print("extract: done")


def parse_hocr(path):
    txt = open(path, encoding="utf-8", errors="replace").read()
    out = []
    for x0, y0, x1, y1, conf, inner in WORD.findall(txt):
        w = html.unescape(re.sub(r"<[^>]+>", "", inner)).strip()
        if w:
            out.append({"t": w, "x": int(x0), "y": int(y0),
                        "w": int(x1) - int(x0), "h": int(y1) - int(y0),
                        "c": int(conf) if conf else -1})
    return out


# ---------------------------------------------------------------- build
def _build_one(r):
    ctrl = r["control"]
    pdir = os.path.join(PAGES, ctrl)
    pj = os.path.join(pdir, "pages.json")
    if not os.path.exists(pj):
        return ctrl, "no-pages"
    # resume: skip a book already built with non-empty text (unless REBUILD=1)
    mf = os.path.join(IIIF, ctrl, "manifest.json")
    tf = os.path.join(TEXT, f"{ctrl}.txt")
    if not os.environ.get("REBUILD") and os.path.exists(mf) \
            and os.path.exists(tf) and os.path.getsize(tf) > 50:
        return ctrl, "cached"
    meta = json.load(open(pj, encoding="utf-8"))
    base = f"{BASE}/{ctrl}"
    canvases, fulltext, nwords = [], [], 0
    for p in meta["pages"]:
        hp = os.path.join(pdir, os.path.splitext(p["file"])[0] + ".hocr")
        words = parse_hocr(hp) if os.path.exists(hp) else []
        if words:
            words, _ = latin_fix.fix_words(words)
        nwords += len(words)
        fulltext.append(" ".join(w["t"] for w in words))
        cid = f"{base}/canvas/p{p['n']}"
        canvas = {"id": cid, "type": "Canvas", "width": p["w"], "height": p["h"],
                  "label": {"none": [f"p. {p['n']}"]},
                  "items": [{"id": f"{base}/page/p{p['n']}/1", "type": "AnnotationPage",
                             "items": [{"id": f"{base}/anno/p{p['n']}-img", "type": "Annotation",
                                        "motivation": "painting",
                                        "body": {"id": f"{base}/{p['file']}",
                                                 "type": "Image", "format": "image/jpeg",
                                                 "width": p["w"], "height": p["h"]},
                                        "target": cid}]}]}
        if words:
            canvas["annotations"] = [{"id": f"{base}/anno/p{p['n']}", "type": "AnnotationPage",
                "items": [{"id": f"{base}/anno/p{p['n']}-w{i}", "type": "Annotation",
                           "motivation": "supplementing",
                           "body": {"type": "TextualBody", "value": w["t"],
                                    "format": "text/plain", "language": "la"},
                           "target": f"{cid}#xywh={w['x']},{w['y']},{w['w']},{w['h']}"}
                          for i, w in enumerate(words)]}]
        canvases.append(canvas)

    title = dc(ctrl, "title") or ctrl
    manifest = {"@context": "http://iiif.io/api/presentation/3/context.json",
        "id": f"{base}/manifest.json", "type": "Manifest",
        "label": {"es": [title]}, "rights": "http://creativecommons.org/licenses/by/4.0/",
        "requiredStatement": {"label": {"en": ["Attribution"]},
            "value": {"en": ["Biblioteca Virtual del Patrimonio Bibliográfico "
                             "(Ministerio de Cultura, España). CC BY 4.0."]}},
        "metadata": [{"label": {"en": ["BVPB record"]},
                      "value": {"none": [f"https://bvpb.mcu.es/es/consulta/registro.do?id={r.get('id')}"]}},
                     {"label": {"en": ["OCR"]},
                      "value": {"en": ["Tesseract (lat) + dictionary long-s correction — automatic, uncorrected"]}}],
        "items": canvases}
    od = os.path.join(IIIF, ctrl); os.makedirs(od, exist_ok=True)
    json.dump(manifest, open(os.path.join(od, "manifest.json"), "w", encoding="utf-8"),
              ensure_ascii=False)
    ft = "\n".join(fulltext)
    open(os.path.join(TEXT, f"{ctrl}.txt"), "w", encoding="utf-8").write(ft)
    return ctrl, (len(canvases), nwords)


def build():
    bs = printed_books()
    print(f"build: {len(bs)} books")
    with ThreadPoolExecutor(max_workers=8) as ex:
        for i, (ctrl, res) in enumerate(ex.map(_build_one, bs), 1):
            print(f"[{i}/{len(bs)}] {ctrl}: {res}")
    print("build: done")


if __name__ == "__main__":
    {"books": books, "extract": extract, "build": build}[sys.argv[1]]()
