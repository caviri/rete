// Build a .rete fixture in a CHILD process: `node build-fixture.mjs <out> [fmt]`
// with the RDF text on stdin.
//
// The memory test measures the wasm heap's high-water mark, and wasm memory
// grows but never shrinks — so a build() in the same process would leave a
// large freed hole that the later measurements allocate into for free, making
// every result look flat. Building here keeps the measuring process's engine
// pristine: it only ever opens the finished file.
import { readFileSync, writeFileSync } from "node:fs";

import { build } from "../dist/index.js";

const [out, format = "nt"] = process.argv.slice(2);
writeFileSync(out, await build(readFileSync(0, "utf8"), format));
