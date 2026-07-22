// ReteSource × real Comunica: a .rete file as an RDF/JS source.
import assert from "node:assert/strict";
import { test } from "node:test";

import { QueryEngine } from "@comunica/query-sparql";

import { build, open, ReteSource } from "../dist/index.js";

const NQ = [
  '<urn:x:alice> <http://www.w3.org/1999/02/22-rdf-syntax-ns#type> <urn:x:Person> .',
  '<urn:x:alice> <http://www.w3.org/2000/01/rdf-schema#label> "Alice" .',
  '<urn:x:alice> <urn:x:age> "42"^^<http://www.w3.org/2001/XMLSchema#integer> .',
  '<urn:x:bob> <http://www.w3.org/1999/02/22-rdf-syntax-ns#type> <urn:x:Person> .',
  '<urn:x:bob> <http://www.w3.org/2000/01/rdf-schema#label> "Bob"@en .',
  '<urn:x:bob> <urn:x:knows> <urn:x:alice> .',
  '<urn:x:alice> <urn:x:reviewed> <urn:x:paper1> <urn:x:graph:reviews> .',
].join("\n");

async function source() {
  return new ReteSource(await open(await build(NQ, "nq")));
}

test("match() streams pattern lookups", async () => {
  const src = await source();
  const quads = [];
  await new Promise((resolve, reject) => {
    const stream = src.match(null, { termType: "NamedNode", value: "urn:x:knows" }, null, null);
    stream.on("readable", () => {
      for (let q = stream.read(); q !== null; q = stream.read()) quads.push(q);
    });
    stream.on("end", resolve);
    stream.on("error", reject);
  });
  assert.equal(quads.length, 1);
  assert.equal(quads[0].subject.value, "urn:x:bob");
  assert.equal(quads[0].object.value, "urn:x:alice");
  assert.equal(quads[0].graph.termType, "DefaultGraph");
});

test("countQuads + named graph union semantics", async () => {
  const src = await source();
  assert.equal(src.countQuads(null, null, null, null), 7); // default + named
  assert.equal(
    src.countQuads(null, null, null, { termType: "NamedNode", value: "urn:x:graph:reviews" }),
    1,
  );
});

test("Comunica runs a multi-pattern join over a ReteSource", async () => {
  const engine = new QueryEngine();
  const bindings = await (
    await engine.queryBindings(
      `PREFIX rdfs: <http://www.w3.org/2000/01/rdf-schema#>
       SELECT ?who ?label ?age WHERE {
         ?s <urn:x:knows> ?who .
         ?who rdfs:label ?label ; <urn:x:age> ?age .
       }`,
      { sources: [await source()] },
    )
  ).toArray();
  assert.equal(bindings.length, 1);
  assert.equal(bindings[0].get("who").value, "urn:x:alice");
  assert.equal(bindings[0].get("label").value, "Alice");
  assert.equal(bindings[0].get("age").value, "42");
  assert.equal(
    bindings[0].get("age").datatype.value,
    "http://www.w3.org/2001/XMLSchema#integer",
  );
});

test("Comunica sees language tags and named graphs", async () => {
  const engine = new QueryEngine();
  const src = await source();

  const labels = await (
    await engine.queryBindings(
      `SELECT ?l WHERE { <urn:x:bob> <http://www.w3.org/2000/01/rdf-schema#label> ?l }`,
      { sources: [src] },
    )
  ).toArray();
  assert.equal(labels[0].get("l").language, "en");

  const inGraph = await (
    await engine.queryBindings(
      `SELECT ?o WHERE { GRAPH <urn:x:graph:reviews> { <urn:x:alice> <urn:x:reviewed> ?o } }`,
      { sources: [src] },
    )
  ).toArray();
  assert.equal(inGraph.length, 1);
  assert.equal(inGraph[0].get("o").value, "urn:x:paper1");
});
