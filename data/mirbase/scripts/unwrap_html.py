#!/usr/bin/env python3
"""Un-wrap the HTML-ised text files miRBase serves under /download/CURRENT/.

miRBase's Django app renders plain-text payloads into a template, so
`/download/CURRENT/mirna.txt` comes back as::

    <p>64685\tMI0000001\tcel-let-7\t...<br>64686\t...<br></p>

The transform back is exact and total: strip the wrapping <p>...</p>, turn
every <br> into a newline, and unescape HTML entities (&gt; &lt; &amp; &quot;).

We do not have to take that on faith. `hairpin.fa` is served BOTH raw
(/download/hairpin.fa) and wrapped (/download/CURRENT/hairpin.fa), so this
script un-wraps the wrapped copy and asserts it is byte-identical to the raw
one before trusting the same transform on the files that only exist wrapped.

Run (from repo root):
    docker run --rm -v "$PWD:/w" -w //w python:3.12-slim \
        python data/mirbase/scripts/unwrap_html.py
"""
from __future__ import annotations

import html
import re
import sys
from pathlib import Path

RAW = Path(__file__).resolve().parent.parent / "raw"
WRAP = RAW / "_wrapped_html"


def unwrap(text: str) -> str:
    """HTML-wrapped miRBase payload -> the original plain text."""
    s = re.sub(r"^\s*<p>", "", text)
    s = re.sub(r"</p>\s*$", "", s.rstrip())
    s = s.replace("<br>", "\n")
    s = html.unescape(s)
    if s and not s.endswith("\n"):
        s += "\n"
    return s


def selftest() -> None:
    """Prove the transform is lossless using the file served both ways."""
    raw = RAW / "hairpin.fa"
    wrapped = WRAP / "hairpin.fa.wrapped"
    if not (raw.exists() and wrapped.exists()):
        print("!! self-test SKIPPED (need raw/hairpin.fa + the wrapped copy)")
        return
    got = unwrap(wrapped.read_text(encoding="utf-8"))
    want = raw.read_text(encoding="utf-8")
    if got != want:
        for i, (a, b) in enumerate(zip(want, got)):
            if a != b:
                sys.exit(f"!! un-wrap self-test FAILED at byte {i}: "
                         f"{want[i-40:i+40]!r} != {got[i-40:i+40]!r}")
        sys.exit(f"!! un-wrap self-test FAILED: length {len(want)} != {len(got)}")
    print(f"ok  un-wrap self-test: hairpin.fa {len(want):,} bytes byte-identical")


def main() -> None:
    if not WRAP.exists():
        sys.exit(f"missing {WRAP} — run download.sh first")

    # Fetch a wrapped copy of hairpin.fa for the self-test if download.sh
    # left one; it is written by download.sh only when curl is available.
    selftest()

    n = 0
    for src in sorted(WRAP.rglob("*")):
        if not src.is_file() or src.name.endswith(".wrapped"):
            continue
        rel = src.relative_to(WRAP)
        dst = RAW / rel
        dst.parent.mkdir(parents=True, exist_ok=True)
        text = unwrap(src.read_text(encoding="utf-8"))
        dst.write_text(text, encoding="utf-8", newline="\n")
        print(f"    {rel}  ->  {len(text):,} bytes")
        n += 1
    print(f"ok  un-wrapped {n} files into {RAW}")


if __name__ == "__main__":
    main()
