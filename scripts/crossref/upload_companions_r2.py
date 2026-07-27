"""Resumable R2 companion upload for the crossref Parquet (and metadata).

Same credentials/endpoint conventions as skills/rete-publish/scripts/upload_r2.py,
plus: existing objects with matching size are SKIPPED, so an interrupted 170 GB
run resumes instead of restarting — and a final rerun that reports "0 to upload"
is the byte-size verification gate before deleting the local copy.

Usage (in a container with boto3; see upload_bucket.sh for the env contract):
  python scripts/crossref/upload_companions_r2.py            # upload + resume
  python scripts/crossref/upload_companions_r2.py --verify   # report-only
"""

import argparse
import mimetypes
import os
import sys
from pathlib import Path

BUCKET = os.environ.get("RETE_BUCKET", "rete")

SOURCES = [
    # (local path, key prefix or exact key)
    (Path("data/crossref/parquet-2026"), "crossref/parquet-2026"),
    (Path("data/crossref/schema.json"), "crossref/schema.json"),
    (Path("data/crossref/croissant.jsonld"), "crossref/croissant.jsonld"),
    (Path("data/crossref/crossref.ttl"), "crossref/crossref.ttl"),
]


def plan():
    out = []
    for src, dest in SOURCES:
        if src.is_file():
            out.append((src, dest))
        elif src.is_dir():
            for p in sorted(p for p in src.rglob("*") if p.is_file()):
                out.append((p, dest + "/" + p.relative_to(src).as_posix()))
    return out


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--verify", action="store_true", help="report-only, upload nothing")
    args = ap.parse_args()

    import boto3
    client = boto3.client(
        "s3",
        endpoint_url=os.environ["S3_API_ENDPOINT"],
        aws_access_key_id=os.environ["ACCESS_KEY_ID"],
        aws_secret_access_key=os.environ["SECRET_ACCESS_KEY"],
        region_name="auto",
    )

    # existing objects under crossref/ -> {key: size}
    remote = {}
    token = None
    while True:
        kw = {"Bucket": BUCKET, "Prefix": "crossref/"}
        if token:
            kw["ContinuationToken"] = token
        resp = client.list_objects_v2(**kw)
        for o in resp.get("Contents", []):
            remote[o["Key"]] = o["Size"]
        if not resp.get("IsTruncated"):
            break
        token = resp["NextContinuationToken"]

    todo, skipped, mismatched = [], 0, []
    for src, key in plan():
        size = src.stat().st_size
        if remote.get(key) == size:
            skipped += 1
        else:
            if key in remote:
                mismatched.append(key)
            todo.append((src, key, size))

    print(f"plan: {len(todo)} to upload, {skipped} already present, "
          f"{len(mismatched)} size-mismatched (will re-upload)", flush=True)
    if args.verify or not todo:
        print("VERIFY: " + ("COMPLETE — every local file is on R2 with matching size"
                            if not todo else "INCOMPLETE"), flush=True)
        return 0 if not todo else 2

    done = 0
    for src, key, size in todo:
        ctype = mimetypes.guess_type(src.name)[0] or "application/octet-stream"
        client.upload_file(str(src), BUCKET, key, ExtraArgs={"ContentType": ctype})
        done += 1
        if done % 50 == 0 or done == len(todo):
            print(f"uploaded {done}/{len(todo)}", flush=True)
    print(f"published {done} object(s) under https://data.graphplaza.com/crossref/", flush=True)
    return 0


if __name__ == "__main__":
    sys.exit(main())
