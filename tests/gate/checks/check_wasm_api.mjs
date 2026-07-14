import fs from "node:fs";
import vm from "node:vm";
import { TextDecoder, TextEncoder } from "node:util";

const root = process.env.RETE_ROOT || "/work";
const gluePath = process.env.RETE_WASM_GLUE ||
  `${root}/web/pkg-nomodules/rete_wasm.js`;
const wasmPath = process.env.RETE_WASM_BINARY ||
  `${root}/web/pkg-nomodules/rete_wasm_bg.wasm`;

function fail(message) {
  throw new Error(`WASM API contract: ${message}`);
}

const context = vm.createContext({
  console,
  TextDecoder,
  TextEncoder,
  URL,
  WebAssembly,
  Uint8Array,
});
vm.runInContext(fs.readFileSync(gluePath, "utf8"), context, {
  filename: gluePath,
});
const init = vm.runInContext("wasm_bindgen", context);
context.wasmBytes = fs.readFileSync(wasmPath);
vm.runInContext("wasm_bindgen.initSync({ module: wasmBytes })", context);
const api = init;

const stableExports = [
  "Graph",
  "RemoteGraph",
  "build",
  "query",
  "query_sparql",
  "header_ranges",
  "summary_overview",
  "shacl",
  "shacl_url",
  "reason",
  "reason_url",
  "check_schema",
];
for (const name of stableExports) {
  if (typeof api[name] !== "function") fail(`missing documented export ${name}`);
}

const leakedAsyncify = Object.keys(api).filter((name) => /asyncify/i.test(name));
if (leakedAsyncify.length) {
  fail(`raw Asyncify controls leaked: ${leakedAsyncify.join(", ")}`);
}
const asyncGlue = fs.readFileSync(`${root}/docs/rete_wasm_async.js`, "utf8");
if (/exports\.asyncify_(?:start|stop|get)/.test(asyncGlue)) {
  fail("raw Asyncify controls are assigned to the public wrapper");
}

const fixture = [
  "<http://example.test/alice> <http://example.test/knows> ",
  "<http://example.test/bob> .\n",
  "<http://example.test/bob> <http://example.test/name> \"Bob\" .\n",
].join("");
const bytes = api.build(fixture, "nt");
const graph = new api.Graph(bytes);

for (const [label, json] of [
  ["info", graph.info()],
  ["query", graph.query(
    "SELECT ?s WHERE { ?s <http://example.test/name> \"Bob\" }",
    "json",
  )],
  ["header_ranges", api.header_ranges(bytes.slice(0, 1024))],
  ["summary_overview", api.summary_overview(bytes)],
]) {
  const parsed = JSON.parse(json);
  if (parsed.schemaVersion !== 1) {
    fail(`${label} did not return schemaVersion 1`);
  }
}

let malformed;
try {
  new api.Graph(new Uint8Array([1, 2, 3]));
} catch (error) {
  malformed = error;
}
if (!(malformed instanceof Error) && Object.prototype.toString.call(malformed) !== "[object Error]") {
  fail("malformed bytes did not throw an Error object");
}

graph.free();
console.log(JSON.stringify({
  verdict: "PASS",
  exports: stableExports,
  schemaVersion: 1,
}));
