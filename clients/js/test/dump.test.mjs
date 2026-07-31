// The lazy quad dump: `graph.dump()`, `graph.nquads()`, `writeNQuads()`,
// `toNQuads()`. Proves round-trip fidelity (dump → rebuild → same graph),
// named-graph preservation, and that the cursor really is incremental.
import assert from "node:assert/strict";
import { mkdtemp, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { PassThrough } from "node:stream";
import test from "node:test";
import { pathToFileURL } from "node:url";

import { build, init, open, wasm } from "../dist/index.js";
import { serveBytes } from "./range-server.mjs";

// A dataset with a default graph AND two named graphs, plus the term shapes a
// serializer can get wrong: an escaped quote, a lang tag, a typed literal.
const NQ = `\
<http://example.org/alice> <http://www.w3.org/1999/02/22-rdf-syntax-ns#type> <http://example.org/Person> .
<http://example.org/alice> <http://www.w3.org/2000/01/rdf-schema#label> "Alice \\"the researcher\\"" .
<http://example.org/bob> <http://example.org/age> "42"^^<http://www.w3.org/2001/XMLSchema#integer> .
<http://example.org/alice> <http://example.org/knows> <http://example.org/bob> <http://example.org/social> .
<http://example.org/bob> <http://example.org/knows> <http://example.org/alice> <http://example.org/social> .
<http://example.org/bob> <http://www.w3.org/2000/01/rdf-schema#label> "Bob"@en <http://example.org/labels> .
`;

const SOCIAL = "http://example.org/social";
const LABELS = "http://example.org/labels";

/** Every quad as a comparable `s p o g` string, order-independent. */
async function collect(graph, options) {
  const out = [];
  for await (const [s, p, o, g] of graph.dump(options)) {
    out.push(`${s.n3} ${p.n3} ${o.n3} ${g === null ? "" : g.n3}`.trim());
  }
  return out.sort();
}

/** The lines of an N-Quads document, sorted; blank/trailing lines dropped. */
const lines = (text) => text.split("\n").filter(Boolean).sort();

test("dump() streams every quad of every graph, as Terms", async () => {
  const g = await open(await build(NQ, "nq"));
  const all = await collect(g);

  assert.equal(all.length, 6);
  assert.equal(all.length, g.quads);
  assert.deepEqual(
    all,
    lines(NQ).map((l) => l.replace(/ \.$/, "").trim()).sort(),
  );
});

test("dump() yields parsed Terms, with null for the default graph", async () => {
  const g = await open(await build(NQ, "nq"));
  const byObject = new Map();
  for await (const quad of g.dump()) byObject.set(quad[2].value, quad);

  const [s, p, o, graph] = byObject.get("Alice \"the researcher\"");
  assert.equal(s.kind, "iri");
  assert.equal(s.value, "http://example.org/alice");
  assert.equal(p.value, "http://www.w3.org/2000/01/rdf-schema#label");
  assert.equal(o.kind, "literal");
  assert.equal(graph, null); // default graph

  assert.equal(byObject.get("42")[2].toJS(), 42);
  assert.equal(byObject.get("Bob")[2].lang, "en");
  assert.equal(byObject.get("Bob")[3].value, LABELS); // named graph Term
});

test("named graphs are preserved and individually selectable", async () => {
  const g = await open(await build(NQ, "nq"));
  assert.deepEqual(g.graphNames().sort(), [LABELS, SOCIAL]);

  // undefined = every graph; null = the default graph only; IRI = that graph.
  const everything = await collect(g);
  const dflt = await collect(g, { graph: null });
  const social = await collect(g, { graph: SOCIAL });
  const labels = await collect(g, { graph: LABELS });

  assert.equal(dflt.length, 3);
  assert.equal(social.length, 2);
  assert.equal(labels.length, 1);
  assert.equal(dflt.length + social.length + labels.length, everything.length);

  // Each per-graph slice carries its own graph tag; the default one carries none.
  assert.ok(dflt.every((l) => !l.includes(SOCIAL) && !l.includes(LABELS)));
  assert.ok(social.every((l) => l.endsWith(`<${SOCIAL}>`)));
  assert.equal(labels[0], `<http://example.org/bob> <http://www.w3.org/2000/01/rdf-schema#label> "Bob"@en <${LABELS}>`);

  // An unknown graph is empty, not an error.
  assert.deepEqual(await collect(g, { graph: "http://example.org/nope" }), []);
});

test("round-trip: dump to N-Quads, rebuild, get the same dataset back", async () => {
  const original = await open(await build(NQ, "nq"));
  const text = await original.toNQuads();

  const rebuilt = await open(await build(text, "nq"));
  assert.equal(rebuilt.quads, original.quads);
  assert.deepEqual(rebuilt.graphNames().sort(), original.graphNames().sort());
  assert.deepEqual(await collect(rebuilt), await collect(original));

  // And the rebuilt file answers the same queries, including in a named graph.
  const q = "SELECT ?o WHERE { GRAPH <http://example.org/social> { <http://example.org/alice> <http://example.org/knows> ?o } }";
  assert.deepEqual(rebuilt.query(q), original.query(q));

  // Byte-for-byte the same serialization the second time around.
  assert.deepEqual(lines(await rebuilt.toNQuads()), lines(text));
});

test("nquads() chunks are complete lines and concatenate to the document", async () => {
  const g = await open(await build(NQ, "nq"));
  const chunks = [];
  for await (const chunk of g.nquads({ batch: 2 })) {
    assert.ok(chunk.endsWith("\n"), "a chunk must end on a line boundary");
    chunks.push(chunk);
  }
  assert.ok(chunks.length > 1, "batch:2 over 6 quads must produce several chunks");
  assert.deepEqual(lines(chunks.join("")), lines(await g.toNQuads()));
});

test("writeNQuads() into a function sink and into a Node stream", async () => {
  const g = await open(await build(NQ, "nq"));
  const expected = await g.toNQuads();

  const parts = [];
  const bytes = await g.writeNQuads((c) => parts.push(c));
  assert.equal(parts.join(""), expected);
  assert.equal(bytes, expected.length);

  const stream = new PassThrough();
  const received = [];
  stream.on("data", (c) => received.push(c.toString()));
  await g.writeNQuads(stream);
  stream.end();
  await new Promise((r) => stream.on("end", r));
  assert.equal(received.join(""), expected);

  // Options pass through: one graph only.
  const only = [];
  await g.writeNQuads((c) => only.push(c), { graph: LABELS });
  assert.equal(only.join("").trim().endsWith(`<${LABELS}> .`), true);
  assert.equal(lines(only.join("")).length, 1);
});

test("raw:true yields the engine's N-Triples tokens unparsed", async () => {
  const g = await open(await build(NQ, "nq"));
  const raw = [];
  for await (const quad of g.dump({ raw: true })) raw.push(quad);

  assert.ok(raw.every((q) => q.slice(0, 3).every((t) => typeof t === "string")));
  const tokens = raw.map(([s, p, o, gr]) => `${s} ${p} ${o}${gr ? " " + gr : ""} .`).sort();
  assert.deepEqual(tokens, lines(NQ));
});

test("breaking out of the loop early stops the scan and frees the cursor", async () => {
  const g = await open(await build(NQ, "nq"));
  let seen = 0;
  for await (const _quad of g.dump({ batch: 1 })) {
    seen += 1;
    break; // triggers the generator's finally → cursor.free()
  }
  assert.equal(seen, 1);
  // The graph is still fully usable afterwards (the cursor held only a borrow).
  assert.equal((await collect(g)).length, 6);
});

test("the same dump works over a lazily opened local file", async () => {
  const dir = await mkdtemp(join(tmpdir(), "rete-js-dump-"));
  const path = join(dir, "graph.rete");
  const bytes = await build(NQ, "nq");
  await writeFile(path, bytes);

  const lazy = await open(pathToFileURL(path).href);
  const embedded = await open(bytes);
  assert.deepEqual(await collect(lazy), await collect(embedded));
  assert.ok(lazy.stats().bytes > 0, "a lazy dump reads through the range reader");
});

test("the same dump works over an HTTP Range remote graph", async () => {
  const bytes = await build(NQ, "nq");
  const server = await serveBytes(bytes);
  try {
    const remote = await open(server.url);
    const local = await open(bytes);
    assert.deepEqual(await collect(remote), await collect(local));
    assert.deepEqual(lines(await remote.toNQuads()), lines(await local.toNQuads()));
    assert.ok(remote.stats().requests > 0);
  } finally {
    await server.close();
  }
});

test("the N-Quads stream loads into Oxigraph, graphs and all", async () => {
  // The reason `nquads()` exists: hand a `.rete` to another RDF store without
  // ever holding the graph twice. Oxigraph parses N-Quads, so the chunks feed
  // it directly — here through its string loader, one chunk at a time.
  const { Store } = await import("oxigraph");
  const g = await open(await build(NQ, "nq"));

  const store = new Store();
  for await (const chunk of g.nquads({ batch: 2 })) {
    store.load(chunk, { format: "application/n-quads" });
  }

  assert.equal(store.size, g.quads);
  // Named graphs survived the hand-off: Oxigraph sees the same three graphs.
  const graphs = new Set([...store.match(null, null, null, null)].map((q) => q.graph.value));
  assert.deepEqual([...graphs].sort(), ["", LABELS, SOCIAL]);
  // ...and the same answers come back out of the other engine.
  assert.equal(
    store.query(`ASK { GRAPH <${SOCIAL}> { <http://example.org/alice> <http://example.org/knows> <http://example.org/bob> } }`),
    true,
  );
  const label = store.query(
    'SELECT ?l WHERE { <http://example.org/alice> <http://www.w3.org/2000/01/rdf-schema#label> ?l }',
  );
  assert.equal([...label][0].get("l").value, 'Alice "the researcher"');
});

test("the engine cursor hands back exactly the batch it was asked for", async () => {
  // Straight at the wasm cursor: this is what bounds the wrapper's memory —
  // `next_batch(n)` returns at most n quads (4 flat tokens each), never the
  // whole graph, and an empty array marks the end.
  await init();
  const engine = new wasm.Graph(await build(NQ, "nq"));
  const cursor = engine.quads(undefined);
  try {
    assert.equal(cursor.next_batch(2).length, 8); // 2 quads × [s, p, o, g]
    assert.equal(cursor.done(), false);
    assert.equal(cursor.next_batch(3).length, 12);
    assert.equal(cursor.next_batch(100).length, 4); // only 1 quad left
    assert.deepEqual(cursor.next_batch(100), []); // exhausted
    assert.equal(cursor.done(), true);
  } finally {
    cursor.free();
    engine.free();
  }
});

test("a bad graph option is rejected, not silently ignored", async () => {
  const g = await open(await build(NQ, "nq"));
  await assert.rejects(async () => {
    // eslint-disable-next-line no-empty
    for await (const _ of g.dump({ graph: 42 })) {}
  }, TypeError);
});
