#!/usr/bin/env python3
"""Mirror bcul's digitized-heritage thumbnails (Patrinum / e-codices / Scriptorium) to
R2 as WebP, so they render inline reliably (instead of live from the source servers),
and emit a supplemental N-Triples graph (bcul_images.nt) of `<item> schema:image <R2>`.
Rebuild bcul later with:  rete build bcul.nt bcul_images.nt -o bcul.rete

Everything is in-memory: each thumbnail is streamed, converted, uploaded, discarded —
nothing is written to local disk except the small supplemental .nt. Resumable: images
already on R2 are reused (their triple is still emitted). Skips Patrinum SVG placeholders.

Usage: set -a; . ./.env; set +a ; python scripts/bcul/mirror_thumbnails_r2.py [--limit N]
"""
import os, io, sys, re, json, argparse, urllib.request
from concurrent.futures import ThreadPoolExecutor
from pathlib import Path
from PIL import Image
import boto3

sys.path.insert(0, str(Path(__file__).resolve().parent))
from jsonl_to_nt import subject_iri, BASE  # noqa

REPO   = Path(__file__).resolve().parents[2]
SRC    = REPO / "data" / "bcul" / "normalized" / "bcul.jsonl"
OUT_NT = REPO / "data" / "bcul" / "bcul_images.nt"
SCHEMA_IMAGE = "http://schema.org/image"
BUCKET, PREFIX = "rete", "bcul/img/"
R2BASE = "https://data.graphplaza.com/bcul/img/"
UA = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/125.0 Safari/537.36"
MAXD, Q = 800, 82

s3 = boto3.client("s3", endpoint_url=os.environ["S3_API_ENDPOINT"],
                  aws_access_key_id=os.environ["ACCESS_KEY_ID"],
                  aws_secret_access_key=os.environ["SECRET_ACCESS_KEY"], region_name="auto")

def safe(x): return re.sub(r"[^A-Za-z0-9._-]", "_", str(x))
def key_of(rec): return f"{PREFIX}{rec['source']}/{safe(rec['local_id'])}.webp"
def url_of(rec): return R2BASE + f"{rec['source']}/{safe(rec['local_id'])}.webp"

def existing_keys():
    keys = set()
    for pg in s3.get_paginator("list_objects_v2").paginate(Bucket=BUCKET, Prefix=PREFIX):
        for o in pg.get("Contents", []):
            keys.add(o["Key"])
    return keys

def mirror(rec, existing):
    key = key_of(rec)
    s = subject_iri(rec)
    if not s:
        return None
    if key in existing:                       # already mirrored → just emit the triple
        return (s, url_of(rec))
    try:
        req = urllib.request.Request(rec["thumbnail_url"], headers={"User-Agent": UA})
        resp = urllib.request.urlopen(req, timeout=60)
        ctype = (resp.headers.get_content_type() or "").lower()
        raw = resp.read()
        if "svg" in ctype or not raw:         # Patrinum placeholder for no real image
            return None
        im = Image.open(io.BytesIO(raw))
        if im.mode not in ("RGB", "L"):
            im = im.convert("RGB")
        w, h = im.size
        sc = min(1.0, MAXD / max(w, h))
        if sc < 1.0:
            im = im.resize((round(w * sc), round(h * sc)), Image.LANCZOS)
        buf = io.BytesIO(); im.save(buf, "WEBP", quality=Q, method=6)
        s3.put_object(Bucket=BUCKET, Key=key, Body=buf.getvalue(), ContentType="image/webp")
        return (s, url_of(rec))
    except Exception:
        return None

def main():
    ap = argparse.ArgumentParser(); ap.add_argument("--limit", type=int, default=None)
    a = ap.parse_args()
    recs = []
    for l in SRC.open(encoding="utf-8"):
        r = json.loads(l)
        if r.get("has_digital") and r.get("thumbnail_url") and r.get("local_id"):
            recs.append(r)
            if a.limit and len(recs) >= a.limit:
                break
    print(f"{len(recs):,} records with a thumbnail; listing R2…", flush=True)
    existing = existing_keys()
    print(f"  {len(existing):,} already on R2; mirroring…", flush=True)
    triples, done = {}, 0
    with ThreadPoolExecutor(max_workers=12) as ex:
        for i, res in enumerate(ex.map(lambda r: mirror(r, existing), recs)):
            if res:
                triples[res[0]] = res[1]
            done += 1
            if done % 5000 == 0:
                print(f"  {done:,}/{len(recs):,} processed, {len(triples):,} images", flush=True)
    with OUT_NT.open("w", encoding="utf-8") as f:
        for s, u in triples.items():
            f.write(f"<{s}> <{SCHEMA_IMAGE}> <{u}> .\n")
    print(f"DONE: {len(triples):,} schema:image triples -> {OUT_NT}", flush=True)

if __name__ == "__main__":
    main()
