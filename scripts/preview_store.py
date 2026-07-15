#!/usr/bin/env python3
"""Validate, publish, and remove immutable playground preview artifacts."""

from __future__ import annotations

import argparse
import json
import mimetypes
import os
import re
import shutil
import tempfile
from pathlib import Path
from typing import Any


ALLOWED = {
    "playground.html",
    "rete_wasm_async.js",
    "rete_wasm_async.wasm",
    "coi-serviceworker.js",
    "wasm-build.json",
}
MAX_ARTIFACT_BYTES = 64 * 1024 * 1024
PREVIEW_ORIGIN = "https://preview.graphplaza.com"
SHA_PATTERN = re.compile(r"[0-9a-f]{40}")


def _validate_sha(value: str, *, name: str = "SHA") -> str:
    if not SHA_PATTERN.fullmatch(value):
        raise ValueError(f"{name} must be 40-character lowercase hex")
    return value


def object_prefix(pr_number: int, head_sha: str) -> str:
    if pr_number < 1 or not SHA_PATTERN.fullmatch(head_sha):
        raise ValueError("PR must be positive and SHA must be 40-character lowercase hex")
    return f"pr-{pr_number}/{head_sha}/"


def preview_url(pr_number: int, head_sha: str) -> str:
    return f"{PREVIEW_ORIGIN}/{object_prefix(pr_number, head_sha)}playground.html"


def validate_artifact(root: Path, head_sha: str) -> list[Path]:
    _validate_sha(head_sha, name="head SHA")
    root = Path(root)
    if not root.is_dir():
        raise ValueError("artifact root must be a directory")

    entries = sorted(root.iterdir(), key=lambda path: path.name)
    if any(path.is_symlink() for path in entries):
        raise ValueError("artifact must not contain symlinks")
    if any(not path.is_file() for path in entries):
        raise ValueError("artifact must contain exactly the preview allowlist")
    if {path.name for path in entries} != ALLOWED:
        raise ValueError("artifact must contain exactly the preview allowlist")
    if sum(path.stat().st_size for path in entries) > MAX_ARTIFACT_BYTES:
        raise ValueError("artifact exceeds 64 MiB")

    manifest_path = root / "wasm-build.json"
    try:
        manifest = json.loads(manifest_path.read_text(encoding="utf-8"))
    except (OSError, UnicodeDecodeError, json.JSONDecodeError) as error:
        raise ValueError(f"invalid wasm-build.json: {error}") from error
    if not isinstance(manifest, dict) or manifest.get("gitCommit") != head_sha:
        raise ValueError("wasm-build.json does not match head SHA")

    try:
        html = (root / "playground.html").read_text(encoding="utf-8")
    except (OSError, UnicodeDecodeError) as error:
        raise ValueError(f"invalid playground.html: {error}") from error
    if f'window.RETE_BUILD = "{head_sha[:12]}";' not in html:
        raise ValueError("playground build stamp does not match head SHA")
    return entries


def _script_safe_json(value: dict[str, Any]) -> str:
    """Encode JSON so PR-controlled strings cannot terminate an inline script."""
    return (
        json.dumps(value, separators=(",", ":"), ensure_ascii=True)
        .replace("<", "\\u003c")
        .replace(">", "\\u003e")
        .replace("&", "\\u0026")
    )


def inject_preview_metadata(html: str, metadata: dict[str, Any]) -> str:
    marker = "window.RETE_PREVIEW = null;"
    if html.count(marker) != 1:
        raise ValueError("preview metadata marker must occur exactly once")
    return html.replace(
        marker,
        f"window.RETE_PREVIEW = {_script_safe_json(metadata)};",
    )


def _validated_metadata(metadata: dict[str, Any]) -> dict[str, Any]:
    try:
        number = int(metadata["number"])
        head_sha = str(metadata["headSha"])
        base_sha = str(metadata["baseSha"])
        title = str(metadata["title"])
    except (KeyError, TypeError, ValueError) as error:
        raise ValueError("preview metadata is incomplete") from error
    object_prefix(number, head_sha)
    _validate_sha(base_sha, name="base SHA")
    return {
        "number": number,
        "headSha": head_sha,
        "baseSha": base_sha,
        "title": title,
    }


def upload_preview(
    client: Any, bucket: str, root: Path, metadata: dict[str, Any]
) -> str:
    metadata = _validated_metadata(metadata)
    files = validate_artifact(Path(root), metadata["headSha"])
    prefix = object_prefix(metadata["number"], metadata["headSha"])

    with tempfile.TemporaryDirectory() as directory:
        stage = Path(directory)
        for source in files:
            shutil.copy2(source, stage / source.name)
        page = stage / "playground.html"
        page.write_text(
            inject_preview_metadata(page.read_text(encoding="utf-8"), metadata),
            encoding="utf-8",
        )

        for source in sorted(stage.iterdir(), key=lambda path: path.name):
            immutable = source.name in {"rete_wasm_async.js", "rete_wasm_async.wasm"}
            client.upload_file(
                str(source),
                bucket,
                prefix + source.name,
                ExtraArgs={
                    "ContentType": mimetypes.guess_type(source.name)[0]
                    or "application/octet-stream",
                    "CacheControl": (
                        "public,max-age=31536000,immutable" if immutable else "no-store"
                    ),
                },
            )
    return preview_url(metadata["number"], metadata["headSha"])


def cleanup_preview(
    client: Any, bucket: str, pr_number: int, keep_sha: str | None = None
) -> int:
    if pr_number < 1:
        raise ValueError("PR must be positive")
    if keep_sha is not None:
        _validate_sha(keep_sha, name="keep SHA")
    prefix = f"pr-{pr_number}/"
    token = None
    deleted = 0
    while True:
        request: dict[str, Any] = {"Bucket": bucket, "Prefix": prefix}
        if token:
            request["ContinuationToken"] = token
        page = client.list_objects_v2(**request)
        keys = [
            item["Key"]
            for item in page.get("Contents", [])
            if not keep_sha or not item["Key"].startswith(prefix + keep_sha + "/")
        ]
        for start in range(0, len(keys), 1000):
            batch = keys[start : start + 1000]
            client.delete_objects(
                Bucket=bucket,
                Delete={"Objects": [{"Key": key} for key in batch], "Quiet": True},
            )
            deleted += len(batch)
        if not page.get("IsTruncated"):
            break
        token = page.get("NextContinuationToken")
        if not token:
            raise RuntimeError("truncated S3 response omitted continuation token")
    return deleted


def make_client():
    try:
        import boto3
    except ImportError as error:
        raise RuntimeError("boto3 is required for preview storage operations") from error

    required = {
        "endpoint_url": "PREVIEW_S3_API_ENDPOINT",
        "aws_access_key_id": "PREVIEW_ACCESS_KEY_ID",
        "aws_secret_access_key": "PREVIEW_SECRET_ACCESS_KEY",
    }
    values = {}
    for argument, variable in required.items():
        value = os.environ.get(variable)
        if not value:
            raise RuntimeError(f"missing required environment variable {variable}")
        values[argument] = value
    return boto3.client("s3", **values)


def _bucket() -> str:
    bucket = os.environ.get("PREVIEW_BUCKET")
    if not bucket:
        raise RuntimeError("missing required environment variable PREVIEW_BUCKET")
    return bucket


def parse_args(argv: list[str] | None = None) -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    commands = parser.add_subparsers(dest="command", required=True)
    upload = commands.add_parser("upload", help="validate and publish one preview")
    upload.add_argument("--root", type=Path, required=True)
    upload.add_argument("--pr", type=int, required=True)
    upload.add_argument("--head-sha", required=True)
    upload.add_argument("--base-sha", required=True)
    upload.add_argument("--title", required=True)
    cleanup = commands.add_parser("cleanup", help="remove a PR preview prefix")
    cleanup.add_argument("--pr", type=int, required=True)
    cleanup.add_argument("--keep-sha")
    return parser.parse_args(argv)


def main(argv: list[str] | None = None) -> int:
    args = parse_args(argv)
    client = make_client()
    bucket = _bucket()
    if args.command == "upload":
        url = upload_preview(
            client,
            bucket,
            args.root,
            {
                "number": args.pr,
                "headSha": args.head_sha,
                "baseSha": args.base_sha,
                "title": args.title,
            },
        )
        print(url)
    else:
        deleted = cleanup_preview(client, bucket, args.pr, args.keep_sha)
        print(json.dumps({"deleted": deleted, "pr": args.pr}, separators=(",", ":")))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
