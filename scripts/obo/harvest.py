#!/usr/bin/env python3
"""Download every OBO Foundry ontology into data/obo/raw/.

Source: OBO Foundry (https://obofoundry.org/) — a registry of ~250 open-licensed
biomedical ontologies (GO, ChEBI, HPO, Uberon, DOID, Mondo, CL, PR, NCBITaxon…).
No scraping: the registry is a machine-readable JSON-LD/YAML file and every
ontology ships a canonical download via purl.obolibrary.org.

Layout under data/obo/raw/:
  _registry/ontologies.jsonld           the full registry (all metadata)
  _registry/ontologies.yml              same, YAML
  _registry/manifest.json               what we fetched: id -> {url, resolved, bytes, license, title}
  <id>/<id>.owl                         the ontology's main product (ontology_purl)
  _errors.jsonl                         failed downloads after retries

Each ontology_purl is the "main OWL edition" (RDF/XML, self-contained) — the same
artifact `rete build` ingests directly. Resumable: existing non-empty files are
skipped. Active ontologies only by default (189); --all includes obsolete/orphaned.

Usage:  python scripts/obo/harvest.py [--all] [--survey] [--limit N]
"""
import argparse
import json
import shutil
import sys
import threading
import time
from concurrent.futures import ThreadPoolExecutor
from pathlib import Path
from urllib.error import HTTPError, URLError
from urllib.request import Request, urlopen

ROOT = Path(__file__).resolve().parents[2]
RAW = ROOT / "data" / "obo" / "raw"
REG = RAW / "_registry"
UA = "rete-obo/1.0 (knowledge-graph research; open-data; contact carlosvivarrios@gmail.com)"
WORKERS = 8

JSONLD = "http://purl.obolibrary.org/meta/ontologies.jsonld"
YML = "https://raw.githubusercontent.com/OBOFoundry/OBOFoundry.github.io/master/registry/ontologies.yml"

lock = threading.Lock()
stats = {"fetched": 0, "skipped": 0, "errors": 0, "bytes": 0}


def log(m):
    print(f"[{time.strftime('%H:%M:%S')}] {m}", flush=True)


def record_error(oid, url, err):
    with lock:
        stats["errors"] += 1
        with open(RAW / "_errors.jsonl", "a", encoding="utf-8") as f:
            f.write(json.dumps({"id": oid, "url": url, "error": str(err), "ts": time.time()}) + "\n")


def fetch_bytes(url, tries=4):
    delay = 3.0
    for k in range(tries):
        try:
            with urlopen(Request(url, headers={"User-Agent": UA}), timeout=120) as r:
                return r.read()
        except HTTPError as e:
            if e.code == 404 or k == tries - 1:
                raise
        except (URLError, TimeoutError, OSError):
            if k == tries - 1:
                raise
        time.sleep(delay)
        delay = min(delay * 2, 40)


def download(oid, url, path, tries=4):
    """Stream url -> path (skip if present). Returns (bytes, resolved_url) or raises."""
    if path.exists() and path.stat().st_size > 0:
        with lock:
            stats["skipped"] += 1
        return path.stat().st_size, None
    path.parent.mkdir(parents=True, exist_ok=True)
    tmp = path.with_suffix(path.suffix + ".part")
    delay = 5.0
    for k in range(tries):
        try:
            with urlopen(Request(url, headers={"User-Agent": UA}), timeout=300) as r:
                resolved = r.url
                with open(tmp, "wb") as fh:
                    shutil.copyfileobj(r, fh, length=1024 * 1024)
            n = tmp.stat().st_size
            if n == 0:
                raise OSError("empty body")
            tmp.replace(path)
            with lock:
                stats["fetched"] += 1
                stats["bytes"] += n
            return n, resolved
        except HTTPError as e:
            if e.code == 404 or k == tries - 1:
                if tmp.exists():
                    tmp.unlink()
                raise
        except (URLError, TimeoutError, OSError):
            if tmp.exists():
                tmp.unlink()
            if k == tries - 1:
                raise
        time.sleep(delay)
        delay = min(delay * 2, 60)


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--all", action="store_true", help="include obsolete/orphaned ontologies too")
    ap.add_argument("--survey", action="store_true", help="don't download; just print the plan")
    ap.add_argument("--limit", type=int, default=None)
    args = ap.parse_args()

    REG.mkdir(parents=True, exist_ok=True)
    # registry
    (REG / "ontologies.jsonld").write_bytes(fetch_bytes(JSONLD))
    try:
        (REG / "ontologies.yml").write_bytes(fetch_bytes(YML))
    except Exception as e:
        log(f"warn: yml fetch failed ({e})")
    reg = json.loads((REG / "ontologies.jsonld").read_bytes())
    onts = reg.get("ontologies", [])
    sel = onts if args.all else [o for o in onts if o.get("activity_status") == "active"]
    sel = [o for o in sel if o.get("ontology_purl")]
    if args.limit:
        sel = sel[: args.limit]
    log(f"registry: {len(onts)} total, downloading {len(sel)} ontologies "
        f"({'all' if args.all else 'active'})")

    if args.survey:
        for o in sel:
            print(f"  {o['id']:16s} {o.get('ontology_purl')}")
        return

    manifest = {}
    manifest_lock = threading.Lock()

    def job(o):
        oid = o["id"]
        url = o["ontology_purl"]
        ext = "." + url.rsplit(".", 1)[-1] if "." in url.rsplit("/", 1)[-1] else ".owl"
        path = RAW / oid / f"{oid}{ext}"
        try:
            n, resolved = download(oid, url, path)
            with manifest_lock:
                manifest[oid] = {
                    "url": url, "resolved": resolved, "file": str(path.relative_to(RAW)),
                    "bytes": n, "title": o.get("title"),
                    "license": (o.get("license") or {}).get("label"),
                    "activity_status": o.get("activity_status"),
                }
        except Exception as e:
            record_error(oid, url, e)
        with lock:
            done = stats["fetched"] + stats["skipped"]
        if done and done % 20 == 0:
            log(f"progress {done}/{len(sel)} | fetched={stats['fetched']} "
                f"skipped={stats['skipped']} errors={stats['errors']} "
                f"{stats['bytes']/1e9:.2f} GB")

    t0 = time.time()
    with ThreadPoolExecutor(max_workers=WORKERS) as ex:
        list(ex.map(job, sel))

    (REG / "manifest.json").write_text(
        json.dumps(manifest, ensure_ascii=False, indent=1), encoding="utf-8")
    log(f"DONE in {(time.time()-t0)/60:.1f} min: fetched={stats['fetched']} "
        f"skipped={stats['skipped']} errors={stats['errors']} "
        f"total {stats['bytes']/1e9:.2f} GB | manifest: {len(manifest)} ontologies")


if __name__ == "__main__":
    sys.stdout.reconfigure(encoding="utf-8", errors="replace")
    main()
