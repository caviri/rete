#!/usr/bin/env python3
"""Parallel, resumable bulk upload of a directory tree to the public R2 bucket.

`upload_r2.py` uploads serially, which is fine for a .rete plus companions but
hopeless for a source tree: the ecosyste.ms harvest is ~140,000 objects, and
one-at-a-time would take days.

This uploader:
  * lists the destination prefix ONCE and skips objects already present at the
    same size, so an interrupted run resumes cheaply instead of re-PUTting
  * uploads with a thread pool
  * sets a sensible Content-Type per extension

Deliberately does NOT set `Content-Encoding: gzip` on `.gz` objects. That would
make browsers transparently inflate them, so a download would no longer be the
bytes we harvested; they are served as `application/gzip` and stay verbatim.

Usage:
    python upload_r2_bulk.py <local-dir> <key-prefix> [--workers 24] [--dry-run]
    python upload_r2_bulk.py data/ecosyste-ms-science/raw ecosyste-ms-science/sources
"""

from __future__ import annotations

import argparse
import concurrent.futures as cf
import os
import sys
import threading
import time
from pathlib import Path, PurePosixPath

BUCKET = os.environ.get("RETE_BUCKET", "rete")
PUBLIC_BASE = "https://data.graphplaza.com"
R2_ENV_NAMES = {"S3_API_ENDPOINT", "ACCESS_KEY_ID", "SECRET_ACCESS_KEY"}

CONTENT_TYPES = {
    ".json": "application/json",
    ".ndjson": "application/x-ndjson",
    ".jsonl": "application/x-ndjson",
    ".html": "text/html; charset=utf-8",
    ".htm": "text/html; charset=utf-8",
    ".gz": "application/gzip",
    ".tsv": "text/tab-separated-values; charset=utf-8",
    ".csv": "text/csv; charset=utf-8",
    ".txt": "text/plain; charset=utf-8",
    ".md": "text/markdown; charset=utf-8",
    ".yaml": "application/yaml",
    ".yml": "application/yaml",
    ".parquet": "application/vnd.apache.parquet",
    ".rete": "application/octet-stream",
}

_lock = threading.Lock()


def load_env(path: Path) -> None:
    if not path.exists():
        return
    for raw in path.read_text(encoding="utf-8").splitlines():
        line = raw.strip()
        if not line or line.startswith("#") or "=" not in line:
            continue
        k, v = line.split("=", 1)
        k = k.strip()
        if k in R2_ENV_NAMES and k not in os.environ:
            os.environ[k] = v.strip()


def client():
    import boto3
    missing = sorted(n for n in R2_ENV_NAMES if not os.environ.get(n))
    if missing:
        raise SystemExit(f"missing R2 configuration: {', '.join(missing)}")
    return boto3.client(
        "s3",
        endpoint_url=os.environ["S3_API_ENDPOINT"],
        aws_access_key_id=os.environ["ACCESS_KEY_ID"],
        aws_secret_access_key=os.environ["SECRET_ACCESS_KEY"],
        region_name="auto",
    )


def content_type(name: str) -> str:
    suf = PurePosixPath(name).suffix.lower()
    return CONTENT_TYPES.get(suf, "application/octet-stream")


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("source", type=Path)
    ap.add_argument("prefix")
    ap.add_argument("--workers", type=int, default=24)
    ap.add_argument("--bucket", default=BUCKET)
    ap.add_argument("--env-file", type=Path, default=Path(".env"))
    ap.add_argument("--dry-run", action="store_true")
    a = ap.parse_args()

    src = a.source.resolve()
    if not src.is_dir():
        raise SystemExit(f"not a directory: {src}")
    prefix = a.prefix.strip("/")

    files = sorted(p for p in src.rglob("*") if p.is_file())
    total_bytes = sum(p.stat().st_size for p in files)
    print(f"local: {len(files):,} files, {total_bytes / 1e9:.2f} GB", flush=True)

    load_env(a.env_file)
    s3 = client()

    existing: dict[str, int] = {}
    pag = s3.get_paginator("list_objects_v2")
    for page in pag.paginate(Bucket=a.bucket, Prefix=prefix + "/"):
        for o in page.get("Contents", []):
            existing[o["Key"]] = o["Size"]
    print(f"remote: {len(existing):,} objects already under {prefix}/", flush=True)

    todo = []
    for p in files:
        key = f"{prefix}/{p.relative_to(src).as_posix()}"
        if existing.get(key) == p.stat().st_size:
            continue
        todo.append((p, key))
    todo_bytes = sum(p.stat().st_size for p, _ in todo)
    print(f"to upload: {len(todo):,} objects, {todo_bytes / 1e9:.2f} GB",
          flush=True)
    if a.dry_run:
        for p, k in todo[:20]:
            print(f"  would upload {p} -> s3://{a.bucket}/{k}")
        print(f"  ... ({len(todo):,} total)")
        return 0
    if not todo:
        print("nothing to do")
        return 0

    done = failed = 0
    sent = 0
    t0 = time.time()

    def one(item):
        p, key = item
        s3.upload_file(str(p), a.bucket, key,
                       ExtraArgs={"ContentType": content_type(p.name)})
        return p.stat().st_size

    with cf.ThreadPoolExecutor(max_workers=a.workers) as ex:
        futs = {ex.submit(one, it): it for it in todo}
        for fut in cf.as_completed(futs):
            p, key = futs[fut]
            try:
                sent += fut.result()
                done += 1
            except Exception as e:  # noqa: BLE001
                failed += 1
                with _lock:
                    print(f"  !! {key}: {e}", flush=True)
                continue
            if done % 2000 == 0:
                el = time.time() - t0
                rate = sent / max(el, 1e-9) / 1e6
                left = (todo_bytes - sent) / max(sent / max(el, 1e-9), 1e-9)
                with _lock:
                    print(f"  {done:,}/{len(todo):,} objects, "
                          f"{sent / 1e9:.2f}/{todo_bytes / 1e9:.2f} GB, "
                          f"{rate:.1f} MB/s, ~{left / 60:.0f} min left",
                          flush=True)

    print(f"uploaded {done:,} objects ({sent / 1e9:.2f} GB), {failed} failed")
    print(f"public base: {PUBLIC_BASE}/{prefix}/")
    return 1 if failed else 0


if __name__ == "__main__":
    sys.exit(main())
