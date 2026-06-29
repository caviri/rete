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
import re
import sys

# Patch spliced into the asyncified wasm-bindgen glue in place of the
# `const import1 = require("env");` line (which throws in a browser). It defines
# the two async imports (env.rete_fetch_ranges = Promise.all of fetch;
# env.rete_file_len = a bytes=0-0 length probe), the Asyncify suspend/rewind
# driver `reteDrive`, and `reteOpenRemote` (opens a resident RemoteGraph by
# driving the raw constructor and wrapping the pointer ONCE after rewind — so the
# unwind pass never registers a garbage instance with the FinalizationRegistry).
# Sits inside the glue closure, so it can use wasm/passStringToWasm0/RemoteGraph/
# takeObject/WASM_VECTOR_LEN directly. Proven end-to-end in dev/asyncify-e2e.cjs.
ASYNC_ENV_JS = """
        // ---- injected: Asyncify env imports + driver (replaces require("env")) ----
        let __reteAD = 0, __retePending = null, __reteSleeping = false, __reteRes = 0;
        function __reteStack() {
          if (!__reteAD) {
            const SIZE = 16 << 20; // 16 MiB Asyncify stack — the engine's recursive eval is deep
            __reteAD = wasm.__wbindgen_malloc(8 + SIZE, 8);
            const d = new DataView(wasm.memory.buffer);
            d.setInt32(__reteAD, __reteAD + 8, true);
            d.setInt32(__reteAD + 4, __reteAD + 8 + SIZE, true);
          }
        }
        function __reteStr(ptr, len) { return new TextDecoder().decode(new Uint8Array(wasm.memory.buffer).slice(ptr, ptr + len)); }
        async function __reteDoFetch(urlPtr, urlLen, offsPtr, lensPtr, n, dstPtr) {
          const url = __reteStr(urlPtr, urlLen);
          const dv = new DataView(wasm.memory.buffer);
          const ranges = [];
          for (let i = 0; i < n; i++) ranges.push([Number(dv.getBigUint64(offsPtr + i*8, true)), dv.getUint32(lensPtr + i*4, true)]);
          const bufs = await Promise.all(ranges.map(([o,l]) =>
            fetch(url, { headers: { Range: 'bytes=' + o + '-' + (o+l-1) } })
              .then((r) => { if (r.status !== 206) throw new Error('Range status ' + r.status + ' (host must support HTTP range)'); return r.arrayBuffer(); })
              .then((b) => new Uint8Array(b))));
          const mem = new Uint8Array(wasm.memory.buffer);
          let pos = dstPtr, total = 0;
          for (const b of bufs) { mem.set(b, pos); pos += b.length; total += b.length; }
          return total;
        }
        async function __reteDoLen(urlPtr, urlLen, outPtr) {
          const url = __reteStr(urlPtr, urlLen);
          const r = await fetch(url, { headers: { Range: 'bytes=0-0' } });
          const cr = r.headers.get('content-range');
          const total = cr ? Number(cr.split('/')[1]) : Number(r.headers.get('content-length') || 0);
          new DataView(wasm.memory.buffer).setBigUint64(outPtr, BigInt(total || 0), true);
          return total > 0 ? 1 : 0;
        }
        function __reteSuspend(makePromise) {
          if (!__reteSleeping) { __retePending = makePromise(); wasm.asyncify_start_unwind(__reteAD); __reteSleeping = true; return 0; }
          wasm.asyncify_stop_rewind(); __reteSleeping = false; return __reteRes;
        }
        function __reteFetchRanges(urlPtr, urlLen, offsPtr, lensPtr, n, dstPtr) { return __reteSuspend(() => __reteDoFetch(urlPtr, urlLen, offsPtr, lensPtr, n, dstPtr)); }
        function __reteFileLen(urlPtr, urlLen, outPtr) { return __reteSuspend(() => __reteDoLen(urlPtr, urlLen, outPtr)); }
        async function __reteDrive(thunk) {
          __reteStack();
          let r = thunk();
          while (wasm.asyncify_get_state() === 1) {
            wasm.asyncify_stop_unwind();
            __reteRes = await __retePending;
            wasm.asyncify_start_rewind(__reteAD);
            r = thunk();
          }
          return r;
        }
        exports.reteDrive = __reteDrive;
        exports.reteOpenRemote = async function (url) {
          __reteStack();
          const ptr0 = passStringToWasm0(url, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
          const len0 = WASM_VECTOR_LEN;
          let ret = wasm.remotegraph_new(ptr0, len0);
          while (wasm.asyncify_get_state() === 1) {
            wasm.asyncify_stop_unwind();
            __reteRes = await __retePending;
            wasm.asyncify_start_rewind(__reteAD);
            ret = wasm.remotegraph_new(ptr0, len0);
          }
          if (ret[2]) throw takeObject(ret[1]);
          const g = Object.create(RemoteGraph.prototype);
          g.__wbg_ptr = ret[0];
          RemoteGraphFinalization.register(g, g.__wbg_ptr, g);
          return g;
        };
        const import1 = { rete_fetch_ranges: __reteFetchRanges, rete_file_len: __reteFileLen };
"""

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
RDFCONV_JS = SRC / "rdfconv.js"  # in-browser JSON-LD / RDF-XML -> N-Triples converters
APP_JS = SRC / "app.js"

# Datasets to embed: (playground key, built .rete file under web/).
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
    # A tiny causal ontology with planted coherence defects — powers the Coherence
    # tab demo (rete build examples/causal.nt -o web/causal.rete).
    ("causal", "causal.rete"),
    # Real-world knowledge graphs ingested for the playground (subgraphs built by the
    # fetch recipes in scripts/; see web/playground-src/catalog.js for the example queries):
    ("linked-jazz", "linked-jazz.rete"),     # jazz musician social network (Linked Jazz, CC BY-SA)
    ("nomisma", "nomisma.rete"),             # coinage of Alexander the Great (Nomisma PELLA, CC-BY)
    ("mimotext", "mimotext.rete"),           # French Enlightenment novels + stylometry (MiMoText, CC0)
    ("openalex-astrocytes", "openalex-astrocytes.rete"),  # astrocyte research citation graph (OpenAlex, CC0)
    ("antarctic-expeditions", "antarctic-expeditions.rete"),  # Heroic-Age expeditions, crews & ships (Wikidata, CC0)
    ("theographic-graph", "theographic-graph.rete"),
    ("monarch", "monarch.rete"),
    ("opencitations", "opencitations.rete"),
    # MIrA — early Irish manuscripts, Wikidata-aligned, with 189 IIIF manifests
    # (scripts/mira_to_nt.py enriches padraicmoran/MIrA's RDF; CC BY-NC-SA 4.0).
    ("mira", "mira.rete"),
    # MIrA↔Wikidata mappings as a shareable SSSOM linkset (scripts/mira_sssom.py) —
    # skos:exactMatch + provenance; federates in to bridge MIrA and Wikidata.
    ("mira-wikidata", "mira-wikidata.rete"),
    # causalgraph: the Fraunhofer IWU causal-graph ontology (OWL→NT via owlready2)
    # + an example Industry-4.0 causal model (scripts/causalgraph_example.py). MIT.
    ("causalgraph", "causalgraph.rete"),
    # Linear A — the complete undeciphered Minoan corpus (1,721 inscriptions linked
    # through their signs & word-sequences). scripts/lineara_to_nt.js from the
    # mwenge/lineara.xyz LinearA Explorer (GORILA/Douros text; images © EFA excluded).
    ("lineara", "lineara.rete"),
    # Remote-lazy (served from the bucket, NOT embedded; see catalog.js): getty-ulan,
    # history, mmm, orkg, factgrid-illuminati, wikidata-1GB/100mb, ohm-full, chemotion,
    # chebi-full, causenet-full. Dropped entirely: citations (synthetic), typed, deps.
]


def die(msg: str) -> None:
    print(f"build_playground: error: {msg}", file=sys.stderr)
    sys.exit(1)


def b64(path: pathlib.Path) -> str:
    return base64.b64encode(path.read_bytes()).decode("ascii")


def main() -> None:
    for p in (TEMPLATE, CSS, CATALOG_JS, CM6_JS, EDITOR_JS, RDFCONV_JS, APP_JS, GLUE_JS, WASM):
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
        .replace(
            "__PLAYGROUND_RDFCONV_JS__",
            RDFCONV_JS.read_text(encoding="utf-8").rstrip(),
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
        "__PLAYGROUND_RDFCONV_JS__",
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

    # The asyncified wasm variant (opt-in "Concurrent reads" toggle): a separate
    # glue + .wasm beside the page, fetched only when the toggle is on (so the
    # default page stays unbloated). Built by scripts/build_playground_async.sh.
    nomod_async = WEB / "pkg-nomodules-async"
    aglue_path = nomod_async / "rete_wasm.js"
    awasm_path = nomod_async / "rete_wasm_bg.wasm"
    if aglue_path.exists() and awasm_path.exists():
        aglue = aglue_path.read_text(encoding="utf-8")
        if 'const import1 = require("env");' not in aglue:
            die('async glue missing the require("env") anchor — rebuild web/pkg-nomodules-async')
        aglue = aglue.replace('const import1 = require("env");', ASYNC_ENV_JS, 1)
        # wasm-bindgen emits one `const importN = require("env")` per env import fn;
        # the imports map only uses import1, so alias the rest to it.
        aglue = re.sub(r'const import(\d+) = require\("env"\);', r"const import\1 = import1;", aglue)
        if fetch_line in aglue:
            aglue = aglue.replace(fetch_line, "            throw new Error('async variant: pass bytes, not a URL');")
        ajs = OUT.parent / "rete_wasm_async.js"
        ajs.write_text(aglue, encoding="utf-8")
        (OUT.parent / "rete_wasm_async.wasm").write_bytes(awasm_path.read_bytes())
        print(f"  async: wrote {ajs} + rete_wasm_async.wasm ({awasm_path.stat().st_size} bytes)")
    else:
        print("  async: web/pkg-nomodules-async not found — Concurrent-reads toggle inert "
              "(run scripts/build_playground_async.sh)")


if __name__ == "__main__":
    main()
