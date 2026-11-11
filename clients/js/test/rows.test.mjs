// The result handle: `queryRows()` pages a SELECT without decoding the rest.
import assert from "node:assert/strict";
import test from "node:test";

import { build, open } from "../dist/index.js";
import { serveBytes } from "./range-server.mjs";

const NT = Array.from({ length: 300 }, (_, i) => {
  const s = `<http://example.org/s${String(i).padStart(3, "0")}>`;
  const lines = [
    `${s} <http://example.org/type> <http://example.org/T${i % 4}> .`,
    `${s} <http://example.org/name> "name ${i}" .`,
  ];
  if (i % 3 === 0) lines.push(`${s} <http://example.org/alt> "alt ${i}"@en .`);
  return lines.join("\n");
}).join("\n");

const flat = (rows) => rows.map((r) => Object.fromEntries(Object.entries(r).map(([k, t]) => [k, t.n3])));

test("pages are the rows of query(), in order", async () => {
  const g = await open(await build(NT));
  for (const q of [
    "SELECT ?s ?n WHERE { ?s <http://example.org/name> ?n }",
    "SELECT ?s ?a WHERE { ?s <http://example.org/type> <http://example.org/T1> . OPTIONAL { ?s <http://example.org/alt> ?a } } ORDER BY DESC(?s)",
    "SELECT DISTINCT ?t WHERE { ?s <http://example.org/type> ?t }",
  ]) {
    const want = flat(g.query(q));
    const h = g.queryRows(q);
    assert.equal(h.length, want.length);
    assert.deepEqual(h.vars, q.includes("DISTINCT") ? ["t"] : q.includes("?a") ? ["s", "a"] : ["s", "n"]);
    const got = [];
    for (let at = 0; at < h.length; at += 40) got.push(...flat(h.page(at, 40)));
    assert.deepEqual(got, want);
    assert.deepEqual(flat(h.all()), want);
    assert.deepEqual(h.page(h.length, 10), []);
    assert.equal(h.page(h.length - 3, 10).length, 3);
    h.free();
  }
});

test("a handle is for SELECT only, and over a remote file it pages too", async () => {
  const data = await build(NT);
  const g = await open(data);
  assert.throws(() => g.queryRows("ASK { ?s ?p ?o }"), /SELECT/);

  const server = await serveBytes(data);
  try {
    const r = await open(server.url);
    const q = "SELECT ?s ?n WHERE { ?s <http://example.org/name> ?n } ORDER BY ?n";
    const h = r.queryRows(q);
    assert.equal(h.length, 300);
    const first = flat(h.page(0, 5));
    assert.deepEqual(first, flat(g.query(q)).slice(0, 5));
    h.free();
  } finally {
    await server.close();
  }
});
