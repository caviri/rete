// Louvain termination through the BUILT package: every graph on which
// `build()` never returned in rete-graph 0.3.2 / rete 0.3.3 must return, both
// through the ESM entry (dist/index.js + rete_wasm_bg.wasm, as Node loads it)
// and through the script-tag bundle with the wasm embedded
// (dist/rete-graph.min.js, as a browser page loads it). The fixtures are the
// core crate's (crates/rete-core/tests/fixtures/louvain-hangs/): the smallest
// hangs the random-graph harness found, and three reported by the Jev Games
// consumer. Each build runs in a child process under a time limit, so a
// regression fails here instead of hanging the suite.
import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { readdirSync } from "node:fs";
import { dirname, join } from "node:path";
import test from "node:test";
import { fileURLToPath, pathToFileURL } from "node:url";

const here = dirname(fileURLToPath(import.meta.url));
const FIXTURES = join(here, "..", "..", "..", "crates", "rete-core", "tests", "fixtures", "louvain-hangs");
const files = readdirSync(FIXTURES).filter((f) => f.endsWith(".nt")).sort();
const ESM = pathToFileURL(join(here, "..", "dist", "index.js")).href;
const BUNDLE = join(here, "..", "dist", "rete-graph.min.js");

// Runs an ES module's text in a child node with a time limit.
function child(code, timeout) {
  const r = spawnSync(process.execPath, ["--input-type=module", "-e", code], {
    timeout,
    encoding: "utf8",
    maxBuffer: 1 << 24,
  });
  return { ok: !r.error && r.status === 0, out: r.stdout ?? "", err: r.stderr ?? "", timedOut: r.error?.code === "ETIMEDOUT" };
}

// Builds every fixture with `api`, opens the result, prints [name, quads] pairs.
const BODY = `
  const out = [];
  for (const f of ${JSON.stringify(files)}) {
    const text = fs.readFileSync(path.join(${JSON.stringify(FIXTURES)}, f), "utf8");
    const g = await api.open(await api.build(text, "nt"));
    out.push([f, g.quads]);
  }
  console.log(JSON.stringify(out));`;

const LOADERS = {
  "ESM entry (dist/index.js)": `const api = await import(${JSON.stringify(ESM)});`,
  "script-tag bundle (dist/rete-graph.min.js)": `
    const { runInThisContext } = await import("node:vm");
    runInThisContext(fs.readFileSync(${JSON.stringify(BUNDLE)}, "utf8"));
    const api = globalThis.rete;`,
};

test("the Louvain fixtures exist", () => {
  assert.ok(files.length >= 5, files.join(", "));
});

for (const [what, load] of Object.entries(LOADERS)) {
  test(`every graph whose build never returned builds: ${what}`, { timeout: 120000 }, () => {
    const r = child(`import fs from "node:fs"; import path from "node:path"; ${load} ${BODY}`, 60000);
    assert.ok(r.ok, r.timedOut ? "a build did not return within 60 s" : r.err.slice(0, 600));
    const got = JSON.parse(r.out);
    assert.equal(got.length, files.length);
    for (const [f, quads] of got) {
      // the statement count each file names (min-15-…, jev-commander-145, …)
      assert.equal(quads, Number(/-(\d+)(?:-level\d)?\.nt$/.exec(f)[1]), f);
    }
  });
}
