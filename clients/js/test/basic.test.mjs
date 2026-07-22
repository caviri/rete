// End-to-end tests over the BUILT package (dist/): bytes, remote over a
// Range server (through the Node sync-XHR bridge), and the script-tag bundle.
import assert from "node:assert/strict";
import { mkdtemp, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import test from "node:test";
import { pathToFileURL } from "node:url";

import { Term, build, open } from "../dist/index.js";
import { serveBytes } from "./range-server.mjs";

const NT = `\
<http://example.org/alice> <http://www.w3.org/1999/02/22-rdf-syntax-ns#type> <http://example.org/Person> .
<http://example.org/alice> <http://www.w3.org/2000/01/rdf-schema#label> "Alice \\"the researcher\\"" .
<http://example.org/alice> <http://example.org/age> "42"^^<http://www.w3.org/2001/XMLSchema#integer> .
<http://example.org/bob> <http://www.w3.org/1999/02/22-rdf-syntax-ns#type> <http://example.org/Person> .
<http://example.org/bob> <http://www.w3.org/2000/01/rdf-schema#label> "Bob"@en .
<http://example.org/bob> <http://example.org/knows> <http://example.org/alice> .
`;

const KNOWS_Q = "SELECT ?s ?o WHERE { ?s <http://example.org/knows> ?o }";

test("build + open bytes + query terms", async () => {
  const data = await build(NT);
  assert.ok(data instanceof Uint8Array && data.length > 0);
  const g = await open(data);
  assert.equal(g.quads, 6);

  const rows = g.query(KNOWS_Q);
  assert.equal(rows.length, 1);
  assert.equal(rows[0].s.kind, "iri");
  assert.equal(rows[0].s.value, "http://example.org/bob");
  assert.equal(rows[0].o.value, "http://example.org/alice");
});

test("term parsing: escapes, lang tags, typed literals", async () => {
  const g = await open(await build(NT));
  const labels = g.query(
    "SELECT ?label WHERE { ?s <http://www.w3.org/2000/01/rdf-schema#label> ?label }",
  );
  const byValue = new Map(labels.map((r) => [r.label.value, r.label]));
  assert.ok(byValue.has('Alice "the researcher"')); // NT escapes resolved
  assert.equal(byValue.get("Bob").lang, "en");
  assert.equal(byValue.get("Bob").n3, '"Bob"@en');

  const [row] = g.query(
    "SELECT ?age WHERE { <http://example.org/alice> <http://example.org/age> ?age }",
  );
  assert.equal(row.age.datatype, "http://www.w3.org/2001/XMLSchema#integer");
  assert.strictEqual(row.age.toJS(), 42);
});

test("ask and construct", async () => {
  const g = await open(await build(NT));
  assert.equal(g.query("ASK { ?s <http://example.org/knows> ?o }"), true);
  assert.equal(g.query("ASK { ?s <http://example.org/hates> ?o }"), false);

  const triples = g.query(
    "CONSTRUCT { ?o <http://example.org/knownBy> ?s } WHERE { ?s <http://example.org/knows> ?o }",
  );
  assert.equal(triples.length, 1);
  assert.equal(triples[0][1].value, "http://example.org/knownBy");
});

test("schema and graphNames come back with clean IRIs", async () => {
  const g = await open(await build(NT));
  const classes = new Map(g.schema().classes);
  assert.equal(classes.get("http://example.org/Person"), 2);
  assert.deepEqual(g.graphNames(), []);
});

test("remote open over HTTP Range via the Node sync-XHR bridge", async () => {
  const data = await build(NT);
  const server = await serveBytes(data);
  try {
    const g = await open(server.url);
    assert.deepEqual(
      g.query(KNOWS_Q).map((r) => r.o.value),
      ["http://example.org/alice"],
    );
    const stats = g.stats();
    assert.equal(stats.fileLength, data.length);
    assert.ok(stats.requests >= 1);
    assert.equal(g.contentHash().length, 32);
  } finally {
    await server.close();
  }
});

test("script-tag bundle: global `rete`, embedded wasm", async () => {
  // Load with CLASSIC script semantics (like a <script src> tag): top-level
  // `var rete` becomes a global there, unlike under an ESM import.
  const { readFile } = await import("node:fs/promises");
  const { runInThisContext } = await import("node:vm");
  runInThisContext(await readFile(new URL("../dist/rete-graph.min.js", import.meta.url), "utf8"));
  const api = globalThis.rete;
  assert.ok(api && typeof api.open === "function");
  const g = await api.open(await api.build("<urn:a> <urn:knows> <urn:b> ."));
  assert.equal(g.quads, 1);
  assert.equal(g.query("ASK { <urn:a> ?p ?o }"), true);
  assert.ok(api.Term.parse("<urn:a>").value === "urn:a");
});

test("file:// opens read a local graph lazily, by byte range", async () => {
  const data = await build(NT);
  const dir = await mkdtemp(join(tmpdir(), "rete-js-"));
  const path = join(dir, "graph.rete");
  await writeFile(path, data);

  const g = await open(pathToFileURL(path).href);
  assert.deepEqual(
    g.query(KNOWS_Q).map((r) => r.o.value),
    ["http://example.org/alice"],
  );
  // Same lazy machinery as a remote open: counted reads, not a whole-file load.
  const stats = g.stats();
  assert.equal(stats.fileLength, data.length);
  assert.ok(stats.requests >= 1);
  assert.equal(g.info().quads, 6);
  assert.deepEqual(g.schema().classes, [["http://example.org/Person", 2]]);
});

test("card, examples and shacl over a lazily opened file", async () => {
  // A card travels inside the file; build() writes none, so this asserts the
  // honest empty answers plus a real SHACL run.
  const dir = await mkdtemp(join(tmpdir(), "rete-js-"));
  const path = join(dir, "graph.rete");
  await writeFile(path, await build(NT));
  const g = await open(pathToFileURL(path).href);

  assert.equal(g.card(), null);
  assert.deepEqual(g.examples(), []);

  const report = g.shacl(`@prefix sh: <http://www.w3.org/ns/shacl#> .
[] a sh:NodeShape ; sh:targetClass <http://example.org/Person> ;
   sh:property [ sh:path <http://www.w3.org/2000/01/rdf-schema#label> ; sh:minCount 1 ] .`);
  assert.equal(report.conforms, true);

  const failing = g.shacl(`@prefix sh: <http://www.w3.org/ns/shacl#> .
[] a sh:NodeShape ; sh:targetClass <http://example.org/Person> ;
   sh:property [ sh:path <http://example.org/email> ; sh:minCount 1 ] .`);
  assert.equal(failing.conforms, false);
  assert.equal(failing.results.length, 2);
});
