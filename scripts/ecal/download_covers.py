#!/usr/bin/env python3
"""Download full-resolution ECAL book covers (BM_DOCUMENT_COVER_PAGE/<id>).

Polite, single-thread, resumable (skips files already on disk). Only records that
actually have a cover (has_digital) are fetched — the endpoint 404s otherwise, and
we detect that so no broken files are written. robots Disallow:/ — user authorized.

Usage:
  python download_covers.py                 # full (resume)
  python download_covers.py --rate 0.83
  python download_covers.py --limit 20
"""
from __future__ import annotations

import argparse
import json
import ssl
import sys
import time
import urllib.request
from pathlib import Path

REPO = Path(__file__).resolve().parents[2]
FULL = "https://cloud7.bibliomaker.ch:33000/French/BM_DOCUMENT_COVER_PAGE/"
UA = "ECAL-twin-research/1.0 (polite low-rate cover fetch; contact carlos.vivarrios@epfl.ch)"
_CTX = ssl.create_default_context(); _CTX.check_hostname = False; _CTX.verify_mode = ssl.CERT_NONE


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--base-dir", default=str(REPO / "data" / "ecal"))
    ap.add_argument("--rate", type=float, default=0.83)   # ~1 req / 1.2 s
    ap.add_argument("--limit", type=int, default=0)
    args = ap.parse_args()

    base = Path(args.base_dir)
    cov_dir = base / "covers"; cov_dir.mkdir(parents=True, exist_ok=True)
    jsonl = base / "normalized" / "ecal.jsonl"
    log = base / "logs" / "covers.log"; log.parent.mkdir(parents=True, exist_ok=True)

    targets = []
    for line in open(jsonl, encoding="utf-8"):
        r = json.loads(line)
        if r.get("has_digital") and r.get("local_id"):
            targets.append(r["local_id"])
    if args.limit:
        targets = targets[:args.limit]

    op = urllib.request.build_opener(urllib.request.HTTPSHandler(context=_CTX))
    min_int = (1.0 / args.rate) if args.rate else 0.0
    last = [0.0]
    ok = skip = fail = 0

    def logline(m):
        line = f"[{time.strftime('%Y-%m-%dT%H:%M:%SZ', time.gmtime())}] {m}"
        print(line, flush=True)
        with open(log, "a", encoding="utf-8") as fh:
            fh.write(line + "\n")

    logline(f"covers: {len(targets):,} to fetch -> {cov_dir}")
    for idx, lid in enumerate(targets, 1):
        out = cov_dir / f"{lid}.jpg"
        if out.exists() and out.stat().st_size > 0:
            skip += 1
            continue
        if min_int:
            dt = time.time() - last[0]
            if dt < min_int:
                time.sleep(min_int - dt)
        last[0] = time.time()
        done = False
        for attempt in range(4):
            try:
                r = urllib.request.Request(FULL + str(lid), headers={"User-Agent": UA})
                resp = op.open(r, timeout=45)
                ctype = resp.headers.get_content_type()
                data = resp.read()
                if ctype and ctype.startswith("image/") and data:
                    out.write_bytes(data)
                    ok += 1
                else:
                    fail += 1
                done = True
                break
            except urllib.error.HTTPError as e:
                if e.code == 404:
                    fail += 1; done = True; break
                time.sleep(min(30, 2 ** attempt * 2))
            except Exception:
                time.sleep(min(30, 2 ** attempt * 2))
        if not done:
            fail += 1
        if idx % 200 == 0:
            logline(f"{idx}/{len(targets)} | saved {ok:,} skip {skip:,} fail {fail:,}")
    logline(f"DONE covers: saved {ok:,}, already-present {skip:,}, failed {fail:,} -> {cov_dir}")


if __name__ == "__main__":
    raise SystemExit(main())
