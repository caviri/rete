#!/usr/bin/env python3
"""Create, find, and publish the ONE GitHub release behind a tag.

Two workflows build assets for a `v*` tag: release.yml (engine archives, wasm,
extensions) and desktop-release.yml (the File Explorer installers). They used
to race: v0.3.4 got a draft "Rete File Explorer v0.3.4" from the desktop
workflow AND a published, empty "Rete 0.3.4" from release.yml, because GitHub's
get-release-by-tag endpoint does not see drafts, so each side thought it was
first. This script is the single place that decides what exists:

  * release.yml owns `v*` releases. Its `release` job runs `ensure` right after
    the version check, which creates a DRAFT (notes from CHANGELOG.md) or
    reuses whatever already exists for the tag. Its `assemble` job uploads the
    assets and runs `publish`.
  * desktop-release.yml never creates a `v*` release. It runs `wait`, which
    polls until release.yml's draft exists, then uploads into it by id.
    For `explorer-v*` tags (push or workflow_dispatch) it is the owner and
    runs `ensure` itself.

Every lookup lists releases (drafts included, which needs a token with
`contents: write`) and matches on `tag_name`, so a re-run of either workflow
finds the existing release instead of creating a second one. More than one
release on a tag is reported as an error rather than silently picking one.

Talks to GitHub only through `gh api`, so tests swap in a fake `gh` on PATH.
The repository comes from GH_REPO or GITHUB_REPOSITORY.

Usage:
    github_release.py classify VERSION
    github_release.py notes VERSION [--changelog CHANGELOG.md] [--output FILE]
    github_release.py find TAG
    github_release.py ensure TAG --name NAME --notes-file FILE
                             [--prerelease auto|true|false] [--no-draft]
    github_release.py wait TAG [--timeout SECONDS] [--interval SECONDS]
    github_release.py publish TAG --version VERSION
"""

from __future__ import annotations

import argparse
import json
import os
import pathlib
import re
import subprocess
import sys
import time

ROOT = pathlib.Path(__file__).resolve().parent.parent

# MAJOR.MINOR.PATCH, an optional semver prerelease (-rc.1, -alpha, -beta.2),
# optional build metadata. Only the prerelease part makes a GitHub prerelease:
# being 0.x does not. Marking every 0.x a prerelease is what pinned the
# "Latest" badge to v0.3.0 while 0.3.1-0.3.4 shipped.
VERSION_RE = re.compile(
    r"^(?P<core>\d+\.\d+\.\d+)(?:-(?P<pre>[0-9A-Za-z.-]+))?(?:\+[0-9A-Za-z.-]+)?$"
)
ENGINE_TAG_RE = re.compile(r"^v(\d+\.\d+\.\d+(?:-[0-9A-Za-z.-]+)?)$")


class ReleaseError(Exception):
    pass


# --- pure logic --------------------------------------------------------------


def parse_version(version: str) -> re.Match:
    match = VERSION_RE.match(version)
    if not match:
        raise ReleaseError(f"not a semantic version: {version!r}")
    return match


def is_prerelease(version: str) -> bool:
    return parse_version(version).group("pre") is not None


def _key(version: str) -> tuple:
    match = parse_version(version)
    core = tuple(int(part) for part in match.group("core").split("."))
    # A final release sorts after any prerelease of the same core version.
    return core + ((1,) if match.group("pre") is None else (0,))


def should_be_latest(version: str, releases: list[dict], tag: str) -> bool:
    """True when `version` is a final release no older than every other
    published final `v*` release. A backport such as v0.2.9 cut after v0.3.4
    must not steal the badge; `explorer-v*`, `py-v*` and `js-v*` never count."""
    if is_prerelease(version):
        return False
    for release in releases:
        if release.get("draft") or release.get("prerelease"):
            continue
        if release.get("tag_name") == tag:
            continue
        match = ENGINE_TAG_RE.match(release.get("tag_name", ""))
        if match and _key(match.group(1)) > _key(version):
            return False
    return True


def changelog_section(version: str, changelog: str) -> str:
    """The body of `## [VERSION] - date` up to the next `## ` heading."""
    heading = re.compile(rf"^## \[{re.escape(version)}\][^\n]*$", re.M)
    match = heading.search(changelog)
    if not match:
        raise ReleaseError(
            f"CHANGELOG.md has no '## [{version}]' section. Add it (move the "
            f"Unreleased entries under it) before tagging v{version}."
        )
    rest = changelog[match.end():]
    following = re.search(r"^## ", rest, re.M)
    body = (rest[: following.start()] if following else rest).strip()
    if not body:
        raise ReleaseError(f"CHANGELOG.md section [{version}] is empty")
    return body


def release_notes(version: str, changelog: str) -> str:
    body = changelog_section(version, changelog)
    return (
        f"{body}\n\n---\n\n"
        "Install, artifact list, download verification and known limitations: "
        "[release guide](https://github.com/caviri/rete/blob/main/docs/release.md). "
        f"Full history: [CHANGELOG.md](https://github.com/caviri/rete/blob/v{version}/CHANGELOG.md).\n"
    )


# --- GitHub access -----------------------------------------------------------


def _repo() -> str:
    repo = os.environ.get("GH_REPO") or os.environ.get("GITHUB_REPOSITORY")
    if not repo:
        raise ReleaseError("set GH_REPO or GITHUB_REPOSITORY (owner/name)")
    return repo


def _gh(args: list[str], payload: dict | None = None) -> str:
    proc = subprocess.run(
        ["gh", "api", *args],
        input=json.dumps(payload) if payload is not None else None,
        capture_output=True,
        text=True,
    )
    if proc.returncode != 0:
        raise ReleaseError(f"gh api {' '.join(args)} failed: {proc.stderr.strip()}")
    return proc.stdout


def _json_stream(text: str) -> list:
    """`gh api --paginate` prints one JSON array per page, back to back."""
    decoder = json.JSONDecoder()
    items, pos = [], 0
    while True:
        while pos < len(text) and text[pos].isspace():
            pos += 1
        if pos >= len(text):
            return items
        value, pos = decoder.raw_decode(text, pos)
        items.extend(value if isinstance(value, list) else [value])


def list_releases() -> list[dict]:
    return _json_stream(_gh(["--paginate", f"repos/{_repo()}/releases?per_page=100"]))


def find_release(tag: str, releases: list[dict] | None = None) -> dict | None:
    if releases is None:
        releases = list_releases()
    matches = [r for r in releases if r.get("tag_name") == tag]
    if len(matches) > 1:
        ids = ", ".join(f"{r['id']} ({'draft' if r.get('draft') else 'published'})" for r in matches)
        raise ReleaseError(
            f"{len(matches)} releases point at {tag}: {ids}. Delete the extras "
            f"by hand; refusing to guess which one is real."
        )
    return matches[0] if matches else None


def ensure_release(tag: str, name: str, notes: str, prerelease: bool, draft: bool) -> tuple[dict, bool]:
    existing = find_release(tag)
    if existing:
        return existing, False
    created = json.loads(
        _gh(
            ["--method", "POST", f"repos/{_repo()}/releases", "--input", "-"],
            {
                "tag_name": tag,
                "name": name,
                "body": notes,
                "draft": draft,
                "prerelease": prerelease,
            },
        )
    )
    return created, True


def wait_release(tag: str, timeout: float, interval: float) -> dict:
    deadline = time.monotonic() + timeout
    while True:
        release = find_release(tag)
        if release:
            return release
        if time.monotonic() >= deadline:
            raise ReleaseError(
                f"no release for {tag} after {timeout:.0f}s. release.yml creates "
                f"it in its 'release' job; check that run (did the tag fail the "
                f"version check?)."
            )
        print(f"waiting for release.yml to create {tag}...", file=sys.stderr)
        time.sleep(interval)


def publish_release(tag: str, version: str) -> tuple[dict, bool]:
    releases = list_releases()
    release = find_release(tag, releases)
    if not release:
        raise ReleaseError(f"no release for {tag} to publish")
    prerelease = is_prerelease(version)
    latest = should_be_latest(version, releases, tag)
    published = json.loads(
        _gh(
            ["--method", "PATCH", f"repos/{_repo()}/releases/{release['id']}", "--input", "-"],
            {
                "draft": False,
                "prerelease": prerelease,
                # A string on purpose: the API takes "true" | "false" | "legacy".
                "make_latest": "true" if latest else "false",
            },
        )
    )
    return published, latest


# --- CLI ---------------------------------------------------------------------


def _emit(key: str, value) -> None:
    """Print for humans, and append to $GITHUB_OUTPUT when running in Actions."""
    value = str(value).lower() if isinstance(value, bool) else str(value)
    print(f"{key}={value}")
    out = os.environ.get("GITHUB_OUTPUT")
    if out:
        with open(out, "a", encoding="utf-8") as handle:
            handle.write(f"{key}={value}\n")


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    sub = parser.add_subparsers(dest="cmd", required=True)

    p = sub.add_parser("classify", help="is VERSION a prerelease?")
    p.add_argument("version")

    p = sub.add_parser("notes", help="release body from the CHANGELOG section")
    p.add_argument("version")
    p.add_argument("--changelog", default=str(ROOT / "CHANGELOG.md"))
    p.add_argument("--output")

    p = sub.add_parser("find", help="print the release id for TAG (drafts included)")
    p.add_argument("tag")

    p = sub.add_parser("ensure", help="reuse TAG's release, or create it")
    p.add_argument("tag")
    p.add_argument("--name", required=True)
    p.add_argument("--notes-file", required=True)
    p.add_argument("--prerelease", choices=["auto", "true", "false"], default="auto")
    p.add_argument("--no-draft", dest="draft", action="store_false")

    p = sub.add_parser("wait", help="poll until TAG's release exists")
    p.add_argument("tag")
    p.add_argument("--timeout", type=float, default=1200)
    p.add_argument("--interval", type=float, default=20)

    p = sub.add_parser("publish", help="undraft TAG's release, set prerelease/latest")
    p.add_argument("tag")
    p.add_argument("--version", required=True)

    args = parser.parse_args(argv)
    try:
        if args.cmd == "classify":
            _emit("prerelease", is_prerelease(args.version))
        elif args.cmd == "notes":
            text = pathlib.Path(args.changelog).read_text(encoding="utf-8")
            notes = release_notes(args.version, text)
            if args.output:
                pathlib.Path(args.output).write_text(notes, encoding="utf-8")
                print(f"wrote {len(notes)} bytes of notes for {args.version} to {args.output}")
            else:
                sys.stdout.write(notes)
        elif args.cmd == "find":
            release = find_release(args.tag)
            if not release:
                print(f"no release for {args.tag}", file=sys.stderr)
                return 3
            _emit("release_id", release["id"])
        elif args.cmd == "ensure":
            if args.prerelease == "auto":
                prerelease = is_prerelease(args.tag.split("v", 1)[-1])
            else:
                prerelease = args.prerelease == "true"
            notes = pathlib.Path(args.notes_file).read_text(encoding="utf-8")
            release, created = ensure_release(args.tag, args.name, notes, prerelease, args.draft)
            state = "draft" if release.get("draft") else "published"
            print(f"{'created' if created else 'reusing'} {state} release {release['id']} for {args.tag}", file=sys.stderr)
            _emit("release_id", release["id"])
        elif args.cmd == "wait":
            release = wait_release(args.tag, args.timeout, args.interval)
            _emit("release_id", release["id"])
        elif args.cmd == "publish":
            release, latest = publish_release(args.tag, args.version)
            print(
                f"published {args.tag}: prerelease={release.get('prerelease')} "
                f"latest={'yes' if latest else 'no'}",
                file=sys.stderr,
            )
            _emit("release_id", release["id"])
    except ReleaseError as err:
        print(f"error: {err}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
