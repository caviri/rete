#!/usr/bin/env python3
"""Phase 7 -- mirror every painting image as a 1200px WebP.

Streaming by design: each worker downloads a JPEG into MEMORY, decodes it,
downscales it, encodes WebP, and writes only that. The ~78 GB of source JPEG is
never written to disk -- only the ~12 GB of WebP output.

Variant chain, in order of preference:

    !HD.jpg      1200px long edge, present for only ~17% of works
    <original>   full resolution, present for ~99.3%  <- the workhorse
    !Large.jpg   600px, present for ~73%

`!Large` is what the metadata's `image` field records, but that derivative is a
genuine 404 for roughly a quarter of the corpus (and `!HD` is missing for
exactly those), so the original is what actually gives near-total coverage.
Measured over a 150-image sample: the chain resolves for 99.3%.

Anything whose long edge exceeds MAX_EDGE is downscaled with LANCZOS; smaller
images are left at native size (never upscaled). WebP q80/method 6 measured
52.8% smaller than the source JPEG on real WikiArt images.

Output is sharded 256 ways -- 223k files in one directory is pathological on
NTFS:

    raw/assets/webp/<cid & 0xff as 2 hex>/<contentId>.webp

Resumable (skips existing output), atomic (.part then rename), and it records a
per-image sidecar row so the graph can cite provenance and dimensions.

    MSYS_NO_PATHCONV=1 docker run --rm -v "D:/pro/rete:/w" -w //w \
      -e PYTHONUNBUFFERED=1 python:3.12-slim \
      bash -lc "pip install -q pillow && python data/wikiart/scripts/mirror_images_webp.py"

Env: WIKIART_WEBP_Q (80), WIKIART_WEBP_METHOD (6), WIKIART_MAX_EDGE (1200),
     WIKIART_IMG_WORKERS (16), WIKIART_LIMIT (0 = all, else stop after N).
"""

import csv
import io
import os
import random
import re
import sys
import time
import urllib.error
import urllib.parse
import urllib.request
from concurrent.futures import ProcessPoolExecutor

RAW = os.path.abspath(os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "raw"))
ASSETS = os.path.join(RAW, "assets")
OUT = os.path.join(ASSETS, "webp")
MANIFEST = os.path.join(ASSETS, "webp_manifest.tsv")
FAILED = os.path.join(ASSETS, "webp_failed.txt")

Q = int(os.environ.get("WIKIART_WEBP_Q", "80"))
METHOD = int(os.environ.get("WIKIART_WEBP_METHOD", "6"))
MAX_EDGE = int(os.environ.get("WIKIART_MAX_EDGE", "1200"))
WORKERS = int(os.environ.get("WIKIART_IMG_WORKERS", "16"))
LIMIT = int(os.environ.get("WIKIART_LIMIT", "0"))

UA = "rete-dataset-harvest/1.0 (+https://github.com/caviri/rete)"
TOKEN = re.compile(r"!([A-Za-z0-9]+)\.(jpg|jpeg|png|gif)$", re.I)
MAX_BYTES = 80 * 1024 * 1024        # refuse absurd originals (largest seen ~250MP)


def variants(image_url):
    """The fetch chain for one painting, best-quality-per-byte first."""
    base = TOKEN.sub("", image_url)
    return (("HD", base + "!HD.jpg"), ("original", base), ("Large", base + "!Large.jpg"))


def out_path(cid):
    return os.path.join(OUT, f"{int(cid) & 0xFF:02x}", f"{cid}.webp")


def ascii_url(u):
    """Percent-encode non-ASCII path characters.

    WikiArt slugs keep their diacritics ("mödling", "ortaköy"), and urllib
    refuses a non-ASCII URL outright with UnicodeEncodeError -- which looked like
    a dead image but is really an unencoded request. Encoding the path fixes it
    (verified: raw -> UnicodeEncodeError, encoded -> HTTP 200).
    """
    s = urllib.parse.urlsplit(u)
    return urllib.parse.urlunsplit((
        s.scheme, s.netloc,
        urllib.parse.quote(s.path, safe="/!$&'()*+,;=:@~"),
        s.query, s.fragment))


def fetch(url, retries=3):
    url = ascii_url(url)
    for attempt in range(retries):
        try:
            req = urllib.request.Request(url, headers={"User-Agent": UA})
            with urllib.request.urlopen(req, timeout=90) as r:
                n = int(r.headers.get("Content-Length") or 0)
                if n > MAX_BYTES:
                    return None
                return r.read(MAX_BYTES + 1)
        except urllib.error.HTTPError as e:
            if e.code in (404, 403, 410):
                return None                 # this variant does not exist
        except Exception:
            pass
        if attempt < retries - 1:
            time.sleep(2 ** attempt + random.random())
    return None


def process(row):
    """Download -> downscale -> WebP, entirely in memory. Runs in a subprocess."""
    from PIL import Image, ImageFile
    Image.MAX_IMAGE_PIXELS = None           # WikiArt has legitimate 250MP scans
    # A handful of source files are genuinely corrupt on WikiArt's side: the
    # response matches its Content-Length exactly but the image data stops early
    # ("image file is truncated"). Decoding what is there beats dropping the
    # work entirely for a display mirror.
    ImageFile.LOAD_TRUNCATED_IMAGES = True

    cid, url = row
    dst = out_path(cid)
    if os.path.exists(dst) and os.path.getsize(dst) > 0:
        return ("skip", cid, "", 0, 0, 0, 0)

    for name, u in variants(url):
        blob = fetch(u)
        if not blob:
            continue
        try:
            with Image.open(io.BytesIO(blob)) as im:
                im = im.convert("RGB")
                w, h = im.size
                if max(w, h) > MAX_EDGE:    # never upscale
                    im.thumbnail((MAX_EDGE, MAX_EDGE), Image.LANCZOS)
                buf = io.BytesIO()
                im.save(buf, "WEBP", quality=Q, method=METHOD)
                ow, oh = im.size
        except Exception:
            continue                        # not a decodable image; try next variant

        os.makedirs(os.path.dirname(dst), exist_ok=True)
        tmp = dst + ".part"
        with open(tmp, "wb") as f:
            f.write(buf.getvalue())
        os.replace(tmp, dst)
        return ("ok", cid, name, len(blob), buf.tell(), ow, oh)

    return ("fail", cid, "", 0, 0, 0, 0)


def load_rows():
    """(contentId, image_url) for every painting, from the asset manifest."""
    src = os.path.join(ASSETS, "all_assets.tsv")
    if not os.path.exists(src):
        sys.exit("run extract_image_urls.py first (assets/all_assets.tsv missing)")
    rows, seen = [], set()
    with open(src, encoding="utf-8") as f:
        for r in csv.DictReader(f, delimiter="\t"):
            cid, url = r.get("content_id"), r.get("image_url")
            if not cid or not url or cid in seen:
                continue
            seen.add(cid)
            rows.append((cid, url))
    return rows


def main():
    os.makedirs(OUT, exist_ok=True)
    rows = load_rows()
    print(f"  {len(rows):,} paintings in the asset manifest")

    done = set()
    if os.path.exists(MANIFEST):
        with open(MANIFEST, encoding="utf-8") as f:
            next(f, None)
            for line in f:
                p = line.split("\t")
                if p:
                    done.add(p[0])
    dead = set()
    if os.path.exists(FAILED):
        dead = {l.strip() for l in open(FAILED, encoding="utf-8") if l.strip()}

    todo = [r for r in rows if r[0] not in done and r[0] not in dead]
    if LIMIT:
        todo = todo[:LIMIT]
    print(f"  {len(done):,} already mirrored, {len(dead):,} known-dead, {len(todo):,} to go")
    print(f"  q={Q} method={METHOD} max_edge={MAX_EDGE} workers={WORKERS}")
    if not todo:
        print("  nothing to do -- mirror complete")
        return

    new = not os.path.exists(MANIFEST)
    mf = open(MANIFEST, "a", encoding="utf-8")
    if new:
        mf.write("content_id\tvariant\tsrc_bytes\twebp_bytes\twidth\theight\n")
    ff = open(FAILED, "a", encoding="utf-8")

    t0 = time.time()
    n = ok = skip = fail = 0
    src_b = out_b = 0
    by_variant = {}
    try:
        with ProcessPoolExecutor(max_workers=WORKERS) as ex:
            for status, cid, variant, sb, ob, w, h in ex.map(process, todo, chunksize=8):
                n += 1
                if status == "ok":
                    ok += 1
                    src_b += sb
                    out_b += ob
                    by_variant[variant] = by_variant.get(variant, 0) + 1
                    mf.write(f"{cid}\t{variant}\t{sb}\t{ob}\t{w}\t{h}\n")
                elif status == "skip":
                    skip += 1
                else:
                    fail += 1
                    ff.write(cid + "\n")
                if n % 200 == 0:
                    mf.flush(); ff.flush()
                    el = time.time() - t0
                    rate = n / el if el else 0
                    eta = (len(todo) - n) / rate if rate else 0
                    sys.stderr.write(
                        f"\r  {n:,}/{len(todo):,}  ok={ok:,} fail={fail:,}  "
                        f"{rate:5.1f}/s  in {src_b/1e9:5.1f}GB -> out {out_b/1e9:5.2f}GB  "
                        f"elapsed {el/60:5.1f}m eta {eta/60:6.1f}m   ")
                    sys.stderr.flush()
    finally:
        sys.stderr.write("\n")
        mf.close(); ff.close()

    print(f"  done: ok={ok:,} skip={skip:,} fail={fail:,}")
    if by_variant:
        print(f"  variants used: {by_variant}")
    if src_b:
        print(f"  {src_b/1e9:.1f} GB fetched -> {out_b/1e9:.2f} GB WebP "
              f"({100*(1-out_b/src_b):.1f}% smaller); mean {out_b/max(ok,1)/1024:.0f}K/image")
    remaining = len(rows) - len(done) - ok - len(dead) - fail
    if remaining > 0:
        print(f"  {remaining:,} remaining -- re-run to continue")


if __name__ == "__main__":
    main()
