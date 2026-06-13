#!/usr/bin/env python3
"""Build the fully-static Historical Atlas page (docs/atlas.html).

Inlines the no-modules WASM engine, its glue, and the embedded history.rete into
web/atlas.template.html — a single offline HTML file: a canvas map (GIS) + a
SPARQL editor + a temporal timeline, all querying the .rete via WASM in-browser.

Prereqs (same as the playground):
  wasm-pack build crates/rete-wasm --target no-modules --out-dir ../../web/pkg-nomodules
  python3 scripts/geo_to_rete.py basemaps --years bc323,1000,1492,1815,1914,1945,1994 \
      --prec 2 --min-bbox 0.3 --max-per-year 90 -o dev/geo/history.nt
  rete build dev/geo/history.nt -o web/history.rete

Usage: python3 scripts/build_atlas.py
"""
from __future__ import annotations

import base64
import pathlib
import sys

ROOT = pathlib.Path(__file__).resolve().parents[1]
WEB = ROOT / "web"
NOMOD = WEB / "pkg-nomodules"
TEMPLATE = WEB / "atlas.template.html"
GLUE_JS = NOMOD / "rete_wasm.js"
WASM = NOMOD / "rete_wasm_bg.wasm"
RETE = WEB / "history.rete"
OUT = ROOT / "docs" / "atlas.html"
WEB_OUT = WEB / "atlas.html"


def die(msg: str) -> None:
    print(f"build_atlas: error: {msg}", file=sys.stderr)
    sys.exit(1)


def b64(path: pathlib.Path) -> str:
    return base64.b64encode(path.read_bytes()).decode("ascii")


def main() -> None:
    for p in (TEMPLATE, GLUE_JS, WASM, RETE):
        if not p.exists():
            die(f"missing required input: {p}")

    glue = GLUE_JS.read_text(encoding="utf-8")
    # Neutralize the no-modules glue's network fetch fallback (we always pass
    # embedded bytes) so the page is provably offline — mirrors build_playground.
    fetch_line = "            module_or_path = fetch(module_or_path);"
    if fetch_line not in glue:
        die("expected fetch fallback line not found in glue; wasm-pack output changed")
    glue = glue.replace(
        fetch_line,
        "            throw new Error('rete atlas is offline-only: pass embedded bytes, not a URL');",
    )

    html = (
        TEMPLATE.read_text(encoding="utf-8")
        .replace("__GLUE_JS__", glue)
        .replace("__WASM_B64__", b64(WASM))
        .replace("__RETE_B64__", b64(RETE))
    )
    for ph in ("__GLUE_JS__", "__WASM_B64__", "__RETE_B64__"):
        if ph in html:
            die(f"unreplaced placeholder: {ph}")

    OUT.write_text(html, encoding="utf-8")
    WEB_OUT.write_text(html, encoding="utf-8")
    print(f"build_atlas: wrote {OUT} ({OUT.stat().st_size} bytes)")
    print(f"  wasm {WASM.stat().st_size} b · history.rete {RETE.stat().st_size} b")


if __name__ == "__main__":
    main()
