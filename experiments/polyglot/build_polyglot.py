#!/usr/bin/env python3
"""
Turn a .rete graph into a self-querying HTML file, two ways:

  --mode polyglot  (default):  [ HTML + inlined wasm engine ][ raw .rete bytes ]
      One object that is BOTH a web page and a real .rete. Serve it from R2 with
      Content-Type: text/html — a browser renders it and the page opens its OWN
      appended tail LAZILY over HTTP range requests: it aborts the rest of its
      own document load, then range-reads only the bytes each query touches.
      Output: a .rete file.

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

# --- the polyglot base marker -------------------------------------------------
# THE contract between this builder and the reader. Keep in lockstep with
# `rete_core::reader`:
#     POLYGLOT_MARKER = b"RETE-BASE:"      POLYGLOT_DIGITS = 16
# `detect_polyglot_base` scans a resource's first HEADER_LEN bytes for the marker
# and parses the fixed-width decimal that follows; that is how the wasm engine
# (and any other reader) finds the .rete inside a file whose byte 0 is `<`.
#
# Fixed width is not cosmetic: the offset can only be computed once the HTML is
# final, so it must be patched into bytes that are ALREADY there — a variable
# width would move the very bytes it describes. TOKEN is therefore exactly
# POLYGLOT_DIGITS characters and substituting it cannot shift a single byte.
#
# This agreement went untested and the two halves silently disagreed: the builder
# emitted a bare 12-digit number 4 MB into the file and `detect_polyglot_base`
# returned None for every polyglot this repo could produce.
# `crates/rete-core/tests/polyglot_roundtrip.rs` now round-trips a file this
# script writes through the real reader, so drift is a test failure.
MARKER = "RETE-BASE:"
POLYGLOT_DIGITS = 16
TOKEN = "__RETE_BASE_16__"
HEADER_LEN = 1024  # rete_core::header::HEADER_LEN — the window a reader reads
assert len(TOKEN) == POLYGLOT_DIGITS, "the offset token must be length-stable"

# The marker rides in an HTML comment near byte 0: browsers ignore it, and it has
# to land inside the first HEADER_LEN bytes because that is all a reader fetches
# before it must know where the graph starts.
MARKER_COMMENT = "<!-- " + MARKER + TOKEN + " -->"


def detect_polyglot_base(head):
    """Mirror of `rete_core::reader::detect_polyglot_base` (keep in lockstep).

    Used below to re-read the file this script just wrote with the reader's own
    algorithm — a builder that cannot find its own marker must not ship.
    """
    pos = head.find(MARKER.encode("ascii"))
    if pos < 0:
        return None
    start = pos + len(MARKER)
    digits = head[start:start + POLYGLOT_DIGITS]
    if len(digits) != POLYGLOT_DIGITS or not digits.isdigit():
        return None
    return int(digits)


# polyglot: the page range-reads its own appended tail through the wasm engine.
#
# It does NOT download the tail. Two things make that true:
#   1. `window.stop()` — served as text/html, the browser was streaming the whole
#      object (graph included) just to render the page. The moment our script
#      runs — at the tail boundary, right after the engine — we abort the rest of
#      that document load.
#   2. `RemoteGraph(location.href)` in a worker — the engine finds this file's
#      `RETE-BASE:` marker, wraps its reader in an `OffsetReader`, and faults in
#      only the dictionary chunks and index tiles each query actually touches.
#      Synchronous range XHR is worker-only, hence the inline blob worker.
POLYGLOT_SRC = r"""
const TAIL_OFFSET = parseInt("__TOKEN__", 10);
try { window.stop(); } catch (e) {}   // stop downloading the graph as page content
const SRC_LABEL = "range-read from byte " + TAIL_OFFSET.toLocaleString();

function reteWorker(){
  // The engine glue is inlined in this page as inert text (type=text/plain), so
  // the worker is built from the page's own bytes — no second network request.
  const glue = document.getElementById("rete-glue").textContent;
  const boot = [
    'self.onmessage = async (e) => {',
    '  const m = e.data;',
    '  try {',
    '    if (m.t === "open") {',
    '      await wasm_bindgen({module_or_path: m.wasm});',
    '      self.__g = new wasm_bindgen.RemoteGraph(m.url);',
    '      const c = self.__g.card();',
    '      self.postMessage({id:m.id, ok:true, v:{info: JSON.parse(self.__g.info()),',
    '        card: c ? JSON.parse(c) : null, stats: JSON.parse(self.__g.stats())}});',
    '    } else if (m.t === "query") {',
    '      const g = self.__g;',
    '      const j = m.reasoned ? g.query_reasoned(m.q, "json") : g.query(m.q, "json");',
    '      self.postMessage({id:m.id, ok:true, v:{result: JSON.parse(j), stats: JSON.parse(g.stats())}});',
    '    }',
    '  } catch (err) {',
    '    self.postMessage({id:m.id, ok:false, error: String((err && err.message) || err)});',
    '  }',
    '};'
  ].join("\n");
  const w = new Worker(URL.createObjectURL(
    new Blob([glue, "\n", boot], {type:"text/javascript"})));
  let seq = 0; const pending = new Map();
  w.onmessage = (e) => {
    const p = pending.get(e.data.id); if(!p) return; pending.delete(e.data.id);
    e.data.ok ? p.res(e.data.v) : p.rej(new Error(e.data.error));
  };
  w.onerror = (e) => { for(const p of pending.values()) p.rej(new Error(e.message||"worker error")); pending.clear(); };
  return (msg, transfer) => new Promise((res, rej) => {
    const id = ++seq; pending.set(id, {res, rej});
    w.postMessage(Object.assign({id}, msg), transfer || []);
  });
}

async function openEngine(setStep){
  setStep("Starting the query engine…");
  const wasmBytes = Uint8Array.from(atob(WASM_B64), c => c.charCodeAt(0));
  const wasmLen = wasmBytes.length;
  const call = reteWorker();
  setStep("Finding the graph inside this file…");
  const v = await call({t:"open", wasm: wasmBytes, url: location.href}, [wasmBytes.buffer]);
  let last = v.stats;
  return {
    info: v.info, card: v.card, wasmLen: wasmLen, label: SRC_LABEL,
    fileLen: v.stats.fileLength, base: v.stats.base, lazy: true,
    async query(q, reasoned){
      const r = await call({t:"query", q: q, reasoned: !!reasoned});
      last = r.stats; return r.result;
    },
    stats(){ return last; },
    // The one place a whole-graph transfer is the right answer: the user asked
    // for the file. Everything else above is ranges.
    async download(){
      const r = await fetch(location.href, {headers:{Range:"bytes=" + TAIL_OFFSET + "-"}});
      if(!r.ok && r.status !== 206) throw new Error("could not read own tail: HTTP " + r.status);
      return new Uint8Array(await r.arrayBuffer());
    }
  };
}
""".replace("__TOKEN__", TOKEN)

POLYGLOT_FOOTER = ("It is served as <code>text/html</code>, and the real "
                   "<code>.rete</code> is appended after <code>&lt;/html&gt;</code>; "
                   "the page finds it through the <code>RETE-BASE:</code> marker in "
                   "its own first bytes and range-reads only what each query needs.")

# embed: the graph is base64 in the file; decode it in memory (no fetch).
EMBED_SRC_HEAD = 'const RETE_B64 = "'
EMBED_SRC_TAIL = r"""";
const SRC_LABEL = "graph embedded in this file";

async function openEngine(setStep){
  setStep("Starting the query engine…");
  const wasmBytes = Uint8Array.from(atob(WASM_B64), c => c.charCodeAt(0));
  await wasm_bindgen({module_or_path: wasmBytes});
  setStep("Decoding the graph out of this very file…");
  const bin = atob(RETE_B64);
  const bytes = new Uint8Array(bin.length);
  for(let i=0;i<bin.length;i++) bytes[i] = bin.charCodeAt(i);
  const g = new wasm_bindgen.Graph(bytes);
  const c = g.card();
  return {
    info: JSON.parse(g.info()), card: c ? JSON.parse(c) : null,
    wasmLen: wasmBytes.length, label: SRC_LABEL,
    fileLen: bytes.length, base: 0, lazy: false,
    async query(q, reasoned){
      return JSON.parse(reasoned ? g.query_reasoned(q, "json") : g.query(q, "json"));
    },
    stats(){ return null; },
    async download(){ return bytes; }
  };
}
"""
EMBED_FOOTER = ("The whole graph is base64-embedded inside this single "
                "<code>.html</code>, so it opens by double-click, fully offline.")


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--rete", required=True)
    ap.add_argument("--out", required=True)
    ap.add_argument("--mode", choices=["polyglot", "embed"], default="polyglot")
    ap.add_argument("--template", default=TEMPLATE)
    # Overridable so a test can build a real polyglot without the 3 MB engine —
    # what it asserts is the marker contract, not the wasm.
    ap.add_argument("--glue", default=GLUE)
    ap.add_argument("--wasm", default=WASM)
    args = ap.parse_args()

    html = open(args.template, encoding="utf-8").read()
    glue = open(args.glue, encoding="utf-8").read()
    wasm_b64 = base64.b64encode(open(args.wasm, "rb").read()).decode("ascii")
    rete = open(args.rete, "rb").read()
    assert rete[:4] == b"RETE", "input is not a .rete file"
    # The glue is inlined inside a <script> element (live in embed mode, inert
    # text in polyglot mode); a literal end tag anywhere in it would close that
    # element early and corrupt the page.
    assert "</script>" not in glue, "engine glue contains a literal </script>"

    html = html.replace("__GLUE_JS__", glue).replace("__WASM_B64__", wasm_b64)

    if args.mode == "embed":
        rete_b64 = base64.b64encode(rete).decode("ascii")
        graph_src = EMBED_SRC_HEAD + rete_b64 + EMBED_SRC_TAIL
        html = html.replace("__GRAPH_SOURCE_JS__", graph_src)
        html = html.replace("__FOOTER_NOTE__", EMBED_FOOTER)
        # No appended tail, so no base offset: the marker would be a lie.
        html = html.replace("__RETE_BASE_MARKER__", "")
        html = html.replace("__GLUE_TYPE__", "text/javascript")
        assert TOKEN not in html, "embed mode must not carry the offset token"
        assert MARKER not in html, "embed mode must not carry the polyglot marker"
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
    html = html.replace("__RETE_BASE_MARKER__", MARKER_COMMENT)
    # The engine glue must NOT execute on the main thread here: the only thing
    # that may touch the graph is the worker, which reads this element's text.
    html = html.replace("__GLUE_TYPE__", "text/plain")
    assert html.count(TOKEN) == 2, f"{TOKEN} must appear twice (marker + engine)"
    # Wrap the appended .rete inside an inert <script type="text/plain"> element:
    # served as text/html, a browser that keeps parsing past our `window.stop()`
    # would otherwise turn N MB of binary into a pathological DOM (freezing the
    # tab). In "script data" state the whole tail is consumed as ONE text token —
    # never rendered, never executed — while the page's own byte-range reads of
    # the tail are unaffected. The closing </script> never appears in the binary,
    # so the element runs to EOF.
    html += '\n<script type="text/plain" id="rete-tail">\n'
    offset = len(html.encode("utf-8"))  # tail begins right after the wrapper opener
    assert offset < 10 ** POLYGLOT_DIGITS
    # Both occurrences are exactly POLYGLOT_DIGITS wide, so this cannot move a byte.
    html = html.replace(TOKEN, str(offset).zfill(POLYGLOT_DIGITS))
    html_bytes = html.encode("utf-8")
    assert len(html_bytes) == offset, "offset drift — token not length-stable"

    with open(args.out, "wb") as f:
        f.write(html_bytes)
        f.write(rete)

    # Read the file back with the READER's algorithm, over the reader's own
    # window. This is the check whose absence let the two halves disagree.
    with open(args.out, "rb") as f:
        head = f.read(HEADER_LEN)
        found = detect_polyglot_base(head)
        assert found is not None, (
            f"no {MARKER} marker in the first {HEADER_LEN} bytes — "
            "detect_polyglot_base would return None for this file")
        assert found == offset, f"marker says {found}, tail is at {offset}"
        f.seek(found)
        assert f.read(4) == b"RETE", "tail magic not at the marked offset"

    print(f"wrote {args.out}  (mode=polyglot; upload to R2 with Content-Type text/html)")
    print(f"  html+engine: {len(html_bytes):,} bytes  ·  .rete tail: {len(rete):,} B @ offset {offset:,}")
    print(f"  total: {len(html_bytes)+len(rete):,} bytes")
    print(f"  {MARKER}{str(offset).zfill(POLYGLOT_DIGITS)} at byte {head.find(MARKER.encode()):,}"
          f" — detect_polyglot_base() -> {found:,}, RETE magic verified there")


if __name__ == "__main__":
    main()
