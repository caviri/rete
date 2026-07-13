#!/usr/bin/env python3
"""Harvest a PORTION of Arxius en Línia (Generalitat de Catalunya) records that have
digital images, mirror each non-reserved image as a downsized WebP to R2, and keep the
original Gencat image link too. Writes records.jsonl for the graph builder.

Respects the `reservat` rights flag: reserved images are kept as links only (not
re-hosted). Descriptions are openly reusable (Llei 37/2007 / Llicència oberta).

Usage: set -a; . ./.env; set +a ; python scripts/arxiu/harvest_and_mirror.py [N]
"""
import os, io, json, sys, urllib.request
from concurrent.futures import ThreadPoolExecutor
from pathlib import Path
from PIL import Image
import boto3

REPO = Path(__file__).resolve().parents[2]
OUT  = REPO / "data" / "arxiu" / "records.jsonl"
BK   = "https://backend.arxiusenlinia.cultura.gencat.cat"
FLAGS = ("busca=*&buscarEnTitol=true&buscarEnDescripcio=true&fonsDocTextual=true&fonsDocNoTextual=true"
         "&tipDocTextual=true&tipDocFotos=true&tipDocFotoQuimica=true&tipDocFotoDigital=true&tipDocMapes=true"
         "&tipDocPostal=true&tipDocProcFotomecanic=true&tipDocProdArt=true&tipDocProdImpr=true&tipDocAudios=true"
         "&tipDocNomesMusicals=false&tipusArxiu=TOTS&selectedTipusCerca=U&classifDescendent=false"
         "&dataExtremaConcreta=0&fromSearch=true&sort=data_inici_dt,asc")
N     = int(sys.argv[1]) if len(sys.argv) > 1 else 500
MAXD, Q = 1400, 80
UA    = "Mozilla/5.0 (rete dataset harvester; +https://github.com/caviri/rete)"
BUCKET, PREFIX = "rete", "arxiu/img/"
R2BASE = "https://data.graphplaza.com/arxiu/img/"

s3 = boto3.client("s3", endpoint_url=os.environ["S3_API_ENDPOINT"],
                  aws_access_key_id=os.environ["ACCESS_KEY_ID"],
                  aws_secret_access_key=os.environ["SECRET_ACCESS_KEY"], region_name="auto")

def get(url):
    req = urllib.request.Request(url, headers={"User-Agent": UA, "Accept": "application/json"})
    return urllib.request.urlopen(req, timeout=90)

def harvest(n):
    recs, page = [], 1
    while len(recs) < n:
        d = json.load(get(f"{BK}/unitat/search/advanced?{FLAGS}&mostrar=AMB_FITXER&page={page}&size=200"))
        rs = d.get("results", [])
        if not rs:
            break
        recs.extend(rs)
        print(f"  page {page}: +{len(rs)} (total {len(recs)})", flush=True)
        page += 1
    return recs[:n]

def mirror(r):
    url = r.get("objecteDigitalUrl")
    if r.get("reservat") or not url or not url.lower().endswith(".jpg"):
        return None
    oid = r.get("primerObjecteDigitalId") or r.get("codiUnitat")
    try:
        req = urllib.request.Request(url, headers={"User-Agent": UA})
        raw = urllib.request.urlopen(req, timeout=90).read()
        im = Image.open(io.BytesIO(raw))
        if im.mode not in ("RGB", "L"):
            im = im.convert("RGB")
        w, h = im.size
        sc = min(1.0, MAXD / max(w, h))
        if sc < 1.0:
            im = im.resize((round(w * sc), round(h * sc)), Image.LANCZOS)
        buf = io.BytesIO(); im.save(buf, "WEBP", quality=Q, method=6)
        s3.put_object(Bucket=BUCKET, Key=f"{PREFIX}{oid}.webp", Body=buf.getvalue(), ContentType="image/webp")
        return R2BASE + f"{oid}.webp"
    except Exception as e:
        print(f"  ! mirror fail {oid}: {e}", flush=True)
        return None

def main():
    print(f"harvesting {N} records with digital images…", flush=True)
    recs = harvest(N)
    print(f"mirroring images (non-reserved) to R2…", flush=True)
    with ThreadPoolExecutor(max_workers=6) as ex:
        webps = list(ex.map(mirror, recs))
    mirrored = 0
    with OUT.open("w", encoding="utf-8") as f:
        for r, wp in zip(recs, webps):
            r["webp"] = wp
            if wp:
                mirrored += 1
            f.write(json.dumps(r, ensure_ascii=False) + "\n")
    print(f"done: {len(recs)} records, {mirrored} images mirrored as WebP -> {OUT}", flush=True)

if __name__ == "__main__":
    main()
