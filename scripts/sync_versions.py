#!/usr/bin/env python3
"""Keep every language client's version tied to the engine version.

The Rust workspace version (`[workspace.package] version` in the root
Cargo.toml) is the single source of truth. Cargo cannot propagate it natively:
`clients/python` and `clients/r/src/rust` are in the workspace `exclude` list so
they can own their lockfiles and toolchains, and an excluded package may not use
`version.workspace = true`. So we propagate it here instead, the same way
`clients/mcpb/build.mjs` already reads the workspace version at build time.

The contract is MAJOR.MINOR lockstep:

    client MAJOR.MINOR == engine MAJOR.MINOR

so "same minor" always means "same engine generation", while each client keeps
its own PATCH component for binding-only fixes (a Python typing fix ships as
0.3.1 without forcing an engine release). `--check` fails when a client's
MAJOR.MINOR has drifted; it deliberately does not care about PATCH.

Usage:
    python3 scripts/sync_versions.py --check    # CI gate, no writes
    python3 scripts/sync_versions.py --write    # realign drifted clients
"""

from __future__ import annotations

import argparse
import pathlib
import re
import sys

ROOT = pathlib.Path(__file__).resolve().parent.parent

# Each target: the file, a regex whose single capture group is the version, and
# a label. The regexes are anchored tightly enough to hit exactly one line —
# `_edit` asserts that, so a manifest reshuffle fails loudly instead of
# rewriting the wrong field.
TARGETS = [
    (
        "clients/python/Cargo.toml",
        re.compile(r'(?m)^version = "([^"]+)"'),
        "python binding crate",
    ),
    (
        "clients/python/pyproject.toml",
        re.compile(r'(?m)^version = "([^"]+)"'),
        "python wheel (PyPI rete-graph)",
    ),
    (
        "clients/js/package.json",
        re.compile(r'(?m)^  "version": "([^"]+)"'),
        "javascript (npm rete-graph)",
    ),
    (
        "clients/r/DESCRIPTION",
        re.compile(r"(?m)^Version: (.+)$"),
        "r package",
    ),
    # The Blender add-on ships rete-graph wheels inside its zip; its default pin
    # and the test-image floor must track the engine minor or the add-on quietly
    # bundles a previous engine generation.
    (
        "clients/blender/build.sh",
        re.compile(r'(?m)^VERSION="\$\{RETE_GRAPH_VERSION:-([^}"]+)\}"'),
        "blender bundled wheel pin",
    ),
    (
        "clients/blender/Dockerfile",
        re.compile(r'"rete-graph>=([0-9.]+)"'),
        "blender test-image floor",
    ),
    # The HF Space installs the engine from PyPI at image build; a stale floor
    # lets a Space rebuild silently resolve an old engine.
    (
        "hf-space/requirements.txt",
        re.compile(r"(?m)^rete-graph>=([0-9.]+)$"),
        "hf-space wheel floor",
    ),
]

# The Pyodide/JupyterLite fallback wheel is served from our own bucket, so its
# URL embeds the version. `piplite.install("rete-graph")` resolves from PyPI and
# needs no pin, but this documented fallback rots silently otherwise.
WHEEL_DOC = "docs/python.md"
WHEEL_RE = re.compile(r"(rete_graph-)(\d+\.\d+\.\d+)(-cp\d+-abi3-[a-z0-9_]+\.whl)")

ENGINE_RE = re.compile(r'(?ms)^\[workspace\.package\].*?^version = "([^"]+)"')


def engine_version() -> str:
    text = (ROOT / "Cargo.toml").read_text(encoding="utf-8")
    match = ENGINE_RE.search(text)
    if not match:
        sys.exit("could not read [workspace.package] version from Cargo.toml")
    return match.group(1)


def minor_of(version: str) -> str:
    """`0.3.1` -> `0.3`. Prerelease/build metadata is ignored for the compare."""
    parts = version.split("+")[0].split("-")[0].split(".")
    if len(parts) < 2:
        sys.exit(f"unparseable version: {version!r}")
    return ".".join(parts[:2])


def _edit(path: pathlib.Path, pattern: re.Pattern, new: str) -> None:
    text = path.read_text(encoding="utf-8")
    matches = pattern.findall(text)
    if len(matches) != 1:
        sys.exit(f"{path}: expected exactly one version match, found {len(matches)}")
    start, end = pattern.search(text).span(1)
    path.write_text(text[:start] + new + text[end:], encoding="utf-8")


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    mode = parser.add_mutually_exclusive_group()
    mode.add_argument("--check", action="store_true", help="report drift, write nothing")
    mode.add_argument("--write", action="store_true", help="realign drifted clients")
    args = parser.parse_args()
    writing = args.write

    engine = engine_version()
    target_minor = minor_of(engine)
    print(f"engine (workspace): {engine}  [{target_minor}.x expected in clients]")

    problems: list[str] = []
    python_versions: dict[str, str] = {}

    for rel, pattern, label in TARGETS:
        path = ROOT / rel
        if not path.exists():
            problems.append(f"{rel}: missing")
            continue
        text = path.read_text(encoding="utf-8")
        found = pattern.search(text)
        if not found:
            problems.append(f"{rel}: no version field matched")
            continue
        current = found.group(1).strip()
        if rel.startswith("clients/python/"):
            python_versions[rel] = current

        if minor_of(current) == target_minor:
            print(f"  ok    {label:32} {current}")
            continue

        if writing:
            new = f"{target_minor}.0"
            _edit(path, pattern, new)
            print(f"  wrote {label:32} {current} -> {new}")
        else:
            problems.append(
                f"{rel}: {label} is {current}, expected {target_minor}.x "
                f"(engine {engine})"
            )

    # The two Python files describe one package; they must agree exactly, not
    # merely share a minor. This is the pairing the manifest comment asks for.
    if len(set(python_versions.values())) > 1:
        pairs = ", ".join(f"{k}={v}" for k, v in python_versions.items())
        problems.append(f"python version split across files: {pairs}")

    # Pyodide fallback wheel URL in the docs.
    wheel_path = ROOT / WHEEL_DOC
    if wheel_path.exists():
        text = wheel_path.read_text(encoding="utf-8")
        pinned = {m.group(2) for m in WHEEL_RE.finditer(text)}
        py_version = python_versions.get("clients/python/pyproject.toml", engine)
        expected = f"{target_minor}.0" if writing else py_version
        for pin in sorted(pinned):
            if minor_of(pin) == target_minor:
                print(f"  ok    {'pyodide wheel url':32} {pin}")
            elif writing:
                text = WHEEL_RE.sub(lambda m: m.group(1) + expected + m.group(3), text)
                wheel_path.write_text(text, encoding="utf-8")
                print(f"  wrote {'pyodide wheel url':32} {pin} -> {expected}")
            else:
                problems.append(
                    f"{WHEEL_DOC}: Pyodide fallback wheel URL pins {pin}, "
                    f"expected {target_minor}.x — the wheel must also be "
                    f"uploaded to the bucket for that URL to resolve"
                )

    if problems:
        print("\nversion drift:", file=sys.stderr)
        for problem in problems:
            print(f"  - {problem}", file=sys.stderr)
        print(
            "\nrun `python3 scripts/sync_versions.py --write` to realign, then "
            "review the diff.",
            file=sys.stderr,
        )
        return 1

    print("all clients match the engine minor line")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
