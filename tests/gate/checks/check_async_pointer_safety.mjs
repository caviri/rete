// Guard the asyncify glue's wasm→JS pointer handling.
//
// wasm32 pointers cross into JS through `i32` imports, so anything the engine
// allocates above 2 GiB arrives SIGN-EXTENDED — a negative number. `mem.set(b,
// negative)` throws `RangeError: offset is out of bounds`, and because wasm
// memory never shrinks, every later read in that worker fails the same way: one
// query bricks the page session.
//
// This really happens: a remote scan of wikidata-1GB pushed the heap to 2050 MB
// and produced dstPtr = -2145787624. The browser matrix did not catch it because
// its async check (check_lazy_async) uses a SMALL dataset — the bug needs a heap
// past 2 GiB, which costs ~150 MB of range reads and ~80 s to reach. Rather than
// pay that on every gate run, assert the property that makes it impossible: every
// pointer that is dereferenced goes through the `>>> 0` normalizer.
//
// Checked against BOTH the generator (the source of truth) and the generated file
// that actually ships, so regenerating cannot silently drop it.
import fs from "node:fs";

const root = process.env.RETE_ROOT || "/work";
const targets = [
  { path: `${root}/scripts/build_playground.py`, label: "generator" },
  { path: `${root}/docs/rete_wasm_async.js`, label: "generated glue" },
];

function fail(message) {
  throw new Error(`async pointer safety: ${message}`);
}

let checked = 0;
for (const { path, label } of targets) {
  if (!fs.existsSync(path)) {
    // The generated glue only exists after a wasm build; the generator always does.
    if (label === "generator") fail(`${path} is missing`);
    continue;
  }
  const src = fs.readFileSync(path, "utf8");
  if (!src.includes("__reteDoFetch")) continue; // not the asyncify glue
  checked++;

  if (!/function __reteP\(ptr\)\s*\{\s*return ptr >>> 0;\s*\}/.test(src)) {
    fail(`${label}: the __reteP(ptr) => ptr >>> 0 normalizer is gone`);
  }
  // The write destination is the one that threw in the wild.
  if (!/let pos = __reteP\(dstPtr\)/.test(src)) {
    fail(`${label}: dstPtr is dereferenced without __reteP — a >2 GiB heap will throw RangeError`);
  }
  // The range table and the length out-param are read through DataView, which
  // rejects negative offsets just as hard.
  if (!/__reteP\(offsPtr\)/.test(src) || !/__reteP\(lensPtr\)/.test(src)) {
    fail(`${label}: offsPtr/lensPtr are read without __reteP`);
  }
  if (!/setBigUint64\(__reteP\(outPtr\)/.test(src)) {
    fail(`${label}: outPtr is written without __reteP`);
  }
  if (!/function __reteStr\(ptr, len\) \{ const p = __reteP\(ptr\);/.test(src)) {
    fail(`${label}: __reteStr slices with a raw pointer`);
  }
}

if (checked === 0) fail("no asyncify glue was checked — the probes are stale");
console.log(`[G0] async glue normalizes every wasm pointer (>2 GiB safe) — ${checked} file(s)`);
