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
  "build_with_card",
  "validate_card",
  "card",
  "card_and_build",
  "card_and_build_url",
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

// --- the in-browser card writer -------------------------------------------
// The playground's Build mode hands a `--card-file` document to the engine, so
// the engine has to (a) refuse exactly what the CLI refuses, and (b) write a
// card the reader gets back.
for (const [label, doc, mustSay] of [
  ["a stray top-level key", '{"title":"T","region":"CH"}', /unknown field `region`/],
  ["a free-text theme", '{"theme":["physics"]}', /not an IRI/],
  ["a theme pointing at keywords", '{"theme":["physics"]}', /keywords/],
  ["an over-deep extra value", '{"extra":{"a":{"b":{"c":{"d":1}}}}}', /level cap/],
  ["a reserved extra key", '{"extra":{"@context":"x"}}', /reserved/],
]) {
  const msg = api.validate_card(doc);
  if (!msg) fail(`validate_card accepted ${label}`);
  if (!mustSay.test(msg)) fail(`validate_card's message for ${label} does not say why: ${msg}`);
}
if (api.validate_card('{"title":"T","keywords":["a"],"extra":{"k":1}}') !== "") {
  fail("validate_card rejected a valid card");
}

const carded = api.build_with_card(fixture, "nt", JSON.stringify({
  title: "Written in a browser",
  keywords: ["b", "a"],
  extra: { internal_id: "X-1" },
}));
const cardedGraph = new api.Graph(carded);
const envelope = JSON.parse(cardedGraph.card_and_build());
if (envelope.schemaVersion !== 1) fail("card_and_build did not return schemaVersion 1");
if (!envelope.card) fail("build_with_card wrote no card");
const writtenCard = JSON.parse(envelope.card);
if (writtenCard.title !== "Written in a browser") fail("the written card lost its title");
// Canonicalized by the same rules the CLI applies: sorted and deduplicated.
if (JSON.stringify(writtenCard.keywords) !== '["a","b"]') {
  fail(`keywords were not canonicalized: ${JSON.stringify(writtenCard.keywords)}`);
}
// Counts are MEASURED by the build, not asserted by the author.
if (writtenCard.triple_count !== 2 || writtenCard.term_count < 1) {
  fail(`counts were not measured: ${JSON.stringify(writtenCard)}`);
}
// The derived profile the browser cannot compute must be ABSENT — not an empty
// list that reads like "measured, and there were none".
for (const derived of ["predicates", "classes", "vocabularies", "queries", "signals", "top_n"]) {
  if (derived in writtenCard) fail(`a browser build claimed a derived field: ${derived}`);
}
// Likewise the build record: null, never an empty object.
if (envelope.build !== null) fail(`a browser build wrote a build record: ${envelope.build}`);

// A cardless build stays byte-identical to the old `build` — the card path must
// not have changed what a card-free file looks like.
const plain = api.build_with_card(fixture, "nt", "");
if (Buffer.compare(Buffer.from(plain), Buffer.from(api.build(fixture, "nt"))) !== 0) {
  fail("build_with_card('') is not byte-identical to build()");
}
if (JSON.parse(api.card_and_build(plain)).card !== null) {
  fail("a cardless file reported a card");
}
cardedGraph.free();

graph.free();
console.log(JSON.stringify({
  verdict: "PASS",
  exports: stableExports,
  schemaVersion: 1,
  browserCardKeys: Object.keys(writtenCard).sort(),
}));
