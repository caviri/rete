#!/usr/bin/env python3
"""Convert the ECAL full-res cover mirror (data/ecal/covers/*.jpg) to WebP.

Full resolution preserved (no downscale) — the user chose full-res WebP. Quality
82, method 6 (best compression). Resumable (skips existing .webp), multiprocess.
Output: data/ecal/covers_webp/<id>.webp — uploaded to R2 as ecal/covers/<id>.webp.

Usage:  python scripts/ecal/covers_to_webp.py [--quality 82] [--jobs N]
"""
from __future__ import annotations

import argparse
import os
import sys
import time
from multiprocessing import Pool
from pathlib import Path

from PIL import Image

REPO = Path(__file__).resolve().parents[2]
SRC = REPO / "data" / "ecal" / "covers"
DST = REPO / "data" / "ecal" / "covers_webp"
LOG = REPO / "data" / "ecal" / "logs" / "webp.log"

_Q = 82


def convert(args):
    src, dst = args
    if dst.exists() and dst.stat().st_size > 0:
        return ("skip", src.stem)
    try:
        with Image.open(src) as im:
            # WebP wants RGB/RGBA; JPEGs can be CMYK/L/P.
            if im.mode not in ("RGB", "RGBA"):
                im = im.convert("RGB")
            im.save(dst, "WEBP", quality=_Q, method=6)
        return ("ok", src.stem)
    except Exception as e:  # a corrupt/partial jpg shouldn't kill the run
        try:
            if dst.exists():
                dst.unlink()
        except Exception:
            pass
        return ("fail", f"{src.stem}: {type(e).__name__} {e}")


def main():
    global _Q
    ap = argparse.ArgumentParser()
    ap.add_argument("--quality", type=int, default=82)
    ap.add_argument("--jobs", type=int, default=max(1, (os.cpu_count() or 2) - 1))
    args = ap.parse_args()
    _Q = args.quality
    DST.mkdir(parents=True, exist_ok=True)
    LOG.parent.mkdir(parents=True, exist_ok=True)

    jobs = sorted(SRC.glob("*.jpg"))
    tasks = [(p, DST / (p.stem + ".webp")) for p in jobs]

    def logline(m):
        line = f"[{time.strftime('%Y-%m-%dT%H:%M:%SZ', time.gmtime())}] {m}"
        print(line, flush=True)
        with open(LOG, "a", encoding="utf-8") as fh:
            fh.write(line + "\n")

    logline(f"webp: {len(tasks):,} covers -> {DST} (q{_Q}, {args.jobs} jobs)")
    ok = skip = fail = 0
    t0 = time.time()
    with Pool(args.jobs) as pool:
        for i, (status, info) in enumerate(pool.imap_unordered(convert, tasks, chunksize=32), 1):
            if status == "ok":
                ok += 1
            elif status == "skip":
                skip += 1
            else:
                fail += 1
                logline(f"FAIL {info}")
            if i % 2000 == 0:
                rate = i / max(0.1, time.time() - t0)
                logline(f"{i}/{len(tasks)} | ok {ok:,} skip {skip:,} fail {fail:,} | {rate:.0f}/s")
    logline(f"DONE webp: ok {ok:,}, skipped {skip:,}, failed {fail:,} -> {DST}")
    # a size summary
    total = sum(p.stat().st_size for p in DST.glob("*.webp"))
    logline(f"webp total: {total/1e9:.2f} GB across {len(list(DST.glob('*.webp'))):,} files")


if __name__ == "__main__":
    raise SystemExit(main())
