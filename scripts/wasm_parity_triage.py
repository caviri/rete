#!/usr/bin/env python3
"""Say WHICH regeneration mistake reddened the browser-artifact parity check.

CI's `wasm` job reruns `scripts/build_wasm.sh` and byte-diffs the tracked
browser artifacts. When that fails, `git diff` prints

    Binary files a/docs/engine/rete_wasm_bg.wasm and b/... differ

and stops — and four unrelated mistakes have produced exactly that line:

  1. STALE      the engine really moved and the committed artifacts predate it.
                The legitimate case: the job is doing its job.
  2. STAMP      RETE_BUILD_STAMP was not the string CI writes into
                docs/playground.html (the workspace version from Cargo.toml).
  3. TARGET DIR the wasm was built in a CARGO_TARGET_DIR that another build had
                already used, so the binary moved without changing size.
  4. LINE NUMS  source lines were added or removed ABOVE panicking code, so the
                `line` field of a baked-in `core::panic::Location` moved. Also
                legitimate — and it fires for edits that "cannot" touch codegen,
                doc comments included, because a doc comment occupies lines.

(1) changes sizes and (2) changes only the stamp line, so both are cheap to
recognise. (3) and (4) have the SAME coarse signature — a handful of bytes at an
unchanged size — which is why (4) is not a footnote: #208 was a doc-comment-only
edit reported as TARGET_DIR, and the target dir was innocent. They are still
distinguishable, by WHICH field of the 16-byte Location record moved: a
relocation moves the pointer, a line shift moves the third u32 and nothing else.
This reads the diff and names the one that happened, with the command that fixes
it.

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


def differing_offsets(a: bytes, b: bytes, cap: int = 64) -> list[int]:
    """Where they differ, up to `cap` positions (enough to characterise; the
    callers only run this once the count is already known to be tiny)."""
    out = []
    for i, (x, y) in enumerate(zip(a, b)):
        if x != y:
            out.append(i)
            if len(out) >= cap:
                break
    return out


def u32_at(blob: bytes, off: int) -> int:
    return int.from_bytes(blob[off : off + 4], "little")


def location_line_shift(old: bytes, new: bytes) -> tuple[int, list[tuple[int, int, int]]] | None:
    """`(delta, [(line_before, line_after, col), …])` when every differing byte
    sits in the `line` field of a plausible `core::panic::Location`, and all of
    them moved by the same delta. `None` when anything else moved.

    rustc lowers a panic site to a 16-byte static — `(ptr to the file path,
    path len, line, col)` — and `#[track_caller]` passes a reference to it. Add
    a line above the code and `line` changes while the other three fields do
    not, which is precisely what separates this from a relocation (the pointer
    moves) and from real code motion (the size changes).

    The records are 4-byte aligned in LINEAR MEMORY, not in the file: what is
    scanned here is a data segment sitting at whatever offset the section and
    segment headers left it at. So the record start is searched for rather than
    computed — the differing byte may be any of the four bytes of `line`."""
    deltas: set[int] = set()
    records: list[tuple[int, int, int]] = []
    limit = min(len(old), len(new))

    def record_at(rec: int) -> tuple[int, int, int] | None:
        """`(line_before, line_after, col)` if a Location whose `line` moved —
        and only `line` — starts at `rec`."""
        if rec < 0 or rec + 16 > limit:
            return None
        ptr, path_len, col = u32_at(old, rec), u32_at(old, rec + 4), u32_at(old, rec + 12)
        # A pointer into linear memory, a plausible path length, a column.
        if not (ptr > 0xFFFF and 2 <= path_len <= 200 and 0 < col <= 1000):
            return None
        # Only `line` may have moved: pointer, length and column must be equal.
        if any(u32_at(old, rec + o) != u32_at(new, rec + o) for o in (0, 4, 12)):
            return None
        line_old, line_new = u32_at(old, rec + 8), u32_at(new, rec + 8)
        if line_old == line_new or not (0 < line_old < 1_000_000):
            return None
        return line_old, line_new, col

    for off in differing_offsets(old, new):
        # `line` occupies rec+8 .. rec+11, so the byte that differs puts the
        # record start in a four-wide window. Exactly one candidate must fit.
        hits = [r for r in (record_at(off - 8 - k) for k in range(4)) if r is not None]
        if len(hits) != 1:
            return None
        line_old, line_new, col = hits[0]
        deltas.add(line_new - line_old)
        records.append((line_old, line_new, col))

    if not records or len(deltas) != 1 or 0 in deltas:
        return None
    return deltas.pop(), records


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
    if ratio >= MOVED_RATIO:
        return Finding(
            path, "STALE", f"same size ({len(old):,} B), {n:,} differing bytes ({ratio:.5%})"
        )

    # Same size, a handful of bytes: TARGET_DIR and LINE_NUMBERS both look like
    # this. Only one of them leaves every Location field but `line` untouched.
    shift = location_line_shift(old, new)
    if shift is not None:
        delta, records = shift
        moved = ", ".join(f"{a}->{b} (col {c})" for a, b, c in records[:4])
        more = f" +{len(records) - 4} more" if len(records) > 4 else ""
        return Finding(
            path,
            "LINE_NUMBERS",
            f"same size ({len(old):,} B), {n:,} differing bytes — "
            f"panic locations all {delta:+d} lines: {moved}{more}",
        )

    return Finding(
        path,
        "TARGET_DIR",
        f"same size ({len(old):,} B), {n:,} differing bytes ({ratio:.5%})",
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
    "LINE_NUMBERS": (
        "SOURCE LINES MOVED — the engine's panic locations shifted. Regenerate and commit.",
        [
            "rustc bakes `line!()` into the binary: every panic site carries a 16-byte",
            "`core::panic::Location` — (file ptr, path len, line, col). Adding or removing",
            "lines above panicking code moves `line` and nothing else, so the artifacts",
            "really are stale even when the edit 'cannot' affect codegen. A doc comment",
            "does this: it never reaches codegen, but it occupies lines (#208).",
            "This is the check working as intended:",
            "    docker compose run --rm -e RETE_SOURCE_REVISION=$(git rev-parse HEAD) wasm",
            "Then commit the artifacts. Nothing is wrong with your target dir.",
        ],
    ),
    "TARGET_DIR": (
        "THE TARGET DIR — same size, a handful of bytes, and no Location moved.",
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
    cause_width = max(len(f.cause) for f in findings)
    print("\n── browser-artifact parity: what actually differs ──")
    for f in findings:
        print(f"  {f.path:<{width}}  {f.cause:<{cause_width}}  {f.detail}")

    # One verdict. A size change beats everything (real code moved); a
    # micro-diff at an unchanged size beats a stamp-only page, because the
    # pages inline the wasm and inherit its bytes. LINE_NUMBERS outranks
    # TARGET_DIR for the same reason: the pages carry the wasm's bytes as
    # base64, where no Location is recognisable, so only the wasm itself can
    # ever carry the evidence — and when it does, it is the explanation for all
    # of them.
    for cause in ("STALE", "LINE_NUMBERS", "TARGET_DIR", "STAMP"):
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
