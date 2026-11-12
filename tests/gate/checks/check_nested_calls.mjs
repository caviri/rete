// Both browser engines parse and run function calls nested 100 deep.
//
// The query parser recurses about a dozen functions for each nested call, and
// the JS engine's stack is about 1 MB. The Asyncify build used to be made
// without -O1 (scripts/build_playground_async.sh): its frames were several
// times the plain build's and a query with `IF(…, IF(…, …))` twenty deep
// failed with "Maximum call stack size exceeded" (a nanopub explorer question
// nested 14, under the page's own frames). The plain build takes about 280.
//
// A stack overflow is a trap, and it leaves the instance's stack pointer where
// it was, so each engine gets its own instance and one failure is not blamed on
// the next probe.
import fs from "node:fs";
import vm from "node:vm";
import { TextDecoder, TextEncoder } from "node:util";
import { expect } from "./_expect.mjs";

const root = process.env.RETE_ROOT || "/work";
const DEPTH = 100;
const engines = [
  {
    label: "sequential",
    glue: `${root}/web/pkg-nomodules/rete_wasm.js`,
    wasm: `${root}/web/pkg-nomodules/rete_wasm_bg.wasm`,
  },
  {
    // The files the pages load: the patched glue and the asyncified engine.
    label: "concurrent (Asyncify)",
    glue: `${root}/docs/rete_wasm_async.js`,
    wasm: `${root}/docs/rete_wasm_async.wasm`,
  },
];

const t = expect("check_nested_calls");

function load({ glue, wasm }) {
  const context = vm.createContext({ console, TextDecoder, TextEncoder, URL, WebAssembly, Uint8Array });
  vm.runInContext(fs.readFileSync(glue, "utf8"), context, { filename: glue });
  context.wasmBytes = fs.readFileSync(wasm);
  vm.runInContext("wasm_bindgen.initSync({ module: wasmBytes })", context);
  return vm.runInContext("wasm_bindgen", context);
}

// IF(?o = "v99", "w99", IF(?o = "v98", "w98", … "none")): the row's "v0" is
// matched by the innermost IF, so every level is evaluated.
function nestedIf(n) {
  let e = '"none"';
  for (let i = 0; i < n; i++) e = `IF(?o = "v${i}", "w${i}", ${e})`;
  return `SELECT ?w WHERE { ?s ?p ?o BIND(${e} AS ?w) }`;
}
function nestedStr(n) {
  let e = "?o";
  for (let i = 0; i < n; i++) e = `STR(${e})`;
  return `SELECT ?w WHERE { ?s ?p ?o BIND(${e} AS ?w) }`;
}

function value(api, rete, query) {
  try {
    // query_sparql returns the rows as [{ var: term }], terms in N-Triples form.
    const rows = JSON.parse(api.query_sparql(rete, query));
    return rows.length === 1 ? rows[0].w : `${rows.length} rows`;
  } catch (error) {
    return `threw: ${String((error && error.message) || error).slice(0, 80)}`;
  }
}

let checked = 0;
for (const engine of engines) {
  try {
    const api = load(engine);
    const rete = api.build('<http://e/s> <http://e/p> "v0" .\n', "nt");
    t.equal(`${engine.label}: IF nested ${DEPTH} deep`, value(api, rete, nestedIf(DEPTH)), '"w0"',
      "nested function calls must parse and evaluate");
    t.equal(`${engine.label}: STR nested ${DEPTH} deep`, value(api, rete, nestedStr(DEPTH)), '"v0"',
      "nested function calls must parse and evaluate");
    checked++;
  } catch (error) {
    t.threw(engine.label, error);
  }
}

t.finish({ engines: checked, depth: DEPTH });
