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
import datetime
import json
import os
import pathlib
import re
import subprocess
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
            // While an unwind is in flight, a DRIVEN wasm-bindgen wrapper still
            // runs its epilogue on whatever the raw export returned — garbage —
            // and takeObject()/getStringFromWasm0()/__wbindgen_free() on garbage
            // corrupt the object heap and the allocator. That is the
            // "null function or function signature mismatch" family: every
            // suspend is one roll of the corruption dice, which is why multi-GB
            // files (dozens–hundreds of suspends per query) died where small
            // ones survived. Guard every public raw export: when a call ends in
            // the UNWINDING state, hand the wrapper a harmless [0,0,0,0] tuple
            // instead (ptr 0 / len 0 / no error — the exact shape wbindgen's own
            // throw path already frees safely); the drive loop discards that
            // pass's value and calls again after the rewind. `instance.exports`
            // is frozen, so rebind the closure's `wasm` to a patchable clone.
            wasm = Object.assign(Object.create(null), wasm);
            for (const k of Object.keys(wasm)) {
              if (typeof wasm[k] !== "function" || k.indexOf("__") === 0 || k.indexOf("asyncify_") === 0) continue;
              const orig = wasm[k];
              wasm[k] = function () {
                const r = orig.apply(null, arguments);
                return wasm.asyncify_get_state() === 1 ? [0, 0, 0, 0] : r;
              };
            }
            // The allocator IS asyncify-instrumented (it can reach panic_fmt),
            // so a wrapper re-marshaling its arguments while the instance is
            // REWINDING (state 2) would make malloc's prologue consume the
            // rewind buffer as if IT were being resumed. Pause the rewind
            // around allocator calls — at state 0 they run normally.
            for (const k of ["__wbindgen_malloc", "__wbindgen_realloc"]) {
              const orig = wasm[k];
              if (!orig) continue;
              wasm[k] = function () {
                if (wasm.asyncify_get_state() === 2) {
                  wasm.asyncify_stop_rewind();
                  const r = orig.apply(null, arguments);
                  wasm.asyncify_start_rewind(__reteAD);
                  return r;
                }
                return orig.apply(null, arguments);
              };
            }
            const SIZE = 16 << 20; // 16 MiB Asyncify stack — the engine's recursive eval is deep
            __reteAD = wasm.__wbindgen_malloc(8 + SIZE, 8);
            const d = new DataView(wasm.memory.buffer);
            d.setInt32(__reteAD, __reteAD + 8, true);
            d.setInt32(__reteAD + 4, __reteAD + 8 + SIZE, true);
          }
        }
        // wasm32 pointers arrive through `i32` imports, so anything the engine
        // allocates above 2 GiB reaches JS SIGN-EXTENDED — a negative number that
        // makes `mem.set(b, ptr)` throw `RangeError: offset is out of bounds`. The
        // heap really does cross 2 GiB on a big remote scan (measured at 2050 MB on
        // wikidata-1GB), and because wasm memory never shrinks, every later read in
        // that worker fails too. `>>> 0` restores the unsigned value; every pointer
        // crossing this boundary goes through it.
        function __reteP(ptr) { return ptr >>> 0; }
        function __reteStr(ptr, len) { const p = __reteP(ptr); return new TextDecoder().decode(new Uint8Array(wasm.memory.buffer).slice(p, p + (len >>> 0))); }
        // cache:'no-store' is REQUIRED on WebKit (desktop Safari, and iOS when a user
        // forces concurrent reads): WebKit caches/coalesces concurrent same-URL Range
        // requests (this Promise.all fires many at once) and can hand back a
        // wrong-length or empty body → the engine decodes corrupt tiles → a wasm trap.
        // But no-store defeats the HTTP cache on EVERY read, so it needlessly taxes
        // Chromium/Firefox (the async default there) with cross-reload re-fetches —
        // and they handle concurrent ranges fine. So gate it to WebKit only.
        var __reteNoStore = (function () { try { var ua = (navigator.userAgent || "").toLowerCase(); return ua.indexOf("safari") >= 0 && ua.indexOf("chrome") < 0 && ua.indexOf("chromium") < 0 && ua.indexOf("android") < 0 && ua.indexOf("edg/") < 0; } catch (e) { return false; } })();
        async function __reteDoFetch(urlPtr, urlLen, offsPtr, lensPtr, n, dstPtr) {
          const url = __reteStr(urlPtr, urlLen);
          const dv = new DataView(wasm.memory.buffer);
          const ranges = [];
          const offsB = __reteP(offsPtr), lensB = __reteP(lensPtr);
            for (let i = 0; i < n; i++) ranges.push([Number(dv.getBigUint64(offsB + i*8, true)), dv.getUint32(lensB + i*4, true)]);
          // Retry each range once after a short pause: this Promise.all fires a
          // BURST of concurrent fetches, and a single transient miss ("Failed to
          // fetch" on a flaky link, a 5xx blip) used to fail the whole query —
          // the sync XHR reader already retries, so async matches it.
          const one = ([o, l], attempt) =>
            fetch(url, { headers: { Range: 'bytes=' + o + '-' + (o+l-1) }, cache: __reteNoStore ? 'no-store' : 'default' })
              .then((r) => { if (r.status !== 206) throw new Error('Range status ' + r.status + ' (host must support HTTP range)'); return r.arrayBuffer(); })
              .then((b) => new Uint8Array(b))
              .catch((e) => {
                if (attempt >= 1) throw e;
                return new Promise((res) => setTimeout(res, 250)).then(() => one([o, l], attempt + 1));
              });
          const bufs = await Promise.all(ranges.map((r) => one(r, 0)));
          const mem = new Uint8Array(wasm.memory.buffer);
          let pos = __reteP(dstPtr), total = 0;
          // Each range MUST land at its fixed slot (cumulative REQUESTED length), and
          // its body MUST be exactly the requested length. A short/over response (the
          // symptom of the WebKit caching bug above) would otherwise misalign every
          // later range and crash the decoder with an inscrutable wasm trap — fail
          // loudly with a diagnosable error instead.
          for (let i = 0; i < bufs.length; i++) {
            const b = bufs[i], want = ranges[i][1];
            if (b.length !== want) throw new Error('Range length mismatch: got ' + b.length + ' of ' + want + ' bytes at offset ' + ranges[i][0] + ' (browser mishandled a concurrent HTTP Range request)');
            mem.set(b, pos); pos += want; total += want;
          }
          return total;
        }
        async function __reteDoLen(urlPtr, urlLen, outPtr) {
          const url = __reteStr(urlPtr, urlLen);
          // No HTTP length signal survives every host (issue #95): a
          // transparently-compressing host (GitHub Pages) advertises the GZIP
          // size in HEAD's Content-Length (58 MB for a 71 MB .rete) while range
          // requests address the identity bytes — and Content-Encoding is not
          // CORS-safelisted, so JS cannot even see the lie. Content-Range names
          // the true total but is HIDDEN unless the host opts in via
          // Access-Control-Expose-Headers (GitHub Pages does not). So read the
          // file's OWN first KiB — the .rete header, whose section directory
          // pins the exact length (sections are back-to-back; the file ends
          // with the 4-byte RETE footer) — and only fall back to the
          // transport's numbers when the resource is not a .rete. A 206's
          // Content-Length is NEVER believed (it is the partial body's size).
          const headerLen = (bytes) => { // max(section offset+len) + 4, or 0
            if (!bytes || bytes.length < 1024) return 0;
            if (bytes[0] !== 0x52 || bytes[1] !== 0x45 || bytes[2] !== 0x54 || bytes[3] !== 0x45) return 0; // "RETE"
            const dv = new DataView(bytes.buffer, bytes.byteOffset, bytes.byteLength);
            const n = dv.getUint16(44, true);
            if (64 + n * 24 > 1024) return 0;
            let end = 1024n;
            for (let i = 0; i < n; i++) {
              const off = dv.getBigUint64(64 + i * 24 + 8, true), len = dv.getBigUint64(64 + i * 24 + 16, true);
              if (len > 0n && off + len > end) end = off + len;
            }
            const t = Number(end + 4n);
            return Number.isSafeInteger(t) ? t : 0;
          };
          // The FIRST cross-origin request to a cold object can transiently come
          // back unreadable (CORS preflight, CDN cold-start), so retry like the
          // sync reader does.
          let total = 0;
          for (let attempt = 0; attempt < 4 && !(total > 0); attempt++) {
            if (attempt) await new Promise((r) => setTimeout(r, 150 * attempt)); // 150, 300, 450 ms
            try {
              const r = await fetch(url, { headers: { Range: 'bytes=0-1023' } });
              const cr = r.status === 206 ? r.headers.get('content-range') : null;
              const crTotal = cr ? Number(cr.split('/')[1]) : 0; // NaN on "bytes a-b/*" → not > 0
              if (r.status === 206) {
                const derived = headerLen(new Uint8Array(await r.arrayBuffer()));
                if (derived > 0 && crTotal === derived) total = derived; // transport agrees — done
                else if (derived > 0) {
                  // Validate against the file itself: its last 4 bytes are the
                  // RETE footer. One extra 4-byte request, only on hosts whose
                  // headers are unusable or disagree.
                  const t = await fetch(url, { headers: { Range: 'bytes=' + (derived - 4) + '-' + (derived - 1) } });
                  const tb = t.status === 206 ? new Uint8Array(await t.arrayBuffer()) : new Uint8Array(0);
                  if (tb.length === 4 && tb[0] === 0x52 && tb[1] === 0x45 && tb[2] === 0x54 && tb[3] === 0x45) total = derived;
                  else if (crTotal > 0) total = crTotal; // truncated .rete on an honest host
                } else if (crTotal > 0) total = crTotal; // not a .rete header — trust a visible total
              } else if (r.status === 200) {
                // Host ignored Range; it cannot serve range reads at all, but a
                // positive length lets the first read fail with the clearer
                // 'Range status 200 (host must support HTTP range)' error.
                total = Number(r.headers.get('content-length') || 0);
                if (r.body && r.body.cancel) r.body.cancel().catch(() => {});
              }
            } catch (e) { /* retry the whole probe */ }
            if (!(total > 0)) { // last resort: HEAD's CORS-safelisted Content-Length
              try { const h = await fetch(url, { method: 'HEAD' }); if (h.ok) total = Number(h.headers.get('content-length') || 0); } catch (e) { /* retry */ }
            }
          }
          new DataView(wasm.memory.buffer).setBigUint64(__reteP(outPtr), BigInt(total > 0 ? total : 0), true);
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
        // Asyncify allows exactly ONE suspended computation per instance: a
        // second entry while the first sleeps shares __reteAD/__reteSleeping and
        // both corrupt — the observed symptom was a fresh open whose length
        // probe was "answered" by a stale suspend in ~4 ms ("could not
        // determine length"). Serialize every driven entry through a promise
        // chain: cheap, and correct by construction.
        let __reteTurn = Promise.resolve();
        function __reteSerial(fn) {
          const run = __reteTurn.then(fn);
          __reteTurn = run.then(function () {}, function () {});
          return run;
        }
        exports.reteDrive = function (thunk) { return __reteSerial(function () { return __reteDrive(thunk); }); };
        exports.reteOpenRemote = function (url) { return __reteSerial(function () { return __reteOpenRemote(url); }); };
        // RAW-driven resident calls — the ROOT FIX for the "null function /
        // signature mismatch" family (proven in tests/gate/.cache/
        // asyncify_probe3.cjs: the wrapper-driven query traps at its first
        // suspend on a 17.5 GB file; the same query raw-driven completes in 12
        // suspend/rewind passes). A generated wasm-bindgen wrapper marshals its
        // arguments and unpacks its result tuple on EVERY drive pass; driving
        // the raw export instead marshals ONCE and touches the result only
        // after the rewind completes — exactly reteOpenRemote's shape.
        async function __reteCallRaw(call, unpackString) {
          __reteStack();
          let ret = call();
          while (wasm.asyncify_get_state() === 1) {
            wasm.asyncify_stop_unwind();
            __reteRes = await __retePending;
            wasm.asyncify_start_rewind(__reteAD);
            ret = call();
          }
          if (!unpackString) return ret;
          if (ret[3]) throw takeObject(ret[2]);
          try { return getStringFromWasm0(ret[0], ret[1]); }
          finally { wasm.__wbindgen_free(ret[0], ret[1], 1); }
        }
        exports.reteQueryRemote = function (g, query, format, reasoned, unionDefault) {
          return __reteSerial(function () {
            const ptr0 = passStringToWasm0(query, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
            const len0 = WASM_VECTOR_LEN;
            const ptr1 = passStringToWasm0(format, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
            const len1 = WASM_VECTOR_LEN;
            // The union-default-graph toggle routes through query_opts (reason +
            // union as i32 flags). The plain/reasoned exports stay the proven
            // path when the toggle is off — same marshal-once, raw-driven shape.
            if (unionDefault) {
              return __reteCallRaw(function () { return wasm.remotegraph_query_opts(g.__wbg_ptr, ptr0, len0, ptr1, len1, reasoned ? 1 : 0, 1); }, true);
            }
            const raw = reasoned ? wasm.remotegraph_query_reasoned : wasm.remotegraph_query;
            return __reteCallRaw(function () { return raw(g.__wbg_ptr, ptr0, len0, ptr1, len1); }, true);
          });
        };
        exports.retePrefixSearchRemote = function (g, prefix, limit) {
          return __reteSerial(function () {
            const ptr0 = passStringToWasm0(prefix, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
            const len0 = WASM_VECTOR_LEN;
            return __reteCallRaw(function () { return wasm.remotegraph_prefix_search(g.__wbg_ptr, ptr0, len0, limit); }, true);
          });
        };
        // RAW-driven generic *_url call (schema_url, check_schema_url, shacl_url,
        // reach_url, why_url, …) — the worker's generic "call" path used to drive
        // the generated WRAPPER through suspend/rewind, which re-marshals its
        // arguments and runs its free()-epilogue on EVERY pass and trapped with
        // "null function or function signature mismatch" at the first suspend
        // (proven in tests/gate/.cache/schema_probe.cjs: wrapper-driven
        // schema_url traps; the same call raw-driven completes in 4 passes).
        // Every *_url export is string-in/string-out with the same multivalue
        // result tuple, so one marshaler covers them: a string becomes a
        // (ptr, len) pair, null/undefined an absent Option (0, 0), a boolean an
        // i32 — marshal ONCE, drive raw, unpack only after the rewind completes.
        exports.reteCallUrlRemote = function (fn) {
          const args = Array.prototype.slice.call(arguments, 1);
          return __reteSerial(function () {
            const raw = wasm[fn];
            if (typeof raw !== "function") return Promise.reject(new Error("no wasm export " + fn));
            const flat = [];
            for (let i = 0; i < args.length; i++) {
              const a = args[i];
              if (typeof a === "string") { flat.push(passStringToWasm0(a, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc), WASM_VECTOR_LEN); }
              else if (a === null || a === undefined) { flat.push(0, 0); }
              else if (typeof a === "boolean") { flat.push(a ? 1 : 0); }
              else { flat.push(a); }
            }
            return __reteCallRaw(function () { return raw.apply(null, flat); }, true);
          });
        };
        async function __reteOpenRemote(url) {
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
        const import1 = { rete_fetch_ranges: __reteFetchRanges, rete_file_len: __reteFileLen,
          // LEAF panic reporter (never in asyncify-imports): the wasm-side hook
          // passes the raw panic Location so a crash logs file:line without any
          // fmt machinery (formatting is instrumented — a panic while the
          // instance is unwinding/rewinding would recurse forever).
          rete_panic_report: function (p, l, line) {
            try { console.error("rete-wasm panic at " + (l ? __reteStr(p, l) : "(unknown)") + ":" + line); } catch (e) { /* ignore */ }
          } };
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


def build_version():
    """A human build stamp surfaced in the topbar and error reports.

    Release builds provide ``RETE_BUILD_STAMP`` for deterministic output.
    Ad-hoc builds retain the convenient short-commit + UTC timestamp.
    """
    if stamp := os.environ.get("RETE_BUILD_STAMP"):
        return stamp
    try:
        commit = subprocess.run(
            ["git", "rev-parse", "--short", "HEAD"],
            cwd=str(ROOT), capture_output=True, text=True, timeout=10,
        ).stdout.strip()
    except Exception:
        commit = ""
    ts = datetime.datetime.now(datetime.timezone.utc).strftime("%Y-%m-%d %H:%M UTC")
    return f"{commit or 'dev'} · {ts}"
CSS = SRC / "styles.css"
CATALOG_JS = SRC / "catalog.js"
CM6_JS = SRC / "cm6.bundle.js"  # bundled CodeMirror 6 (see cm6/README / package.json)
EDITOR_JS = SRC / "editor.js"
RDFCONV_JS = SRC / "rdfconv.js"  # in-browser JSON-LD / RDF-XML -> N-Triples converters
APP_JS = SRC / "app.js"
VERSIONS_JS = SRC / "versions.js"

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
    # opencitations is REMOTE-LAZY (~34 GB on R2, not embeddable) — see catalog.js.
    # NOTE: mira, mira-wikidata, causalgraph and lineara moved to REMOTE-LAZY
    # (served from the bucket, range-read; see catalog.js kind:"remote-lazy"), so
    # they are intentionally NOT embedded here.
    # Remote-lazy (served from the bucket, NOT embedded; see catalog.js): getty-ulan,
    # history, mmm, orkg, factgrid-illuminati, wikidata-1GB/100mb, ohm-full, chemotion,
    # chebi-full, causenet-full, mira, mira-wikidata, causalgraph, lineara.
    # Dropped entirely: citations (synthetic), typed, deps.
]


def die(msg: str) -> None:
    print(f"build_playground: error: {msg}", file=sys.stderr)
    sys.exit(1)


def b64(path: pathlib.Path) -> str:
    return base64.b64encode(path.read_bytes()).decode("ascii")


def main() -> None:
    for p in (TEMPLATE, CSS, CATALOG_JS, CM6_JS, EDITOR_JS, RDFCONV_JS, VERSIONS_JS, APP_JS, GLUE_JS, WASM):
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
        .replace(
            "__PLAYGROUND_VERSIONS_JS__",
            VERSIONS_JS.read_text(encoding="utf-8").rstrip(),
        )
        .replace("__PLAYGROUND_APP_JS__", APP_JS.read_text(encoding="utf-8").rstrip())
        .replace("__BUILD_VERSION__", build_version())
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
        "__PLAYGROUND_VERSIONS_JS__",
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
