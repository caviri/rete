#!/usr/bin/env python3
"""Build the self-contained RETE network explorer → docs/explorer.html.

A single, offline `.html` (double-click, no server, no network) that bundles:
  - the wasm-bindgen *no-modules* rete glue (defines a global ``wasm_bindgen``),
    inlined verbatim,
  - the real rete ``.wasm`` engine, base64-embedded (the DATA layer: opens a
    ``.rete`` in memory and answers SPARQL),
  - a tiny AssemblyScript force-layout ``.wasm``, base64-embedded (the DRAW layer),
  - one example ``.rete`` dataset, base64-embedded,
  - the riso-print explorer UI (web/explorer.template.html) + app
    (web/explorer-src/explorer.js).

Prereqs (all via Docker, same as the playground):
  wasm-pack build crates/rete-wasm --target no-modules --out-dir ../../web/pkg-nomodules --no-opt
  rete build <inputs> -o web/<dataset>.rete

Usage:
  python3 scripts/build_explorer.py [--dataset mira] [--out docs/explorer.html]
"""
import argparse
import base64
import json
import pathlib
import sys

ROOT = pathlib.Path(__file__).resolve().parent.parent
WEB = ROOT / "web"
NOMOD = WEB / "pkg-nomodules"
GLUE_JS = NOMOD / "rete_wasm.js"
WASM = NOMOD / "rete_wasm_bg.wasm"
SRC = WEB / "explorer-src"
LAYOUT_WASM = SRC / "layout.wasm"
EXPLORER_JS = SRC / "explorer.js"
TEMPLATE = WEB / "explorer.template.html"
DEFAULT_OUT = ROOT / "docs" / "explorer.html"

# Light metadata for the datasets we ship; falls back to {title: key} otherwise.
DATASET_META = {
    "mira": {
        "title": "MIrA · manuscritos con vínculos irlandeses",
        "license": "CC BY-NC-SA 4.0",
        "source": "https://www.mira.ie",
    },
    "linked-jazz": {
        "title": "Linked Jazz · red social de músicos de jazz",
        "license": "CC BY-SA",
        "source": "https://linkedjazz.org",
    },
    "nomisma": {
        "title": "Nomisma · monedas de Alejandro Magno (PELLA)",
        "license": "CC-BY",
        "source": "http://nomisma.org",
    },
    "mimotext": {
        "title": "MiMoText · novelas francesas de la Ilustración",
        "license": "CC0",
        "source": "https://mimotext.uni-trier.de",
    },
    "lineara": {
        "title": "Linear A · corpus de inscripciones (SigLA)",
        "license": "ver fuente",
        "source": "https://lineara.xyz",
    },
}


def die(msg: str) -> None:
    print(f"build_explorer: {msg}", file=sys.stderr)
    sys.exit(1)


def b64(path: pathlib.Path) -> str:
    return base64.b64encode(path.read_bytes()).decode("ascii")


def main() -> None:
    ap = argparse.ArgumentParser()
    ap.add_argument("--dataset", default="mira",
                    help="playground key of a built web/<key>.rete to embed")
    ap.add_argument("--out", default=str(DEFAULT_OUT))
    args = ap.parse_args()

    dataset_file = WEB / f"{args.dataset}.rete"
    required = [TEMPLATE, EXPLORER_JS, GLUE_JS, WASM, LAYOUT_WASM, dataset_file]
    for p in required:
        if not p.exists():
            die(f"missing required input: {p}")

    # Neutralize the no-modules glue's fetch() fallback so the page is provably
    # offline (we always hand it embedded bytes, never a URL). Same patch as the
    # playground build.
    glue = GLUE_JS.read_text(encoding="utf-8")
    fetch_line = "            module_or_path = fetch(module_or_path);"
    if fetch_line not in glue:
        die("expected fetch fallback line not found in glue; wasm-pack output "
            "changed — update build_explorer.py")
    glue = glue.replace(
        fetch_line,
        "            throw new Error("
        "'rete explorer is offline-only: pass embedded bytes, not a URL');",
    )

    meta = DATASET_META.get(args.dataset, {"title": args.dataset})
    meta = {**meta, "key": args.dataset}

    html = (
        TEMPLATE.read_text(encoding="utf-8")
        .replace("__GLUE_JS__", glue)
        .replace("__WASM_B64__", b64(WASM))
        .replace("__LAYOUT_WASM_B64__", b64(LAYOUT_WASM))
        .replace("__DATASET_B64__", b64(dataset_file))
        .replace("__DATASET_KEY__", args.dataset)
        .replace("__DATASET_META_JSON__",
                 json.dumps(meta, ensure_ascii=False, separators=(",", ":")))
        # explorer.js last: it must not contain the other placeholders
        .replace("__EXPLORER_JS__", EXPLORER_JS.read_text(encoding="utf-8").rstrip())
    )

    for ph in ("__GLUE_JS__", "__WASM_B64__", "__LAYOUT_WASM_B64__",
               "__DATASET_B64__", "__DATASET_KEY__", "__DATASET_META_JSON__",
               "__EXPLORER_JS__"):
        if ph in html:
            die(f"unreplaced template placeholder: {ph}")

    out = pathlib.Path(args.out)
    out.parent.mkdir(parents=True, exist_ok=True)
    out.write_text(html, encoding="utf-8")
    print(f"build_explorer: wrote {out}")
    print(f"  rete wasm:   {WASM.stat().st_size:>9,} bytes")
    print(f"  layout wasm: {LAYOUT_WASM.stat().st_size:>9,} bytes")
    print(f"  dataset:     {dataset_file.stat().st_size:>9,} bytes ({args.dataset})")
    print(f"  output:      {out.stat().st_size:>9,} bytes")


if __name__ == "__main__":
    main()
