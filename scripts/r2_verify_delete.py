"""Verify-then-delete: remove a local file ONLY if R2 already holds it at the exact
same byte size. Safe by construction — anything not confirmed on R2 is kept.

Reads one or more manifests of `localpath=r2key` lines (same format the uploaders
use). Reports freed bytes and any file skipped (missing locally, or size mismatch =
not safely backed). Never deletes on a HEAD error.

Usage:
  python scripts/r2_verify_delete.py --manifest a.txt --manifest b.txt
  python scripts/r2_verify_delete.py --manifest a.txt --dry-run
"""
import argparse
import os

import boto3

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


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--manifest", action="append", default=[])
    ap.add_argument("--dry-run", action="store_true")
    args = ap.parse_args()

    pairs = []
    for m in args.manifest:
        for line in open(m):
            line = line.strip()
            if line and not line.startswith("#") and "=" in line:
                pairs.append(line.split("=", 1))

    s3, bucket = client()
    freed = 0
    deleted = kept = 0
    for local, key in pairs:
        lp = local if os.path.isabs(local) else os.path.join(ROOT, local)
        if not os.path.exists(lp):
            continue  # already gone
        sz = os.path.getsize(lp)
        try:
            rs = s3.head_object(Bucket=bucket, Key=key)["ContentLength"]
        except Exception as e:
            print(f"  KEEP (R2 miss: {str(e)[:40]})  {local}", flush=True); kept += 1; continue
        if rs != sz:
            print(f"  KEEP (size {sz} != R2 {rs})  {local}", flush=True); kept += 1; continue
        if args.dry_run:
            print(f"  would delete {sz/1e9:6.2f} GB  {local}  (R2 ok)", flush=True)
        else:
            os.remove(lp)
            print(f"  deleted {sz/1e9:6.2f} GB  {local}", flush=True)
        freed += sz
        deleted += 1
    verb = "would free" if args.dry_run else "freed"
    print(f"DONE: {deleted} files {verb} {freed/1e9:.2f} GB; {kept} kept (not confirmed on R2)", flush=True)


if __name__ == "__main__":
    main()
