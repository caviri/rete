// The browser engine reads RDF 1.2 Turtle/TriG, and its blank nodes stay apart.
//
// `rete-wasm` builds rete-core with `rdf12-turtle`, the `oxttl` 0.2 reader. That
// reader labels every anonymous blank node (`[]`, a reifier `<< … >>`, an
// annotation `{| … |}`) with `oxrdf` 0.3's `BlankNode::default()`, a random
// u128 drawn through `rand` 0.9 from getrandom 0.3, which the browser engine
// feeds from crypto.getRandomValues (the `wasm-js` feature). This check proves
// three things against the SHIPPED no-modules package:
//
//  1. RDF 1.2 Turtle and TriG build under `build_with_card_syntax(…, "rdf12")`
//     into the graph RDF 1.2 says they mean: triple terms, reifiers,
//     annotations, directional literals, named graphs.
//  2. The RDF-star default is untouched: it refuses `<<(` by name, and the new
//     export under "rdf-star" is byte-identical to `build_with_card`.
//  3. Blank nodes from separately parsed documents never collide: two builds in
//     one engine instance and one in a SECOND instance (a fresh module, as a
//     second worker or page load would be), merged into one store, keep every
//     blank node distinct. A counter restarted per instance would fail here.
//
// Each engine instance is a bare `node:vm` context given Node's Web Crypto as
// `crypto`, which is exactly the global a browser worker has.
import fs from "node:fs";
import vm from "node:vm";
import { webcrypto } from "node:crypto";
import { TextDecoder, TextEncoder } from "node:util";
import { expect } from "./_expect.mjs";

const root = process.env.RETE_ROOT || "/work";
const gluePath = process.env.RETE_WASM_GLUE || `${root}/web/pkg-nomodules/rete_wasm.js`;
const wasmPath = process.env.RETE_WASM_BINARY || `${root}/web/pkg-nomodules/rete_wasm_bg.wasm`;

const t = expect("check_wasm_rdf12");

function engine() {
  const context = vm.createContext({
    console, TextDecoder, TextEncoder, URL, WebAssembly, Uint8Array, crypto: webcrypto,
  });
  vm.runInContext(fs.readFileSync(gluePath, "utf8"), context, { filename: gluePath });
  context.wasmBytes = fs.readFileSync(wasmPath);
  vm.runInContext("wasm_bindgen.initSync({ module: wasmBytes })", context);
  return vm.runInContext("wasm_bindgen", context);
}

// Every binding of `name` in a SELECT result, as the stored token strings.
function column(api, bytes, sparql, name) {
  const g = new api.Graph(bytes);
  try {
    const res = JSON.parse(g.query(sparql, "json"));
    const rows = res.rows || (res.results && res.results.bindings) || [];
    return rows.map((r) => {
      const v = r[name];
      return v && typeof v === "object" ? (v.value !== undefined ? v.value : JSON.stringify(v)) : v;
    });
  } finally {
    g.free();
  }
}

const RDF = "http://www.w3.org/1999/02/22-rdf-syntax-ns#";
const TTL = `@prefix ex: <http://example.test/> .
ex:alice ex:says <<( ex:bob ex:knows ex:carol )>> .
<< ex:bob ex:knows ex:dave >> ex:source ex:wiki .
ex:erin ex:knows ex:frank {| ex:since "2020" |} .
ex:gina ex:name "Gina"@en--ltr .
`;
const TRIG = `@prefix ex: <http://example.test/> .
ex:g {
  ex:alice ex:says <<( ex:bob ex:knows ex:carol )>> .
  << ex:bob ex:knows ex:dave >> ex:source ex:wiki .
}
`;
const ANON = `[] <http://example.test/p> "x" .\n`;
const SUBJECTS = `SELECT ?s WHERE { ?s <http://example.test/p> "x" }`;

let summary = {};
try {
  const a = engine();
  const b = engine();
  t.equal("export:build_with_card_syntax", typeof a.build_with_card_syntax, "function");

  // 1. RDF 1.2 Turtle.
  const ttl = a.build_with_card_syntax(TTL, "ttl", "", "rdf12");
  const ttlInfo = JSON.parse(a.info(ttl));
  // 1 triple-term statement; reifier: rdf:reifies + ex:source; annotation: the
  // asserted triple + rdf:reifies + ex:since; the directional literal.
  t.equal("rdf12 turtle statements", ttlInfo.quads, 7);
  const reifiers = column(a, ttl, `SELECT ?r WHERE { ?r <${RDF}reifies> ?t }`, "r");
  t.equal("rdf12 turtle reifiers", reifiers.length, 2, "one reifier and one annotation");
  t.ok("reifiers are blank nodes", reifiers.every((r) => String(r).startsWith("_:")),
    `reifiers were ${JSON.stringify(reifiers)}`);
  t.equal("reifiers are distinct", new Set(reifiers).size, 2);
  const says = column(a, ttl, `SELECT ?o WHERE { <http://example.test/alice> <http://example.test/says> ?o }`, "o");
  t.equal("triple term stored", says.length, 1);
  t.match("triple term is the canonical token", String(says[0]), /^<<\s*<http:\/\/example\.test\/bob>/);
  const asserted = column(a, ttl, `SELECT ?o WHERE { <http://example.test/erin> <http://example.test/knows> ?o }`, "o");
  t.equal("annotated triple is asserted", asserted.length, 1);

  // 1b. RDF 1.2 TriG: same surface, inside a named graph.
  const trig = a.build_with_card_syntax(TRIG, "trig", "", "rdf12");
  const trigInfo = JSON.parse(a.info(trig));
  t.equal("rdf12 trig statements", trigInfo.quads, 3);
  t.equal("rdf12 trig named graphs", trigInfo.namedGraphs, 1);

  // 2. The default is untouched.
  let refusal = "";
  try { a.build_with_card(TTL, "ttl", ""); } catch (e) { refusal = String(e && e.message || e); }
  t.match("rdf-star refuses <<( by name", refusal, /rdf12/, "the RDF-star parse must fail and name the flag");
  let unknown = "";
  try { a.build_with_card_syntax(TTL, "ttl", "", "rdf13"); } catch (e) { unknown = String(e && e.message || e); }
  t.match("unknown syntax refused", unknown, /unknown quoted-triple syntax/);
  const star = `<http://example.test/a> <http://example.test/p> "x" .\n`;
  for (const [fmt, syntax] of [["nt", "rdf-star"], ["nt", ""], ["ttl", "rdf-star"]]) {
    const same = Buffer.compare(
      Buffer.from(a.build_with_card_syntax(star, fmt, "", syntax)),
      Buffer.from(a.build_with_card(star, fmt, "")),
    ) === 0;
    t.ok(`byte-identical to build_with_card (${fmt}, ${JSON.stringify(syntax)})`, same);
  }

  // 3. Blank nodes of separately parsed documents never collide, within one
  // instance or across two, and stay apart when merged into one store.
  const labels = [
    ...column(a, a.build_with_card_syntax(ANON, "ttl", "", "rdf12"), SUBJECTS, "s"),
    ...column(a, a.build_with_card_syntax(ANON, "ttl", "", "rdf12"), SUBJECTS, "s"),
    ...column(b, b.build_with_card_syntax(ANON, "ttl", "", "rdf12"), SUBJECTS, "s"),
    // The RDF-star reader (oxttl 0.1, getrandom 0.2) labels `[]` the same way.
    ...column(b, b.build_with_card(ANON, "ttl", ""), SUBJECTS, "s"),
  ];
  t.equal("one blank node per document", labels.length, 4);
  t.equal("blank nodes of four parses are distinct", new Set(labels).size, 4,
    `labels were ${JSON.stringify(labels)}`);
  const merged = labels.map((l) => `${l} <http://example.test/p> "x" .`).join("\n") + "\n";
  const mergedSubjects = column(a, a.build(merged, "nt"), SUBJECTS, "s");
  t.equal("merged store keeps every blank node", new Set(mergedSubjects).size, 4);
  summary = { ttlStatements: ttlInfo.quads, trigStatements: trigInfo.quads, blankLabels: labels };
} catch (error) {
  t.threw("RDF 1.2 in the browser engine", error);
}
t.finish(summary);
