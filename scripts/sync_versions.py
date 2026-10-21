#!/usr/bin/env python3
"""Keep every channel's version equal to the engine version.

The Rust workspace version (`[workspace.package] version` in the root
Cargo.toml) is the single source of truth. Cargo cannot propagate it natively:
`clients/python` and `clients/r/src/rust` are in the workspace `exclude` list so
they can own their lockfiles and toolchains, and an excluded package may not use
`version.workspace = true`. So we propagate it here instead, the same way
`clients/mcpb/build.mjs` already reads the workspace version at build time.

The contract is ONE version:

    every client, pin and floor == the engine version, exactly

because one `vX.Y.Z` tag releases every channel at once (crates.io, PyPI, npm,
the GitHub release, the container images; docs/release.md). Until 0.3.6 the
clients released from their own `py-v*`/`js-v*` tags and only had to share the
engine's MAJOR.MINOR, so a binding could ship a patch on its own. With a single
tag there is no such thing as a client-only release, and a client that names
another version is a tag the release workflows will refuse.

Usage:
    python3 scripts/sync_versions.py --check             # CI gate, no writes
    python3 scripts/sync_versions.py --check-tag v0.3.7  # release workflows
    python3 scripts/sync_versions.py --set 0.3.7         # cut a release
    python3 scripts/sync_versions.py --write             # realign to the engine
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
    # The Blender add-on ships rete-graph wheels inside its zip. A release
    # builds them in the same run (release.yml), so these two only matter for a
    # local build that downloads from PyPI; they still move with every release
    # so a local build never bundles a previous engine.
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
    # The relay image installs the engine at build time: from the wheels a
    # release builds in the same run, or, for the HF Space and a plain local
    # `docker build`, from PyPI. A stale floor lets that PyPI path silently
    # resolve an old engine.
    (
        "clients/relay/requirements.txt",
        re.compile(r"(?m)^rete-graph>=([0-9.]+)$"),
        "clients/relay wheel floor",
    ),
    # mybinder.org builds its image from this file at launch, from PyPI. It is
    # the one consumer that resolves the new version from the registry, so a
    # Binder launch from `main` fails for the half hour between the release PR
    # merging and the tag's PyPI upload; nothing in CI or the release reads it.
    (
        "binder/requirements.txt",
        re.compile(r"(?m)^rete-graph==([0-9.]+)$"),
        "binder pin",
    ),
    # The Java client: a parent POM, two modules that name it as <parent>, and
    # the wasm FFI crate whose CARGO_PKG_VERSION is what `Rete.version()`
    # returns (so the test asserting it moves with them). Not covered until
    # 0.3.4, which left the POMs at 0.3.0 through three engine releases.
    (
        "clients/java/pom.xml",
        re.compile(
            r"(?s)<artifactId>rete-parent</artifactId>\s*(?:<!--.*?-->\s*)?<version>([^<]+)</version>"
        ),
        "java parent pom",
    ),
    (
        "clients/java/rete-client/pom.xml",
        re.compile(r"<artifactId>rete-parent</artifactId>\s*<version>([^<]+)</version>"),
        "java rete-client parent ref",
    ),
    (
        "clients/java/rete-rdf4j/pom.xml",
        re.compile(r"<artifactId>rete-parent</artifactId>\s*<version>([^<]+)</version>"),
        "java rete-rdf4j parent ref",
    ),
    (
        "clients/java/ffi/Cargo.toml",
        re.compile(r'(?m)^version = "([^"]+)"'),
        "java wasm ffi crate",
    ),
    (
        "clients/java/rete-client/src/test/java/io/github/caviri/rete/ReteTest.java",
        re.compile(r'assertEquals\("([^"]+)", rete\.version\(\)\)'),
        "java engine-version test",
    ),
]

ENGINE_RE = re.compile(r'(?ms)^\[workspace\.package\].*?^version = "([^"]+)"')

# The workspace members that depend on rete-core pin it exactly, so a published
# crate can only ever resolve the sibling it was built against.
CORE_PIN_RE = re.compile(r'rete-core = \{ version = "=([^"]+)"')
_CORE_PIN_FILES = [
    "crates/bench/Cargo.toml",
    "crates/rete-graph/Cargo.toml",
    "crates/rete-cli/Cargo.toml",
    "crates/rete-wasm/Cargo.toml",
]

VERSION_FORMAT = re.compile(r"\d+\.\d+\.\d+")


def engine_version() -> str:
    text = (ROOT / "Cargo.toml").read_text(encoding="utf-8")
    match = ENGINE_RE.search(text)
    if not match:
        sys.exit("could not read [workspace.package] version from Cargo.toml")
    return match.group(1)


def _edit(path: pathlib.Path, pattern: re.Pattern, new: str) -> None:
    text = path.read_text(encoding="utf-8")
    matches = pattern.findall(text)
    if len(matches) != 1:
        sys.exit(f"{path}: expected exactly one version match, found {len(matches)}")
    start, end = pattern.search(text).span(1)
    path.write_text(text[:start] + new + text[end:], encoding="utf-8")


def _stamp(version: str) -> int:
    """Stamp one exact version everywhere — the release cut.

    Every target moves, the PyPI floors and the Binder pin included. Up to
    0.3.6 those waited for a second "raise the PyPI floors" PR, because the
    relay image and the Blender add-on installed rete-graph FROM PyPI while the
    release built them; a floor naming an unpublished version failed those
    builds. A release now builds both from the wheels of its own run, so the
    floors can name the version being released in the same commit.
    """
    if not VERSION_FORMAT.fullmatch(version):
        sys.exit(f"--set expects MAJOR.MINOR.PATCH, got {version!r}")

    print(f"stamping {version} on the workspace and every client")
    _edit(ROOT / "Cargo.toml", ENGINE_RE, version)
    print(f"  wrote {'engine (workspace)':32} {version}")

    # The publishable crates pin rete-core EXACTLY (`=X.Y.Z`) so a crates.io
    # release can never resolve a sibling from a different engine build. That
    # pin is not a client version, so it is not in TARGETS — but leaving it
    # behind makes the workspace refuse to resolve at all ("candidate versions
    # found which didn't match"), which is how this step was discovered.
    for rel in _CORE_PIN_FILES:
        _edit(ROOT / rel, CORE_PIN_RE, version)
        print(f"  wrote {'rete-core pin: ' + rel.split('/')[1]:32} ={version}")

    for rel, pattern, label in TARGETS:
        path = ROOT / rel
        if not path.exists():
            sys.exit(f"{rel}: missing — cannot cut a release with a target gone")
        _edit(path, pattern, version)
        print(f"  wrote {label:32} {version}")

    print("\nnow run --check to confirm every channel names the same version")
    return 0


def _check(engine: str, writing: bool) -> list[str]:
    """Every channel against the engine version; returns the problems found."""
    print(f"engine (workspace): {engine}  [every channel must name it exactly]")
    problems: list[str] = []

    # These are `=` pins: they must equal the engine, or `cargo` refuses to
    # resolve the workspace at all.
    for rel in _CORE_PIN_FILES:
        found = CORE_PIN_RE.search((ROOT / rel).read_text(encoding="utf-8"))
        if not found:
            problems.append(f"{rel}: no exact rete-core pin matched")
        elif found.group(1) != engine:
            if writing:
                _edit(ROOT / rel, CORE_PIN_RE, engine)
                print(f"  wrote rete-core pin in {rel} ={found.group(1)} -> ={engine}")
            else:
                problems.append(
                    f"{rel}: pins rete-core ={found.group(1)}, engine is {engine} "
                    f"— the workspace will not resolve"
                )
        else:
            print(f"  ok    {'rete-core pin: ' + rel.split('/')[1]:32} ={found.group(1)}")

    for rel, pattern, label in TARGETS:
        path = ROOT / rel
        if not path.exists():
            problems.append(f"{rel}: missing")
            continue
        found = pattern.search(path.read_text(encoding="utf-8"))
        if not found:
            problems.append(f"{rel}: no version field matched")
            continue
        current = found.group(1).strip()
        if current == engine:
            print(f"  ok    {label:32} {current}")
        elif writing:
            _edit(path, pattern, engine)
            print(f"  wrote {label:32} {current} -> {engine}")
        else:
            problems.append(f"{rel}: {label} is {current}, engine is {engine}")
    return problems


def _report(problems: list[str]) -> int:
    if problems:
        print("\nversion drift:", file=sys.stderr)
        for problem in problems:
            print(f"  - {problem}", file=sys.stderr)
        print(
            "\nrun `python3 scripts/sync_versions.py --set X.Y.Z` (or `--write` "
            "to realign on the current engine version), then review the diff.",
            file=sys.stderr,
        )
        return 1
    print("every channel names the engine version")
    return 0


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    mode = parser.add_mutually_exclusive_group()
    mode.add_argument("--check", action="store_true", help="report drift, write nothing")
    mode.add_argument(
        "--check-tag",
        metavar="TAG",
        help="what every release workflow runs on a tag: TAG must be "
        "v<engine version> and every channel must name that version",
    )
    mode.add_argument(
        "--write", action="store_true", help="realign every channel on the engine version"
    )
    mode.add_argument(
        "--set",
        metavar="VERSION",
        help="cut a release: stamp VERSION on the workspace, the rete-core pins, "
        "every client, the PyPI floors and the Binder pin",
    )
    args = parser.parse_args()

    if args.set:
        return _stamp(args.set)

    engine = engine_version()
    if args.check_tag is not None:
        if args.check_tag != f"v{engine}":
            print(
                f"tag {args.check_tag} does not name the workspace version "
                f"{engine} (expected v{engine})",
                file=sys.stderr,
            )
            return 1
        print(f"tag {args.check_tag} names the workspace version")
    return _report(_check(engine, writing=args.write))


if __name__ == "__main__":
    raise SystemExit(main())
