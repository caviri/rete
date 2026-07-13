#!/usr/bin/env python3
"""Mirror the first folio of each e-codices manuscript (from its IIIF manifest) to R2 as
WebP, and append `<manuscript> schema:image <R2>` to bcul_images.nt. The manuscripts'
schema:thumbnailUrl in the graph is a Patrinum placeholder; this gives them a real folio
image that renders inline. Reads data/bcul/_mss.tsv (lines: ?m=<iri>  ?man=<manifest>).

In-memory (stream → convert → upload → discard). Idempotent: subjects already present in
bcul_images.nt are skipped. Run in a container with boto3 + pillow and the R2 .env loaded.
"""
import os, io, re, json, sys, urllib.request
from pathlib import Path
from PIL import Image
import boto3

REPO   = Path(__file__).resolve().parents[2]
TSV    = REPO / "data" / "bcul" / "_mss.tsv"
OUT_NT = REPO / "data" / "bcul" / "bcul_images.nt"
SCHEMA_IMAGE = "http://schema.org/image"
BUCKET, PREFIX = "rete", "bcul/img/ecodices/"
R2BASE = "https://data.graphplaza.com/bcul/img/ecodices/"
UA = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/125.0 Safari/537.36"
MAXD, Q = 900, 82

s3 = boto3.client("s3", endpoint_url=os.environ["S3_API_ENDPOINT"],
                  aws_access_key_id=os.environ["ACCESS_KEY_ID"],
                  aws_secret_access_key=os.environ["SECRET_ACCESS_KEY"], region_name="auto")

def fetch(url, accept="*/*"):
    return urllib.request.urlopen(urllib.request.Request(
        url, headers={"User-Agent": UA, "Accept": accept}), timeout=90).read()

def first_image_url(manifest):
    """First canvas's IIIF image service → a downscaled folio JPEG."""
    seq = manifest["sequences"][0]
    canv = seq["canvases"][0]
    img = canv["images"][0]["resource"]
    svc = img.get("service")
    if svc and svc.get("@id"):
        return svc["@id"].rstrip("/") + "/full/!900,900/0/default.jpg"
    return img["@id"]

def main():
    pairs = []
    for ln in TSV.read_text(encoding="utf-8").splitlines():
        m = re.search(r"\?m=<([^>]+)>\s+\?man=<([^>]+)>", ln)
        if m:
            pairs.append((m.group(1), m.group(2)))
    print(f"{len(pairs)} manuscripts", flush=True)

    have = set()
    if OUT_NT.exists():
        for ln in OUT_NT.read_text(encoding="utf-8").splitlines():
            mm = re.match(r"<([^>]+)>", ln)
            if mm:
                have.add(mm.group(1))

    new = []
    for iri, man in pairs:
        if iri in have:
            print(f"  skip (already have) {iri}", flush=True); continue
        slug = re.sub(r"[^A-Za-z0-9._-]", "_", man.split("/iiif/")[1].split("/")[0])
        key = f"{PREFIX}{slug}.webp"
        try:
            manifest = json.loads(fetch(man, "application/json"))
            img_url = first_image_url(manifest)
            raw = fetch(img_url, "image/jpeg")
            im = Image.open(io.BytesIO(raw))
            if im.mode not in ("RGB", "L"):
                im = im.convert("RGB")
            w, h = im.size
            sc = min(1.0, MAXD / max(w, h))
            if sc < 1.0:
                im = im.resize((round(w * sc), round(h * sc)), Image.LANCZOS)
            buf = io.BytesIO(); im.save(buf, "WEBP", quality=Q, method=6)
            s3.put_object(Bucket=BUCKET, Key=key, Body=buf.getvalue(), ContentType="image/webp")
            url = R2BASE + f"{slug}.webp"
            new.append((iri, url))
            print(f"  OK {slug}  {im.size}  {len(buf.getvalue())//1024} KB", flush=True)
        except Exception as e:
            print(f"  ! fail {slug}: {e}", flush=True)

    if new:
        with OUT_NT.open("a", encoding="utf-8") as f:
            for s, u in new:
                f.write(f"<{s}> <{SCHEMA_IMAGE}> <{u}> .\n")
    print(f"DONE: appended {len(new)} folio triples -> {OUT_NT}", flush=True)

if __name__ == "__main__":
    main()
