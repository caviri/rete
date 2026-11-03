// RDF shared in the chat. Claude Desktop does not hand attachments to an
// extension, so the model passes the file's text to load_rdf, which builds it
// into a graph held in memory, `chat:<name>`, queryable with every tool — the
// SPARQL ones on the query workers too — with no folder granted at all.
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
const PREFIXES = `@prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> .
@prefix ex: <https://example.org/> .
`;
const ADA = `ex:ada a ex:Person ; rdfs:label "Ada Lovelace" ; ex:knows ex:charles .\n`;
const CHARLES = `ex:charles a ex:Person ; rdfs:label "Charles Babbage" .\n`;
const LABELS = 'SELECT ?l WHERE { ?s <http://www.w3.org/2000/01/rdf-schema#label> ?l } ORDER BY ?l';

/** A server with `dir` granted, or none at all. */
async function connect(t, dir) {
  const client = new Client({ name: "chat", version: "0" });
  await client.connect(
    new StdioClientTransport({ command: process.execPath, args: dir ? [SERVER, dir] : [SERVER] }),
  );
  t.after(() => client.close());
  return (name, args) => client.callTool({ name, arguments: args });
}

const payload = (result) => {
  assert.ok(!result.isError, `tool errored: ${result.content?.[0]?.text}`);
  return JSON.parse(result.content[0].text);
};

const refusal = (result) => {
  assert.ok(result.isError, `expected an error, got ${result.content?.[0]?.text}`);
  return result.content[0].text;
};

test("RDF shared in the chat is queryable with no folder granted", async (t) => {
  const call = await connect(t);

  // A file too long for one call arrives in parts; the prefixes of the first
  // part carry over to the second.
  const first = payload(await call("load_rdf", { name: "people.ttl", rdf: PREFIXES + ADA }));
  assert.equal(first.dataset, "chat:people");
  assert.equal(first.statements, 3);
  assert.equal(first.format, "ttl");
  const both = payload(await call("load_rdf", { name: "people.ttl", rdf: CHARLES, append: true }));
  assert.equal(both.statements, 5);
  assert.equal(both.parts, 2);

  // SPARQL runs on a query worker, which opens the same bytes.
  const rows = payload(await call("sparql_query", { dataset: "chat:people", query: LABELS }));
  assert.deepEqual(rows.rows.map((r) => r.l), ["Ada Lovelace", "Charles Babbage"]);
  assert.equal(rows.stats.where, "in memory (RDF shared in the chat)");

  // The other tools read it on the server's thread; the bare name works too.
  const schema = payload(await call("dataset_schema", { dataset: "people" }));
  assert.ok(schema.classes.some((c) => JSON.stringify(c).includes("https://example.org/Person")));
  const found = payload(await call("find_entities", { dataset: "chat:people", text: "Ada" }));
  assert.equal(found.hits[0].subject, "https://example.org/ada");

  const listed = payload(await call("list_datasets", { source: "chat" }));
  assert.equal(listed.chat.length, 1);
  assert.equal(listed.chat[0].key, "chat:people");
  assert.equal(listed.chat[0].parts, 2);
  assert.equal(listed.counts.published, 0);

  // A part that does not parse is not added, and says so.
  const broken = refusal(await call("load_rdf", { name: "people.ttl", rdf: "ex:oops ex:p", append: true }));
  assert.match(broken, /NOT added: chat:people still holds its 2 earlier parts/);
  const still = payload(await call("sparql_query", { dataset: "chat:people", query: LABELS }));
  assert.equal(still.count, 2);

  // Without append, a new load replaces the graph — and the workers, which
  // had the old bytes open, answer from the new ones.
  const nt = "<https://example.org/x> <http://www.w3.org/2000/01/rdf-schema#label> \"X\" .\n";
  const replaced = payload(await call("load_rdf", { name: "people.nt", rdf: nt }));
  assert.equal(replaced.format, "nt");
  assert.equal(replaced.parts, 1);
  const fresh = payload(await call("sparql_query", { dataset: "chat:people", query: LABELS }));
  assert.deepEqual(fresh.rows.map((r) => r.l), ["X"]);

  assert.match(refusal(await call("load_rdf", { name: "p.ttl", rdf: nt, save_as: "p" })), /no folder granted/);
  assert.match(refusal(await call("sparql_query", { dataset: "chat:nobody", query: LABELS })), /load_rdf/);
  assert.match(refusal(await call("load_rdf", { name: "p.ttl" })), /one of the two/);
});

test("a chat graph can be saved, and an RDF file in a granted folder loaded by path", async (t) => {
  const dir = await mkdtemp(join(tmpdir(), "rete-mcpb-chat-"));
  await writeFile(join(dir, "people.rete"), await build(PREFIXES + ADA + CHARLES, "ttl"));
  await writeFile(
    join(dir, "extra.nq"),
    "<https://example.org/y> <https://example.org/p> \"y\" <https://example.org/g> .\n",
  );
  const call = await connect(t, dir);

  // The syntax comes from the file's extension: N-Quads, not Turtle.
  const byPath = payload(await call("load_rdf", { name: "extra", path: "extra.nq" }));
  assert.equal(byPath.format, "nq");
  assert.equal(byPath.info.namedGraphs, 1);

  const saved = payload(
    await call("load_rdf", { name: "people.ttl", rdf: PREFIXES + ADA, save_as: "from-chat" }),
  );
  assert.equal(saved.saved.dataset, "from-chat");
  assert.ok(saved.saved.path.endsWith("from-chat.rete"));
  const local = payload(await call("sparql_query", { dataset: "from-chat", query: LABELS }));
  assert.equal(local.count, 1);
  assert.equal(local.stats.where, "local file (lazy byte-range reads)");

  // A clash between a local file and a chat graph: the bare name is the
  // local file, chat:<name> the chat graph.
  assert.equal(payload(await call("sparql_query", { dataset: "people", query: LABELS })).count, 2);
  assert.equal(payload(await call("sparql_query", { dataset: "chat:people", query: LABELS })).count, 1);

  assert.match(refusal(await call("load_rdf", { name: "x", path: "../outside.ttl" })), /outside the directories/);
});

// A tiny Jelly 1.1 file (3 quads, one in the default graph), written by hand
// and checked against jelly-cli; the other clients use the same bytes.
const JELLY = Buffer.from(
  "bb010a0e0a0c1002481050085808700278020a1752151213687474703a2f2f6578616d706c652e6f72672f0a094a071205616c6963650a094a0712056b6e6f77730a074a051203626f620a084a0612046e616d650a064a04120267310a2c5a2a1228687474703a2f2f7777772e77332e6f72672f323030312f584d4c536368656d6123696e74656765720a0c1a0a0a0208012a004a007a000a171a152a0210045a0b0a05416c6963651202656e6a0210050a0a1a085a060a0234321801",
  "hex",
);

test("a Jelly file in a granted folder loads by path, and can be built into a .rete", async (t) => {
  const dir = await mkdtemp(join(tmpdir(), "rete-mcpb-jelly-"));
  await writeFile(join(dir, "tiny.jelly"), JELLY);
  const call = await connect(t, dir);

  // The syntax comes from the extension; Jelly has named graphs here.
  const loaded = payload(await call("load_rdf", { name: "tiny", path: "tiny.jelly" }));
  assert.equal(loaded.format, "jelly");
  assert.equal(loaded.statements, 3);
  assert.equal(loaded.info.namedGraphs, 1);

  const built = payload(await call("build_rete", { path: "tiny.jelly", output_path: "tiny-built" }));
  assert.equal(built.info.quads, 3);

  // Binary: it cannot be sent as text, and a part cannot be appended to it.
  assert.match(refusal(await call("load_rdf", { name: "t.jelly", rdf: "text", format: "jelly" })), /binary/);
  assert.match(
    refusal(await call("load_rdf", { name: "tiny", path: "tiny.jelly", append: true })),
    /cannot be appended/,
  );
});
