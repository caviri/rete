// The explorer's N-Triples extract and its quoted-triple surface.
//
//   node --test experiments/rete-file-explorer/test/rete-fs.test.mjs
//
// rete-fs.js is shared by the browser explorer and the Tauri app, and its
// N-Triples extract wrote the stored RDF-star token `<<s p o>>`, which current
// RDF 1.2 parsers reject. It now writes what `rete export` writes (#262):
// `<<( s p o )>>` by default, the stored token under "rdf-star", a refusal by
// name for a quoted triple in a subject. The expected strings are the ones
// `rete_core::terms::rdf12_triple_term`'s own tests pin.
import { test } from "node:test";
import assert from "node:assert/strict";
import { extract, ntriplesBody, rdf12TripleTerm } from "../js/rete-fs.js";

const QT = "<<<http://ex/a> <http://ex/b> <http://ex/c>>>";
const QT12 = "<<( <http://ex/a> <http://ex/b> <http://ex/c> )>>";

test("a term that is not a quoted triple comes back unchanged", () => {
  for (const t of [
    "<http://example.org/x>",
    "_:b0",
    '"lit"@en',
    '"5"^^<http://www.w3.org/2001/XMLSchema#integer>',
    '"a > b"',
    '"a > b << c"@en-gb',
  ]) {
    assert.equal(rdf12TripleTerm(t), t);
  }
});

test("an object triple term gets the RDF 1.2 surface, nested ones too", () => {
  assert.equal(rdf12TripleTerm(QT), QT12);
  assert.equal(
    rdf12TripleTerm(`<<<http://ex/x> <http://ex/y> ${QT}>>`),
    `<<( <http://ex/x> <http://ex/y> ${QT12} )>>`,
  );
  // Term boundaries come from a tokenizer, not from splitting on spaces.
  assert.equal(
    rdf12TripleTerm('<<_:b1 <http://ex/p> "a > b << c"@en>>'),
    '<<( _:b1 <http://ex/p> "a > b << c"@en )>>',
  );
  assert.equal(
    rdf12TripleTerm('<<<http://ex/s> <http://ex/p> "say \\"hi\\""^^<http://ex/dt>>>'),
    '<<( <http://ex/s> <http://ex/p> "say \\"hi\\""^^<http://ex/dt> )>>',
  );
});

test("a quoted triple in a subject slot, or a malformed one, has no spelling", () => {
  assert.equal(rdf12TripleTerm(`<<${QT} <http://ex/p> <http://ex/o>>>`), null);
  assert.equal(rdf12TripleTerm(`<<<http://ex/a> <http://ex/b> <<${QT} <http://ex/p> <http://ex/o>>>>>`), null);
  assert.equal(rdf12TripleTerm("<<<http://ex/a> <http://ex/b>>>"), null);
});

const PLAIN = [
  ["<http://ex/s>", "<http://ex/p>", '"a > b << c"@en'],
  ["<http://ex/s>", "<http://ex/p>", "_:b0"],
  ["_:b0", "<http://ex/p>", '"5"^^<http://www.w3.org/2001/XMLSchema#integer>'],
];
// What the extract wrote before this change: the tokens joined, verbatim.
const legacy = (triples) => triples.map((t) => `${t.join(" ")} .`).join("\n");

test("a graph with no quoted triple is byte-for-byte the same under both surfaces", () => {
  assert.equal(ntriplesBody(PLAIN), legacy(PLAIN));
  assert.equal(ntriplesBody(PLAIN, "rdf12"), legacy(PLAIN));
  assert.equal(ntriplesBody(PLAIN, "rdf-star"), legacy(PLAIN));
});

test("rdf12 by default, the stored token under rdf-star", () => {
  const g = [["<http://ex/m>", "<http://ex/about>", QT]];
  assert.equal(ntriplesBody(g), `<http://ex/m> <http://ex/about> ${QT12} .`);
  assert.equal(ntriplesBody(g, "rdf-star"), legacy(g));
});

test("a subject-position quoted triple is refused by name under rdf12", () => {
  const g = [[QT, "<http://ex/src>", "<http://ex/wiki>"]];
  assert.throws(() => ntriplesBody(g), /quoted triple in the subject position[\s\S]*OBJECT position only[\s\S]*rdf-star/);
  const nested = [["<http://ex/m>", "<http://ex/p>", `<<${QT} <http://ex/y> <http://ex/z>>>`]];
  assert.throws(() => ntriplesBody(nested), /nested in the SUBJECT/);
  // The RDF-star surface has a spelling for both.
  assert.equal(ntriplesBody(g, "rdf-star"), legacy(g));
  assert.throws(() => ntriplesBody(g, "rdfstar"), /unknown quotedTripleSyntax/);
});

// extract() end to end over a stub engine context.
const ctxWith = (vars, rows) => ({ select: async () => ({ vars, rows }) });

test("extract writes the RDF 1.2 surface by default and honours the option", async () => {
  const node = { view: "types", iri: "http://ex/Claim" };
  const ctx = ctxWith(["s", "p", "o"], [{ s: "<http://ex/m>", p: "<http://ex/about>", o: QT }]);
  const def = await extract(ctx, node, { format: "nt" });
  assert.equal(def.body, `<http://ex/m> <http://ex/about> ${QT12} .`);
  assert.equal(def.mime, "application/n-triples");
  const star = await extract(ctx, node, { format: "nt", quotedTripleSyntax: "rdf-star" });
  assert.equal(star.body, `<http://ex/m> <http://ex/about> ${QT} .`);

  // A predicate folder puts the predicate back in the middle.
  const pred = await extract(ctxWith(["s", "o"], [{ s: "<http://ex/m>", o: QT }]),
    { view: "predicates", iri: "http://ex/about" }, { format: "nt" });
  assert.equal(pred.body, `<http://ex/m> <http://ex/about> ${QT12} .`);

  // And refuses, rather than writing, a quoted triple as a subject.
  const subj = ctxWith(["s", "p", "o"], [{ s: QT, p: "<http://ex/p>", o: "<http://ex/o>" }]);
  await assert.rejects(extract(subj, node, { format: "nt" }), /subject position/);
});
