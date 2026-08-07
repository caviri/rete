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
//
// Probes are COLLECTED, not thrown (see _expect.mjs) — every missing normalizer
// gets named in one FAIL verdict instead of the first one killing the run and
// leaving CI a stack-trace tail. The PASS line stays the `[G0] …` string the
// runner greps for.
import fs from "node:fs";
import { expect } from "./_expect.mjs";

const root = process.env.RETE_ROOT || "/work";
const targets = [
  { path: `${root}/scripts/build_playground.py`, label: "generator" },
  { path: `${root}/docs/rete_wasm_async.js`, label: "generated glue" },
];

const t = expect("check_async_pointer_safety");

let checked = 0;
try {
  for (const { path, label } of targets) {
    if (!fs.existsSync(path)) {
      // The generated glue only exists after a wasm build; the generator always does.
      if (label === "generator") t.fail("generatorPresent", `${path} is missing`, { actual: "missing", expected: path });
      continue;
    }
    const src = fs.readFileSync(path, "utf8");
    if (!src.includes("__reteDoFetch")) continue; // not the asyncify glue
    checked++;

    t.ok(`${label}: __reteP normalizer`,
      /function __reteP\(ptr\)\s*\{\s*return ptr >>> 0;\s*\}/.test(src),
      "the __reteP(ptr) => ptr >>> 0 normalizer is gone");
    // The write destination is the one that threw in the wild.
    t.ok(`${label}: dstPtr via __reteP`,
      /let pos = __reteP\(dstPtr\)/.test(src),
      "dstPtr is dereferenced without __reteP — a >2 GiB heap will throw RangeError");
    // The range table and the length out-param are read through DataView, which
    // rejects negative offsets just as hard.
    t.ok(`${label}: offsPtr/lensPtr via __reteP`,
      /__reteP\(offsPtr\)/.test(src) && /__reteP\(lensPtr\)/.test(src),
      "offsPtr/lensPtr are read without __reteP");
    t.ok(`${label}: outPtr via __reteP`,
      /setBigUint64\(__reteP\(outPtr\)/.test(src),
      "outPtr is written without __reteP");
    t.ok(`${label}: __reteStr via __reteP`,
      /function __reteStr\(ptr, len\) \{ const p = __reteP\(ptr\);/.test(src),
      "__reteStr slices with a raw pointer");
  }

  t.ok("someGlueWasChecked", checked > 0, "no asyncify glue was checked — the probes are stale");
} catch (error) {
  t.threw("async pointer safety", error);
}

// This check's runner contract is a grep for `[G0]` on stdout, and that string is
// also the green log line — so the pass output stays byte-for-byte what it was,
// and the JSON verdict is emitted only when something actually failed.
if (t.failed) t.finish({ files: checked });
else console.log(`[G0] async glue normalizes every wasm pointer (>2 GiB safe) — ${checked} file(s)`);
