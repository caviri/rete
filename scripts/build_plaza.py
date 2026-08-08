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

    # The bundled cards under docs/plaza/data/ are an INPUT, not an output.
    # They are tracked (see .gitignore's `!/docs/plaza/data/*.rete` un-ignore)
    # and nothing in the tree — or in the bucket — reproduces them: the R2
    # objects are older, smaller builds (causal is 5,924 B there against the
    # 28,652 B card-bearing file here), and history.rete was built from
    # dev/geo/history.nt, which is gitignored. A rebuild that cannot find
    # web/<x>.rete must therefore keep what is already committed rather than
    # regenerate it, so carry the directory across the wipe below.
    preserved: dict[str, bytes] = {}
    if (OUT / "data").exists():
        preserved = {f.name: f.read_bytes() for f in (OUT / "data").glob("*.rete")}

    manifest = json.loads((SRC / "plaza.json").read_text(encoding="utf-8"))
    bundled = [
        pathlib.PurePosixPath(ds["rete"]).name
        for ds in manifest.get("datasets", [])
        if ds.get("rete", "").startswith("../../web/")
    ]

    # Pre-flight BEFORE the wipe below, so a tree that cannot produce a complete
    # gallery is left exactly as it was found rather than half-rebuilt. A tile
    # with no card file would ship pointing at ../../web/<x>.rete, which
    # resolves outside the Pages root and 404s — that used to be a printed
    # warning, which is how a 28 KB card silently became a 6 KB stub.
    missing = [b for b in bundled if not (WEB / b).exists() and b not in preserved]
    if missing:
        die(
            "no card file for: "
            + ", ".join(missing)
            + f"\n  Looked in {WEB}/ (a fresh build) and {OUT / 'data'}/ (the committed cards)."
            + "\n  Build them into web/ first — or restore docs/plaza/data/ with"
            + " `git checkout -- docs/plaza/data`. Do NOT fetch them from"
            + " data.graphplaza.com: the bucket holds older, smaller builds."
        )

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
    #    `rete` paths, and fix doc links. Remote URLs are left as-is. The
    #    pre-flight above guarantees every bundled card resolves to one of the
    #    two branches here.
    copied = []
    kept = []
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
                # No fresh build to copy — keep the committed card verbatim.
                (OUT / "data" / base).write_bytes(preserved[base])
                ds["rete"] = "data/" + base
                kept.append(base)
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
    print(
        f"build_plaza: wrote {OUT} — {len(copied)} card files copied,"
        f" {len(kept)} kept from the committed set, {total // 1024} KiB total"
    )
    if copied:
        print(f"  copied from web/: {', '.join(copied)}")
    if kept:
        print(f"  kept as committed: {', '.join(kept)}")


if __name__ == "__main__":
    main()
