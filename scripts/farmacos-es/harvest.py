#!/usr/bin/env python3
"""Harvest CIMA (AEMPS) - all medicines authorized in Spain, into data/farmacos-es/raw/.

Source: https://cima.aemps.es/ (REST API, public) + Nomenclator de Prescripcion.

Layout under data/farmacos-es/raw/:
  nomenclator/prescripcion.zip + extracted XML   (full catalog + dictionaries)
  medicamentos/pages/page_NNNN.json              (list endpoint, 200/page)
  medicamentos/detalle/<nregistro>.json          (full per-medicine JSON)
  docs/ft/FT_<nregistro>.html                    (ficha tecnica, HTML)
  docs/p/P_<nregistro>.html                      (prospecto, HTML)
  docs/ft_secc/<nregistro>.json                  (ficha tecnica segmented sections)
  docs/p_secc/<nregistro>.json                   (prospecto segmented sections)
  notas/<nregistro>.json                         (AEMPS safety notes, when flagged)
  materiales/<nregistro>.json                    (informative materials, when flagged)
  presentaciones/pages/page_NNNN.json            (CN-level presentations)
  psuministro/page_NNNN.json                     (active supply problems)
  _errors.jsonl                                  (failed URLs after retries)

Resumable: existing non-empty files are skipped. Usage:
  python harvest.py [--limit N] [--only phase]   (phases: nomenclator,pages,detalle,docs,notas,materiales,presentaciones,psuministro)
"""
import argparse
import json
import sys
import threading
import time
import zipfile
from concurrent.futures import ThreadPoolExecutor
from pathlib import Path
from urllib.error import HTTPError, URLError
from urllib.request import Request, urlopen

BASE = "https://cima.aemps.es/cima/rest"
ROOT = Path(__file__).resolve().parents[2] / "data" / "farmacos-es"
RAW = ROOT / "raw"
UA = "rete-farmacos-es/1.0 (knowledge-graph research; contact: carlosvivarrios@gmail.com)"
WORKERS = 8
PAGE_SIZE = 200

lock = threading.Lock()
stats = {"fetched": 0, "skipped": 0, "missing": 0, "errors": 0}


def log(msg):
    with lock:
        print(f"[{time.strftime('%H:%M:%S')}] {msg}", flush=True)


def record_error(url, err):
    with lock:
        stats["errors"] += 1
        with open(RAW / "_errors.jsonl", "a", encoding="utf-8") as f:
            f.write(json.dumps({"url": url, "error": str(err), "ts": time.time()}) + "\n")


def fetch(url, accept=None, tries=5):
    """GET url -> bytes, None on 404, raises after retries."""
    url = url.replace(" ", "%20")  # some nregistros contain spaces, e.g. "09608011 IP4"
    headers = {"User-Agent": UA}
    if accept:
        headers["Accept"] = accept
    delay = 2.0
    for attempt in range(tries):
        try:
            with urlopen(Request(url, headers=headers), timeout=90) as r:
                return r.read()
        except HTTPError as e:
            if e.code == 404:
                return None
            if attempt == tries - 1:
                raise
        except (URLError, TimeoutError, OSError):
            if attempt == tries - 1:
                raise
        time.sleep(delay)
        delay = min(delay * 2, 30)


def save(path, url, accept=None):
    """Fetch url into path unless already present. Returns bytes or None."""
    if path.exists() and path.stat().st_size > 0:
        with lock:
            stats["skipped"] += 1
        return path.read_bytes()
    try:
        data = fetch(url, accept=accept)
    except Exception as e:
        record_error(url, e)
        return None
    if data is None or len(data) == 0 or data == b"[]" or data == b"{}":
        with lock:
            stats["missing"] += 1
        return None
    path.parent.mkdir(parents=True, exist_ok=True)
    tmp = path.with_suffix(path.suffix + ".tmp")
    tmp.write_bytes(data)
    tmp.replace(path)
    with lock:
        stats["fetched"] += 1
        if stats["fetched"] % 500 == 0:
            print(f"[{time.strftime('%H:%M:%S')}] fetched={stats['fetched']} "
                  f"skipped={stats['skipped']} missing={stats['missing']} errors={stats['errors']}", flush=True)
    return data


def run_pool(jobs, label):
    """jobs: list of (path, url, accept) tuples."""
    todo = [j for j in jobs if not (j[0].exists() and j[0].stat().st_size > 0)]
    log(f"{label}: {len(jobs)} items, {len(jobs) - len(todo)} already on disk, {len(todo)} to fetch")
    with ThreadPoolExecutor(max_workers=WORKERS) as ex:
        list(ex.map(lambda j: save(j[0], j[1], j[2]), jobs))


def paginated(endpoint, out_dir, label, limit=None):
    """Fetch all pages of a CIMA list endpoint. Returns list of parsed page dicts."""
    out_dir.mkdir(parents=True, exist_ok=True)
    first = save(out_dir / "page_0001.json", f"{BASE}/{endpoint}pagina=1")
    if first is None:
        log(f"{label}: first page unavailable, skipping")
        return []
    meta = json.loads(first)
    total = meta.get("totalFilas", 0)
    npages = (total + PAGE_SIZE - 1) // PAGE_SIZE
    if limit:
        npages = min(npages, limit)
    log(f"{label}: {total} rows, {npages} pages")
    jobs = [(out_dir / f"page_{p:04d}.json", f"{BASE}/{endpoint}pagina={p}", None)
            for p in range(2, npages + 1)]
    run_pool(jobs, label)
    pages = []
    for p in range(1, npages + 1):
        f = out_dir / f"page_{p:04d}.json"
        if f.exists():
            pages.append(json.loads(f.read_bytes()))
    return pages


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--limit", type=int, default=None, help="cap items per phase (smoke test)")
    ap.add_argument("--only", default=None, help="comma-separated phases to run")
    args = ap.parse_args()
    phases = set(args.only.split(",")) if args.only else None

    def enabled(name):
        return phases is None or name in phases

    RAW.mkdir(parents=True, exist_ok=True)
    t0 = time.time()

    # --- Phase: nomenclator (full-catalog ZIP + dictionaries) ---
    if enabled("nomenclator"):
        zpath = RAW / "nomenclator" / "prescripcion.zip"
        if not zpath.exists():
            log("nomenclator: downloading prescripcion.zip")
            zpath.parent.mkdir(parents=True, exist_ok=True)
            data = fetch("https://listadomedicamentos.aemps.gob.es/prescripcion.zip")
            zpath.write_bytes(data)
        try:
            with zipfile.ZipFile(zpath) as z:
                z.extractall(zpath.parent)
            log(f"nomenclator: extracted {len(zipfile.ZipFile(zpath).namelist())} files")
        except zipfile.BadZipFile as e:
            record_error("prescripcion.zip", e)

    # --- Phase: medicine list pages ---
    nregs = []
    if enabled("pages") or enabled("detalle") or enabled("docs") or enabled("notas") or enabled("materiales"):
        pages = paginated("medicamentos?", RAW / "medicamentos" / "pages", "medicamentos",
                          limit=(1 if args.limit else None))
        for pg in pages:
            for med in pg.get("resultados", []):
                nregs.append(med["nregistro"])
        if args.limit:
            nregs = nregs[: args.limit]
        log(f"medicamentos: {len(nregs)} nregistros collected")

    # --- Phase: per-medicine detail JSON ---
    det_dir = RAW / "medicamentos" / "detalle"
    if enabled("detalle"):
        jobs = [(det_dir / f"{n}.json", f"{BASE}/medicamento?nregistro={n}", None) for n in nregs]
        run_pool(jobs, "detalle")

    # --- Phases driven by the detail JSONs: docs / notas / materiales ---
    if enabled("docs") or enabled("notas") or enabled("materiales"):
        doc_jobs, notas_jobs, mat_jobs = [], [], []
        for n in nregs:
            f = det_dir / f"{n}.json"
            if not f.exists():
                continue
            try:
                med = json.loads(f.read_bytes())
            except json.JSONDecodeError:
                continue
            for d in med.get("docs", []):
                tipo, url_html, secc = d.get("tipo"), d.get("urlHtml"), d.get("secc")
                sub = {1: "ft", 2: "p"}.get(tipo)
                if not sub:
                    continue  # other doc types are PDF-only; URLs stay in the detail JSON
                if url_html:
                    doc_jobs.append((RAW / "docs" / sub / url_html.rsplit("/", 1)[-1], url_html, None))
                if secc:
                    doc_jobs.append((RAW / "docs" / f"{sub}_secc" / f"{n}.json",
                                     f"{BASE}/docSegmentado/contenido/{tipo}?nregistro={n}",
                                     "application/json"))
            if med.get("notas"):
                notas_jobs.append((RAW / "notas" / f"{n}.json", f"{BASE}/notas?nregistro={n}", None))
            if med.get("materialesInf"):
                mat_jobs.append((RAW / "materiales" / f"{n}.json", f"{BASE}/materiales?nregistro={n}", None))
        if enabled("docs"):
            run_pool(doc_jobs, "docs")
        if enabled("notas"):
            run_pool(notas_jobs, "notas")
        if enabled("materiales"):
            run_pool(mat_jobs, "materiales")

    # --- Phase: presentations (CN level) ---
    if enabled("presentaciones"):
        paginated("presentaciones?", RAW / "presentaciones" / "pages", "presentaciones",
                  limit=(1 if args.limit else None))

    # --- Phase: supply problems ---
    if enabled("psuministro"):
        paginated("psuministro?", RAW / "psuministro", "psuministro")

    log(f"DONE in {(time.time() - t0) / 60:.1f} min: fetched={stats['fetched']} "
        f"skipped={stats['skipped']} missing={stats['missing']} errors={stats['errors']}")


if __name__ == "__main__":
    sys.stdout.reconfigure(encoding="utf-8", errors="replace")
    main()
