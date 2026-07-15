#!/usr/bin/env python3
"""Turn the OLD Patrinum books (date_start < CUTOFF, default 1900) into static level-0
IIIF: render every PDF page → JPEG on R2, write a IIIF Presentation v2 manifest (canvases
carry the static image directly, NO image service = level 0), and emit
`<item> b:iiifManifest <R2 manifest url>` so they light up in the playground's existing
IIIF viewer (deep-zoom, page-turning) — the same cell the 15 e-codices mss already use.

Everything in-memory (PDF streamed → pages rendered → uploaded → discarded). Resumable:
a record whose manifest.json is already on R2 is skipped but still emits its triple.
Run in a container with boto3 + pymupdf + pillow and the R2 .env loaded.

Rebuild bcul with:  rete build bcul.nt bcul_images.nt bcul_pdf.nt bcul_iiif.nt -o bcul.rete
"""
import os, io, sys, json, argparse, urllib.request
from concurrent.futures import ThreadPoolExecutor
from pathlib import Path
import fitz  # PyMuPDF
from PIL import Image
import boto3

sys.path.insert(0, str(Path(__file__).resolve().parent))
from jsonl_to_nt import subject_iri  # noqa

REPO   = Path(__file__).resolve().parents[2]
SRC    = REPO / "data" / "bcul" / "normalized" / "bcul.jsonl"
OUT_NT = REPO / "data" / "bcul" / "bcul_iiif.nt"
IIIFMAN = "https://data.bcu-lausanne.ch/iiifManifest"
BUCKET  = "rete"
PREFIX  = "bcul/iiif/"
R2BASE  = "https://data.graphplaza.com/bcul/iiif/"
UA = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/125.0 Safari/537.36"
TARGET  = 1600     # px, long edge of each rendered page
QUALITY = 80       # WebP quality (≈25-30% smaller than JPEG at the same look)
MAX_PAGES = 700    # safety cap per book

s3 = boto3.client("s3", endpoint_url=os.environ["S3_API_ENDPOINT"],
                  aws_access_key_id=os.environ["ACCESS_KEY_ID"],
                  aws_secret_access_key=os.environ["SECRET_ACCESS_KEY"], region_name="auto")

def fetch(url):
    return urllib.request.urlopen(urllib.request.Request(url, headers={"User-Agent": UA}), timeout=180).read()

def existing_manifests():
    keys = set()
    for pg in s3.get_paginator("list_objects_v2").paginate(Bucket=BUCKET, Prefix=PREFIX, Delimiter="/"):
        for p in pg.get("CommonPrefixes", []):
            keys.add(p["Prefix"])  # bcul/iiif/<id>/
    # a folder exists only once we've put its manifest; confirm the manifest object
    return keys

def put(key, body, ctype):
    s3.put_object(Bucket=BUCKET, Key=key, Body=body, ContentType=ctype)

def render_book(rec):
    """Render all pages of all the record's PDFs → JPEGs on R2; return list of (url,w,h)."""
    rid = rec["local_id"]
    pdf_urls = [x["url"] for x in (rec.get("files") or []) if str(x.get("url", "")).lower().endswith(".pdf")]
    canvases = []
    n = 0
    # Encode pages on the main thread (PyMuPDF + WebP) while a pool uploads them
    # concurrently — network I/O overlaps CPU, ~5-8× the sequential pace.
    with ThreadPoolExecutor(max_workers=12) as pool:
        futs = []
        for pu in pdf_urls:
            try:
                raw = fetch(pu)
                doc = fitz.open(stream=raw, filetype="pdf")
            except Exception as e:
                print(f"  ! open fail {rid} {pu}: {e}", flush=True)
                continue
            for page in doc:
                if n >= MAX_PAGES:
                    print(f"  … {rid} capped at {MAX_PAGES} pages", flush=True)
                    break
                n += 1
                scale = TARGET / max(page.rect.width, page.rect.height, 1)
                pix = page.get_pixmap(matrix=fitz.Matrix(scale, scale), alpha=False)
                im = Image.frombytes("RGB", [pix.width, pix.height], pix.samples)
                buf = io.BytesIO(); im.save(buf, "WEBP", quality=QUALITY, method=4)
                key = f"{PREFIX}{rid}/p{n:04d}.webp"
                futs.append(pool.submit(put, key, buf.getvalue(), "image/webp"))
                canvases.append((R2BASE + f"{rid}/p{n:04d}.webp", pix.width, pix.height))
            doc.close()
            if n >= MAX_PAGES:
                break
        for f in futs:
            f.result()  # all pages uploaded before we write the manifest
    return canvases

def manifest_json(rec, canvases):
    rid = rec["local_id"]
    base = R2BASE + rid
    pub = rec.get("publication") or {}
    meta = []
    if pub.get("date"): meta.append({"label": "Date", "value": str(pub["date"])})
    cr = ", ".join(c["name"] for c in (rec.get("creators") or []) if c.get("name"))
    if cr: meta.append({"label": "Author", "value": cr})
    if pub.get("place"): meta.append({"label": "Published", "value": pub["place"]})
    meta.append({"label": "Source", "value": rec.get("record_url", "")})
    doi = (rec.get("identifiers") or {}).get("doi") or []
    if doi: meta.append({"label": "DOI", "value": doi[0]})
    seq = {"@id": f"{base}/sequence/normal", "@type": "sc:Sequence", "canvases": []}
    for i, (url, w, h) in enumerate(canvases, 1):
        seq["canvases"].append({
            "@id": f"{base}/canvas/p{i}", "@type": "sc:Canvas", "label": f"p. {i}",
            "width": w, "height": h,
            "images": [{
                "@id": f"{base}/annotation/p{i}", "@type": "oa:Annotation", "motivation": "sc:painting",
                "on": f"{base}/canvas/p{i}",
                "resource": {"@id": url, "@type": "dctypes:Image", "format": "image/webp", "width": w, "height": h},
            }],
        })
    return {
        "@context": "http://iiif.io/api/presentation/2/context.json",
        "@id": f"{base}/manifest.json", "@type": "sc:Manifest",
        "label": rec.get("title") or rid,
        "metadata": meta,
        "attribution": "Bibliothèque cantonale et universitaire de Lausanne — Patrinum (patrinum.ch)",
        "sequences": [seq],
    }

def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--cutoff", type=int, default=1900)
    ap.add_argument("--limit", type=int, default=None)  # smoke-test: only the first N books
    a = ap.parse_args()

    books = []
    for line in SRC.open(encoding="utf-8"):
        r = json.loads(line)
        if r.get("source") != "patrinum":
            continue
        y = r.get("date_start")
        if not isinstance(y, int) or y >= a.cutoff:
            continue
        if not any(str(x.get("url", "")).lower().endswith(".pdf") for x in (r.get("files") or [])):
            continue
        books.append(r)
    books.sort(key=lambda r: r.get("date_start") or 9999)
    if a.limit:
        books = books[:a.limit]
    print(f"{len(books)} pre-{a.cutoff} Patrinum books with a PDF", flush=True)

    done_prefixes = existing_manifests()
    triples = {}
    for i, rec in enumerate(books, 1):
        rid = rec["local_id"]; s = subject_iri(rec)
        if not s:
            continue
        man_url = R2BASE + f"{rid}/manifest.json"
        if f"{PREFIX}{rid}/" in done_prefixes:
            triples[s] = man_url
            print(f"[{i}/{len(books)}] skip {rid} (already on R2)", flush=True)
            continue
        y = rec.get("date_start")
        print(f"[{i}/{len(books)}] {rid} ({y}) {(rec.get('title') or '')[:55]} …", flush=True)
        try:
            canvases = render_book(rec)
            if not canvases:
                print(f"  ! no pages for {rid}", flush=True); continue
            man = manifest_json(rec, canvases)
            put(f"{PREFIX}{rid}/manifest.json",
                json.dumps(man, ensure_ascii=False).encode("utf-8"), "application/json")
            triples[s] = man_url
            print(f"  ✓ {rid}: {len(canvases)} pages + manifest", flush=True)
        except Exception as e:
            print(f"  ! fail {rid}: {e}", flush=True)
        # rewrite the triples file as we go (crash-safe)
        with OUT_NT.open("w", encoding="utf-8") as f:
            for su, u in triples.items():
                f.write(f"<{su}> <{IIIFMAN}> <{u}> .\n")
    print(f"DONE: {len(triples)} IIIF manifests -> {OUT_NT}", flush=True)

if __name__ == "__main__":
    main()
