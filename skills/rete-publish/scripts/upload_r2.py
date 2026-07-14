#!/usr/bin/env python3
"""Upload a dataset file or companion directory to the public R2 bucket."""

from __future__ import annotations

import argparse
import mimetypes
import os
from pathlib import Path, PurePosixPath


BUCKET = "rete"
PUBLIC_BASE = "https://data.graphplaza.com"
R2_ENV_NAMES = {"S3_API_ENDPOINT", "ACCESS_KEY_ID", "SECRET_ACCESS_KEY"}


def load_env_file(path: Path) -> None:
    """Load only R2 credentials, tolerating the repository's CRLF env file."""
    if not path.exists():
        return
    for raw_line in path.read_text(encoding="utf-8").splitlines():
        line = raw_line.strip()
        if not line or line.startswith("#") or "=" not in line:
            continue
        name, value = line.split("=", 1)
        name = name.strip()
        if name in R2_ENV_NAMES and name not in os.environ:
            os.environ[name] = value.strip()


def clean_key(key: str) -> str:
    if not key or key.startswith("/") or key.endswith("/"):
        raise ValueError("the R2 key must be a non-empty relative object path")
    path = PurePosixPath(key.replace("\\", "/"))
    if any(part in ("", ".", "..") for part in path.parts):
        raise ValueError("the R2 key cannot contain empty, '.' or '..' segments")
    return path.as_posix()


def upload_plan(source: Path, destination: str | None) -> list[tuple[Path, str]]:
    source = source.resolve()
    if not source.exists():
        raise ValueError(f"source does not exist: {source}")
    if source.is_file():
        key = destination if destination is not None else f"{source.stem}/{source.name}"
        return [(source, clean_key(key))]

    prefix = clean_key(destination if destination is not None else source.name)
    files = sorted(path for path in source.rglob("*") if path.is_file())
    if not files:
        raise ValueError(f"source directory is empty: {source}")
    return [
        (path, f"{prefix}/{path.relative_to(source).as_posix()}") for path in files
    ]


def make_client():
    try:
        import boto3
    except ImportError as error:
        raise SystemExit("boto3 is required; run through upload_bucket.sh") from error

    missing = sorted(name for name in R2_ENV_NAMES if not os.environ.get(name))
    if missing:
        raise SystemExit(f"missing R2 configuration: {', '.join(missing)}")
    return boto3.client(
        "s3",
        endpoint_url=os.environ["S3_API_ENDPOINT"],
        aws_access_key_id=os.environ["ACCESS_KEY_ID"],
        aws_secret_access_key=os.environ["SECRET_ACCESS_KEY"],
        region_name="auto",
    )


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("source", type=Path)
    parser.add_argument(
        "destination",
        nargs="?",
        help="R2 object key for a file, or key prefix for a directory",
    )
    parser.add_argument("--env-file", type=Path, default=Path(".env"))
    parser.add_argument("--bucket", default=os.environ.get("RETE_BUCKET", BUCKET))
    parser.add_argument("--dry-run", action="store_true")
    args = parser.parse_args()

    plan = upload_plan(args.source, args.destination)
    if args.dry_run:
        for source, key in plan:
            print(f"would upload {source} -> s3://{args.bucket}/{key}")
        return 0

    load_env_file(args.env_file)
    client = make_client()
    for source, key in plan:
        content_type = mimetypes.guess_type(source.name)[0] or "application/octet-stream"
        print(f"upload {source} -> s3://{args.bucket}/{key}")
        client.upload_file(
            str(source),
            args.bucket,
            key,
            ExtraArgs={"ContentType": content_type},
        )
    print(f"published {len(plan)} object(s) under {PUBLIC_BASE}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
