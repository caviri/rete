#!/usr/bin/env python3
"""Generate a fully self-contained, static playground at docs/playground.html.

The output runs entirely from WebAssembly with no server and no network fetches:
- the wasm-bindgen *no-modules* glue (a classic script exposing a global
  ``wasm_bindgen``) is inlined verbatim,
- the ``.wasm`` binary is embedded as base64 and handed to the initializer as
  bytes (so it never ``fetch``es), and
- the example ``.rete`` datasets are embedded as a base64 map.

Prerequisites (all via Docker, see CLAUDE.md / docs):
  wasm-pack build crates/rete-wasm --target no-modules --out-dir web/pkg-nomodules
  rete build examples/<x>.nt -o web/<x>.rete      # for each dataset below

Run (deterministic):
  python3 scripts/build_playground.py
"""

import base64
import json
import pathlib
import sys

ROOT = pathlib.Path(__file__).resolve().parent.parent
WEB = ROOT / "web"
NOMOD = WEB / "pkg-nomodules"
TEMPLATE = WEB / "playground.template.html"
OUT = ROOT / "docs" / "playground.html"

GLUE_JS = NOMOD / "rete_wasm.js"
WASM = NOMOD / "rete_wasm_bg.wasm"

# Datasets to embed: (playground key, built .rete file under web/).
# `citations` is the real OpenCitations network (citations of the AlphaFold paper,
# 10.1038/s41586-021-03819-2) enriched with clearly-labelled synthetic metadata;
# real DOIs / edges / years, fabricated titles/authors/venues/keywords.
DATASETS = [
    ("research", "research.rete"),
    ("typed", "typed.rete"),
    ("deps", "deps.rete"),
    ("papers", "papers.rete"),
    ("researchers", "researchers.rete"),
    ("citations", "enriched-all.rete"),
]


def die(msg: str) -> None:
    print(f"build_playground: error: {msg}", file=sys.stderr)
    sys.exit(1)


def b64(path: pathlib.Path) -> str:
    return base64.b64encode(path.read_bytes()).decode("ascii")


def main() -> None:
    for p in (TEMPLATE, GLUE_JS, WASM):
        if not p.exists():
            die(f"missing required input: {p} (build the no-modules wasm first)")

    datasets_b64 = {}
    for name, filename in DATASETS:
        f = WEB / filename
        if not f.exists():
            die(f"missing dataset {f} (build it into web/{filename} first)")
        datasets_b64[name] = b64(f)

    glue = GLUE_JS.read_text(encoding="utf-8")
    # The no-modules glue keeps a `fetch()` fallback in its async initializer for
    # the case where you pass a URL/string. We never do (we always hand it the
    # embedded bytes), so that branch is dead code — but to make the page provably
    # network-free (and to keep the `no fetch(` grep clean), neutralize the single
    # fetch line. If anyone ever passes a URL here it now fails loudly instead of
    # silently going to the network.
    fetch_line = "            module_or_path = fetch(module_or_path);"
    if fetch_line not in glue:
        die(
            "expected fetch fallback line not found in glue; "
            "wasm-pack output changed — update build_playground.py"
        )
    glue = glue.replace(
        fetch_line,
        "            throw new Error("
        "'rete playground is offline-only: pass embedded bytes, not a URL');",
    )
    wasm_b64 = b64(WASM)
    # Deterministic, sorted JSON for the datasets map.
    datasets_json = json.dumps(datasets_b64, sort_keys=True, separators=(",", ":"))

    template = TEMPLATE.read_text(encoding="utf-8")
    html = (
        template.replace("__GLUE_JS__", glue)
        .replace("__WASM_B64__", wasm_b64)
        .replace("__DATASETS_B64__", datasets_json)
    )

    OUT.parent.mkdir(parents=True, exist_ok=True)
    OUT.write_text(html, encoding="utf-8")

    sizes = ", ".join(f"{n}={len(datasets_b64[n])}b64" for n, _ in DATASETS)
    print(f"build_playground: wrote {OUT}")
    print(f"  wasm: {WASM.stat().st_size} bytes -> {len(wasm_b64)} base64 chars")
    print(f"  datasets: {sizes}")
    print(f"  output: {OUT.stat().st_size} bytes")


if __name__ == "__main__":
    main()
