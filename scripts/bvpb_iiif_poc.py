#!/usr/bin/env python
# -*- coding: utf-8 -*-
"""
IIIF + OCR proof-of-concept for ONE BVPB Ramón Llull book.

  extract  : pull the first N page images out of the PDF (lossless - the pages
             are JPEG-in-PDF) with PyMuPDF, record pixel dimensions.
  build    : after Tesseract has written <page>.hocr next to each image, parse
             the hOCR (word + bbox + confidence) and emit
               - manifest.json : a IIIF Presentation 3.0 manifest (one Canvas per
                 page; a painting Annotation for the image; a supplementing
                 AnnotationPage of the OCR words, each targeting canvas#xywh).
               - viewer.html   : a self-contained proof viewer (image + hoverable/
                 selectable OCR word overlay, no external libraries).
             and print OCR-quality stats.

OCR itself runs in the rete-ocr image between the two phases:
  docker run --rm -v "$PWD/<dir>:/work" rete-ocr:latest \
    sh -c 'cd /work && for f in *.jpg; do tesseract "$f" "${f%.jpg}" hocr -l lat; done'
"""
import sys, os, re, json, glob, html
try:
    import importlib.util as _il
    _sp = _il.spec_from_file_location("latin_ocr_fix",
            os.path.join(os.path.dirname(__file__), "latin_ocr_fix.py"))
    latin_fix = _il.module_from_spec(_sp); _sp.loader.exec_module(latin_fix)
except Exception:
    latin_fix = None

ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), "..",
                                    "data", "bvpb", "ramon_llull"))
POC  = os.path.join(ROOT, "iiif_poc")
# Base URL the manifest's image/id fields point at. Serve the book dir here, or
# override with env IIIF_BASE (e.g. an R2 path once published).
BASE = os.environ.get("IIIF_BASE", "http://127.0.0.1:8892")
NPAGES = int(os.environ.get("IIIF_NPAGES", "8"))


def load_rec(control):
    for line in open(os.path.join(ROOT, "meta", "records.jsonl"), encoding="utf-8"):
        if line.strip():
            r = json.loads(line)
            if r["control"] == control:
                return r
    sys.exit(f"control {control} not found")


def extract(control):
    import fitz  # PyMuPDF
    rec = load_rec(control)
    pdf = os.path.join(ROOT, "pdf", f"{control}__{rec['pdf_paths'][0]}.pdf")
    outdir = os.path.join(POC, control)
    os.makedirs(outdir, exist_ok=True)
    doc = fitz.open(pdf)
    n = min(NPAGES, doc.page_count)
    pages = []
    for i in range(n):
        pg = doc[i]
        imgs = pg.get_images(full=True)
        fn = f"page-{i+1:04d}.jpg"
        if imgs:                                   # lossless: pull the embedded JPEG
            xref = max(imgs, key=lambda im: im[2] * im[3])[0]
            info = doc.extract_image(xref)
            ext = info["ext"]
            fn = f"page-{i+1:04d}.{ext}"
            open(os.path.join(outdir, fn), "wb").write(info["image"])
            w, h = info["width"], info["height"]
        else:                                      # fallback: rasterize at 150 dpi
            pm = pg.get_pixmap(dpi=150)
            pm.save(os.path.join(outdir, fn)); w, h = pm.width, pm.height
        pages.append({"n": i + 1, "file": fn, "w": w, "h": h})
        print(f"  page {i+1}: {fn}  {w}x{h}")
    json.dump({"control": control, "title": rec.get("title"), "id": rec.get("id"),
               "pages": pages}, open(os.path.join(outdir, "pages.json"), "w",
                                     encoding="utf-8"), ensure_ascii=False, indent=1)
    print(f"extract: {n} pages -> {outdir}")
    print(f"\nNow OCR them:\n  docker run --rm -v \"{outdir}:/work\" rete-ocr:latest \\\n"
          f"    sh -c 'cd /work && for f in *.jpg *.png; do [ -e \"$f\" ] && "
          f"tesseract \"$f\" \"${{f%.*}}\" hocr -l lat; done'")


WORD = re.compile(
    r'class=["\']ocrx_word["\'][^>]*title=["\']bbox (\d+) (\d+) (\d+) (\d+)'
    r'(?:;\s*x_wconf (\d+))?["\'][^>]*>(.*?)</span>', re.S)


def parse_hocr(path):
    txt = open(path, encoding="utf-8", errors="replace").read()
    out = []
    for x0, y0, x1, y1, conf, inner in WORD.findall(txt):
        w = html.unescape(re.sub(r"<[^>]+>", "", inner)).strip()
        if not w:
            continue
        out.append({"t": w, "x": int(x0), "y": int(y0),
                    "w": int(x1) - int(x0), "h": int(y1) - int(y0),
                    "c": int(conf) if conf else -1})
    return out


def build(control):
    outdir = os.path.join(POC, control)
    meta = json.load(open(os.path.join(outdir, "pages.json"), encoding="utf-8"))
    base = f"{BASE}/{control}"
    canvases, all_words, confs = [], 0, []

    for p in meta["pages"]:
        hp = os.path.join(outdir, os.path.splitext(p["file"])[0] + ".hocr")
        words = parse_hocr(hp) if os.path.exists(hp) else []
        if latin_fix and words:                      # dictionary-guided long-s repair
            words, _ = latin_fix.fix_words(words)    # each word gains 't' (fixed) + 'raw'
        p["words"] = words
        all_words += len(words)
        confs += [w["c"] for w in words if w["c"] >= 0]
        cid = f"{base}/canvas/p{p['n']}"
        img_url = f"{base}/{p['file']}"
        anno_img = {"id": f"{base}/anno/p{p['n']}-img", "type": "Annotation",
                    "motivation": "painting",
                    "body": {"id": img_url, "type": "Image", "format": "image/jpeg",
                             "width": p["w"], "height": p["h"]},
                    "target": cid}
        ocr_annos = [{
            "id": f"{base}/anno/p{p['n']}-w{i}", "type": "Annotation",
            "motivation": "supplementing",
            "body": {"type": "TextualBody", "value": w["t"],
                     "format": "text/plain", "language": "la"},
            "target": f"{cid}#xywh={w['x']},{w['y']},{w['w']},{w['h']}"}
            for i, w in enumerate(words)]
        canvas = {"id": cid, "type": "Canvas", "width": p["w"], "height": p["h"],
                  "label": {"none": [f"p. {p['n']}"]},
                  "items": [{"id": f"{base}/page/p{p['n']}/1",
                             "type": "AnnotationPage", "items": [anno_img]}]}
        if ocr_annos:
            canvas["annotations"] = [{"id": f"{base}/page/p{p['n']}/ocr",
                                      "type": "AnnotationPage", "items": ocr_annos}]
        canvases.append(canvas)

    manifest = {
        "@context": "http://iiif.io/api/presentation/3/context.json",
        "id": f"{base}/manifest.json", "type": "Manifest",
        "label": {"es": [meta.get("title") or control]},
        "rights": "http://creativecommons.org/licenses/by/4.0/",
        "requiredStatement": {"label": {"en": ["Attribution"]},
            "value": {"en": ["Biblioteca Virtual del Patrimonio Bibliográfico "
                             "(Ministerio de Cultura, España). CC BY 4.0."]}},
        "metadata": [
            {"label": {"en": ["BVPB record"]},
             "value": {"none": [f"https://bvpb.mcu.es/es/consulta/registro.do?id={meta.get('id')}"]}},
            {"label": {"en": ["Control number"]}, "value": {"none": [control]}},
            {"label": {"en": ["OCR"]}, "value": {"en": ["Tesseract (lat) - automatic, uncorrected"]}}],
        "items": canvases}
    json.dump(manifest, open(os.path.join(outdir, "manifest.json"), "w",
                             encoding="utf-8"), ensure_ascii=False, indent=1)

    write_viewer(outdir, meta, control)

    mean = sum(confs) / len(confs) if confs else 0
    good = sum(1 for c in confs if c >= 60)
    print(f"build: {len(canvases)} canvases, {all_words} OCR words")
    print(f"  mean word confidence: {mean:.1f} | words >=60 conf: {good}/{len(confs)}"
          f" ({100*good/len(confs) if confs else 0:.0f}%)")
    print(f"  manifest.json + viewer.html -> {outdir}")
    # a taste of the recognised text (page 1, first ~30 words)
    if meta["pages"] and meta["pages"][0].get("words"):
        sample = " ".join(w["t"] for w in meta["pages"][0]["words"][:30])
        print(f"  page 1 OCR sample: {sample!r}")


def write_viewer(outdir, meta, control):
    """Self-contained proof viewer: page image + positioned transparent OCR word
    boxes (hover shows the recognised token; drag-select copies the text). No libs."""
    pages_js = json.dumps([{"file": p["file"], "w": p["w"], "h": p["h"],
                            "words": p.get("words", [])} for p in meta["pages"]],
                          ensure_ascii=False)
    title = html.escape(meta.get("title") or control)
    doc = """<!doctype html><html lang="es"><head><meta charset="utf-8">
<title>IIIF/OCR PoC — %s</title>
<style>
 body{font:14px/1.5 system-ui,sans-serif;margin:0;background:#1a1a1e;color:#ddd}
 header{padding:10px 16px;background:#26262c;position:sticky;top:0;z-index:5}
 header b{color:#fff} header .att{color:#999;font-size:12px}
 #nav{margin:8px 0} button{background:#3a3a44;color:#eee;border:0;padding:4px 10px;border-radius:5px;cursor:pointer;margin-right:6px}
 #wrap{position:relative;margin:16px auto;width:max-content;box-shadow:0 4px 24px #000}
 #wrap img{display:block;max-width:96vw;height:auto}
 .w{position:absolute;border:1px solid rgba(90,180,255,.35);background:rgba(90,180,255,.10);cursor:text}
 .w:hover{background:rgba(90,180,255,.30);border-color:#5ab4ff}
 .w.lo{border-color:rgba(255,140,90,.4)}          /* low-confidence word */
 #tip{position:fixed;background:#000;color:#8fd0ff;padding:2px 7px;border-radius:4px;font-size:13px;pointer-events:none;display:none;z-index:9}
 label{font-size:12px;color:#bbb}
</style></head><body>
<header><b>IIIF + OCR proof-of-concept</b> — %s
 <div class="att">BVPB / Ministerio de Cultura · CC BY 4.0 · OCR: Tesseract (lat), uncorrected · toggle the overlay to see the text layer</div>
 <div id="nav"><button onclick="go(-1)">‹ Prev</button><span id="pg"></span><button onclick="go(1)">Next ›</button>
  <label><input type="checkbox" id="ov" checked onchange="draw()"> show OCR overlay</label></div>
</header>
<div id="wrap"><img id="im"><div id="boxes"></div></div>
<div id="tip"></div>
<script>
const PAGES=%s; let i=0;
const im=document.getElementById('im'),boxes=document.getElementById('boxes'),tip=document.getElementById('tip');
function draw(){
 const p=PAGES[i]; im.src=p.file; document.getElementById('pg').textContent=` p.${i+1}/${PAGES.length} · ${p.words.length} words `;
 im.onload=()=>{ const s=im.clientWidth/p.w; boxes.innerHTML='';
  if(!document.getElementById('ov').checked)return;
  for(const w of p.words){ const d=document.createElement('div'); d.className='w'+(w.c>=0&&w.c<60?' lo':'');
   d.style.left=(w.x*s)+'px'; d.style.top=(w.y*s)+'px'; d.style.width=(w.w*s)+'px'; d.style.height=(w.h*s)+'px';
   d.title=w.t+(w.c>=0?' ['+w.c+']':''); d.textContent=w.t; d.style.color='transparent'; d.style.overflow='hidden';
   d.onmousemove=e=>{tip.style.display='block';tip.style.left=(e.clientX+12)+'px';tip.style.top=(e.clientY+12)+'px';tip.textContent=w.t+(w.c>=0?'  ('+w.c+'%%)':'');};
   d.onmouseleave=()=>tip.style.display='none'; boxes.appendChild(d);} };
}
function go(d){ i=(i+d+PAGES.length)%%PAGES.length; draw(); }
draw();
</script></body></html>""" % (title, title, pages_js)
    open(os.path.join(outdir, "viewer.html"), "w", encoding="utf-8").write(doc)


if __name__ == "__main__":
    phase = sys.argv[1]; control = sys.argv[2] if len(sys.argv) > 2 else "BVPB20070009925"
    {"extract": extract, "build": build}[phase](control)
