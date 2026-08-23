#!/usr/bin/env python3
"""Probe playground `.rete` objects and enforce the stable R2 browser contract."""

from __future__ import annotations

import argparse
from collections import defaultdict
from concurrent.futures import ThreadPoolExecutor, as_completed
import json
from pathlib import Path
import re
import struct
import subprocess
import sys
from urllib.error import HTTPError, URLError
from urllib.request import Request, urlopen


ROOT = Path(__file__).resolve().parents[1]
CATALOG = ROOT / "web" / "playground-src" / "catalog.js"
LOCK = ROOT / "web" / "datasets.lock.json"
RANGE_END = 1023
READABLE_FORMAT_VERSIONS = {5, 6}
REQUIRED_EXPOSED = {"content-range"}

# Typed section directory (SPEC.md 4.1): a 64-byte core, then `section_count`
# entries of 24 bytes — kind u16, flags u16, reserved u32, offset u64, length u64.
SECTION_COUNT_OFFSET = 44
SECTION_DIR_OFFSET = 64
SECTION_ENTRY_LEN = 24
SECTION_TEXT_INDEX = 6
SECTION_NAMES = {
    1: "Metadata",
    2: "Dictionary",
    3: "Index",
    4: "PyramidMeta",
    5: "NamedGraphs",
    6: "TextIndex",
    7: "BuildInfo",
}


def lower_headers(headers: dict[str, str]) -> dict[str, str]:
    return {name.lower(): value for name, value in headers.items()}


def parse_sections(body: bytes) -> list[dict]:
    """Decode the header's section directory from the first 1024 bytes."""
    if len(body) < RANGE_END + 1 or body[:4] != b"RETE":
        return []
    count = struct.unpack_from("<H", body, SECTION_COUNT_OFFSET)[0]
    if SECTION_DIR_OFFSET + count * SECTION_ENTRY_LEN > RANGE_END + 1:
        return []
    sections = []
    for index in range(count):
        position = SECTION_DIR_OFFSET + index * SECTION_ENTRY_LEN
        kind, flags = struct.unpack_from("<HH", body, position)
        offset, length = struct.unpack_from("<QQ", body, position + 8)
        sections.append(
            {
                "kind": kind,
                "name": SECTION_NAMES.get(kind, f"Unknown({kind})"),
                "flags": flags,
                "offset": offset,
                "length": length,
            }
        )
    return sections


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
    if format_version not in READABLE_FORMAT_VERSIONS:
        errors.append(f"expected readable format byte 5 or 6, got {format_version}")
    content_hash = body[8:24].hex() if len(body) >= 24 else None
    sections = parse_sections(body)
    text_index_bytes = sum(
        section["length"] for section in sections if section["kind"] == SECTION_TEXT_INDEX
    )

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
        "sections": [section["name"] for section in sections],
        "textIndexBytes": text_index_bytes,
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


def catalog_targets(catalog: dict) -> list[dict]:
    base = catalog["remoteBase"].rstrip("/")
    targets: list[dict] = []
    for dataset in catalog.get("datasets", []):
        key = dataset["key"]
        # `textIndex: true` is the catalog's DECLARATION that the published
        # object carries a TEXT_INDEX section. Carried per target so the probe
        # can hold the declaration to the bytes actually served.
        declares = dataset.get("textIndex") is True
        shards = dataset.get("shards") or []
        if shards:
            targets.extend(
                {"key": f"{key}#{index}", "dataset": key, "textIndex": declares, "url": url}
                for index, url in enumerate(shards, 1)
            )
            continue
        targets.append(
            {
                "key": key,
                "dataset": key,
                "textIndex": declares,
                "url": dataset.get("url") or f"{base}/{key}/{key}.rete",
            }
        )
    return targets


def text_index_failures(targets: list[dict], results: list[dict]) -> list[str]:
    """Hold every `textIndex: true` declaration to the section directory served.

    A full-text index is opt-in at build time, so the catalog's claim and the
    file's sections drift silently: `FILTER(CONTAINS(…))` still answers without
    an index, by full scan, and nothing else notices. Compared per DATASET (not
    per shard) because the declaration is a dataset-level fact.
    """
    declared = {target["dataset"]: target["textIndex"] for target in targets}
    expected: dict[str, int] = defaultdict(int)
    for target in targets:
        expected[target["dataset"]] += 1
    observed: dict[str, int] = defaultdict(int)
    probed: dict[str, int] = defaultdict(int)
    by_key = {target["key"]: target["dataset"] for target in targets}
    for result in results:
        dataset = by_key.get(result["key"], result["key"])
        if result["errors"]:
            continue
        probed[dataset] += 1
        observed[dataset] += result.get("textIndexBytes") or 0
    failures = []
    for dataset, declares in sorted(declared.items()):
        # Only judge a dataset every one of whose shards answered: a partial
        # sweep cannot prove the ABSENCE of an index the missing shard may hold.
        if probed.get(dataset, 0) != expected[dataset]:
            continue
        has_index = observed[dataset] > 0
        if declares and not has_index:
            failures.append(
                f"{dataset}: catalog declares textIndex but the published file has "
                "NO TEXT_INDEX section (drop the claim, or rebuild with --text-index)"
            )
        elif has_index and not declares:
            failures.append(
                f"{dataset}: the published file carries a TEXT_INDEX ({observed[dataset]:,} bytes) "
                "the catalog never declares — add `textIndex: true` and say so in the description"
            )
    return failures


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
            "sections": [],
            "textIndexBytes": 0,
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
                    f"{result['size']} bytes {result['contentHash']} "
                    f"[{'+'.join(result['sections'])}]"
                )

    # The catalog's `textIndex: true` declaration vs the section directory the
    # bucket actually serves. Free: it reads the 1024 bytes already fetched.
    claim_failures = text_index_failures(targets, results)
    for message in claim_failures:
        print(f"FAIL text-index claim — {message}")

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
    print(
        f"text-index claims: {len(claim_failures)} mismatch(es) between "
        "`textIndex:` in the catalog and the TEXT_INDEX section served"
    )
    return 1 if (failures or claim_failures) else 0


if __name__ == "__main__":
    raise SystemExit(main())
