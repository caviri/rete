#!/usr/bin/env python3
"""Back up the deps-dev raw Parquet to Cloudflare R2 (no local deletion).

Mirrors data/deps-dev/raw/*.parquet -> r2:<bucket>/deps-dev/raw/<file>, public at
https://data.graphplaza.com/deps-dev/raw/<file>. Resumable/idempotent: an object
already present with the same byte size is skipped, so a killed run just resumes.
Smallest-first so creds/pipeline validate before the 64 GB PackageVersions.

Direct boto3, reading ACCESS_KEY_ID / SECRET_ACCESS_KEY / S3_API_ENDPOINT from
.env (same pattern as scripts/r2_upload_folder.py). Bucket defaults to `rete`.
"""
import glob
import json
import os
import threading

import boto3
from botocore.config import Config
from botocore.exceptions import ClientError
from boto3.s3.transfer import TransferConfig

ROOT = "/w"
RAW = os.path.join(ROOT, "data", "deps-dev", "raw")
PREFIX = "deps-dev/raw"
BUCKET = os.environ.get("RETE_BUCKET", "rete")


def load_env():
    env = {}
    with open(os.path.join(ROOT, ".env"), encoding="utf-8") as f:
        for line in f:
            if "=" in line and not line.strip().startswith("#"):
                k, v = line.split("=", 1)
                env[k.strip()] = v.strip().strip('"').strip("'")
    return env


env = load_env()
s3 = boto3.client(
    "s3", endpoint_url=env["S3_API_ENDPOINT"],
    aws_access_key_id=env["ACCESS_KEY_ID"],
    aws_secret_access_key=env["SECRET_ACCESS_KEY"],
    region_name="auto",
    config=Config(retries={"max_attempts": 10, "mode": "adaptive"}),
)
CFG = TransferConfig(multipart_threshold=64 * 2**20, multipart_chunksize=128 * 2**20,
                     max_concurrency=8, use_threads=True)


def remote_size(key):
    try:
        return s3.head_object(Bucket=BUCKET, Key=key)["ContentLength"]
    except ClientError as e:
        if e.response["Error"]["Code"] in ("404", "NoSuchKey", "NotFound"):
            return None
        raise


def upload_one(src, key, size):
    done, step, lock = [0], [0], threading.Lock()

    def cb(n):
        with lock:
            done[0] += n
            d = int((100 * done[0] / size) // 10) if size else 10
            if d > step[0]:
                step[0] = d
                print(f"      {key}: {done[0]/2**30:.1f}/{size/2**30:.1f} GiB "
                      f"({100*done[0]/size:.0f}%)", flush=True)

    s3.upload_file(src, BUCKET, key, Config=CFG, Callback=cb,
                   ExtraArgs={"ContentType": "application/x-parquet"})


files = sorted(glob.glob(os.path.join(RAW, "*.parquet")), key=os.path.getsize)
manifest = {"dataset": "deps-dev", "prefix": PREFIX, "files": [], "count": len(files),
            "total_bytes": sum(os.path.getsize(f) for f in files)}
print(f"=== deps-dev -> r2:{PREFIX}/  ({len(files)} files, "
      f"{manifest['total_bytes']/2**30:.1f} GB) ===", flush=True)

up = up_b = skip = skip_b = 0
for i, src in enumerate(files, 1):
    name = os.path.basename(src)
    key = f"{PREFIX}/{name}"
    size = os.path.getsize(src)
    manifest["files"].append({"key": key, "size": size})
    if remote_size(key) == size:
        print(f"  [{i}/{len(files)}] skip {name} ({size/2**30:.1f} GB already on R2)", flush=True)
        skip += 1; skip_b += size
        continue
    print(f"  [{i}/{len(files)}] upload {name} ({size/2**30:.1f} GB)", flush=True)
    upload_one(src, key, size)
    up += 1; up_b += size
    print(f"  [{i}/{len(files)}] done {name}", flush=True)

s3.put_object(Bucket=BUCKET, Key=f"{PREFIX}/_parquet_manifest.json",
              Body=json.dumps(manifest, ensure_ascii=False).encode("utf-8"),
              ContentType="application/json")
print(f"=== DONE: uploaded {up} ({up_b/2**30:.1f} GB), "
      f"skipped {skip} ({skip_b/2**30:.1f} GB) — local copies KEPT ===", flush=True)
