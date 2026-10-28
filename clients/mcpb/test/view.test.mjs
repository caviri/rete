// The graph view (an MCP App), at the protocol level: what the host sees in
// tools/list and resources/read, and the tools the view and Claude call. The
// view itself (the HTML) is driven in a real browser by view-ui.test.mjs.
import assert from "node:assert/strict";
import { dirname, join } from "node:path";
import test from "node:test";
import { fileURLToPath } from "node:url";

import { Client } from "@modelcontextprotocol/sdk/client/index.js";
import { StdioClientTransport } from "@modelcontextprotocol/sdk/client/stdio.js";

import { fixture } from "./view-fixture.mjs";

const here = dirname(fileURLToPath(import.meta.url));
const SERVER = join(here, "..", "build", "server", "index.mjs");

async function connect(t, dir) {
  const client = new Client({ name: "view", version: "0" });
  await client.connect(new StdioClientTransport({ command: process.execPath, args: [SERVER, dir] }));
  t.after(() => client.close());
  const call = async (name, args) => {
    const r = await client.callTool({ name, arguments: args });
    assert.ok(!r.isError, `${name} errored: ${r.content?.[0]?.text}`);
    return r;
  };
  const data = async (name, args) => JSON.parse((await call(name, args)).content[0].text);
  return { client, call, data };
}

const EDGES = "PREFIX ex: <https://example.org/> CONSTRUCT { ?a ex:knows ?b } WHERE { ?a ex:knows ?b }";

test("the host sees a UI tool, its resource, and app-only tools", async (t) => {
  const { client } = await connect(t, await fixture());
  const { tools } = await client.listTools();
  const byName = Object.fromEntries(tools.map((x) => [x.name, x]));
  assert.equal(byName.graph_view._meta.ui.resourceUri, "ui://rete/graph-view.html");
  for (const name of ["graph_view_sync", "graph_view_expand", "graph_view_search", "graph_view_labels"]) {
    assert.deepEqual(byName[name]._meta.ui.visibility, ["app"], name);
  }
  // Claude's own tools: no UI of their own, visible to the model.
  assert.equal(byName.graph_view_update._meta?.ui, undefined);

  const { contents } = await client.readResource({ uri: "ui://rete/graph-view.html" });
  assert.equal(contents[0].mimeType, "text/html;profile=mcp-app");
  assert.match(contents[0].text, /^<!doctype html>/);
  assert.match(contents[0].text, /<script>[\s\S]+cytoscape[\s\S]+<\/script>/);
  assert.doesNotMatch(contents[0].text, /<script[^>]+src=/, "everything inline: the sandbox has no network");
});

test("a view opens on a query, syncs, takes Claude's changes and reports the user's", async (t) => {
  const { call, data } = await connect(t, await fixture());
  const opened = await call("graph_view", { dataset: "people", query: EDGES, title: "who knows whom" });
  const { view_id, nodes, edges } = opened.structuredContent;
  assert.match(view_id, /^v\d+$/);
  assert.deepEqual([nodes, edges], [3, 3]);
  assert.match(opened.content[0].text, /graph_view_update/);

  // The view starts from nothing: it gets the whole graph.
  const first = await data("graph_view_sync", { view_id, since: 0 });
  assert.equal(first.snapshot.nodes.length, 3);
  assert.equal(first.snapshot.edges.length, 3);
  assert.ok(first.snapshot.nodes.every((n) => n.iri?.startsWith("https://example.org/")));

  // Labels and types, asked for by the view.
  const ids = first.snapshot.nodes.map((n) => n.id);
  const labelled = await data("graph_view_labels", { view_id, ids, since: first.version });
  const labels = labelled.patches.find((p) => p.op === "labels").labels;
  assert.equal(labels["<https://example.org/ada>"].label, "Ada Lovelace");
  assert.deepEqual(labels["<https://example.org/ada>"].types, ["https://example.org/Person"]);

  // Claude changes the look and adds a query's results.
  const update = await data("graph_view_update", {
    view_id,
    options: { group_by: "type", edge_labels: true, layout: "circle" },
    query: "PREFIX ex: <https://example.org/> CONSTRUCT { ?p ex:worksFor ?o . ?o a ex:Project } WHERE { ?p ex:worksFor ?o }",
  });
  assert.equal(update.nodes, 4);
  assert.match(update.done.join(" "), /added 1 nodes/);
  const after = await data("graph_view_sync", { view_id, since: labelled.version });
  assert.deepEqual(after.patches.map((p) => p.op), ["options", "add"]);
  assert.equal(after.patches[0].options.layout, "circle");

  // The user selects something; Claude reads it.
  await data("graph_view_sync", {
    view_id,
    since: after.version,
    state: { selection: [{ id: "<https://example.org/ada>", iri: "https://example.org/ada", label: "Ada Lovelace" }], selection_count: 1 },
  });
  const state = await data("graph_view_state", { view_id });
  assert.equal(state.user.open, true);
  assert.equal(state.user.selection[0].label, "Ada Lovelace");

  // Expanding a node brings both directions.
  const grown = await data("graph_view_expand", { view_id, iri: "https://example.org/engine", since: after.version });
  const added = grown.patches.find((p) => p.op === "add" && p.focus);
  assert.ok(added.nodes.some((n) => n.id === "<https://example.org/lab>"), "the incoming ex:hosts edge brings ex:lab");
  assert.equal(added.focus, "<https://example.org/engine>");

  // A view far behind (or new) gets everything again.
  const whole = await data("graph_view_sync", { view_id, since: 0 });
  assert.ok(whole.snapshot.nodes.length >= 5);
  assert.equal(whole.snapshot.options.layout, "circle");
});

test("SELECT rows become nodes, links and properties; search adds entities", async (t) => {
  const { call, data } = await connect(t, await fixture());
  const opened = await call("graph_view", {
    dataset: "people",
    query:
      "PREFIX ex: <https://example.org/> PREFIX rdfs: <http://www.w3.org/2000/01/rdf-schema#> " +
      "SELECT ?person ?name ?org WHERE { ?person rdfs:label ?name ; ex:worksFor ?org }",
  });
  const { view_id } = opened.structuredContent;
  const { snapshot } = await data("graph_view_sync", { view_id, since: 0 });
  const ada = snapshot.nodes.find((n) => n.id === "<https://example.org/ada>");
  assert.equal(ada.label, "Ada Lovelace", "a ?name column names the row's entity");
  assert.ok(ada.props.some(([p, v]) => p === "?name" && v === "Ada Lovelace"));
  assert.ok(snapshot.edges.some((e) => e.label === "?org" && e.target === "<https://example.org/engine>"));
  assert.deepEqual(snapshot.table.vars, ["person", "name", "org"]);
  assert.equal(snapshot.table.rows.length, 2);

  const found = await data("graph_view_search", { view_id, text: "Mary", since: 0 });
  assert.equal(found.found, 1);

  // An entity's neighbourhood as the starting point.
  const around = await call("graph_view", { dataset: "people", iri: "https://example.org/charles" });
  assert.ok(around.structuredContent.nodes >= 3);
});

test("bad calls say what is wrong", async (t) => {
  const { client } = await connect(t, await fixture());
  const r = await client.callTool({ name: "graph_view_update", arguments: { view_id: "v999", options: { labels: false } } });
  assert.ok(r.isError);
  assert.match(r.content[0].text, /no graph view v999/);
  const ask = await client.callTool({ name: "graph_view", arguments: { dataset: "people", query: "ASK { ?s ?p ?o }" } });
  assert.ok(ask.isError);
  assert.match(ask.content[0].text, /no graph to show/);
});
