// Unit-test the SHIPPED async length probe (__reteDoLen in docs/rete_wasm_async.js):
// a cold first request that yields no readable length must be RETRIED, not hard-fail
// with "could not determine length". Extracts the real function and drives it with a
// mock fetch. Usage: node check_lenprobe.mjs
import fs from "node:fs";

const GLUE = process.env.GLUE || "/work/docs/rete_wasm_async.js";
const TOTAL = 1610612736; // 1.5 GB, like gbif-birds

function extract(src, name) {
  const start = src.indexOf("async function " + name);
  if (start < 0) return null;
  let depth = 0, i = src.indexOf("{", start), end = -1;
  for (; i < src.length; i++) { const c = src[i]; if (c === "{") depth++; else if (c === "}" && --depth === 0) { end = i + 1; break; } }
  return src.slice(start, end);
}

function makeFn(fnSrc, fetchImpl) {
  const wasm = { memory: { buffer: new ArrayBuffer(16) } };
  const __reteStr = () => "http://host/f.rete";
  const fn = new Function("fetch", "__reteStr", "wasm", "DataView", "setTimeout",
    fnSrc + "\n;return __reteDoLen;")(fetchImpl, __reteStr, wasm, DataView, setTimeout);
  return { fn, wasm };
}
const readTotal = (wasm) => Number(new DataView(wasm.memory.buffer).getBigUint64(0, true));

// A fetch that returns NO usable length for the first `failFor` attempts (each
// attempt = one HEAD + one ranged GET), then valid headers.
function flakyFetch(failFor) {
  const st = { head: 0, range: 0, calls: 0 };
  const fn = (url, opts) => {
    st.calls++;
    const isHead = opts && opts.method === "HEAD";
    if (isHead) {
      st.head++;
      if (st.head <= failFor) return Promise.resolve({ ok: false, headers: { get: () => null } });
      return Promise.resolve({ ok: true, headers: { get: (k) => (String(k).toLowerCase() === "content-length" ? String(TOTAL) : null) } });
    }
    st.range++;
    if (st.range <= failFor) return Promise.resolve({ headers: { get: () => null } });
    return Promise.resolve({ headers: { get: (k) => (String(k).toLowerCase() === "content-range" ? `bytes 0-0/${TOTAL}` : null) } });
  };
  return { fn, st };
}

const main = async () => {
  const src = fs.readFileSync(GLUE, "utf8");
  const fnSrc = extract(src, "__reteDoLen");
  const checks = [];
  if (!fnSrc) { console.log(JSON.stringify({ verdict: "FAIL", why: "__reteDoLen not found in " + GLUE })); process.exit(1); }
  // The whole point: the retry must be present (a lone attempt would hard-fail).
  checks.push(["source has a retry loop", /for\s*\(/.test(fnSrc) || /while\s*\(/.test(fnSrc)]);

  // 1) happy path — first attempt succeeds.
  { const { fn, st } = flakyFetch(0); const env = makeFn(fnSrc, fn);
    const ret = await env.fn(0, 0, 0);
    checks.push(["happy path returns 1 + correct total", ret === 1 && readTotal(env.wasm) === TOTAL]);
    checks.push(["happy path is a single attempt", st.head <= 1]); }

  // 2) THE bug — first attempt yields nothing, retry recovers.
  { const { fn, st } = flakyFetch(1); const env = makeFn(fnSrc, fn);
    const ret = await env.fn(0, 0, 0);
    checks.push(["recovers after 1 cold failure", ret === 1 && readTotal(env.wasm) === TOTAL]);
    checks.push(["actually retried (>1 attempt)", st.head + st.range > 2]); }

  // 3) all attempts fail — returns 0 cleanly (caller surfaces the transient error).
  { const { fn } = flakyFetch(99); const env = makeFn(fnSrc, fn);
    const ret = await env.fn(0, 0, 0);
    checks.push(["persistent failure returns 0 (no crash)", ret === 0]); }

  const pass = checks.every(([, ok]) => ok);
  console.log(JSON.stringify({ verdict: pass ? "PASS" : "FAIL", checks: checks.map(([n, ok]) => `${ok ? "✓" : "✗"} ${n}`) }, null, 2));
  process.exit(pass ? 0 : 1);
};
main().catch((e) => { console.log(JSON.stringify({ verdict: "FAIL", err: String(e).slice(0, 200) })); process.exit(1); });
