#!/usr/bin/env python3
"""Atomically promote finalized experimental 0x04 R2 objects to stable 0x05.

The 0x05 generation intentionally retains the exact 0x04 payload layout. The
file content hash covers payload sections, not the header, so this operation
changes only header byte 4. Large objects use an in-place multipart upload:
the first part is downloaded and patched while every remaining part is copied
inside R2, then completion atomically replaces the old object.
"""

from __future__ import annotations

import argparse
from pathlib import Path
import os
import sys
from urllib.parse import unquote, urlsplit


ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / "scripts"))

from check_dataset_catalog import catalog_targets, load_catalog  # noqa: E402


BUCKET = "rete"
R2_HOST = "data.graphplaza.com"
HEADER_LEN = 1024
PART_SIZE = 64 * 1024 * 1024
SOURCE_VERSION = 4
TARGET_VERSION = 5
R2_ENV_NAMES = {"S3_API_ENDPOINT", "ACCESS_KEY_ID", "SECRET_ACCESS_KEY"}


def load_env_file(path: Path) -> None:
    """Load only the three R2 values, stripping the repository's CRLF safely."""
    for raw_line in path.read_text(encoding="utf-8").splitlines():
        line = raw_line.strip()
        if not line or line.startswith("#") or "=" not in line:
            continue
        name, value = line.split("=", 1)
        name = name.strip()
        if name in R2_ENV_NAMES:
            os.environ[name] = value.strip()


def object_key(url: str) -> str:
    parts = urlsplit(url)
    if parts.scheme != "https" or parts.netloc != R2_HOST:
        raise ValueError(f"not an R2 catalog URL: {url}")
    key = unquote(parts.path).lstrip("/")
    if not key or key.endswith("/"):
        raise ValueError(f"URL does not name an R2 object: {url}")
    return key


def part_ranges(size: int, part_size: int = PART_SIZE) -> list[tuple[int, int, int]]:
    if size <= 0:
        raise ValueError("object size must be positive")
    if part_size < 5 * 1024 * 1024:
        raise ValueError("multipart part size must be at least 5 MiB")
    ranges = []
    start = 0
    part_number = 1
    while start < size:
        end = min(size, start + part_size) - 1
        ranges.append((part_number, start, end))
        part_number += 1
        start = end + 1
    return ranges


def promote_header(header: bytes) -> bytes:
    if len(header) < HEADER_LEN:
        raise ValueError(f"expected a {HEADER_LEN}-byte header")
    if header[:4] != b"RETE":
        raise ValueError("bad RETE magic")
    if header[4] != SOURCE_VERSION:
        raise ValueError(f"expected source format byte {SOURCE_VERSION}, got {header[4]}")
    if int.from_bytes(header[6:8], "little") != HEADER_LEN:
        raise ValueError("expected the finalized 1024-byte header layout")
    promoted = bytearray(header)
    promoted[4] = TARGET_VERSION
    return bytes(promoted)


def object_options(head: dict) -> dict:
    mapping = {
        "ContentType": "ContentType",
        "CacheControl": "CacheControl",
        "ContentDisposition": "ContentDisposition",
        "ContentEncoding": "ContentEncoding",
        "ContentLanguage": "ContentLanguage",
        "Expires": "Expires",
        "WebsiteRedirectLocation": "WebsiteRedirectLocation",
        "Metadata": "Metadata",
    }
    return {
        destination: head[source]
        for source, destination in mapping.items()
        if head.get(source) not in (None, "", {})
    }


def get_range(client, bucket: str, key: str, start: int, end: int) -> bytes:
    response = client.get_object(Bucket=bucket, Key=key, Range=f"bytes={start}-{end}")
    return response["Body"].read()


def inspect_source(client, bucket: str, key: str) -> dict:
    head = client.head_object(Bucket=bucket, Key=key)
    size = head["ContentLength"]
    if size < HEADER_LEN + 4:
        raise ValueError(f"{key}: object is too small to be a .rete file")
    header = get_range(client, bucket, key, 0, HEADER_LEN - 1)
    footer = get_range(client, bucket, key, size - 4, size - 1)
    if footer != b"RETE":
        raise ValueError(f"{key}: missing RETE footer")
    if header[:4] != b"RETE":
        raise ValueError(f"{key}: missing RETE header magic")
    if int.from_bytes(header[6:8], "little") != HEADER_LEN:
        raise ValueError(f"{key}: not the finalized 1024-byte layout")
    return {
        "head": head,
        "size": size,
        "header": header,
        "contentHash": header[8:24].hex(),
        "version": header[4],
    }


def promote_small(client, bucket: str, key: str, source: dict) -> None:
    body = get_range(client, bucket, key, 0, source["size"] - 1)
    body = promote_header(body[:HEADER_LEN]) + body[HEADER_LEN:]
    client.put_object(
        Bucket=bucket,
        Key=key,
        Body=body,
        **object_options(source["head"]),
    )


def promote_multipart(client, bucket: str, key: str, source: dict) -> None:
    upload = client.create_multipart_upload(
        Bucket=bucket,
        Key=key,
        **object_options(source["head"]),
    )
    upload_id = upload["UploadId"]
    completed = []
    ranges = part_ranges(source["size"])
    try:
        first_number, first_start, first_end = ranges[0]
        first = get_range(client, bucket, key, first_start, first_end)
        first = promote_header(first[:HEADER_LEN]) + first[HEADER_LEN:]
        response = client.upload_part(
            Bucket=bucket,
            Key=key,
            UploadId=upload_id,
            PartNumber=first_number,
            Body=first,
        )
        completed.append({"PartNumber": first_number, "ETag": response["ETag"]})

        for part_number, start, end in ranges[1:]:
            response = client.upload_part_copy(
                Bucket=bucket,
                Key=key,
                UploadId=upload_id,
                PartNumber=part_number,
                CopySource={"Bucket": bucket, "Key": key},
                CopySourceRange=f"bytes={start}-{end}",
            )
            completed.append(
                {
                    "PartNumber": part_number,
                    "ETag": response["CopyPartResult"]["ETag"],
                }
            )
        client.complete_multipart_upload(
            Bucket=bucket,
            Key=key,
            UploadId=upload_id,
            MultipartUpload={"Parts": completed},
        )
    except Exception:
        client.abort_multipart_upload(
            Bucket=bucket,
            Key=key,
            UploadId=upload_id,
        )
        raise


def promote_one(client, bucket: str, key: str, apply: bool) -> dict:
    before = inspect_source(client, bucket, key)
    if before["version"] == TARGET_VERSION:
        return {"key": key, "status": "already-stable", **before}
    promote_header(before["header"])
    if not apply:
        return {"key": key, "status": "would-promote", **before}

    if before["size"] <= PART_SIZE:
        promote_small(client, bucket, key, before)
    else:
        promote_multipart(client, bucket, key, before)

    after = inspect_source(client, bucket, key)
    if after["size"] != before["size"]:
        raise RuntimeError(f"{key}: size changed during promotion")
    if after["contentHash"] != before["contentHash"]:
        raise RuntimeError(f"{key}: payload content hash changed during promotion")
    if after["version"] != TARGET_VERSION:
        raise RuntimeError(f"{key}: stable format byte was not persisted")
    return {"key": key, "status": "promoted", **after}


def make_client():
    missing = [
        name
        for name in ("S3_API_ENDPOINT", "ACCESS_KEY_ID", "SECRET_ACCESS_KEY")
        if not os.environ.get(name)
    ]
    if missing:
        raise RuntimeError(f"missing R2 environment variables: {', '.join(missing)}")
    import boto3
    from botocore.config import Config

    return boto3.client(
        "s3",
        endpoint_url=os.environ["S3_API_ENDPOINT"],
        aws_access_key_id=os.environ["ACCESS_KEY_ID"],
        aws_secret_access_key=os.environ["SECRET_ACCESS_KEY"],
        region_name="auto",
        config=Config(
            read_timeout=600,
            connect_timeout=60,
            retries={"max_attempts": 5, "mode": "standard"},
        ),
    )


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    selection = parser.add_mutually_exclusive_group(required=True)
    selection.add_argument("--all", action="store_true")
    selection.add_argument("--key", action="append", help="catalog key (repeatable)")
    parser.add_argument("--apply", action="store_true", help="mutate R2; default is dry-run")
    parser.add_argument("--bucket", default=BUCKET)
    parser.add_argument("--env-file", type=Path, help="gitignored CRLF .env with R2 credentials")
    args = parser.parse_args()

    if args.env_file:
        load_env_file(args.env_file)

    targets = catalog_targets(load_catalog())
    if args.key:
        requested = set(args.key)
        targets = [target for target in targets if target["key"] in requested]
        missing = requested - {target["key"] for target in targets}
        if missing:
            parser.error(f"unknown catalog key(s): {', '.join(sorted(missing))}")

    r2_targets = []
    for target in targets:
        try:
            r2_targets.append((target["key"], object_key(target["url"])))
        except ValueError:
            print(f"SKIP {target['key']}: external URL")

    client = make_client()
    failures = 0
    for catalog_key, key in r2_targets:
        try:
            result = promote_one(client, args.bucket, key, args.apply)
            print(
                f"{result['status'].upper():14s} {catalog_key}: "
                f"{result['size']} bytes {result['contentHash']}"
            )
        except Exception as error:
            failures += 1
            print(f"FAIL {catalog_key}: {type(error).__name__}: {error}", file=sys.stderr)
    action = "promotion" if args.apply else "dry-run"
    print(f"{action}: {len(r2_targets) - failures}/{len(r2_targets)} object(s)")
    return 1 if failures else 0


if __name__ == "__main__":
    raise SystemExit(main())
