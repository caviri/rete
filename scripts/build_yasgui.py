#!/usr/bin/env python3
"""Build the self-contained yasgui-wasm SPARQL IDE → docs/yasgui.html.

A single static `.html` — a Yasgui-style SPARQL IDE (tabs, CodeMirror editor,
Table/Response result views, share links) where the *endpoint* is a `.rete`
file: a pasted URL (queried lazily over HTTP range by the wasm engine) or a
local file opened/dropped into the page. Bundles:

  - the CodeMirror 6 IIFE the playground uses (web/playground-src/cm6.bundle.js),
  - the wasm-bindgen *no-modules* rete glue (global ``wasm_bindgen``), inlined
    verbatim — and reused as the first half of the Blob-spawned worker,
  - the rete ``.wasm`` engine, base64-embedded,
  - the worker body (web/yasgui-src/worker.js) in a ``text/plain`` script tag,
  - the app (web/yasgui-src/app.js) + a curated catalog of published datasets.

No dataset is embedded: remote files are read over HTTP range, local files
stay in browser memory.

Prereqs (via Docker, same as the playground / explorer):
  wasm-pack build crates/rete-wasm --target no-modules --out-dir ../../web/pkg-nomodules --no-opt

Usage:
  python scripts/build_yasgui.py [--out docs/yasgui.html]
"""
import argparse
import base64
import datetime
import json
import pathlib
import subprocess
import sys

ROOT = pathlib.Path(__file__).resolve().parent.parent
WEB = ROOT / "web"
NOMOD = WEB / "pkg-nomodules"
GLUE_JS = NOMOD / "rete_wasm.js"
WASM = NOMOD / "rete_wasm_bg.wasm"
CM6_JS = WEB / "playground-src" / "cm6.bundle.js"
SRC = WEB / "yasgui-src"
APP_JS = SRC / "app.js"
WORKER_JS = SRC / "worker.js"
TEMPLATE = WEB / "yasgui.template.html"
DEFAULT_OUT = ROOT / "docs" / "yasgui.html"

R2 = "https://data.graphplaza.com"

# Curated endpoint picker: published remote-lazy datasets that make good first
# queries. `query` (optional) replaces the editor's default template when the
# dataset is picked; keep every crafted query cheap on a lazy remote file.
CATALOG = [
    {
        "name": "getty-ulan — who taught whom",
        "url": f"{R2}/getty-ulan/getty-ulan.rete",
        "size": "205k triples",
        "blurb": "Getty ULAN artist lineage: 28,300 artists, gvp:teacherOf master→pupil edges, names + one-line bios. Try Rembrandt's pupils.",
        "query": (
            "PREFIX gvp: <http://vocab.getty.edu/ontology#>\n"
            "PREFIX skos: <http://www.w3.org/2004/02/skos/core#>\n"
            "# Rembrandt van Rijn (ULAN 500011051) taught…\n"
            "SELECT ?pupil ?name WHERE {\n"
            "  <http://vocab.getty.edu/ulan/500011051> gvp:teacherOf ?pupil .\n"
            "  ?pupil skos:prefLabel ?name .\n"
            "}\n"
        ),
    },
    {
        "name": "boe — Spanish law in force",
        "url": f"{R2}/boe/boe.rete",
        "size": "465k triples",
        "blurb": "BOE Legislación Consolidada as an ELI graph: 12,330 laws with repeals/amendments/citations. Toggle 🧠 reason for OWL 2 QL entailment.",
        "query": (
            "SELECT ?p (COUNT(*) AS ?n) WHERE {\n"
            "  ?s ?p ?o .\n"
            "} GROUP BY ?p ORDER BY DESC(?n) LIMIT 20\n"
        ),
    },
    {
        "name": "scrolls — Herculaneum papyri",
        "url": f"{R2}/scrolls/scrolls.rete",
        "size": "18k triples",
        "blurb": "Vesuvius Challenge scroll data: carbonized Herculaneum scrolls, their segments and scan assets.",
    },
    {
        "name": "worldcup — FIFA 2022 KG",
        "url": f"{R2}/worldcup/worldcup.rete",
        "size": "8k triples",
        "blurb": "All 64 matches, goals, stadiums and player careers of the 2022 World Cup, with multi-source predictions as PROV.",
    },
    {
        "name": "mtg — Magic: The Gathering",
        "url": f"{R2}/mtg/mtg.rete",
        "size": "920k triples",
        "blurb": "34,633 MTG cards with Scryfall imagery, sets, rulings and Oracle text.",
    },
    {
        "name": "lineara — Linear A corpus",
        "url": f"{R2}/lineara/lineara.rete",
        "size": "1,721 inscriptions",
        "blurb": "The undeciphered Bronze-Age script: inscriptions linked to signs and words (SigLA).",
    },
    {
        "name": "nidm — neuroimaging cohort",
        "url": f"{R2}/nidm/nidm.rete",
        "size": "3.2k triples",
        "blurb": "A real 272-subject cohort (OpenNeuro ds000030) in the Neuroimaging Data Model, with full W3C PROV provenance.",
    },
    {
        "name": "peirce — C.S. Peirce papers",
        "url": f"{R2}/peirce/peirce.rete",
        "size": "36k triples",
        "blurb": "The Charles S. Peirce papers at Houghton Library: manuscript hierarchy, correspondence, IIIF scans.",
    },
    {
        "name": "subtitles — one film, 20 languages",
        "url": f"{R2}/subtitles/tears_of_steel.rete",
        "size": "14k triples",
        "blurb": "Tears of Steel (CC BY) subtitles as a temporal graph: every line in 20 languages, aligned on the timeline.",
    },
    {
        "name": "wikidata-100MB — a bigger bite",
        "url": f"{R2}/wikidata-100MB/wikidata.rete",
        "size": "100 MB file",
        "blurb": "A 100 MB Wikidata slice — watch the stats line: a pointed query touches kilobytes of it.",
    },
]


def die(msg: str) -> None:
    print(f"build_yasgui: {msg}", file=sys.stderr)
    sys.exit(1)


def b64(path: pathlib.Path) -> str:
    return base64.b64encode(path.read_bytes()).decode("ascii")


def main() -> None:
    ap = argparse.ArgumentParser()
    ap.add_argument("--out", default=str(DEFAULT_OUT))
    args = ap.parse_args()

    for p in (TEMPLATE, APP_JS, WORKER_JS, CM6_JS, GLUE_JS, WASM):
        if not p.exists():
            die(f"missing required input: {p}")

    chunks = {
        "__CM6_JS__": CM6_JS.read_text(encoding="utf-8").rstrip(),
        "__GLUE_JS__": GLUE_JS.read_text(encoding="utf-8").rstrip(),
        "__WORKER_JS__": WORKER_JS.read_text(encoding="utf-8").rstrip(),
    }
    # Inlined scripts must not close their own <script> tag.
    for name, text in chunks.items():
        if "</script" in text:
            die(f"{name} contains a literal </script> — cannot inline")

    try:
        sha = subprocess.run(
            ["git", "rev-parse", "--short=12", "HEAD"],
            cwd=ROOT, capture_output=True, text=True, check=True,
        ).stdout.strip()
    except Exception:
        sha = "unknown"
    stamp = f"Built {datetime.date.today().isoformat()} from {sha}."

    html = TEMPLATE.read_text(encoding="utf-8")
    for ph, text in chunks.items():
        html = html.replace(ph, text)
    html = (
        html
        .replace("__WASM_B64__", b64(WASM))
        .replace("__CATALOG_JSON__",
                 json.dumps(CATALOG, ensure_ascii=False, separators=(",", ":")))
        .replace("__BUILD_STAMP__", stamp)
        # app.js last: it must not contain the other placeholders
        .replace("__APP_JS__", APP_JS.read_text(encoding="utf-8").rstrip())
    )

    for ph in ("__CM6_JS__", "__GLUE_JS__", "__WORKER_JS__", "__WASM_B64__",
               "__CATALOG_JSON__", "__BUILD_STAMP__", "__APP_JS__"):
        if ph in html:
            die(f"unreplaced template placeholder: {ph}")

    out = pathlib.Path(args.out)
    out.parent.mkdir(parents=True, exist_ok=True)
    out.write_text(html, encoding="utf-8")
    print(f"build_yasgui: wrote {out}")
    print(f"  rete wasm: {WASM.stat().st_size:>9,} bytes")
    print(f"  cm6:       {CM6_JS.stat().st_size:>9,} bytes")
    print(f"  output:    {out.stat().st_size:>9,} bytes")


if __name__ == "__main__":
    main()
