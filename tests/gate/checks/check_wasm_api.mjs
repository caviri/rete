// The documented WASM surface. Every assertion is COLLECTED, not thrown (see
// _expect.mjs): the gate runner reads the last JSON object this prints, so a
// contract break has to arrive as `{"verdict":"FAIL", failures:[…]}` with the
// value that was actually found — not as a stack trace the runner clips to 160
// characters.
import fs from "node:fs";
import vm from "node:vm";
import { TextDecoder, TextEncoder } from "node:util";
import { expect } from "./_expect.mjs";

const root = process.env.RETE_ROOT || "/work";
const gluePath = process.env.RETE_WASM_GLUE ||
  `${root}/web/pkg-nomodules/rete_wasm.js`;
const wasmPath = process.env.RETE_WASM_BINARY ||
  `${root}/web/pkg-nomodules/rete_wasm_bg.wasm`;

const t = expect("check_wasm_api");

const stableExports = [
  "Graph",
  "RemoteGraph",
  "build",
  "build_with_card",
  "build_with_derived_card",
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
let browserCardKeys = [];

try {
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

  for (const name of stableExports) {
    t.equal(`export:${name}`, typeof api[name], "function", "documented export must be a function");
  }

  const leakedAsyncify = Object.keys(api).filter((name) => /asyncify/i.test(name));
  t.deepEqual("leakedAsyncifyControls", leakedAsyncify, [], "raw Asyncify controls must not be exported");
  const asyncGlue = fs.readFileSync(`${root}/docs/rete_wasm_async.js`, "utf8");
  t.ok("asyncifyControlsOffThePublicWrapper", !/exports\.asyncify_(?:start|stop|get)/.test(asyncGlue),
    "raw Asyncify controls are assigned to the public wrapper");

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
    let parsed = null;
    try { parsed = JSON.parse(json); } catch (error) { t.threw(`${label}:parse`, error); }
    if (parsed) t.equal(`${label}.schemaVersion`, parsed.schemaVersion, 1);
  }

  let malformed;
  try {
    new api.Graph(new Uint8Array([1, 2, 3]));
  } catch (error) {
    malformed = error;
  }
  t.ok("malformedBytesThrowAnError",
    malformed instanceof Error || Object.prototype.toString.call(malformed) === "[object Error]",
    `malformed bytes did not throw an Error object (got ${Object.prototype.toString.call(malformed)})`);

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
    if (!msg) t.fail(`validate_card rejects ${label}`, "validate_card accepted it", { actual: "", expected: `matches ${mustSay}` });
    else t.match(`validate_card explains ${label}`, msg, mustSay, "the message must say why");
  }
  t.equal("validate_card accepts a valid card", api.validate_card('{"title":"T","keywords":["a"],"extra":{"k":1}}'), "");

  const carded = api.build_with_card(fixture, "nt", JSON.stringify({
    title: "Written in a browser",
    keywords: ["b", "a"],
    extra: { internal_id: "X-1" },
  }));
  const cardedGraph = new api.Graph(carded);
  const envelope = JSON.parse(cardedGraph.card_and_build());
  t.equal("card_and_build.schemaVersion", envelope.schemaVersion, 1);
  t.ok("build_with_card wrote a card", !!envelope.card, "build_with_card wrote no card");
  const writtenCard = envelope.card ? JSON.parse(envelope.card) : {};
  browserCardKeys = Object.keys(writtenCard).sort();
  t.equal("writtenCard.title", writtenCard.title, "Written in a browser", "the written card lost its title");
  // Canonicalized by the same rules the CLI applies: sorted and deduplicated.
  t.equal("writtenCard.keywords", JSON.stringify(writtenCard.keywords), '["a","b"]', "keywords were not canonicalized");
  // Counts are MEASURED by the build, not asserted by the author.
  t.equal("writtenCard.triple_count", writtenCard.triple_count, 2, "counts were not measured");
  t.ok("writtenCard.term_count", writtenCard.term_count >= 1, `term_count was ${writtenCard.term_count}`);
  // `build_with_card` stays CURATED-ONLY. Not because the browser cannot derive
  // — it can, see `build_with_derived_card` below — but because this function's
  // bytes are a shipped contract: a caller who did not ask for the extra passes
  // must keep getting the file they always got. Absence here is deliberate, and
  // it must be absence, not an empty list that reads like a measured zero.
  for (const derived of ["predicates", "classes", "vocabularies", "queries", "signals", "top_n"]) {
    t.ok(`browserCardOmits:${derived}`, !(derived in writtenCard), "a browser build claimed a derived field");
  }
  // Likewise the build record: null, never an empty object.
  t.equal("browserBuildRecord", envelope.build, null, "a browser build must not write a build record");

  // --- the opt-in derived card (#152) ---------------------------------------
  // The other half of the same contract: ask for derivation and the browser
  // computes the profile the CLI computes, from the code the CLI runs.
  // Its own input, typed: `classes` is derived from rdf:type assertions, and
  // the shared 2-triple fixture has none — an absent `classes` there would be
  // honest absence, not a defect, so it cannot witness derivation.
  const typedFixture = fixture +
    "<http://example.test/alice> <http://www.w3.org/1999/02/22-rdf-syntax-ns#type> " +
    "<http://xmlns.com/foaf/0.1/Person> .\n";
  const derivedBytes = api.build_with_derived_card(typedFixture, "nt", JSON.stringify({
    title: "Derived in a browser",
    keywords: ["b", "a"],
  }));
  const derivedGraph = new api.Graph(derivedBytes);
  const derivedEnvelope = JSON.parse(derivedGraph.card_and_build());
  const derivedCard = derivedEnvelope.card ? JSON.parse(derivedEnvelope.card) : {};
  t.equal("derivedCard.title", derivedCard.title, "Derived in a browser");
  t.equal("derivedCard.keywords", JSON.stringify(derivedCard.keywords), '["a","b"]',
    "the curated half must still be canonicalized");
  for (const derived of ["predicates", "classes", "vocabularies", "queries", "signals", "top_n"]) {
    t.ok(`derivedCardHas:${derived}`, derived in derivedCard,
      `build_with_derived_card did not write ${derived}`);
  }
  // The cap the lists were derived under is the CLI's, so the two agree.
  t.equal("derivedCard.top_n", derivedCard.top_n, 100);
  t.ok("derivedQueryCount", (derivedCard.queries || []).length > 0, "no starter queries were generated");
  // Shape only, deliberately: this harness runs the engine in a bare
  // `node:vm` context with no Web Crypto, and every aggregate query traps
  // there (`could not initialize thread_rng`) — an environment limit of the
  // sandbox, not of the file. That the generated queries RETURN ROWS is
  // asserted where a real engine runs them: rete-core's
  // `every_generated_query_returns_rows`, and check_card_examples in a browser.
  for (const q of derivedCard.queries || []) {
    const wellFormed = q && typeof q.id === "string" && typeof q.sparql === "string" &&
      q.sparql.includes("PREFIX rdf:") && !q.sparql.includes("{{");
    t.ok(`derivedQueryWellFormed:${q && q.id}`, wellFormed,
      "a generated query is missing its PREFIX block or left a placeholder unsubstituted");
  }
  // Still no build record: its cost figures come from RUNNING the queries,
  // which is a benchmark, not a build.
  t.equal("derivedBuildRecord", derivedEnvelope.build, null,
    "a browser build must not write a build record, derived or not");
  derivedGraph.free();

  // A cardless build stays byte-identical to the old `build` — the card path must
  // not have changed what a card-free file looks like.
  const plain = api.build_with_card(fixture, "nt", "");
  t.ok("build_with_card('') is byte-identical to build()",
    Buffer.compare(Buffer.from(plain), Buffer.from(api.build(fixture, "nt"))) === 0);
  t.equal("cardless file reports no card", JSON.parse(api.card_and_build(plain)).card, null);
  cardedGraph.free();

  graph.free();
} catch (error) {
  t.threw("WASM API contract", error);
}

t.finish({
  exports: stableExports,
  schemaVersion: 1,
  browserCardKeys,
});
