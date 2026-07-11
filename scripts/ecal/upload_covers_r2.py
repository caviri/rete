#!/usr/bin/env python3
"""Bulk-upload the ECAL WebP covers to R2 (bucket `rete`, key ecal/covers/<id>.webp).

Served at https://data.graphplaza.com/ecal/covers/<id>.webp. Threaded, resumable
(a local manifest of done keys), sets Content-Type image/webp. Reads creds from env
(source .env first: `set -a; . ./.env; set +a`).

Usage:  python scripts/ecal/upload_covers_r2.py [--jobs 16]
"""
from __future__ import annotations

import argparse
import os
import sys
import threading
from concurrent.futures import ThreadPoolExecutor
from pathlib import Path

import boto3
from botocore.config import Config

REPO = Path(__file__).resolve().parents[2]
SRC = REPO / "data" / "ecal" / "covers_webp"
DONE = REPO / "data" / "ecal" / "logs" / "webp_uploaded.txt"
BUCKET = os.environ.get("RETE_BUCKET", "rete")
PREFIX = "ecal/covers/"


def client():
    return boto3.client(
        "s3",
        endpoint_url=os.environ.get("S3_API_ENDPOINT") or os.environ["BUCKET_ENDPOINT"],
        aws_access_key_id=os.environ["ACCESS_KEY_ID"],
        aws_secret_access_key=os.environ["SECRET_ACCESS_KEY"],
        region_name="auto",
        config=Config(retries={"max_attempts": 5, "mode": "standard"}, max_pool_connections=64),
    )


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--jobs", type=int, default=16)
    args = ap.parse_args()

    DONE.parent.mkdir(parents=True, exist_ok=True)
    done = set()
    if DONE.exists():
        done = {l.strip() for l in open(DONE, encoding="utf-8") if l.strip()}

    files = sorted(SRC.glob("*.webp"))
    todo = [p for p in files if (PREFIX + p.name) not in done]
    print(f"upload: {len(files):,} webp, {len(done):,} already done, {len(todo):,} to go -> {BUCKET}/{PREFIX}",
          flush=True)

    s3 = client()
    lock = threading.Lock()
    counters = {"ok": 0, "fail": 0}
    fh = open(DONE, "a", encoding="utf-8")

    def up(p: Path):
        key = PREFIX + p.name
        try:
            s3.upload_file(str(p), BUCKET, key,
                           ExtraArgs={"ContentType": "image/webp", "CacheControl": "public, max-age=31536000"})
            with lock:
                counters["ok"] += 1
                fh.write(key + "\n")
                if counters["ok"] % 1000 == 0:
                    fh.flush()
                    print(f"  {counters['ok']:,}/{len(todo):,} uploaded (fail {counters['fail']})", flush=True)
        except Exception as e:
            with lock:
                counters["fail"] += 1
                if counters["fail"] <= 20:
                    print(f"  FAIL {key}: {type(e).__name__} {e}", flush=True)

    with ThreadPoolExecutor(max_workers=args.jobs) as ex:
        list(ex.map(up, todo))
    fh.flush(); fh.close()
    print(f"DONE upload: ok {counters['ok']:,}, fail {counters['fail']:,}", flush=True)
    return 1 if counters["fail"] else 0


if __name__ == "__main__":
    raise SystemExit(main())
