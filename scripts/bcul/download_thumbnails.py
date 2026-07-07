#!/usr/bin/env python3
"""Download local copies of miniatures for digitized records (resumable, parallel).

Reads a normalized JSONL, and for every record with `has_digital` and a
`thumbnail_url`, fetches the image to thumbnails/<source>/<local_id>.jpg.
Skips: already-downloaded files, and SVG placeholders (Patrinum's nanna service
returns a tiny image/svg+xml icon for records without a real digitized image).

Usage:
  python download_thumbnails.py --source patrinum --workers 8
  python download_thumbnails.py --source patrinum --limit 500   # sample
"""
from __future__ import annotations

import argparse
import concurrent.futures as cf
import json
import threading
from pathlib import Path

import sys
sys.path.insert(0, str(Path(__file__).resolve().parent))
from http_util import Fetcher, BROWSER_UA  # noqa: E402

REPO = Path(__file__).resolve().parents[2]


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--base-dir", default=str(REPO / "data" / "bcul"))
    ap.add_argument("--source", required=True)
    ap.add_argument("--workers", type=int, default=8)
    ap.add_argument("--limit", type=int, default=0)
    args = ap.parse_args()

    base = Path(args.base_dir)
    jsonl = base / "normalized" / f"{args.source}.jsonl"
    tdir = base / "thumbnails" / args.source
    tdir.mkdir(parents=True, exist_ok=True)

    targets = []
    with open(jsonl, encoding="utf-8") as fh:
        for line in fh:
            line = line.strip()
            if not line:
                continue
            r = json.loads(line)
            if r.get("has_digital") and r.get("thumbnail_url") and r.get("local_id"):
                targets.append((r["local_id"], r["thumbnail_url"]))
    if args.limit:
        targets = targets[:args.limit]

    f = Fetcher(rate=0, retries=4, timeout=45, ua=BROWSER_UA)  # nanna CDN 403s bot UAs
    lock = threading.Lock()
    stats = {"ok": 0, "skip": 0, "placeholder": 0, "fail": 0, "done": 0}

    def fetch_one(item):
        lid, url = item
        out = tdir / f"{lid}.jpg"
        if out.exists() and out.stat().st_size > 0:
            with lock:
                stats["skip"] += 1
            return
        try:
            data, ctype, status = f.get(url)
        except Exception:
            with lock:
                stats["fail"] += 1
            return
        with lock:
            stats["done"] += 1
            if data and ctype and ctype.startswith("image/") and "svg" not in ctype:
                out.write_bytes(data)
                stats["ok"] += 1
            elif ctype and "svg" in ctype:
                stats["placeholder"] += 1
            else:
                stats["fail"] += 1
            if stats["done"] % 1000 == 0:
                print(f"  processed {stats['done']:,}/{len(targets):,} | saved {stats['ok']:,} "
                      f"placeholder {stats['placeholder']:,} fail {stats['fail']:,}", flush=True)

    print(f"{args.source}: {len(targets):,} digitized records to check ({args.workers} workers)")
    with cf.ThreadPoolExecutor(max_workers=args.workers) as ex:
        list(ex.map(fetch_one, targets))
    print(f"done: saved {stats['ok']:,} miniatures, {stats['skip']:,} already present, "
          f"{stats['placeholder']:,} placeholders skipped, {stats['fail']:,} failed -> {tdir}")


if __name__ == "__main__":
    main()
