#!/usr/bin/env python3
"""Harvest Arxius en Línia (Generalitat de Catalunya) records that have digital images,
mirror each non-reserved image as a downsized WebP to R2, and keep the original Gencat
link. Can do the whole archive (--all) or a slice, optionally filtered to one archive
(--arxiu <codiArxiu>). Resumable: images already on R2 are skipped.

Respects the `reservat` rights flag (reserved images kept as links only). Descriptions
are openly reusable (Llei 37/2007 / Llicència oberta).

Usage: set -a; . ./.env; set +a
       python scripts/arxiu/harvest_and_mirror.py --all --arxiu 330 --out data/arxiu/records_330.jsonl
"""
import os, io, json, sys, argparse, urllib.request
from concurrent.futures import ThreadPoolExecutor
from pathlib import Path
from PIL import Image
import boto3

BK = "https://backend.arxiusenlinia.cultura.gencat.cat"
FLAGS = ("busca=*&buscarEnTitol=true&buscarEnDescripcio=true&fonsDocTextual=true&fonsDocNoTextual=true"
         "&tipDocTextual=true&tipDocFotos=true&tipDocFotoQuimica=true&tipDocFotoDigital=true&tipDocMapes=true"
         "&tipDocPostal=true&tipDocProcFotomecanic=true&tipDocProdArt=true&tipDocProdImpr=true&tipDocAudios=true"
         "&tipDocNomesMusicals=false&tipusArxiu=TOTS&selectedTipusCerca=U&classifDescendent=false"
         "&dataExtremaConcreta=0&fromSearch=true")
MAXD, Q = 1400, 80
UA = "Mozilla/5.0 (rete dataset harvester; +https://github.com/caviri/rete)"
BUCKET, PREFIX = "rete", "arxiu/img/"
R2BASE = "https://data.graphplaza.com/arxiu/img/"

s3 = boto3.client("s3", endpoint_url=os.environ["S3_API_ENDPOINT"],
                  aws_access_key_id=os.environ["ACCESS_KEY_ID"],
                  aws_secret_access_key=os.environ["SECRET_ACCESS_KEY"], region_name="auto")

def get(url):
    return urllib.request.urlopen(urllib.request.Request(url, headers={"User-Agent": UA, "Accept": "application/json"}), timeout=120)

def list_existing():
    keys = set()
    for pg in s3.get_paginator("list_objects_v2").paginate(Bucket=BUCKET, Prefix=PREFIX):
        for o in pg.get("Contents", []):
            keys.add(o["Key"])
    return keys

def harvest(limit, arxiu, sort):
    recs, page = [], 1
    af = f"&codiArxiu={arxiu}" if arxiu else ""
    while True:
        d = json.load(get(f"{BK}/unitat/search/advanced?{FLAGS}&sort={sort}&mostrar=AMB_FITXER{af}&page={page}&size=200"))
        total = d.get("totalResults", 0)
        rs = d.get("results", [])
        if not rs:
            break
        recs.extend(rs)
        print(f"  page {page}: +{len(rs)} ({len(recs)}/{total})", flush=True)
        page += 1
        if (limit and len(recs) >= limit) or len(recs) >= total:
            break
    return recs[:limit] if limit else recs

def mirror(r, existing):
    url = r.get("objecteDigitalUrl")
    if r.get("reservat") or not url or not url.lower().endswith(".jpg"):
        return None
    oid = r.get("primerObjecteDigitalId") or r.get("codiUnitat")
    key = f"{PREFIX}{oid}.webp"
    if key in existing:
        return R2BASE + f"{oid}.webp"
    try:
        raw = urllib.request.urlopen(urllib.request.Request(url, headers={"User-Agent": UA}), timeout=120).read()
        im = Image.open(io.BytesIO(raw))
        if im.mode not in ("RGB", "L"):
            im = im.convert("RGB")
        w, h = im.size
        sc = min(1.0, MAXD / max(w, h))
        if sc < 1.0:
            im = im.resize((round(w * sc), round(h * sc)), Image.LANCZOS)
        buf = io.BytesIO(); im.save(buf, "WEBP", quality=Q, method=6)
        s3.put_object(Bucket=BUCKET, Key=key, Body=buf.getvalue(), ContentType="image/webp")
        return R2BASE + f"{oid}.webp"
    except Exception as e:
        print(f"  ! mirror fail {oid}: {e}", flush=True)
        return None

def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--all", action="store_true")
    ap.add_argument("--limit", type=int, default=500)
    ap.add_argument("--arxiu", type=int, default=None)
    ap.add_argument("--sort", default="data_inici_dt,asc")
    ap.add_argument("--out", default="data/arxiu/records.jsonl")
    a = ap.parse_args()
    limit = None if a.all else a.limit
    print(f"harvesting {'ALL' if a.all else limit} records"
          + (f" from archive {a.arxiu}" if a.arxiu else "") + f" (sort {a.sort}) …", flush=True)
    recs = harvest(limit, a.arxiu, a.sort)
    print(f"got {len(recs)} records; checking R2 for already-mirrored images…", flush=True)
    existing = list_existing()
    print(f"  {len(existing)} images already on R2; mirroring the rest…", flush=True)
    with ThreadPoolExecutor(max_workers=6) as ex:
        webps = list(ex.map(lambda r: mirror(r, existing), recs))
    Path(a.out).parent.mkdir(parents=True, exist_ok=True)
    mirrored = 0
    with open(a.out, "w", encoding="utf-8") as f:
        for r, wp in zip(recs, webps):
            r["webp"] = wp
            if wp:
                mirrored += 1
            f.write(json.dumps(r, ensure_ascii=False) + "\n")
    print(f"DONE: {len(recs)} records, {mirrored} images as WebP -> {a.out}", flush=True)

if __name__ == "__main__":
    main()
