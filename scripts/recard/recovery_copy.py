#!/usr/bin/env python3
"""Take a dated, server-side copy of a published `.rete` before overwriting it.

A re-card rewrites the whole file, so publishing one **replaces** the object a
reader is already fetching. There is no undo unless a copy of the old bytes
exists under a second key first. This makes that copy — and makes it the cheap
way: R2 copies **server side**, so nothing is downloaded or uploaded, and R2
does not bill per-request storage operations at the scale of a catalogue.

The copy is named after the live object's own `Last-Modified`, following the
`gharchive/gharchive-2026-06.rete` convention of a dated sibling in the same
prefix:

    gbif-birds/gbif-birds.rete   ->  gbif-birds/gbif-birds-2026-07-14.rete
    subtitles/tears_of_steel.rete -> subtitles/tears_of_steel-2026-07-14.rete
    deps-dev/deps-dev-cargo.rete -> deps-dev/deps-dev-cargo-2026-07-27.rete

Dating from `Last-Modified` rather than from today is deliberate: the name then
says *which* version was preserved, so a second re-card months later does not
collide with the first copy and does not overwrite the only recovery point.

Idempotent. A copy that already exists with the same byte size is left alone;
one that exists with a DIFFERENT size is an error, never an overwrite — that
would be destroying the recovery point the command exists to create.

Usage:
  python scripts/recard/recovery_copy.py gbif-birds/gbif-birds.rete ...
  python scripts/recard/recovery_copy.py --manifest keys.txt    # one key per line
  python scripts/recard/recovery_copy.py --dry-run <keys...>    # print, touch nothing

Reads ACCESS_KEY_ID / SECRET_ACCESS_KEY / S3_API_ENDPOINT from `.env`, like the
other R2 scripts here. Bucket from `$RETE_BUCKET` (default `rete`).
"""
import os
import sys

import boto3

ROOT = os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))


def client():
    env = {}
    with open(os.path.join(ROOT, ".env"), encoding="utf-8") as fh:
        for line in fh:
            line = line.strip()
            if "=" in line and not line.startswith("#"):
                k, v = line.split("=", 1)
                env[k.strip()] = v.strip().strip('"').strip("'")
    missing = [k for k in ("S3_API_ENDPOINT", "ACCESS_KEY_ID", "SECRET_ACCESS_KEY")
               if not env.get(k)]
    if missing:
        raise SystemExit(f"missing from .env: {', '.join(missing)}")
    return boto3.client(
        "s3", endpoint_url=env["S3_API_ENDPOINT"],
        aws_access_key_id=env["ACCESS_KEY_ID"],
        aws_secret_access_key=env["SECRET_ACCESS_KEY"],
    ), os.environ.get("RETE_BUCKET", "rete")


def dated_key(key: str, last_modified) -> str:
    """`a/b.rete` + 2026-07-14 -> `a/b-2026-07-14.rete`."""
    stem, dot, ext = key.rpartition(".")
    if not dot:
        stem, ext = key, ""
        dot = ""
    return f"{stem}-{last_modified.strftime('%Y-%m-%d')}{dot}{ext}"


def main() -> int:
    args = sys.argv[1:]
    dry = False
    if "--dry-run" in args:
        dry = True
        args.remove("--dry-run")
    keys = []
    if args and args[0] == "--manifest":
        with open(args[1], encoding="utf-8") as fh:
            keys = [ln.strip() for ln in fh if ln.strip() and not ln.startswith("#")]
    else:
        keys = args
    if not keys:
        raise SystemExit(__doc__)

    s3, bucket = client()
    rc = 0
    for key in keys:
        try:
            head = s3.head_object(Bucket=bucket, Key=key)
        except Exception as exc:                       # noqa: BLE001 - report, continue
            print(f"MISSING  {key}: {exc}")
            rc = 1
            continue
        size, lm = head["ContentLength"], head["LastModified"]
        dst = dated_key(key, lm)
        try:
            existing = s3.head_object(Bucket=bucket, Key=dst)
        except Exception:                              # noqa: BLE001 - absent is normal
            existing = None
        if existing is not None:
            if existing["ContentLength"] == size:
                print(f"HAVE     {dst}  ({size} bytes, already the recovery copy)")
                continue
            print(f"CONFLICT {dst}: exists at {existing['ContentLength']} bytes, "
                  f"source is {size} — refusing to overwrite a recovery point")
            rc = 1
            continue
        if dry:
            print(f"WOULD    {key} -> {dst}  ({size} bytes, Last-Modified {lm:%Y-%m-%d})")
            continue
        # Server-side: the bytes never leave R2. Multipart is unnecessary below
        # 5 GB, which every file in this catalogue's re-card scope is under; a
        # larger one fails loudly here rather than silently truncating.
        if size > 5 * 1024 ** 3:
            print(f"TOO BIG  {key}: {size} bytes needs a multipart copy")
            rc = 1
            continue
        s3.copy_object(Bucket=bucket, Key=dst,
                       CopySource={"Bucket": bucket, "Key": key})
        got = s3.head_object(Bucket=bucket, Key=dst)["ContentLength"]
        if got != size:
            print(f"BAD      {dst}: copied {got} bytes, expected {size}")
            rc = 1
            continue
        print(f"COPIED   {key} -> {dst}  ({size} bytes)")
    return rc


if __name__ == "__main__":
    sys.exit(main())
