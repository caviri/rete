#!/usr/bin/env python3
"""Build the self-contained Lombardi drawing experiment → docs/lombardi.html.

The page redraws Mark Lombardi's network drawings as vectors, reading every
node, arc and card live out of `lombardi.rete` over HTTP range. Like the yasgui
IDE it ships as ONE file:

  - the wasm-bindgen *no-modules* rete glue (global ``wasm_bindgen``), inlined
    and reused as the first half of the worker source,
  - the rete ``.wasm`` engine, base64-embedded,
  - the engine worker, shared VERBATIM with yasgui (web/yasgui-src/worker.js) —
    RemoteGraph reads over synchronous XHR, which only a worker may do,
  - the page's own app.js.

Rebuild the wasm first if the engine changed:
  wasm-pack build crates/rete-wasm --target no-modules --out-dir ../../web/pkg-nomodules
"""
import argparse
import base64
import datetime
import pathlib
import subprocess
import sys

ROOT = pathlib.Path(__file__).resolve().parent.parent
WEB = ROOT / "web"
NOMOD = WEB / "pkg-nomodules"
GLUE_JS = NOMOD / "rete_wasm.js"
WASM = NOMOD / "rete_wasm_bg.wasm"
WORKER_JS = WEB / "yasgui-src" / "worker.js"      # shared, not forked
APP_JS = WEB / "lombardi-src" / "app.js"
TEMPLATE = WEB / "lombardi.template.html"
DEFAULT_OUT = ROOT / "docs" / "lombardi.html"

RETE_URL = "https://data.graphplaza.com/lombardi/lombardi.rete"


def die(msg: str) -> None:
    print(f"build_lombardi_page: error: {msg}", file=sys.stderr)
    sys.exit(1)


def b64(path: pathlib.Path) -> str:
    return base64.b64encode(path.read_bytes()).decode("ascii")


def main() -> None:
    ap = argparse.ArgumentParser()
    ap.add_argument("--out", default=str(DEFAULT_OUT))
    ap.add_argument("--rete-url", default=RETE_URL)
    args = ap.parse_args()

    for p in (TEMPLATE, APP_JS, WORKER_JS, GLUE_JS, WASM):
        if not p.exists():
            die(f"missing required input: {p}")

    chunks = {
        "__GLUE_JS__": GLUE_JS.read_text(encoding="utf-8").rstrip(),
        "__WORKER_JS__": WORKER_JS.read_text(encoding="utf-8").rstrip(),
    }
    # Inlined scripts must not close their own <script> tag.
    for name, text in chunks.items():
        if "</script" in text:
            die(f"{name} contains a literal </script> — cannot inline")

    try:
        sha = subprocess.run(["git", "rev-parse", "--short=12", "HEAD"],
                             cwd=ROOT, capture_output=True, text=True, check=True).stdout.strip()
    except Exception:
        sha = "unknown"

    html = TEMPLATE.read_text(encoding="utf-8")
    for ph, text in chunks.items():
        html = html.replace(ph, text)
    html = (
        html
        .replace("__WASM_B64__", b64(WASM))
        .replace("__RETE_URL__", args.rete_url)
        .replace("__BUILD_STAMP__", f"Built {datetime.date.today().isoformat()} from {sha}.")
        # app.js last: it must not contain the other placeholders
        .replace("__APP_JS__", APP_JS.read_text(encoding="utf-8").rstrip())
    )

    for ph in ("__GLUE_JS__", "__WORKER_JS__", "__WASM_B64__", "__RETE_URL__",
               "__BUILD_STAMP__", "__APP_JS__"):
        if ph in html:
            die(f"unreplaced template placeholder: {ph}")

    out = pathlib.Path(args.out)
    out.parent.mkdir(parents=True, exist_ok=True)
    out.write_text(html, encoding="utf-8")
    print(f"build_lombardi_page: wrote {out}")
    print(f"  rete wasm: {WASM.stat().st_size:>9,} bytes")
    print(f"  output:    {out.stat().st_size:>9,} bytes")


if __name__ == "__main__":
    main()
