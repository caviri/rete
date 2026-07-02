#!/usr/bin/env python
# -*- coding: utf-8 -*-
"""
Harvest the Ramón Llull collection from the Biblioteca Virtual del Patrimonio
Bibliográfico (BVPB, Spanish Ministry of Culture).  Content is CC BY 4.0.

Bounded, curated microsite: the author-authority "Ramón Llull" (BVPB20110039301)
resolves to 146 records.  Each record exposes:
  - registro.do?control=BVPB...        (HTML view, all displayed fields + numeric id)
  - download_registro.do?...rdf_dc_export  (clean RDF Dublin Core metadata)
  - one or more catalogo_imagenes/grupo.do?path=NNN  -> the whole digitised
    object as a single PDF (page images, DCTDecode)

Phases (argv):  enum | meta | pdf | all
State is kept in meta/records.jsonl so every phase is resumable.

Plain requests + BeautifulSoup, polite single-threaded rate limiting.
"""
import sys, os, re, json, time, random, zipfile, io
import requests
from bs4 import BeautifulSoup

ROOT   = os.path.join(os.path.dirname(__file__), "..", "data", "bvpb", "ramon_llull")
ROOT   = os.path.abspath(ROOT)
META   = os.path.join(ROOT, "meta"); RDF = os.path.join(ROOT, "rdf")
PDF    = os.path.join(ROOT, "pdf");  RAW = os.path.join(ROOT, "raw")
for d in (META, RDF, PDF, RAW): os.makedirs(d, exist_ok=True)
RECORDS = os.path.join(META, "records.jsonl")

BASE      = "https://bvpb.mcu.es/ramon_llull/es/consulta"
IMG       = "https://bvpb.mcu.es/ramon_llull/es/catalogo_imagenes"
AUTHORITY = "BVPB20110039301"
DELAY     = 1.3      # seconds between requests (polite)
UA = ("Mozilla/5.0 (compatible; ramon-llull-heritage-harvest/1.0; "
      "CC-BY-4.0 metadata; +mailto:carlosvivarrios@gmail.com)")

S = requests.Session()
S.headers.update({"User-Agent": UA, "Accept-Language": "es"})


def get(url, tries=4, stream=False, timeout=120):
    last = None
    for i in range(tries):
        try:
            r = S.get(url, stream=stream, timeout=timeout)
            if r.status_code == 200:
                return r
            last = f"HTTP {r.status_code}"
        except Exception as e:
            last = str(e)
        time.sleep(2 * (i + 1) + random.random())
    print(f"  ! give up {url} ({last})")
    return None


def nap():
    time.sleep(DELAY + random.random() * 0.6)


# ---------------------------------------------------------------- enum
def enum():
    """Collect all record control numbers from the paginated result list."""
    r = get(f"{BASE}/resultados_navegacion.do?busq_autoridadesbib={AUTHORITY}")
    if not r:
        sys.exit("enum: first page failed")
    h = r.text
    total = int(re.search(r"de\s+(\d+)", h).group(1))
    sess  = re.search(r"resultados_navegacion\.do\?id=(\d+)", h).group(1)
    print(f"enum: {total} records, session id={sess}")

    def controls(html):
        return re.findall(r"data-analytics-recordid='(BVPB\d+)'", html)

    seen, order = set(), []
    def add(html):
        for c in controls(html):
            if c not in seen and c != AUTHORITY:
                seen.add(c); order.append(c)

    # Fixed alphabetical sort so pages tile exactly (default relevance sort drifts).
    def page(pos):
        return (f"{BASE}/resultados_navegacion.do?id={sess}"
                f"&campoOrden=tituloorden&ordenDesc=N&posicion={pos}")

    for pos in range(1, total + 1, 50):
        nap()
        rp = get(page(pos))
        if rp: add(rp.text)
        print(f"  page @pos {pos}: total so far {len(order)}")
    if len(order) < total:  # one stable re-sweep to catch any stragglers
        for pos in range(1, total + 1, 50):
            nap(); rp = get(page(pos))
            if rp: add(rp.text)
        print(f"  re-sweep: total {len(order)}")

    with open(RECORDS, "w", encoding="utf-8") as f:
        for c in order:
            f.write(json.dumps({"control": c}) + "\n")
    print(f"enum: wrote {len(order)} controls -> {RECORDS}")


def load_records():
    if not os.path.exists(RECORDS): return []
    out = []
    with open(RECORDS, encoding="utf-8") as f:
        for line in f:
            line = line.strip()
            if line: out.append(json.loads(line))
    return out


def save_records(recs):
    tmp = RECORDS + ".tmp"
    with open(tmp, "w", encoding="utf-8") as f:
        for r in recs:
            f.write(json.dumps(r, ensure_ascii=False) + "\n")
    os.replace(tmp, RECORDS)


# ---------------------------------------------------------------- meta
def meta():
    recs = load_records()
    for i, rec in enumerate(recs, 1):
        if rec.get("id") and rec.get("meta_ok"):
            continue
        ctrl = rec["control"]
        nap()
        r = get(f"{BASE}/registro.do?control={ctrl}")
        if not r:
            continue
        html = r.content.decode("latin-1", "replace")
        open(os.path.join(RAW, f"{ctrl}.html"), "w", encoding="utf-8").write(html)
        soup = BeautifulSoup(html, "lxml")

        mid = re.search(r"registro\.do\?id=(\d+)", html)
        rec["id"] = mid.group(1) if mid else None
        rec["grupos"] = sorted(set(re.findall(r"grupo\.do\?path=(\d+)", html)))

        t = soup.find("title")
        rec["title"] = (t.get_text().split(">")[-1].strip() if t else None)

        # clean RDF Dublin Core export (structured metadata backbone)
        if rec["id"]:
            nap()
            rr = get(f"{BASE}/download_registro.do?id={rec['id']}"
                     f"&formato=rdf_dc_export&encoding=ISO-8859-1&holdings=S&salida=salDescarga")
            if rr and rr.content.lstrip().startswith(b"<?xml"):
                open(os.path.join(RDF, f"{ctrl}.rdf.xml"), "wb").write(rr.content)
                rec["rdf"] = f"rdf/{ctrl}.rdf.xml"

        rec["meta_ok"] = True
        print(f"[{i}/{len(recs)}] {ctrl} id={rec['id']} grupos={rec['grupos']} :: {rec['title']}")
        if i % 10 == 0:
            save_records(recs)
    save_records(recs)
    ok = sum(1 for r in recs if r.get("meta_ok"))
    ng = sum(len(r.get("grupos") or []) for r in recs)
    print(f"meta: {ok}/{len(recs)} records, {ng} PDF groups referenced")


# ---------------------------------------------------------------- pdf
def human(n):
    for u in "B KB MB GB".split():
        if n < 1024: return f"{n:.1f}{u}"
        n /= 1024
    return f"{n:.1f}TB"


def is_pdf_group(path):
    """The digitised object is served as application/pdf from ONE of a record's
    grupo paths; the other is an HTML image-viewer.  HEAD-probe the content-type."""
    try:
        h = S.head(f"{IMG}/grupo.do?path={path}", timeout=60, allow_redirects=True)
        return "pdf" in h.headers.get("Content-Type", "").lower()
    except Exception:
        return False


def pdf():
    recs = load_records()
    done = tot = viewers = 0
    for k, rec in enumerate(recs, 1):
        ctrl = rec["control"]
        pdf_paths, viewer_paths = [], []
        for path in (rec.get("grupos") or []):
            out = os.path.join(PDF, f"{ctrl}__{path}.pdf")
            if os.path.exists(out) and os.path.getsize(out) > 1024:
                pdf_paths.append(path); done += 1; tot += os.path.getsize(out); continue
            nap()
            if not is_pdf_group(path):
                viewer_paths.append(path); viewers += 1; continue   # HTML viewer variant
            nap()
            r = get(f"{IMG}/grupo.do?path={path}", stream=True, timeout=600)
            if not r:
                continue
            tmp = out + ".part"; size = 0
            with open(tmp, "wb") as f:
                for chunk in r.iter_content(1 << 16):
                    if chunk:
                        f.write(chunk); size += len(chunk)
            if open(tmp, "rb").read(5) != b"%PDF-":
                os.replace(tmp, out + ".bin")
                print(f"  ! not a PDF path={path}; kept as .bin"); continue
            os.replace(tmp, out)
            pdf_paths.append(path); done += 1; tot += size
            print(f"[{k}/{len(recs)}] {ctrl}__{path}.pdf  {human(size)}  (cum {human(tot)})")
        rec["pdf_paths"] = pdf_paths
        rec["viewer_paths"] = viewer_paths
        if k % 10 == 0:
            save_records(recs)
    save_records(recs)
    print(f"pdf: {done} PDFs saved, {viewers} viewer groups skipped, total {human(tot)}")


if __name__ == "__main__":
    phase = sys.argv[1] if len(sys.argv) > 1 else "all"
    if phase in ("enum", "all"): enum()
    if phase in ("meta", "all"): meta()
    if phase in ("pdf",  "all"): pdf()
