#!/usr/bin/env bash
# Stage the GitHub Pages site into a directory (pages.yml runs this; run it
# locally the same way).
#
#   scripts/stage_pages.sh <out-dir>
#
# Layout of <out-dir>:
#   /            the landing page (site/)
#   /docs/       the docs tree (docs/), with the playground build stamp
#   /<old path>  every old docs URL keeps answering at its old place:
#                .html -> a small redirect stub to /docs/<path> (query and
#                #anchor kept), everything else -> a plain copy.
#
# Env: EXPECTED_SHA (stamped into the playground; default: git HEAD, 12 chars)
#      SITE_BASE    (default https://caviri.github.io/rete/; used for canonical)
set -euo pipefail

out="${1:?usage: stage_pages.sh <out-dir>}"
root="$(cd "$(dirname "$0")/.." && pwd)"
: "${EXPECTED_SHA:=$(git -C "$root" rev-parse --short=12 HEAD)}"
: "${SITE_BASE:=https://caviri.github.io/rete/}"
export EXPECTED_SHA SITE_BASE

rm -rf "$out"
mkdir -p "$out/docs"
cp -a "$root/docs/." "$out/docs/"
cp -a "$root/site/." "$out/"
touch "$out/.nojekyll"

python3 - "$root/docs" "$out" <<'PY'
import os, re, shutil, sys
from html import escape
from pathlib import Path
from urllib.parse import quote

src, out = Path(sys.argv[1]), Path(sys.argv[2])
base = os.environ["SITE_BASE"].rstrip("/") + "/"

# Build stamp: the deployed check compares it with the pushed commit.
page = out / "docs" / "playground.html"
text = page.read_text(encoding="utf-8")
text, n = re.subn(r'window\.RETE_BUILD = "[^"]*";',
                  f'window.RETE_BUILD = "{os.environ["EXPECTED_SHA"]}";', text, count=1)
if n != 1:
    raise SystemExit("playground build stamp was not found exactly once")
page.write_text(text, encoding="utf-8")

STUB = """<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<title>Moved</title>
<link rel="canonical" href="{canon}">
<meta http-equiv="refresh" content="0; url={rel}">
<script>location.replace({rel_js} + location.search + location.hash);</script>
</head>
<body><p>This page moved to <a href="{rel}">{canon}</a>.</p></body>
</html>
"""

stubs = copies = 0
for f in sorted(p for p in src.rglob("*") if p.is_file()):
    rel = f.relative_to(src).as_posix()
    dest = out / rel
    if rel == "index.html":
        continue  # "/" is the landing page; it forwards /index.html itself
    if dest.exists():
        raise SystemExit(f"{rel}: collides with a landing file")
    dest.parent.mkdir(parents=True, exist_ok=True)
    if rel.endswith(".html"):
        depth = rel.count("/")
        target = "../" * depth + "docs/" + quote(rel)
        canon = base + "docs/" + quote(rel)
        dest.write_text(STUB.format(canon=escape(canon, True), rel=escape(target, True),
                                    rel_js='"' + target + '"'), encoding="utf-8")
        stubs += 1
    else:
        shutil.copyfile(f, dest)
        copies += 1
print(f"staged: {stubs} redirect stubs, {copies} kept files")
PY
du -sh "$out" | sed 's/^/size: /'
