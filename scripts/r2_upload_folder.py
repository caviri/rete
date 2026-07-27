"""Recursively upload a dataset's Parquet companions to Cloudflare R2, mirroring
the local directory structure under an R2 key prefix. Resumable and idempotent:
an object already present with the same byte size is skipped, so a killed run
just re-runs and continues. Multipart, with per-file + running progress.

Direct boto3 (reads ACCESS_KEY_ID / SECRET_ACCESS_KEY / S3_API_ENDPOINT from
.env) — deliberately NOT `uv run`, which fails under parallel-build RAM
contention. Bucket defaults to `rete`; objects are public at
https://data.graphplaza.com/<key>.

  R2 key = "<prefix>/<path of the .parquet file relative to <localdir>>"

Usage:
  python scripts/r2_upload_folder.py                 # all scholar datasets, smallest-first
  python scripts/r2_upload_folder.py --only ror      # one dataset (validation)
  python scripts/r2_upload_folder.py --dry-run       # list what would upload, no writes
"""
import argparse
import glob
import json
import os
import sys
import threading

import boto3
from botocore.config import Config
from botocore.exceptions import ClientError
from boto3.s3.transfer import TransferConfig

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.dirname(HERE)

# (local dir under data/, R2 key prefix) — smallest-first so we validate the
# pipeline and get quick wins before the 410 GB openaire tail.
DATASETS = [
    ("ror", "ror"),
    ("epfl-infoscience", "epfl-infoscience"),
    ("dblp", "dblp"),
    ("cordis", "cordis"),
    ("go-triple", "gotriple"),
    ("zenodo", "zenodo"),
    ("epfl-graph", "epfl-graph"),
    ("opencitations", "opencitations"),
    ("orcid", "orcid"),
    ("datacite", "datacite"),
    ("openaire", "openaire"),
]


def load_env():
    env = {}
    with open(os.path.join(ROOT, ".env"), encoding="utf-8") as f:
        for line in f:
            if "=" in line and not line.strip().startswith("#"):
                k, v = line.split("=", 1)
                env[k.strip()] = v.strip().strip('"').strip("'")
    return env


def client(env):
    return boto3.client(
        "s3", endpoint_url=env["S3_API_ENDPOINT"],
        aws_access_key_id=env["ACCESS_KEY_ID"],
        aws_secret_access_key=env["SECRET_ACCESS_KEY"],
        region_name="auto",
        # long unattended run: be patient with transient network errors
        config=Config(retries={"max_attempts": 10, "mode": "adaptive"}),
    )


def remote_size(s3, bucket, key):
    try:
        return s3.head_object(Bucket=bucket, Key=key)["ContentLength"]
    except ClientError as e:
        if e.response["Error"]["Code"] in ("404", "NoSuchKey", "NotFound"):
            return None
        raise


CFG = TransferConfig(multipart_threshold=64 * 2**20, multipart_chunksize=64 * 2**20,
                     max_concurrency=8, use_threads=True)


def upload_one(s3, bucket, src, key):
    size = os.path.getsize(src)
    done = [0]
    step = [0]  # last printed decile (0..10)
    lock = threading.Lock()

    def cb(n):
        with lock:
            done[0] += n
            pct = 100 * done[0] / size if size else 100
            # throttle to ~10% marks so a long unattended log stays readable
            d = int(pct // 10)
            if d > step[0]:
                step[0] = d
                print(f"      {key}: {done[0]/2**20:.0f}/{size/2**20:.0f} MiB ({pct:.0f}%)",
                      flush=True)

    s3.upload_file(src, bucket, key, ExtraArgs={"ContentType": "application/x-parquet"},
                   Config=CFG, Callback=cb)
    print()


def main():
    ap = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    ap.add_argument("--only", default="", help="single dataset localdir name")
    ap.add_argument("--dry-run", action="store_true")
    args = ap.parse_args()

    env = load_env()
    bucket = os.environ.get("RETE_BUCKET", "rete")
    s3 = None if args.dry_run else client(env)

    todo = [d for d in DATASETS if not args.only or d[0] == args.only]
    if not todo:
        print(f"no dataset matches --only {args.only!r}", file=sys.stderr)
        sys.exit(2)

    g_up = g_skip = 0
    g_up_b = g_skip_b = 0
    for localdir, prefix in todo:
        base = os.path.join(ROOT, "data", localdir)
        files = sorted(glob.glob(os.path.join(base, "**", "*.parquet"), recursive=True))
        if not files:
            print(f"[{localdir}] no local parquet — skipping"); continue
        total_b = sum(os.path.getsize(f) for f in files)
        print(f"\n=== {localdir} -> r2:{prefix}/  "
              f"({len(files)} files, {total_b/2**30:.1f} GB) ===", flush=True)

        manifest = {"dataset": localdir, "prefix": prefix, "files": [], "count": len(files),
                    "total_bytes": total_b}
        up = skip = up_b = skip_b = 0
        for i, src in enumerate(files, 1):
            rel = os.path.relpath(src, base).replace("\\", "/")
            key = f"{prefix}/{rel}"
            size = os.path.getsize(src)
            manifest["files"].append({"key": key, "size": size})
            if args.dry_run:
                print(f"  [{i}/{len(files)}] WOULD upload {key} ({size/2**20:.1f} MiB)")
                continue
            rs = remote_size(s3, bucket, key)
            if rs == size:
                skip += 1; skip_b += size
                continue
            tag = "re-upload(size!=)" if rs is not None else "upload"
            print(f"  [{i}/{len(files)}] {tag} {key} ({size/2**20:.1f} MiB)", flush=True)
            upload_one(s3, bucket, src, key)
            up += 1; up_b += size
        if not args.dry_run:
            # folder marker / contents record
            s3.put_object(Bucket=bucket, Key=f"{prefix}/_parquet_manifest.json",
                          Body=json.dumps(manifest, ensure_ascii=False).encode("utf-8"),
                          ContentType="application/json")
        print(f"  [{localdir}] uploaded {up} ({up_b/2**30:.1f} GB), "
              f"skipped {skip} ({skip_b/2**30:.1f} GB already on R2)", flush=True)
        g_up += up; g_skip += skip; g_up_b += up_b; g_skip_b += skip_b

    print(f"\n=== DONE: uploaded {g_up} files ({g_up_b/2**30:.1f} GB), "
          f"skipped {g_skip} ({g_skip_b/2**30:.1f} GB) ===")


if __name__ == "__main__":
    main()
