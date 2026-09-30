// RDF 1.2 in the JS client: reading RDF 1.2 Turtle/TriG, writing the RDF 1.2
// triple-term surface from the N-Quads writers, the card's quoted-triple
// signal, and the property blank-node labelling must keep — two separate
// parses never share a label.
import assert from "node:assert/strict";
import test from "node:test";

import { build, open } from "../dist/index.js";
import { serveBytes } from "./range-server.mjs";

const RDF = "http://www.w3.org/1999/02/22-rdf-syntax-ns#";

// A triple term; a reifier; an annotation; a directional language literal.
const TTL12 = `@prefix ex: <http://example.test/> .
ex:alice ex:says <<( ex:bob ex:knows ex:carol )>> .
<< ex:bob ex:knows ex:dave >> ex:source ex:wiki .
ex:erin ex:knows ex:frank {| ex:since "2020" |} .
ex:gina ex:name "Gina"@en--ltr .
`;

const TRIG12 = `@prefix ex: <http://example.test/> .
ex:g {
  ex:alice ex:says <<( ex:bob ex:knows ex:carol )>> .
  << ex:bob ex:knows ex:dave >> ex:source ex:wiki .
}
`;

const TTL_STAR = `@prefix ex: <http://example.test/> .
ex:claim ex:states << ex:a ex:p ex:b >> .
`;

// Object-position quoted triples, one nested in another's object slot.
const NT_QUOTED =
  "<http://ex/a> <http://ex/p> <http://ex/b> .\n" +
  "<http://ex/claim> <http://ex/states> << <http://ex/a> <http://ex/p> <http://ex/b> >> .\n" +
  "<http://ex/meta> <http://ex/about> " +
  "<< <http://ex/claim> <http://ex/states> << <http://ex/a> <http://ex/p> <http://ex/b> >> >> .\n";

const NT_SUBJECT =
  "<< <http://ex/a> <http://ex/p> <http://ex/b> >> <http://ex/src> <http://ex/wiki> .\n";

const PLAIN = '<http://ex/a> <http://ex/p> <http://ex/b> .\n<http://ex/b> <http://ex/p> "x"@en .\n';

const ANON = '[] <http://example.test/p> "x" .\n';
const SUBJECTS = 'SELECT ?s WHERE { ?s <http://example.test/p> "x" }';

const CARD = JSON.stringify({ title: "t" });

async function quadSet(g) {
  const out = new Set();
  for await (const q of g.dump({ raw: true })) out.add(JSON.stringify(q));
  return out;
}

// --- reading -----------------------------------------------------------------

test("reads RDF 1.2 Turtle", async () => {
  const g = await open(await build(TTL12, "ttl", { quotedTripleSyntax: "rdf12" }));
  assert.equal(g.quads, 7);
  const reifiers = g.query(`SELECT ?r WHERE { ?r <${RDF}reifies> ?t }`);
  assert.equal(reifiers.length, 2);
  assert.equal(new Set(reifiers.map((r) => r.r.value)).size, 2, "reifiers must be distinct");
  assert.equal(
    g.query("ASK { <http://example.test/erin> <http://example.test/knows> <http://example.test/frank> }"),
    true,
    "an annotated triple is asserted",
  );
});

test("reads RDF 1.2 TriG", async () => {
  const g = await open(await build(TRIG12, "trig", { quotedTripleSyntax: "rdf12" }));
  assert.equal(g.quads, 3);
  assert.deepEqual(g.graphNames(), ["http://example.test/g"]);
});

test("rdf-star is the default reader, and unchanged", async () => {
  await assert.rejects(build(TTL12, "ttl"), /rdf12/);
  await assert.rejects(build(PLAIN, "nt", { quotedTripleSyntax: "rdf13" }), /quoted-triple syntax/);
  const plain = await build(PLAIN);
  assert.deepEqual(await build(PLAIN, "nt", { quotedTripleSyntax: "rdf-star" }), plain);
  assert.deepEqual(await build(PLAIN, "nt", { quotedTripleSyntax: "rdf12" }), plain);
  // The same Turtle bytes are two graphs under the two readers.
  assert.equal((await open(await build(TTL_STAR, "ttl"))).quads, 1);
  assert.equal((await open(await build(TTL_STAR, "ttl", { quotedTripleSyntax: "rdf12" }))).quads, 2);
});

// --- writing -----------------------------------------------------------------

test("the N-Quads writers spell RDF 1.2 triple terms by default", async () => {
  const g = await open(await build(NT_QUOTED));
  const text = await g.toNQuads();
  assert.ok(text.includes("<<( <http://ex/a> <http://ex/p> <http://ex/b> )>>"), text);
  assert.ok(text.includes("<<( <http://ex/claim> <http://ex/states> <<( "), "nested terms respelled");
  assert.ok(!text.replaceAll("<<( ", "").includes("<<"), text);
  const star = await g.toNQuads({ quotedTripleSyntax: "rdf-star" });
  assert.ok(star.includes("<<<http://ex/a> <http://ex/p> <http://ex/b>>>"), star);
  // writeNQuads takes the option too.
  const parts = [];
  await g.writeNQuads((c) => parts.push(c), { quotedTripleSyntax: "rdf-star" });
  assert.equal(parts.join(""), star);
  await assert.rejects(g.toNQuads({ quotedTripleSyntax: "turtle" }), /quoted-triple syntax/);
});

test("RDF 1.2 round trip: Turtle -> N-Quads -> .rete is the same graph", async () => {
  const first = await open(await build(TTL12, "ttl", { quotedTripleSyntax: "rdf12" }));
  const dump = await first.toNQuads();
  assert.ok(dump.includes("<<( "), dump);
  const again = await open(await build(dump, "nq"));
  assert.deepEqual(await quadSet(again), await quadSet(first));
  const star = await open(await build(await first.toNQuads({ quotedTripleSyntax: "rdf-star" }), "nq"));
  assert.deepEqual(await quadSet(star), await quadSet(first));
});

test("the RDF 1.2 dump loads in Oxigraph; the stored spelling does not", async () => {
  const { Store } = await import("oxigraph");
  const g = await open(await build(NT_QUOTED));
  const store = new Store();
  for await (const chunk of g.nquads({ batch: 1 })) {
    store.load(chunk, { format: "application/n-quads" });
  }
  assert.equal(store.size, 3);
  assert.throws(() =>
    new Store().load(
      "<http://ex/c> <http://ex/s> <<<http://ex/a> <http://ex/p> <http://ex/b>>> .\n",
      { format: "application/n-quads" },
    ),
  );
});

test("a subject-position quoted triple is refused under rdf12 only", async () => {
  const g = await open(await build(NT_SUBJECT));
  await assert.rejects(g.toNQuads(), /rdf-star/);
  assert.ok((await g.toNQuads({ quotedTripleSyntax: "rdf-star" })).startsWith("<<<http://ex/a>"));
});

test("a graph without quoted triples is written identically either way", async () => {
  const g = await open(await build(PLAIN));
  assert.equal(await g.toNQuads(), await g.toNQuads({ quotedTripleSyntax: "rdf-star" }));
});

test("the RDF 1.2 writer works over a remote graph", async () => {
  const { wasm } = await import("../dist/index.js");
  const data = wasm.build_with_card(NT_QUOTED, "nt", CARD);
  const server = await serveBytes(data);
  try {
    const g = await open(server.url);
    assert.ok((await g.toNQuads()).includes("<<( "));
    assert.equal(g.card().signals.quoted_triples.present, true);
  } finally {
    await server.close();
  }
});

// --- the card ----------------------------------------------------------------

test("the card reports quoted triples from the header", async () => {
  const { wasm } = await import("../dist/index.js");
  const card = async (text) => (await open(wasm.build_with_card(text, "nt", CARD))).card();
  assert.deepEqual((await card(NT_QUOTED)).signals.quoted_triples, {
    present: true,
    export_surfaces: ["rdf12", "rdf-star"],
    export_default: "rdf12",
  });
  assert.deepEqual((await card(PLAIN)).signals.quoted_triples, { present: false });
  const bytes = wasm.build_with_card(NT_QUOTED, "nt", CARD);
  assert.ok(!Buffer.from(bytes).includes("quoted_triples"), "measured, never stored");
  assert.equal((await open(await build(NT_QUOTED))).card(), null, "no card stays no card");
});

// --- blank nodes -------------------------------------------------------------

test("blank nodes of separate parses stay distinct when merged", async () => {
  const labels = [];
  for (const syntax of ["rdf12", "rdf12", "rdf-star", "rdf-star"]) {
    const g = await open(await build(ANON, "ttl", { quotedTripleSyntax: syntax }));
    labels.push(...g.query(SUBJECTS).map((r) => r.s.value));
  }
  assert.equal(labels.length, 4, String(labels));
  assert.ok(labels.every((l) => /^_:\S+$/.test(l)), String(labels));
  assert.equal(new Set(labels).size, 4, `blank nodes collided: ${labels}`);
  const merged = labels.map((l) => `${l} <http://example.test/p> "x" .\n`).join("");
  const g = await open(await build(merged));
  assert.equal(new Set(g.query(SUBJECTS).map((r) => r.s.value)).size, 4);
});
