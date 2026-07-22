// Drive the built server exactly as Claude Desktop does: spawn it, speak MCP
// over stdio, call every tool. Covers the two paths that matter — a LOCAL
// .rete read lazily off disk, and a PUBLISHED one read over HTTP Range.
import assert from "node:assert/strict";
import { mkdtemp, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import test from "node:test";
import { fileURLToPath } from "node:url";

import { Client } from "@modelcontextprotocol/sdk/client/index.js";
import { StdioClientTransport } from "@modelcontextprotocol/sdk/client/stdio.js";
import { build } from "rete-graph";

const here = dirname(fileURLToPath(import.meta.url));
const SERVER = join(here, "..", "build", "server", "index.mjs");
const TURTLE = `@prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> .
@prefix ex: <https://example.org/> .
ex:ada a ex:Person ; rdfs:label "Ada Lovelace" ; ex:knows ex:charles .
ex:charles a ex:Person ; rdfs:label "Charles Babbage" .
`;

/** A granted folder holding one real .rete file. */
async function fixture() {
  const dir = await mkdtemp(join(tmpdir(), "rete-mcpb-"));
  await writeFile(join(dir, "people.rete"), await build(TURTLE, "ttl"));
  return dir;
}

/**
 * A client wired to a freshly spawned server. Closing is registered on the
 * test context, so a failing assertion still reaps the child process (an
 * orphan would keep the whole test file alive until it times out).
 */
async function connect(t, dir) {
  const client = new Client({ name: "smoke", version: "0" });
  await client.connect(
    new StdioClientTransport({ command: process.execPath, args: [SERVER, dir] }),
  );
  t.after(() => client.close());
  return client;
}

const payload = (result) => {
  assert.ok(!result.isError, `tool errored: ${result.content?.[0]?.text}`);
  return JSON.parse(result.content[0].text);
};

test("serves the declared tools", async (t) => {
  const client = await connect(t, await fixture());
  const { tools } = await client.listTools();
  const names = tools.map((t) => t.name).sort();
  assert.deepEqual(names, [
    "build_rete",
    "dataset_card",
    "dataset_schema",
    "describe_entity",
    "example_queries",
    "find_entities",
    "list_datasets",
    "sparql_query",
    "validate_shacl",
  ]);
});

test("queries a local graph lazily, by file name", async (t) => {
  const dir = await fixture();
  const client = await connect(t, dir);

  const listed = payload(await client.callTool({ name: "list_datasets", arguments: {} }));
  assert.equal(listed.local.length, 1);
  assert.equal(listed.local[0].key, "people");
  assert.ok(listed.published.length > 0, "the catalogue snapshot should be bundled");

  const rows = payload(
    await client.callTool({
      name: "sparql_query",
      arguments: {
        dataset: "people",
        query:
          'SELECT ?l WHERE { ?s <http://www.w3.org/2000/01/rdf-schema#label> ?l } ORDER BY ?l',
      },
    }),
  );
  assert.equal(rows.count, 2);
  assert.equal(rows.rows[0].l, "Ada Lovelace");
  assert.equal(rows.stats.where, "local file (lazy byte-range reads)");
  assert.ok(rows.stats.bytesRead > 0);

  const found = payload(
    await client.callTool({ name: "find_entities", arguments: { dataset: "people", text: "Ada" } }),
  );
  assert.equal(found.hits[0].subject, "https://example.org/ada");

  const described = payload(
    await client.callTool({
      name: "describe_entity",
      arguments: { dataset: "people", iri: "https://example.org/charles" },
    }),
  );
  assert.ok(described.incoming.some((s) => s.p === "https://example.org/knows"));

});

test("refuses a path outside the granted folders", async (t) => {
  const client = await connect(t, await fixture());
  const result = await client.callTool({
    name: "sparql_query",
    arguments: { dataset: "/etc/passwd", query: "ASK { ?s ?p ?o }" },
  });
  assert.ok(result.isError);
  assert.match(result.content[0].text, /outside the directories/);
});

test("builds a graph offline and queries it straight away", async (t) => {
  const dir = await fixture();
  const client = await connect(t, dir);

  const built = payload(
    await client.callTool({
      name: "build_rete",
      arguments: { rdf: TURTLE, output_path: "fresh", format: "ttl" },
    }),
  );
  assert.equal(built.dataset, "fresh");
  assert.equal(built.info.quads, 5);

  const asked = payload(
    await client.callTool({
      name: "sparql_query",
      arguments: { dataset: "fresh", query: "ASK { ?s a <https://example.org/Person> }" },
    }),
  );
  assert.equal(asked.boolean, true);
});

test("validates SHACL shapes", async (t) => {
  const client = await connect(t, await fixture());
  const report = payload(
    await client.callTool({
      name: "validate_shacl",
      arguments: {
        dataset: "people",
        shapes: `@prefix sh: <http://www.w3.org/ns/shacl#> .
@prefix ex: <https://example.org/> .
@prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> .
[] a sh:NodeShape ; sh:targetClass ex:Person ;
   sh:property [ sh:path rdfs:label ; sh:minCount 1 ] .`,
      },
    }),
  );
  assert.equal(report.report.conforms, true);
});

test("reads a published graph over HTTP Range", { concurrency: 1 }, async (t) => {
  const client = await connect(t, await fixture());

  const card = payload(
    await client.callTool({ name: "dataset_card", arguments: { dataset: "boe" } }),
  );
  assert.match(card.card.title, /BOE/);
  // The card tier is index-free: a couple of small reads, not the whole file.
  assert.ok(card.stats.fractionRead < 0.5, `read too much for a card: ${card.stats.fractionRead}`);

  const schema = payload(
    await client.callTool({ name: "dataset_schema", arguments: { dataset: "boe" } }),
  );
  assert.ok(schema.classes.length > 0);

  const examples = payload(
    await client.callTool({ name: "example_queries", arguments: { dataset: "boe" } }),
  );
  assert.ok(examples.queries.length > 0, "boe ships example queries");

});

test("says something useful about a sharded dataset", async (t) => {
  const client = await connect(t, await fixture());
  const result = await client.callTool({
    name: "dataset_card",
    arguments: { dataset: "wikidata-xxl" },
  });
  assert.ok(result.isError);
  // Not "unknown dataset": it exists, it just isn't one file.
  assert.match(result.content[0].text, /published as 6 shards/);
  assert.match(result.content[0].text, /https:\/\/.*\.rete/);
});

test("suggests near matches for an unknown name", async (t) => {
  const client = await connect(t, await fixture());
  const result = await client.callTool({
    name: "dataset_schema",
    arguments: { dataset: "peopl" },
  });
  assert.ok(result.isError);
  assert.match(result.content[0].text, /Did you mean: people/);
});
