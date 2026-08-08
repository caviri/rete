#!/usr/bin/env python3
"""Say WHICH regeneration mistake reddened the browser-artifact parity check.

CI's `wasm` job reruns `scripts/build_wasm.sh` and byte-diffs the tracked
browser artifacts. When that fails, `git diff` prints

    Binary files a/docs/engine/rete_wasm_bg.wasm and b/... differ

and stops — and three unrelated mistakes have produced exactly that line:

  1. STALE      the engine really moved and the committed artifacts predate it.
                The legitimate case: the job is doing its job.
  2. STAMP      RETE_BUILD_STAMP was not the string CI writes into
                docs/playground.html (the workspace version from Cargo.toml).
  3. TARGET DIR the wasm was built in a CARGO_TARGET_DIR that another build had
                already used, so the binary moved without changing size.

They are distinguishable from the bytes: (1) changes sizes, (2) changes only the
stamp line, (3) changes a handful of bytes at an unchanged size. This reads the
diff and names the one that happened, with the command that fixes it.

    python3 -P scripts/wasm_parity_triage.py [path …]

Exit status is 0 whatever it finds — it explains, it does not judge; the
`git diff --exit-code` that called it is what fails the build.
"""

from __future__ import annotations

import re
import subprocess
import sys
from pathlib import Path

# The set CI guards, used when no path is given. Keep in step with the `git
# diff` in .github/workflows/ci.yml (which passes them explicitly).
# web/explore-100mb.html is deliberately absent: it is gitignored, so it can
# never appear in a diff.
GUARDED = [
    "docs/playground.html",
    "docs/engine",
    "docs/explore-100mb.html",
    "docs/rete_wasm_async.js",
    "docs/rete_wasm_async.wasm",
]

# The two places build_playground.py writes the stamp into the page.
STAMP_PATTERNS = (
    re.compile(rb'window\.RETE_BUILD = "([^"]*)";'),
    re.compile(rb'class="build-ver"[^>]*>build ([^<]*)<'),
)

# Below this share of differing bytes, at an unchanged size, the binary did not
# gain or lose code — it moved. Observed signature: 13 bytes in 3,254,668.
MOVED_RATIO = 0.001


def git(*args: str) -> str:
    return subprocess.run(
        ["git", *args], capture_output=True, text=True, check=False
    ).stdout


def workspace_version() -> str:
    text = Path("Cargo.toml").read_text(encoding="utf-8")
    m = re.search(r'(?ms)^\[workspace\.package\].*?^version = "([^"]+)"', text)
    return m.group(1) if m else "?"


def stamp_of(blob: bytes) -> str | None:
    for pattern in STAMP_PATTERNS:
        m = pattern.search(blob)
        if m:
            return m.group(1).decode("utf-8", "replace")
    return None


def without_stamp(blob: bytes) -> bytes:
    """The page with its build stamp blanked, so two stamps of DIFFERENT length
    (`0.3.2` against a 40-character SHA — the #199 shape) still compare equal
    everywhere else. Only the two known stamp sites are touched, never every
    occurrence of the version string."""
    out = blob
    for pattern in STAMP_PATTERNS:
        out = pattern.sub(
            lambda m: m.group(0).replace(m.group(1), b"\0STAMP\0") if m.group(1) else m.group(0),
            out,
        )
    return out


def differing_bytes(a: bytes, b: bytes) -> int:
    return sum(x != y for x, y in zip(a, b))


class Finding:
    def __init__(self, path: str, cause: str, detail: str):
        self.path, self.cause, self.detail = path, cause, detail


def classify(path: str, old: bytes, new: bytes, version: str) -> Finding:
    if not old:
        return Finding(path, "STALE", "not committed (new file)")

    # The stamp check comes FIRST, because changing the stamp changes the size
    # of the page and would otherwise be read as "the engine moved".
    was, now = stamp_of(old), stamp_of(new)
    if was is not None and now is not None and was != now:
        if without_stamp(old) == without_stamp(new):
            return Finding(
                path,
                "STAMP",
                f'rebuilt as "{now}", committed as "{was}" '
                f'(the workspace version is "{version}") — nothing else differs',
            )

    if len(old) != len(new):
        return Finding(
            path, "STALE", f"{len(old):,} B committed vs {len(new):,} B rebuilt"
        )

    n = differing_bytes(old, new)
    ratio = n / max(len(old), 1)
    cause = "TARGET_DIR" if ratio < MOVED_RATIO else "STALE"
    return Finding(
        path, cause, f"same size ({len(old):,} B), {n:,} differing bytes ({ratio:.5%})"
    )


FIXES = {
    "STALE": (
        "STALE ARTIFACTS — the engine moved and the committed files predate it.",
        [
            "This is the check working as intended. Regenerate and commit:",
            "    docker compose run --rm wasm        # or: bash scripts/build_wasm.sh",
            "Or take CI's own bytes from the `wasm-build-<sha>` artifact this job",
            "uploads even when it fails.",
        ],
    ),
    "STAMP": (
        "THE BUILD STAMP — the page carries a different string from the one CI writes.",
        [
            "scripts/build_wasm.sh defaults RETE_BUILD_STAMP to the [workspace.package]",
            "version, and CI passes no stamp, so that is what CI's rebuild carries. A",
            "page stamped with anything else was built with RETE_BUILD_STAMP set",
            "explicitly (right for a release, wrong for a commit). Regenerate without it:",
            "    docker compose run --rm -e RETE_SOURCE_REVISION=$(git rev-parse HEAD) wasm",
        ],
    ),
    "TARGET_DIR": (
        "THE TARGET DIR — same size, a handful of bytes, the binary merely moved.",
        [
            "This is the signature reported in 35adffeb for a wasm built in a",
            "CARGO_TARGET_DIR shared with a host build: identical size, data-symbol",
            "addresses off by 4. scripts/wasm_target_dir.sh gives the wasm build a dir",
            "of its own, so an artifact with this signature means the build did not go",
            "through scripts/build_wasm.sh, or something wrote into its dir:",
            "    rm -rf \"${CARGO_TARGET_DIR:-target}/wasm32\"*",
            "    docker compose run --rm wasm",
        ],
    ),
}


def main(argv: list[str]) -> int:
    paths = argv or GUARDED
    changed = [p for p in git("diff", "--name-only", "--", *paths).splitlines() if p]
    if not changed:
        print("browser artifacts: no diff — nothing to explain.")
        return 0

    version = workspace_version()
    findings = []
    for path in changed:
        old = subprocess.run(
            ["git", "show", f"HEAD:{path}"], capture_output=True, check=False
        ).stdout
        new = Path(path).read_bytes() if Path(path).exists() else b""
        findings.append(classify(path, old, new, version))

    width = max(len(f.path) for f in findings)
    print("\n── browser-artifact parity: what actually differs ──")
    for f in findings:
        print(f"  {f.path:<{width}}  {f.cause:<10}  {f.detail}")

    # One verdict. A size change beats everything (real code moved); a
    # micro-diff at an unchanged size beats a stamp-only page, because the
    # pages inline the wasm and inherit its bytes.
    for cause in ("STALE", "TARGET_DIR", "STAMP"):
        if any(f.cause == cause for f in findings):
            headline, lines = FIXES[cause]
            print(f"\nVERDICT: {headline}")
            for line in lines:
                print(f"  {line}")
            break
    print()
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
