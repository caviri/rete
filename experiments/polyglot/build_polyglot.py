#!/usr/bin/env python3
"""
Turn a .rete graph into a self-querying HTML file, two ways:

  --mode polyglot  (default):  [ HTML + inlined wasm engine ][ raw .rete bytes ]
      One object that is BOTH a web page and a real .rete. Serve it from R2 with
      Content-Type: text/html — a browser renders it and the page range-reads its
      OWN appended tail to open the graph. Output: a .rete file.

  --mode embed:  a single self-contained .html with the graph base64-embedded.
      No server, no fetch, no range requests — the page decodes the graph from
      itself in memory. FULLY PORTABLE: double-click it, works offline, email it.
      Output: an .html file (bigger, since base64 inflates the bytes ~33%).

Usage:
  # polyglot object for R2 (served as text/html):
  python experiments/polyglot/build_polyglot.py --mode polyglot \
      --rete data/swissubase/swissubase-demo.rete \
      --out  data/swissubase/swissubase.polyglot-demo.rete

  # portable, double-click, offline single file:
  python experiments/polyglot/build_polyglot.py --mode embed \
      --rete data/swissubase/swissubase-demo.rete \
      --out  data/swissubase/swissubase.portable.html
"""
import argparse
import base64
import os

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.abspath(os.path.join(HERE, "..", ".."))

TEMPLATE = os.path.join(HERE, "explorer.template.html")
GLUE = os.path.join(ROOT, "web", "pkg-nomodules", "rete_wasm.js")
WASM = os.path.join(ROOT, "web", "pkg-nomodules", "rete_wasm_bg.wasm")
TOKEN = "__OFFSET12__"  # exactly 12 chars; -> 12-digit offset (length-stable)

# polyglot: the page reads its own appended tail over an HTTP Range request.
POLYGLOT_SRC = (
    'const TAIL_OFFSET = parseInt("' + TOKEN + '", 10);\n'
    'async function getGraphBytes(){\n'
    '  const r = await fetch(location.href, {headers:{Range:`bytes=${TAIL_OFFSET}-`}});\n'
    '  if(!r.ok && r.status!==206) throw new Error("could not read own tail: HTTP "+r.status);\n'
    '  return new Uint8Array(await r.arrayBuffer());\n'
    '}\n'
    'const SRC_LABEL = "tail read from byte "+TAIL_OFFSET.toLocaleString();'
)
POLYGLOT_FOOTER = ("It is served as <code>text/html</code>, and the real "
                   "<code>.rete</code> is appended after <code>&lt;/html&gt;</code>; "
                   "the page range-reads its own tail to open it.")

# embed: the graph is base64 in the file; decode it in memory (no fetch).
EMBED_SRC_HEAD = 'const RETE_B64 = "'
EMBED_SRC_TAIL = (
    '";\n'
    'async function getGraphBytes(){\n'
    '  const bin = atob(RETE_B64);\n'
    '  const arr = new Uint8Array(bin.length);\n'
    '  for(let i=0;i<bin.length;i++) arr[i]=bin.charCodeAt(i);\n'
    '  return arr;\n'
    '}\n'
    'const SRC_LABEL = "graph embedded in this file";'
)
EMBED_FOOTER = ("The whole graph is base64-embedded inside this single "
                "<code>.html</code>, so it opens by double-click, fully offline.")


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--rete", required=True)
    ap.add_argument("--out", required=True)
    ap.add_argument("--mode", choices=["polyglot", "embed"], default="polyglot")
    ap.add_argument("--template", default=TEMPLATE)
    args = ap.parse_args()

    html = open(args.template, encoding="utf-8").read()
    glue = open(GLUE, encoding="utf-8").read()
    wasm_b64 = base64.b64encode(open(WASM, "rb").read()).decode("ascii")
    rete = open(args.rete, "rb").read()
    assert rete[:4] == b"RETE", "input is not a .rete file"

    html = html.replace("__GLUE_JS__", glue).replace("__WASM_B64__", wasm_b64)

    if args.mode == "embed":
        rete_b64 = base64.b64encode(rete).decode("ascii")
        graph_src = EMBED_SRC_HEAD + rete_b64 + EMBED_SRC_TAIL
        html = html.replace("__GRAPH_SOURCE_JS__", graph_src)
        html = html.replace("__FOOTER_NOTE__", EMBED_FOOTER)
        assert TOKEN not in html, "embed mode must not carry the offset token"
        with open(args.out, "w", encoding="utf-8") as f:
            f.write(html)
        size = len(html.encode("utf-8"))
        print(f"wrote {args.out}  (portable single-file HTML, mode=embed)")
        print(f"  {size:,} bytes  ·  graph {len(rete):,} B -> {len(rete_b64):,} b64 chars")
        print("  double-click it in a browser — no server, works offline.")
        return

    # polyglot
    html = html.replace("__GRAPH_SOURCE_JS__", POLYGLOT_SRC)
    html = html.replace("__FOOTER_NOTE__", POLYGLOT_FOOTER)
    assert html.count(TOKEN) == 1, f"{TOKEN} must appear exactly once"
    # Wrap the appended .rete inside an inert <script type="text/plain"> element:
    # served as text/html, the browser would otherwise parse all N MB of binary
    # into a pathological DOM (freezing the tab). In "script data" state the whole
    # tail is consumed as ONE text token — never rendered, never executed — while
    # the page's own byte-range read of the tail is unaffected. The closing
    # </script> never appears in the binary, so the element runs to EOF.
    html += '\n<script type="text/plain" id="rete-tail">\n'
    offset = len(html.encode("utf-8"))  # tail begins right after the wrapper opener
    assert offset < 10**12
    html = html.replace(TOKEN, str(offset).zfill(12))  # 12 -> 12 chars, length-stable
    html_bytes = html.encode("utf-8")
    assert len(html_bytes) == offset, "offset drift — token not length-stable"

    with open(args.out, "wb") as f:
        f.write(html_bytes)
        f.write(rete)
    with open(args.out, "rb") as f:
        f.seek(offset)
        assert f.read(4) == b"RETE", "tail magic not at the patched offset"
    print(f"wrote {args.out}  (mode=polyglot; upload to R2 with Content-Type text/html)")
    print(f"  html+engine: {len(html_bytes):,} bytes  ·  .rete tail: {len(rete):,} B @ offset {offset:,}")
    print(f"  total: {len(html_bytes)+len(rete):,} bytes  ·  verified RETE magic at offset")


if __name__ == "__main__":
    main()
