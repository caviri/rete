#!/usr/bin/env python3
"""Restore ignored playground datasets from the tracked generated page.

The embedded datasets are intentionally not tracked as standalone binary files.
The generated playground is tracked, however, and contains the exact bytes that
must be used for a reproducible rebuild. Clean CI runners stage those bytes into
``web/`` before regenerating the page.
"""

from __future__ import annotations

import argparse
import base64
import binascii
import json
import re
import sys
from pathlib import Path

from build_playground import DATASETS


ROOT = Path(__file__).resolve().parent.parent
DEFAULT_HTML = ROOT / "docs" / "playground.html"
DEFAULT_WEB_DIR = ROOT / "web"
DATASET_MAP_PATTERN = re.compile(r"\bconst\s+RETE_DATASETS_B64\s*=\s*")


class StageError(RuntimeError):
    """An expected deterministic staging precondition was not met."""


def read_embedded_map(html_path: Path) -> dict[str, str]:
    try:
        html = html_path.read_text(encoding="utf-8")
    except OSError as error:
        raise StageError(f"cannot read {html_path}: {error}") from error

    match = DATASET_MAP_PATTERN.search(html)
    if match is None:
        raise StageError(f"{html_path} has no RETE_DATASETS_B64 assignment")

    try:
        value, _ = json.JSONDecoder().raw_decode(html, match.end())
    except json.JSONDecodeError as error:
        raise StageError(f"invalid RETE_DATASETS_B64 JSON in {html_path}: {error}") from error
    if not isinstance(value, dict) or not all(
        isinstance(key, str) and isinstance(encoded, str)
        for key, encoded in value.items()
    ):
        raise StageError("RETE_DATASETS_B64 must be a string-to-string object")
    return value


def decode_v5(name: str, encoded: str) -> bytes:
    try:
        payload = base64.b64decode(encoded, validate=True)
    except (binascii.Error, ValueError) as error:
        raise StageError(f"dataset {name!r} is not valid base64: {error}") from error

    if (
        len(payload) < 9
        or payload[:4] != b"RETE"
        or payload[4] != 5
        or payload[-4:] != b"RETE"
    ):
        raise StageError(f"dataset {name!r} is not a complete rete format v5 file")
    return payload


def stage(
    html_path: Path, web_dir: Path, allow_update: bool = False
) -> tuple[list[Path], list[Path], list[Path]]:
    embedded = read_embedded_map(html_path)
    decoded: list[tuple[str, Path, bytes]] = []
    for name, filename in DATASETS:
        try:
            encoded = embedded[name]
        except KeyError as error:
            raise StageError(f"tracked playground is missing dataset {name!r}") from error
        decoded.append((name, web_dir / filename, decode_v5(name, encoded)))

    # Validate the complete set before writing anything. Existing ignored files
    # are never silently replaced: a mismatch means the local build inputs have
    # drifted from the tracked release artifact and should be investigated.
    existing: list[Path] = []
    updated: list[Path] = []
    missing: list[tuple[Path, bytes]] = []
    for name, path, payload in decoded:
        if path.exists():
            if path.read_bytes() != payload:
                # Drift is the normal reading of a mismatch, and refusing is
                # right: it stops a stray local build from silently changing
                # what ships. But REPLACING an embedded dataset on purpose —
                # rebuilding one with a Dataset Card, say — lands here too, and
                # without a way to say "this is deliberate" the only way through
                # was to bypass this script entirely. --allow-update is that
                # way: keep the local file and let the page be rebuilt around it.
                if not allow_update:
                    raise StageError(
                        f"existing dataset {path} does not match tracked {name!r} bytes"
                        " (pass --allow-update if you meant to replace it)"
                    )
                updated.append(path)
                continue
            existing.append(path)
        else:
            missing.append((path, payload))

    web_dir.mkdir(parents=True, exist_ok=True)
    staged: list[Path] = []
    for path, payload in missing:
        path.write_bytes(payload)
        staged.append(path)
    return staged, existing, updated


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--html", type=Path, default=DEFAULT_HTML)
    parser.add_argument("--web-dir", type=Path, default=DEFAULT_WEB_DIR)
    parser.add_argument(
        "--allow-update",
        action="store_true",
        help="keep local web/*.rete that differ from the tracked page, instead of"
        " refusing. For DELIBERATE replacement of an embedded dataset; the page"
        " is then rebuilt around the local bytes.",
    )
    return parser.parse_args()


def main() -> int:
    args = parse_args()
    try:
        staged, existing, updated = stage(args.html, args.web_dir, args.allow_update)
    except (OSError, StageError) as error:
        print(f"stage_playground_datasets: error: {error}", file=sys.stderr)
        return 1

    print(
        "stage_playground_datasets: "
        f"staged {len(staged)}, verified {len(existing)} embedded datasets"
        + (f", kept {len(updated)} locally-updated" if updated else "")
    )
    for path in staged:
        print(f"  staged: {path}")
    for path in updated:
        print(f"  kept local (--allow-update): {path}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
