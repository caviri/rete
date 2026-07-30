"""Upload specific local files to the R2 `rete` bucket at chosen keys.

Companion to r2_upload_folder.py (which mirrors whole parquet dirs). This one takes
explicit `localpath=r2key` pairs — used to archive individual built `.rete` files
(e.g. superseded/monolithic builds) to R2 before reclaiming local disk. Idempotent:
skips any object already present with the same byte size; multipart-uploads large
files; reads ACCESS_KEY_ID / SECRET_ACCESS_KEY / S3_API_ENDPOINT from .env.

Usage:
  python scripts/r2_upload_files.py \
    data/databnf/databnf-full.rete=databnf/databnf-full.rete \
    web/gharchive-2025-07-22.rete=gharchive/gharchive-2025-07-22.rete
  python scripts/r2_upload_files.py --manifest tierB.txt   # one pair per line
"""
import os
import sys

import boto3
from boto3.s3.transfer import TransferConfig

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))


def client():
    env = {}
    for line in open(os.path.join(ROOT, ".env")):
        line = line.strip()
        if "=" in line and not line.startswith("#"):
            k, v = line.split("=", 1)
            env[k.strip()] = v.strip().strip('"').strip("'")
    return boto3.client(
        "s3", endpoint_url=env["S3_API_ENDPOINT"],
        aws_access_key_id=env["ACCESS_KEY_ID"],
        aws_secret_access_key=env["SECRET_ACCESS_KEY"],
    ), os.environ.get("RETE_BUCKET", "rete")


def remote_size(s3, bucket, key):
    try:
        return s3.head_object(Bucket=bucket, Key=key)["ContentLength"]
    except Exception:
        return None


def main():
    args = sys.argv[1:]
    pairs = []
    if args and args[0] == "--manifest":
        for line in open(args[1]):
            line = line.strip()
            if line and not line.startswith("#") and "=" in line:
                pairs.append(line.split("=", 1))
    else:
        for a in args:
            if "=" in a:
                pairs.append(a.split("=", 1))
    if not pairs:
        raise SystemExit("no localpath=r2key pairs given")

    s3, bucket = client()
    cfg = TransferConfig(multipart_threshold=64 * 1024 * 1024,
                         multipart_chunksize=64 * 1024 * 1024,
                         max_concurrency=8, use_threads=True)
    done = 0
    for local, key in pairs:
        lp = local if os.path.isabs(local) else os.path.join(ROOT, local)
        if not os.path.exists(lp):
            print(f"  MISSING {local} — skip", flush=True); continue
        sz = os.path.getsize(lp)
        rs = remote_size(s3, bucket, key)
        if rs == sz:
            print(f"  = already on R2 ({sz/1e9:.2f} GB)  {key}", flush=True); done += 1; continue
        print(f"  ^ uploading {sz/1e9:.2f} GB  {local} -> {key}", flush=True)
        ctype = "application/x-parquet" if key.endswith(".parquet") else "application/octet-stream"
        s3.upload_file(lp, bucket, key,
                       ExtraArgs={"ContentType": ctype}, Config=cfg)
        rs2 = remote_size(s3, bucket, key)
        ok = "OK" if rs2 == sz else f"SIZE MISMATCH ({rs2} != {sz})"
        print(f"    {ok}  https://data.graphplaza.com/{key}", flush=True)
        done += 1
    print(f"DONE: {done}/{len(pairs)} files on R2", flush=True)


if __name__ == "__main__":
    main()
