#!/usr/bin/env python3
"""Backfill WebP thumbnails for non-reserved media across the harvested records
(records_*.jsonl) WITHOUT re-harvesting metadata. Handles both JPG (image) and PDF
(render first page). Everything is in-memory — the original is streamed, converted,
uploaded to R2, and discarded; nothing is written to local disk. Resumable: media
already on R2 are skipped. Updates each record's `webp` field in place.

Usage: set -a; . ./.env; set +a ; python scripts/arxiu/backfill_media.py
"""
import os, io, json, glob, urllib.request
from concurrent.futures import ThreadPoolExecutor
from PIL import Image
import fitz  # PyMuPDF
import boto3

MAXD, Q = 1400, 80
UA = "Mozilla/5.0 (rete dataset harvester; +https://github.com/caviri/rete)"
BUCKET, PREFIX = "rete", "arxiu/img/"
R2BASE = "https://data.graphplaza.com/arxiu/img/"

s3 = boto3.client("s3", endpoint_url=os.environ["S3_API_ENDPOINT"],
                  aws_access_key_id=os.environ["ACCESS_KEY_ID"],
                  aws_secret_access_key=os.environ["SECRET_ACCESS_KEY"], region_name="auto")

def existing_keys():
    keys = set()
    for pg in s3.get_paginator("list_objects_v2").paginate(Bucket=BUCKET, Prefix=PREFIX):
        for o in pg.get("Contents", []):
            keys.add(o["Key"])
    return keys

def to_webp(im):
    if im.mode not in ("RGB", "L"):
        im = im.convert("RGB")
    w, h = im.size
    sc = min(1.0, MAXD / max(w, h))
    if sc < 1.0:
        im = im.resize((round(w * sc), round(h * sc)), Image.LANCZOS)
    buf = io.BytesIO(); im.save(buf, "WEBP", quality=Q, method=6)
    return buf.getvalue()

def mirror(r, existing):
    url = r.get("objecteDigitalUrl") or ""
    lo = url.lower()
    if r.get("reservat") or r.get("webp") or not (lo.endswith(".jpg") or lo.endswith(".pdf")):
        return None
    oid = r.get("primerObjecteDigitalId") or r.get("codiUnitat")
    key = f"{PREFIX}{oid}.webp"
    if key in existing:
        return R2BASE + f"{oid}.webp"
    try:
        raw = urllib.request.urlopen(urllib.request.Request(url, headers={"User-Agent": UA}), timeout=120).read()
        if lo.endswith(".pdf"):
            doc = fitz.open(stream=raw, filetype="pdf")
            pix = doc.load_page(0).get_pixmap(dpi=150)
            im = Image.open(io.BytesIO(pix.tobytes("png"))); doc.close()
        else:
            im = Image.open(io.BytesIO(raw))
        webp = to_webp(im)
        s3.put_object(Bucket=BUCKET, Key=key, Body=webp, ContentType="image/webp")
        return R2BASE + f"{oid}.webp"
    except Exception as e:
        print(f"  ! {oid} ({'pdf' if lo.endswith('.pdf') else 'jpg'}): {e}", flush=True)
        return None

def main():
    print("listing R2…", flush=True)
    existing = existing_keys()
    print(f"  {len(existing)} media already on R2", flush=True)
    for path in sorted(glob.glob("data/arxiu/records_*.jsonl")):
        rows = [json.loads(l) for l in open(path, encoding="utf-8")]
        todo = [r for r in rows if not r.get("reservat") and not r.get("webp")
                and (r.get("objecteDigitalUrl") or "").lower().endswith((".jpg", ".pdf"))]
        if not todo:
            print(f"{path}: nothing to backfill", flush=True); continue
        print(f"{path}: backfilling {len(todo)} media…", flush=True)
        with ThreadPoolExecutor(max_workers=6) as ex:
            res = dict(zip((id(r) for r in todo), ex.map(lambda r: mirror(r, existing), todo)))
        n = 0
        for r in rows:
            wp = res.get(id(r))
            if wp:
                r["webp"] = wp; n += 1
        with open(path, "w", encoding="utf-8") as f:
            for r in rows:
                f.write(json.dumps(r, ensure_ascii=False) + "\n")
        print(f"  +{n} WebP (updated {path})", flush=True)
    print("done.", flush=True)

if __name__ == "__main__":
    main()
