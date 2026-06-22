#!/usr/bin/env python3
"""Generate a self-contained, static playground at docs/playground.html.

The output runs entirely from WebAssembly with no server and no app-load network
requests:
- the wasm-bindgen *no-modules* glue (a classic script exposing a global
  ``wasm_bindgen``) is inlined verbatim,
- the ``.wasm`` binary is embedded as base64 and handed to the initializer as
  bytes (so it never ``fetch``es), and
- the example ``.rete`` datasets are embedded as a base64 map.

The page still includes a user-initiated URL loader for custom `.rete` files.

Prerequisites (all via Docker, see CLAUDE.md / docs):
  wasm-pack build crates/rete-wasm --target no-modules --out-dir ../../web/pkg-nomodules
  rete build examples/<x>.nt -o web/<x>.rete      # for each dataset below

Run (deterministic):
  uv run python scripts/build_playground.py
"""

import base64
import json
import pathlib
import sys

ROOT = pathlib.Path(__file__).resolve().parent.parent
WEB = ROOT / "web"
SRC = WEB / "playground-src"
NOMOD = WEB / "pkg-nomodules"
TEMPLATE = WEB / "playground.template.html"
OUT = ROOT / "docs" / "playground.html"
# The Wikidata-100MB lazy explorer: a self-contained static page (inlines the
# wasm glue + binary; rete worker built from a blob; DuckDB-WASM from CDN).
# Rendered into both docs/ (the published site) and web/ (local serving, so the
# index.html link works there too).
EXPLORE_TEMPLATE = WEB / "explore-100mb.template.html"
EXPLORE_OUTS = (ROOT / "docs" / "explore-100mb.html", WEB / "explore-100mb.html")

GLUE_JS = NOMOD / "rete_wasm.js"
WASM = NOMOD / "rete_wasm_bg.wasm"
CSS = SRC / "styles.css"
CATALOG_JS = SRC / "catalog.js"
CM6_JS = SRC / "cm6.bundle.js"  # bundled CodeMirror 6 (see cm6/README / package.json)
EDITOR_JS = SRC / "editor.js"
APP_JS = SRC / "app.js"

# Datasets to embed: (playground key, built .rete file under web/).
# `citations` is the real OpenCitations network (citations of the AlphaFold paper,
# 10.1038/s41586-021-03819-2) enriched with clearly-labelled synthetic metadata;
# real DOIs / edges / years, fabricated titles/authors/venues/keywords.
# The scholar datasets come from scripts/synth_graph.py; regenerate with:
#   python3 scripts/synth_graph.py --papers 250 --seed 42 -o /tmp/scholar.nt
#   python3 scripts/synth_graph.py --papers 250 --noise 0.25 --seed 42 -o /tmp/scholar-noisy.nt
#   rete build /tmp/scholar.nt -o web/scholar.rete
#   rete build /tmp/scholar-noisy.nt -o web/scholar-noisy.rete
# (changing size/noise/seed invalidates the IRIs and counts pinned in
# web/playground-src/catalog.js — update them together).
DATASETS = [
    ("scholar", "scholar.rete"),
    ("scholar-noisy", "scholar-noisy.rete"),
    ("typed", "typed.rete"),
    ("deps", "deps.rete"),
    # A tiny causal ontology with planted coherence defects — powers the Coherence
    # tab demo (rete build examples/causal.nt -o web/causal.rete).
    ("causal", "causal.rete"),
    ("citations", "enriched-all.rete"),
    # Historical world borders (aourednik/historical-basemaps, GPL-3.0) as
    # GeoSPARQL geometry + time — built by scripts/geo_to_rete.py:
    #   python3 scripts/geo_to_rete.py basemaps \
    #     --years bc323,1000,1492,1815,1914,1945,1994 --prec 2 --min-bbox 0.3 \
    #     --max-per-year 90 -o dev/geo/history.nt
    #   rete build dev/geo/history.nt -o web/history.rete
    ("history", "history.rete"),
    # Real-world knowledge graphs ingested for the playground (subgraphs built by the
    # fetch recipes in scripts/; see web/playground-src/catalog.js for the example queries):
    ("linked-jazz", "linked-jazz.rete"),     # jazz musician social network (Linked Jazz, CC BY-SA)
    ("nomisma", "nomisma.rete"),             # coinage of Alexander the Great (Nomisma PELLA, CC-BY)
    ("mimotext", "mimotext.rete"),           # French Enlightenment novels + stylometry (MiMoText, CC0)
    ("mmm", "mmm.rete"),                     # medieval manuscript provenance (Mapping Manuscript Migrations)
    ("openalex-astrocytes", "openalex-astrocytes.rete"),  # astrocyte research citation graph (OpenAlex, CC0)
    ("antarctic-expeditions", "antarctic-expeditions.rete"),  # Heroic-Age expeditions, crews & ships (Wikidata, CC0)
    ("factgrid-illuminati", "factgrid-illuminati.rete"),
    ("theographic-graph", "theographic-graph.rete"),
    ("monarch", "monarch.rete"),
    ("opencitations", "opencitations.rete"),
    ("orkg", "orkg.rete"),
    # getty-ulan is remote-lazy (2.96 MB, in the bucket) — not embedded; see catalog.js.
]


def die(msg: str) -> None:
    print(f"build_playground: error: {msg}", file=sys.stderr)
    sys.exit(1)


def b64(path: pathlib.Path) -> str:
    return base64.b64encode(path.read_bytes()).decode("ascii")


def main() -> None:
    for p in (TEMPLATE, CSS, CATALOG_JS, CM6_JS, EDITOR_JS, APP_JS, GLUE_JS, WASM):
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
        .replace("__PLAYGROUND_CSS__", CSS.read_text(encoding="utf-8").rstrip())
        .replace(
            "__PLAYGROUND_CATALOG_JS__",
            CATALOG_JS.read_text(encoding="utf-8").rstrip(),
        )
        .replace(
            "__PLAYGROUND_CM6_JS__",
            CM6_JS.read_text(encoding="utf-8").rstrip(),
        )
        .replace(
            "__PLAYGROUND_EDITOR_JS__",
            EDITOR_JS.read_text(encoding="utf-8").rstrip(),
        )
        .replace("__PLAYGROUND_APP_JS__", APP_JS.read_text(encoding="utf-8").rstrip())
    )
    placeholders = (
        "__GLUE_JS__",
        "__WASM_B64__",
        "__DATASETS_B64__",
        "__PLAYGROUND_CSS__",
        "__PLAYGROUND_CATALOG_JS__",
        "__PLAYGROUND_CM6_JS__",
        "__PLAYGROUND_EDITOR_JS__",
        "__PLAYGROUND_APP_JS__",
    )
    missing = [p for p in placeholders if p in html]
    if missing:
        die("unreplaced template placeholder(s): " + ", ".join(missing))

    OUT.parent.mkdir(parents=True, exist_ok=True)
    OUT.write_text(html, encoding="utf-8")

    sizes = ", ".join(f"{n}={len(datasets_b64[n])}b64" for n, _ in DATASETS)
    print(f"build_playground: wrote {OUT}")
    print(f"  wasm: {WASM.stat().st_size} bytes -> {len(wasm_b64)} base64 chars")
    print(f"  datasets: {sizes}")
    print(f"  output: {OUT.stat().st_size} bytes")

    # The lazy explorer: inline the same glue + wasm + shared widgets module.
    if EXPLORE_TEMPLATE.exists():
        widgets = (SRC / "widgets.js").read_text(encoding="utf-8").rstrip()
        ex = (
            EXPLORE_TEMPLATE.read_text(encoding="utf-8")
            .replace("__GLUE_JS__", glue)
            .replace("__WASM_B64__", wasm_b64)
            .replace("__WIDGETS_JS__", widgets)
        )
        leftover = [p for p in ("__GLUE_JS__", "__WASM_B64__", "__WIDGETS_JS__") if p in ex]
        if leftover:
            die("unreplaced explore placeholder(s): " + ", ".join(leftover))
        # The COI service worker must sit beside the explorer (same origin) so
        # the page can register it to gain cross-origin isolation (→ parallel
        # range reads). Copy it next to each explorer output.
        coi_src = WEB / "coi-serviceworker.js"
        coi_text = coi_src.read_text(encoding="utf-8") if coi_src.exists() else None
        for out in EXPLORE_OUTS:
            out.write_text(ex, encoding="utf-8")
            print(f"  explorer: wrote {out} ({out.stat().st_size} bytes)")
            if coi_text is not None:
                coi_out = out.parent / "coi-serviceworker.js"
                coi_out.write_text(coi_text, encoding="utf-8")
                print(f"  explorer: wrote {coi_out}")


if __name__ == "__main__":
    main()
