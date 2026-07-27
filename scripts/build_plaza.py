#!/usr/bin/env python3
"""Assemble a GitHub-Pages-ready copy of the plaza gallery at docs/plaza/.

The experiment in `experiments/plaza/` is written to be served from the repo
root: it reads each dataset's card live from `../../web/*.rete`, loads the WASM
worker from `../../../web/pkg-nomodules`, and links to `../../docs/*.html`. None
of those resolve under `docs/` (the Pages site root), so this script copies the
static site into `docs/plaza/` and rewrites the paths to be self-contained:

  ../../web/<x>.rete   ->  data/<x>.rete        (the bundled card files, copied)
  ../../../web/pkg-nomodules/  ->  ../pkg-nomodules/   (in plaza-worker.js)
  ../../docs/<x>.html  ->  ../<x>.html          (links back into the docs site)

Remote (`https://data.graphplaza.com/…`) dataset URLs are left untouched — those cards are
read live over HTTP range from the bucket. Run after `build_playground.py` (it
needs the WASM build in `web/pkg-nomodules`):

    uv run python scripts/build_plaza.py
"""

import json
import pathlib
import re
import shutil
import sys

ROOT = pathlib.Path(__file__).resolve().parent.parent
SRC = ROOT / "experiments" / "plaza"
WEB = ROOT / "web"
NOMOD = WEB / "pkg-nomodules"
OUT = ROOT / "docs" / "plaza"


def die(msg: str) -> None:
    print("build_plaza: " + msg, file=sys.stderr)
    raise SystemExit(1)


def main() -> None:
    if not SRC.exists():
        die(f"missing source: {SRC}")
    if not (NOMOD / "rete_wasm.js").exists():
        die(f"missing WASM build {NOMOD} — run scripts/build_playground.py first")

    # Fresh output (only ever touches docs/plaza).
    if OUT.exists():
        shutil.rmtree(OUT)
    OUT.mkdir(parents=True)
    (OUT / "data").mkdir()

    # 1. Static site: html, css, js/, vendor/.
    for name in ("index.html", "dataset.html", "ontology.html", "styles.css"):
        shutil.copy2(SRC / name, OUT / name)
    shutil.copytree(SRC / "js", OUT / "js")
    shutil.copytree(SRC / "vendor", OUT / "vendor")

    # 2. The no-modules WASM build the live-explore worker importScripts.
    pkg = OUT / "pkg-nomodules"
    pkg.mkdir()
    for f in ("rete_wasm.js", "rete_wasm_bg.wasm"):
        shutil.copy2(NOMOD / f, pkg / f)

    # 3. plaza-worker.js: point importScripts/module_or_path at ../pkg-nomodules/.
    worker = OUT / "js" / "plaza-worker.js"
    worker.write_text(
        worker.read_text(encoding="utf-8").replace("../../../web/pkg-nomodules/", "../pkg-nomodules/"),
        encoding="utf-8",
        newline="\n",
    )

    # 4. plaza.json: copy each bundled ../../web/<x>.rete into data/, rewrite the
    #    `rete` paths, and fix doc links. Remote URLs are left as-is.
    manifest = json.loads((SRC / "plaza.json").read_text(encoding="utf-8"))
    copied = []
    for ds in manifest.get("datasets", []):
        rete = ds.get("rete", "")
        if rete.startswith("../../web/"):
            base = pathlib.PurePosixPath(rete).name
            srcf = WEB / base
            if srcf.exists():
                shutil.copy2(srcf, OUT / "data" / base)
                ds["rete"] = "data/" + base
                copied.append(base)
            else:
                print(f"  warning: bundled file missing, leaving header-only: {base}")
        for link in ds.get("links", []) or []:
            u = link.get("url", "")
            if u.startswith("../../docs/"):
                link["url"] = "../" + u[len("../../docs/"):]
        for comp in ds.get("companions", []) or []:
            u = comp.get("url", "")
            if u.startswith("../../data/"):
                # local companion tables aren't published to Pages; point at the
                # repo on GitHub so the link still resolves to something real.
                comp["url"] = "https://github.com/caviri/rete/tree/main/" + u[len("../../"):]
    # newline="\n": the repository is LF-only (`* text=auto eol=lf`), and
    # write_text would otherwise emit CRLF on Windows — which git normalizes on
    # commit, so the file shows as modified after every rebuild for no reason.
    (OUT / "plaza.json").write_text(
        json.dumps(manifest, indent=2, ensure_ascii=False), encoding="utf-8", newline="\n"
    )

    # 5. Belt-and-braces: nothing under docs/plaza should still climb out of it.
    residual = []
    for p in OUT.rglob("*"):
        if p.suffix in (".html", ".js", ".json") and p.is_file():
            for ln in p.read_text(encoding="utf-8", errors="ignore").splitlines():
                if "../../" in ln and "data.graphplaza.com" not in ln and "github.com" not in ln:
                    residual.append(f"{p.relative_to(OUT)}: {ln.strip()[:90]}")
    if residual:
        print("  note: residual ../../ references (review if a feature 404s):")
        for r in residual[:12]:
            print("    " + r)

    total = sum(f.stat().st_size for f in OUT.rglob("*") if f.is_file())
    print(f"build_plaza: wrote {OUT} — {len(copied)} card files copied, {total // 1024} KiB total")
    print(f"  cards: {', '.join(copied)}")


if __name__ == "__main__":
    main()
