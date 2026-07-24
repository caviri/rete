#!/usr/bin/env python3
"""Probe playground `.rete` objects and enforce the stable R2 browser contract."""

from __future__ import annotations

import argparse
from concurrent.futures import ThreadPoolExecutor, as_completed
import json
from pathlib import Path
import re
import subprocess
import sys
from urllib.error import HTTPError, URLError
from urllib.request import Request, urlopen


ROOT = Path(__file__).resolve().parents[1]
CATALOG = ROOT / "web" / "playground-src" / "catalog.js"
LOCK = ROOT / "web" / "datasets.lock.json"
RANGE_END = 1023
REQUIRED_EXPOSED = {"content-range"}


def lower_headers(headers: dict[str, str]) -> dict[str, str]:
    return {name.lower(): value for name, value in headers.items()}


def parse_content_range(value: str) -> tuple[int, int, int] | None:
    match = re.fullmatch(r"bytes\s+(\d+)-(\d+)/(\d+)", value.strip(), re.I)
    if not match:
        return None
    return tuple(int(group) for group in match.groups())


def validate_probe(
    *,
    url: str,
    final_url: str,
    status: int,
    headers: dict[str, str],
    body: bytes,
    expected: dict | None = None,
) -> dict:
    """Validate one 0..1023 range response and return lock-compatible facts."""
    errors: list[str] = []
    normalized = lower_headers(headers)
    if final_url != url:
        errors.append(f"redirected to {final_url}")
    if status != 206:
        errors.append(f"expected HTTP 206, got {status}")
    if "bytes" not in normalized.get("accept-ranges", "").lower():
        errors.append("Accept-Ranges must include bytes")
    if not normalized.get("access-control-allow-origin"):
        errors.append("Access-Control-Allow-Origin is missing")
    exposed = {
        part.strip().lower()
        for part in normalized.get("access-control-expose-headers", "").split(",")
        if part.strip()
    }
    if not REQUIRED_EXPOSED.issubset(exposed):
        errors.append("CORS must expose Content-Range")

    content_range = parse_content_range(normalized.get("content-range", ""))
    size = content_range[2] if content_range else None
    if content_range is None:
        errors.append("Content-Range must be `bytes 0-1023/N`")
    else:
        start, end, total = content_range
        if (start, end) != (0, RANGE_END):
            errors.append(f"unexpected Content-Range {start}-{end}")
        if total < RANGE_END + 1:
            errors.append(f"Content-Range total {total} is smaller than 1024")

    if len(body) != RANGE_END + 1:
        errors.append(f"expected a 1024-byte range body, got {len(body)}")
    if body[:4] != b"RETE":
        errors.append("header magic is not RETE")
    format_version = body[4] if len(body) > 4 else None
    if format_version != 5:
        errors.append(f"expected stable format byte 5, got {format_version}")
    content_hash = body[8:24].hex() if len(body) >= 24 else None

    if expected:
        if expected.get("formatVersion") != format_version:
            errors.append("format version does not match datasets.lock.json")
        if expected.get("contentHash") != content_hash:
            errors.append("content hash does not match datasets.lock.json")
        if expected.get("size") != size:
            errors.append(
                f"size does not match datasets.lock.json ({size} != {expected.get('size')})"
            )

    return {
        "url": url,
        "formatVersion": format_version,
        "contentHash": content_hash,
        "size": size,
        "errors": errors,
    }


def load_catalog(path: Path = CATALOG) -> dict:
    """Evaluate the static catalog in a sandboxed Node VM and return plain JSON."""
    program = r"""
const fs = require("fs");
const vm = require("vm");
const source = fs.readFileSync(process.argv[1], "utf8");
const sandbox = {window: {}};
vm.runInNewContext(source, sandbox, {filename: process.argv[1]});
process.stdout.write(JSON.stringify(sandbox.window.RETE_PLAYGROUND_CATALOG));
"""
    completed = subprocess.run(
        ["node", "-e", program, str(path)],
        check=True,
        capture_output=True,
        text=True,
    )
    return json.loads(completed.stdout)


def catalog_targets(catalog: dict) -> list[dict[str, str]]:
    base = catalog["remoteBase"].rstrip("/")
    targets: list[dict[str, str]] = []
    for dataset in catalog.get("datasets", []):
        key = dataset["key"]
        shards = dataset.get("shards") or []
        if shards:
            targets.extend(
                {"key": f"{key}#{index}", "url": url}
                for index, url in enumerate(shards, 1)
            )
            continue
        targets.append(
            {
                "key": key,
                "url": dataset.get("url") or f"{base}/{key}/{key}.rete",
            }
        )
    return targets


def probe(target: dict[str, str], expected: dict | None, timeout: int) -> dict:
    request = Request(
        target["url"],
        headers={
            "Range": f"bytes=0-{RANGE_END}",
            "Origin": "https://caviri.github.io",
            "User-Agent": "rete-release-catalog-check/1",
        },
    )
    try:
        with urlopen(request, timeout=timeout) as response:
            body = response.read(RANGE_END + 1)
            result = validate_probe(
                url=target["url"],
                final_url=response.geturl(),
                status=response.status,
                headers=dict(response.headers.items()),
                body=body,
                expected=expected,
            )
    except HTTPError as error:
        body = error.read(RANGE_END + 1)
        result = validate_probe(
            url=target["url"],
            final_url=error.geturl(),
            status=error.code,
            headers=dict(error.headers.items()),
            body=body,
            expected=expected,
        )
    except (OSError, URLError) as error:
        result = {
            "url": target["url"],
            "formatVersion": None,
            "contentHash": None,
            "size": None,
            "errors": [f"request failed: {error}"],
        }
    result["key"] = target["key"]
    return result


def load_lock(path: Path) -> dict[str, dict]:
    if not path.is_file():
        return {}
    document = json.loads(path.read_text(encoding="utf-8"))
    return {entry["key"]: entry for entry in document.get("datasets", [])}


def write_lock(path: Path, results: list[dict]) -> None:
    # MERGE with the existing lock: a --key run probes a subset, and the
    # unprobed datasets' entries must survive (a plain rewrite once wiped
    # 80+ entries from the lock).
    merged = load_lock(path)
    for result in results:
        merged[result["key"]] = {
            "key": result["key"],
            "url": result["url"],
            "formatVersion": result["formatVersion"],
            "contentHash": result["contentHash"],
            "size": result["size"],
            "cardUrl": result["url"],
        }
    document = {
        "schemaVersion": 1,
        "generatedFrom": "release-1.0.0-rc1",
        "datasets": [merged[key] for key in sorted(merged)],
    }
    path.write_text(json.dumps(document, indent=2) + "\n", encoding="utf-8")


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    selection = parser.add_mutually_exclusive_group(required=True)
    selection.add_argument("--all", action="store_true", help="probe every catalog graph")
    selection.add_argument("--key", action="append", help="probe one key (repeatable)")
    parser.add_argument("--catalog", type=Path, default=CATALOG)
    parser.add_argument("--lock", type=Path, default=LOCK)
    parser.add_argument("--write-lock", action="store_true")
    parser.add_argument("--report", type=Path)
    parser.add_argument("--timeout", type=int, default=30)
    parser.add_argument("--jobs", type=int, default=8)
    args = parser.parse_args()

    targets = catalog_targets(load_catalog(args.catalog))
    if args.key:
        requested = set(args.key)
        targets = [target for target in targets if target["key"] in requested]
        missing = requested - {target["key"] for target in targets}
        if missing:
            parser.error(f"unknown catalog key(s): {', '.join(sorted(missing))}")

    locked = load_lock(args.lock)
    results: list[dict] = []
    with ThreadPoolExecutor(max_workers=max(1, args.jobs)) as executor:
        futures = {
            executor.submit(probe, target, locked.get(target["key"]), args.timeout): target
            for target in targets
        }
        for future in as_completed(futures):
            result = future.result()
            results.append(result)
            if result["errors"]:
                print(f"FAIL {result['key']}: {'; '.join(result['errors'])}")
            else:
                print(
                    f"OK   {result['key']}: v{result['formatVersion']} "
                    f"{result['size']} bytes {result['contentHash']}"
                )

    results.sort(key=lambda item: item["key"])
    if args.report:
        args.report.write_text(json.dumps(results, indent=2) + "\n", encoding="utf-8")
    failures = [result for result in results if result["errors"]]
    if args.write_lock:
        if failures:
            print("refusing to write datasets.lock.json while catalog probes fail", file=sys.stderr)
        else:
            write_lock(args.lock, results)
            print(f"wrote {args.lock}")
    print(f"catalog: {len(results) - len(failures)}/{len(results)} stable object(s)")
    return 1 if failures else 0


if __name__ == "__main__":
    raise SystemExit(main())
