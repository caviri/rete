// FILTER type-error diagnostics through the JS client: `lastWarnings()` and the
// `warnings` member of the raw envelope. Row counts must not change — these are
// the shapes an agent reported as "CONTAINS / STRSTARTS / REGEX unsupported".
import assert from "node:assert/strict";
import test from "node:test";

import { build, open } from "../dist/index.js";
import { serveBytes } from "./range-server.mjs";

const TTL = `@prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> .
<http://ex.org/map/geneva-1572> rdfs:label "Geneva town plan"@en ; <http://ex.org/year> 1572 .`;
const P = "PREFIX rdfs: <http://www.w3.org/2000/01/rdf-schema#> ";

// [query, rows, expected warning (function, argument, argKind, severity) or null]
const TABLE = [
  [`SELECT ?s WHERE { ?s rdfs:label ?l FILTER(CONTAINS(?l, "Geneva")) }`, 1, null],
  [`SELECT ?s WHERE { ?s rdfs:label ?l FILTER(CONTAINS(?s, "geneva")) }`, 0, ["CONTAINS", 1, "iri", "type-error"]],
  [`SELECT ?s WHERE { ?s rdfs:label ?l FILTER(CONTAINS(STR(?s), "geneva")) }`, 1, null],
  [`SELECT ?s WHERE { ?s <http://ex.org/year> ?y FILTER(STRSTARTS(?y, "15")) }`, 0, ["STRSTARTS", 1, "numeric", "type-error"]],
  [`SELECT ?s WHERE { ?s <http://ex.org/year> ?y FILTER(STRSTARTS(STR(?y), "15")) }`, 1, null],
  [`SELECT ?s WHERE { ?s rdfs:label ?l FILTER(REGEX(?s, "geneva")) }`, 0, ["REGEX", 1, "iri", "type-error"]],
  [`SELECT ?s WHERE { ?s rdfs:label ?l FILTER(CONTAINS(?l, "Geneva"@fr)) }`, 0, ["CONTAINS", 2, "language-mismatch", "type-error"]],
  [`SELECT ?s WHERE { ?s rdfs:label ?l FILTER(CONTAINS(?l, "geneva")) }`, 0, ["CONTAINS", 2, "case-sensitive", "hint"]],
];

function check(g) {
  for (const [q, rows, want] of TABLE) {
    const result = g.query(P + q);
    assert.equal(result.length, rows, q);
    const w = g.lastWarnings();
    if (!want) {
      assert.deepEqual(w, [], q);
      continue;
    }
    assert.equal(w.length, 1, `${q}: ${JSON.stringify(w)}`);
    const [fn, arg, kind, severity] = want;
    assert.equal(w[0].function, fn);
    assert.equal(w[0].argument, arg);
    assert.equal(w[0].argKind, kind);
    assert.equal(w[0].severity, severity);
    assert.equal(typeof w[0].message, "string");
    assert.ok(w[0].hint.length > 0);
  }
}

test("query(): row counts unchanged, warnings via lastWarnings()", async () => {
  const g = await open(await build(TTL, "ttl"));
  check(g);

  const iri = g.query(P + TABLE[1][0]);
  const [w] = g.lastWarnings();
  assert.equal(w.sample, "<http://ex.org/map/geneva-1572>");
  assert.match(w.message, /CONTAINS received an IRI as argument 1/);
  assert.match(w.hint, /STR\(\)/);
  // Rows stay a plain array: nothing extra to iterate or serialize.
  assert.deepEqual(Object.keys(iri), []);
  assert.equal(JSON.stringify(iri), "[]");

  // The next query resets them; a copy is returned, not internal state.
  g.lastWarnings().push("x");
  g.query(P + TABLE[0][0]);
  assert.deepEqual(g.lastWarnings(), []);

  // ASK (a boolean result) reports through the same channel.
  assert.equal(g.query(P + `ASK { ?s rdfs:label ?l FILTER(STRENDS(?s, "1572")) }`), false);
  assert.equal(g.lastWarnings()[0].function, "STRENDS");
});

test("queryRaw(): the envelope carries `warnings` only when there are some", async () => {
  const g = await open(await build(TTL, "ttl"));
  const bad = g.queryRaw(P + TABLE[1][0]);
  assert.equal(bad.kind, "select");
  assert.equal(bad.warnings.length, 1);
  assert.equal(bad.warnings[0].argKind, "iri");
  const good = g.queryRaw(P + TABLE[0][0]);
  assert.equal("warnings" in good, false);
});

test("remote graph over HTTP Range reports the same warnings", async () => {
  const server = await serveBytes(await build(TTL, "ttl"));
  try {
    check(await open(server.url));
  } finally {
    await server.close();
  }
});

test("a failing query leaves no stale warnings behind", async () => {
  const g = await open(await build(TTL, "ttl"));
  g.query(P + TABLE[1][0]);
  assert.equal(g.lastWarnings().length, 1);
  assert.throws(() => g.query("SELECT ?s WHERE { ?s ?p ?o FILTER(<http://ex.org/fn#m>(?o)) }"),
    /function <http:\/\/ex.org\/fn#m> is not implemented/);
  assert.deepEqual(g.lastWarnings(), []);
});
