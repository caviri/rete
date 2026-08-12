// End-to-end test of the ASYNCIFIED real engine against a LIVE remote .rete.
//
// Loads the no-modules glue from web/pkg-nomodules-async, patches the one line
// `const import1 = require("env");` -> our async env import + Asyncify driver
// (the SAME patch the playground worker will use), inits the wasm, then runs a
// real SPARQL query through `reteDrive(() => sparql_url(...))`. The reads happen
// CONCURRENTLY via Promise.all of fetch, with the engine suspended/resumed by
// Asyncify — no SharedArrayBuffer, no cross-origin isolation.
//
//   RETE_URL='https://…/wikidata.rete?token=…' node dev/asyncify-e2e.cjs
const fs = require('fs');
const crypto = require('node:crypto');
const {
  buildResidentReport,
  parseExpectedPin,
  validateRemotePin,
  writeExclusiveJsonReport,
} = require('./asyncify_e2e_report.cjs');

// ---- the patch: replaces `const import1 = require("env");` in the glue --------
const ASYNC_ENV_JS = `
        // ---- injected: Asyncify env import + driver ----
        let __reteAD = 0, __retePending = null, __reteSleeping = false, __reteRes = 0, __reteReqs = 0, __reteBytes = 0;
        function __reteStack() {
          if (!__reteAD) {
            const SIZE = 16 << 20; // 16 MiB Asyncify stack (the engine's recursive eval is deep)
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
              .then((r) => { if (r.status !== 206) throw new Error('Range status ' + r.status); return r.arrayBuffer(); })
              .then((b) => new Uint8Array(b))));
          const mem = new Uint8Array(wasm.memory.buffer);
          let pos = dstPtr, total = 0;
          for (const b of bufs) { mem.set(b, pos); pos += b.length; total += b.length; }
          __reteReqs += ranges.length; __reteBytes += total;
          return total;
        }
        async function __reteDoLen(urlPtr, urlLen, outPtr) {
          const url = __reteStr(urlPtr, urlLen);
          const r = await fetch(url, { headers: { Range: 'bytes=0-0' } });
          const cr = r.headers.get('content-range');
          const total = cr ? Number(cr.split('/')[1]) : Number(r.headers.get('content-length') || 0);
          __reteReqs += 1;
          new DataView(wasm.memory.buffer).setBigUint64(outPtr, BigInt(total || 0), true);
          return total > 0 ? 1 : 0;
        }
        // Generic suspend: 1st entry kicks off the Promise + unwinds; on rewind
        // (2nd entry) returns the resolved result. Shared by both async imports.
        function __reteSuspend(makePromise) {
          if (!__reteSleeping) {
            __retePending = makePromise();
            wasm.asyncify_start_unwind(__reteAD);
            __reteSleeping = true;
            return 0;
          }
          wasm.asyncify_stop_rewind();
          __reteSleeping = false;
          return __reteRes;
        }
        function __reteFetchRanges(urlPtr, urlLen, offsPtr, lensPtr, n, dstPtr) {
          return __reteSuspend(() => __reteDoFetch(urlPtr, urlLen, offsPtr, lensPtr, n, dstPtr));
        }
        function __reteFileLen(urlPtr, urlLen, outPtr) {
          return __reteSuspend(() => __reteDoLen(urlPtr, urlLen, outPtr));
        }
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
        exports.reteAsyncStats = () => ({ requests: __reteReqs, bytes: __reteBytes });
        // Open a resident RemoteGraph via the RAW export, driving the suspend loop and
        // wrapping the pointer ONCE after rewind — so the unwind pass never creates a
        // garbage instance for the FinalizationRegistry to free(0).
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
        const import1 = { rete_fetch_ranges: __reteFetchRanges, rete_file_len: __reteFileLen, rete_panic_report: (p, l, line) => console.error("rete-wasm panic at " + (l ? __reteStr(p, l) : "?") + ":" + line) };
`;

const URL_ = process.env.RETE_URL;
if (!URL_) { console.error('set RETE_URL'); process.exit(1); }

async function probeRemotePin(url, expected) {
  const pin = { url, expected, actual: {} };
  try {
    const response = await fetch(url, { method: 'HEAD', cache: 'no-store' });
    pin.actual = {
      status: response.status,
      contentLength: response.headers.get('content-length') || '',
      etag: response.headers.get('etag') || '',
      acceptRanges: response.headers.get('accept-ranges') || '',
    };
  } catch (error) {
    pin.actual = { error: String(error) };
  }
  return pin;
}

// Prefer the PRODUCTION-patched glue (docs/, written by build_playground.py) so this
// tests the exact shipped artifact; fall back to patching the raw glue in-memory.
let src, WASM;
if (fs.existsSync('docs/rete_wasm_async.js')) {
  src = fs.readFileSync('docs/rete_wasm_async.js', 'utf8');
  WASM = 'docs/rete_wasm_async.wasm';
  console.log('(testing the PRODUCTION-patched glue: docs/rete_wasm_async.js)');
} else {
  src = fs.readFileSync('web/pkg-nomodules-async/rete_wasm.js', 'utf8');
  WASM = 'web/pkg-nomodules-async/rete_wasm_bg.wasm';
  src = src.replace('const import1 = require("env");', ASYNC_ENV_JS);
  src = src.replace(/const import(\d+) = require\("env"\);/g, 'const import$1 = import1;');
}

const m = { exports: {} };
new Function('module', 'exports', 'require', 'fetch', 'TextDecoder', 'WebAssembly', 'performance', 'globalThis',
  src + '\n;module.exports = wasm_bindgen;')(m, m.exports, require, fetch, TextDecoder, WebAssembly, performance, globalThis);
const wb = m.exports;

(async () => {
  const q = process.env.RETE_Q ||
    'PREFIX wdt: <http://www.wikidata.org/prop/direct/>\n' +
    'PREFIX wd: <http://www.wikidata.org/entity/>\n' +
    'PREFIX rdfs: <http://www.w3.org/2000/01/rdf-schema#>\n' +
    'SELECT ?p ?who WHERE { ?p wdt:P737 wd:Q859 ; rdfs:label ?who . FILTER(LANG(?who)="en") } LIMIT 5';
  const expectedPin = parseExpectedPin(
    process.env.RETE_EXPECT_LENGTH,
    process.env.RETE_EXPECT_ETAG,
  );
  let pinsBefore = null;
  let pinsAfter = null;
  let pinErrors = [];
  if (expectedPin) {
    pinsBefore = await probeRemotePin(URL_, expectedPin);
    pinErrors = validateRemotePin(pinsBefore).map((error) => `before: ${error}`);
    if (pinErrors.length > 0) {
      const report = {
        verdict: 'FAIL',
        url: URL_,
        query: q,
        pinsBefore,
        pinsAfter,
        pinErrors,
      };
      writeExclusiveJsonReport(process.env.RETE_REPORT_PATH, report);
      console.log(`evidence ${JSON.stringify(report)}`);
      process.exit(1);
      return;
    }
  }

  const wasmBytes = fs.readFileSync(WASM);
  await wb({ module_or_path: wasmBytes }).catch(() => wb(wasmBytes)); // init (new or legacy signature)

  // Resident session: open ONCE, query TWICE (the 2nd must hit the cache).
  const t0 = performance.now();
  const g = await wb.reteOpenRemote(URL_);
  const openMs = Math.round(performance.now() - t0);
  const s0 = JSON.parse(g.stats());

  const t1 = performance.now();
  const out = await (wb.reteQueryRemote ? wb.reteQueryRemote(g, q, 'table', false) : wb.reteDrive(() => g.query(q, 'table')));
  const q1ms = Math.round(performance.now() - t1);
  const s1 = JSON.parse(g.stats());

  const t2 = performance.now();
  await (wb.reteQueryRemote ? wb.reteQueryRemote(g, q, 'table', false) : wb.reteDrive(() => g.query(q, 'table')));
  const q2ms = Math.round(performance.now() - t2);
  const s2 = JSON.parse(g.stats());

  const parsed = JSON.parse(out);
  const rows = (parsed.rows || []).length;
  if (expectedPin) {
    pinsAfter = await probeRemotePin(URL_, expectedPin);
    pinErrors.push(...validateRemotePin(pinsAfter).map((error) => `after: ${error}`));
  }
  const report = buildResidentReport({
    url: URL_,
    query: q,
    openMs,
    query1Ms: q1ms,
    query2Ms: q2ms,
    stats0: s0,
    stats1: s1,
    stats2: s2,
    result: parsed,
    resultSha256: crypto.createHash('sha256').update(out).digest('hex'),
    pinsBefore,
    pinsAfter,
    pinErrors,
  });
  writeExclusiveJsonReport(process.env.RETE_REPORT_PATH, report);
  console.log(`open ${openMs} ms (${s0.bytes} B) · query1 ${q1ms} ms (+${s1.bytes - s0.bytes} B, ${s1.requests - s0.requests} reqs) · query2 ${q2ms} ms (+${s2.bytes - s1.bytes} B ${s2.requests - s1.requests} reqs = cache reuse)`);
  console.log(`result kind=${parsed.kind} vars=${JSON.stringify(parsed.vars)} rows=${rows}`);
  console.log(out.slice(0, 360));
  console.log((s2.requests - s1.requests) === 0 ? '\n✓ resident session works: 2nd query served from cache (0 new requests)' : '\n(note: 2nd query refetched — cache check)');
  console.log(`evidence ${JSON.stringify(report)}`);
  process.exit(report.verdict === 'PASS' ? 0 : 1);
})().catch((e) => { console.error('E2E FAILED:', e); process.exit(1); });
