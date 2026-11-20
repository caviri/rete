// The graph view in a real browser, behind a host built on the MCP Apps SDK's
// own AppBridge — the host side Claude Desktop implements: the page runs in a
// sandboxed iframe, gets the tool's input and result, and its tool calls go
// through the bridge to a real server process. Needs Chromium, so it runs only
// with RETE_UI_TEST=1 (the Playwright image has it; see clients/mcpb/README.md).
// RETE_UI_SHOT=<path> also saves screenshots.
import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import { dirname, join } from "node:path";
import test from "node:test";
import { fileURLToPath } from "node:url";

import { Client } from "@modelcontextprotocol/sdk/client/index.js";
import { StdioClientTransport } from "@modelcontextprotocol/sdk/client/stdio.js";

import { fixture } from "./view-fixture.mjs";

const here = dirname(fileURLToPath(import.meta.url));
const SERVER = join(here, "..", "build", "server", "index.mjs");
const enabled = process.env.RETE_UI_TEST === "1";
const skip = !enabled && "set RETE_UI_TEST=1 (needs Chromium)";

const KNOWS =
  "PREFIX ex: <https://example.org/> CONSTRUCT { ?a ex:knows ?b . ?a a ?t } WHERE { ?a ex:knows ?b ; a ?t }";

/**
 * A page that hosts the view in a sandboxed iframe, as Claude Desktop does,
 * once Claude has called graph_view with `args`. What the view sends the
 * model lands in `toModel` (context updates) and `messages` (chat messages).
 */
async function host(t, args, { caps = {}, route, csp } = {}) {
  const { chromium } = await import("playwright");
  const { build } = await import("esbuild");

  const dir = await fixture();
  const client = new Client({ name: "ui-host", version: "0" });
  await client.connect(new StdioClientTransport({ command: process.execPath, args: [SERVER, dir] }));
  t.after(() => client.close());

  let html = (await client.readResource({ uri: "ui://rete/graph-view.html" })).contents[0].text;
  // A host that enforces a content security policy on the page, as Claude does.
  if (csp) html = html.replace("<head>", () => `<head><meta http-equiv="Content-Security-Policy" content="${csp}">`);
  const result = await client.callTool({ name: "graph_view", arguments: args });
  const viewId = result.structuredContent.view_id;

  // The host side: the SDK's AppBridge, bundled for the page.
  const bridge = await build({
    stdin: {
      contents: `import { AppBridge, PostMessageTransport } from "@modelcontextprotocol/ext-apps/app-bridge";
        window.AppBridge = AppBridge; window.PostMessageTransport = PostMessageTransport;`,
      resolveDir: join(here, ".."),
    },
    bundle: true,
    format: "iife",
    platform: "browser",
    write: false,
  });

  const browser = await chromium.launch();
  t.after(() => browser.close());
  const page = await browser.newPage({ viewport: { width: 1100, height: 640 } });
  await route?.(page);
  const toModel = [];
  const messages = [];
  const links = [];
  const saw = { context: toModel, message: messages, link: links };
  await page.exposeFunction("hostCallTool", async (params) => client.callTool(params));
  await page.exposeFunction("hostSaw", (kind, params) => saw[kind].push(params));
  await page.setContent(
    `<!doctype html><html><body style="margin:0"><iframe id="f" sandbox="allow-scripts" style="border:0;width:1080px;height:600px"></iframe><script>${bridge.outputFiles[0].text}</script></body></html>`,
  );
  await page.evaluate(
    async ({ html, args, result, caps }) => {
      const iframe = document.getElementById("f");
      const bridge = new window.AppBridge(
        null,
        { name: "test host", version: "0" },
        { openLinks: {}, serverTools: {}, updateModelContext: { text: {} }, message: { text: {} }, downloadFile: {}, ...caps },
        { hostContext: { theme: "light", displayMode: "inline", availableDisplayModes: ["inline", "fullscreen"], platform: "desktop" } },
      );
      bridge.oncalltool = (params) => window.hostCallTool(params);
      bridge.onupdatemodelcontext = async (params) => (window.hostSaw("context", params), {});
      bridge.onmessage = async (params) => (window.hostSaw("message", params), {});
      bridge.onrequestdisplaymode = async ({ mode }) => ({ mode });
      bridge.onopenlink = async ({ url }) => (window.hostSaw("link", url), {});
      bridge.oninitialized = () => {
        bridge.sendToolInput({ arguments: args });
        bridge.sendToolResult(result);
      };
      await bridge.connect(new window.PostMessageTransport(iframe.contentWindow, iframe.contentWindow));
      iframe.srcdoc = html;
    },
    { html, args, result, caps },
  );

  const view = page.frameLocator("#f");
  const frame = page.frames().find((f) => f !== page.mainFrame());
  const inView = (fn, arg) => frame.evaluate(fn, arg);
  const until = async (fn, arg, what) => {
    for (let i = 0; i < 100; i++) {
      if (await inView(fn, arg).catch(() => false)) return;
      await page.waitForTimeout(100);
    }
    assert.fail(`timed out waiting for ${what}`);
  };
  const shot = async (suffix) => {
    if (!process.env.RETE_UI_SHOT) return;
    await page.waitForTimeout(600);
    await page.locator("#f").screenshot({ path: process.env.RETE_UI_SHOT.replace(/\.png$/, `${suffix}.png`) });
  };
  return { client, dir, page, view, inView, until, toModel, messages, links, viewId, shot };
}

test("the graph view works in a sandboxed iframe behind an MCP Apps host", { skip }, async (t) => {
  const { client, page, view, inView, until, toModel, messages, viewId, shot } = await host(t, {
    dataset: "people",
    query: KNOWS,
    title: "who knows whom",
  });

  // It loads its view, then names the nodes.
  await until(() => window.__reteView?.cy?.nodes().length === 3, null, "3 nodes drawn");
  await until(() => window.__reteView.cy.getElementById("<https://example.org/ada>").data("label") === "Ada Lovelace", null, "labels");
  assert.equal(await inView(() => window.__reteView.viewId), viewId);

  // The checkboxes sit in the Show menu, which closes on a click elsewhere.
  const menu = () => view.locator("#show").evaluate((d) => (d.open = true));

  // Labels off and on.
  await menu();
  await view.locator("#opt-labels").uncheck();
  assert.equal(await inView(() => window.__reteView.cy.nodes()[0].style("label")), "");
  await view.locator("#opt-labels").check();

  // Layers: by type; hiding one hides its nodes; boxes draw them.
  // Charles's type comes with the labels the view asks for: then all three are people.
  await until(() => window.__reteView.groups.get("Person")?.count === 3, null, "a Person layer of 3");
  await view.locator('#layers input[data-layer="Person"]').uncheck();
  assert.equal(await inView(() => window.__reteView.cy.nodes(".hidden").length), 3);
  await view.locator('#layers input[data-layer="Person"]').check();
  await menu();
  await view.locator("#opt-boxes").check();
  assert.equal(await inView(() => window.__reteView.cy.nodes(".box").length), 1);
  await view.locator("#opt-boxes").uncheck();

  // Zoom.
  const z0 = await inView(() => window.__reteView.cy.zoom());
  await view.locator("#zoom-in").click();
  assert.ok((await inView(() => window.__reteView.cy.zoom())) > z0);

  // Selecting reports to the server (graph_view_state) and to the model's context.
  await inView(() => window.__reteView.cy.getElementById("<https://example.org/ada>").select());
  await until(() => window.__reteView.selection.length === 1, null, "the selection");
  let state;
  for (let i = 0; i < 50; i++) {
    state = JSON.parse((await client.callTool({ name: "graph_view_state", arguments: { view_id: viewId } })).content[0].text);
    if (state.user.selection?.length) break;
    await page.waitForTimeout(100);
  }
  assert.equal(state.user.selection[0].label, "Ada Lovelace");
  for (let i = 0; i < 30 && !toModel.length; i++) await page.waitForTimeout(100);
  assert.match(toModel.at(-1).content[0].text, /selected 1 node\(s\): Ada Lovelace/);

  // Claude changes the open view.
  await client.callTool({
    name: "graph_view_update",
    arguments: {
      view_id: viewId,
      options: { layout: "circle", edge_labels: true },
      query: "PREFIX ex: <https://example.org/> CONSTRUCT { ?p ex:worksFor ?o } WHERE { ?p ex:worksFor ?o }",
    },
  });
  await until(() => window.__reteView.options.layout === "circle" && window.__reteView.cy.nodes().length === 4, null, "Claude's update");
  assert.equal(await view.locator("#opt-layout").inputValue(), "circle");
  assert.ok(await view.locator("#opt-edge-labels").isChecked());

  // Double-click expands a node: ex:lab hosts the engine.
  await inView(() => window.__reteView.cy.getElementById("<https://example.org/engine>").emit("dbltap"));
  await until(() => window.__reteView.cy.getElementById("<https://example.org/lab>").nonempty(), null, "the expansion");

  // The table.
  await view.locator('.tabs button[data-tab="table"]').click();
  assert.ok((await view.locator("#rows tbody tr").count()) > 0);
  assert.match(await view.locator("#table-count").textContent(), /rows/);
  await view.locator('.tabs button[data-tab="graph"]').click();

  // "Ask Claude" posts a message into the conversation.
  await inView(() => window.__reteView.cy.getElementById("<https://example.org/ada>").select());
  await until(() => window.__reteView.selection.length >= 1, null, "a selection to ask about");
  await view.locator('#details [data-act="ask"]').click();
  for (let i = 0; i < 30 && !messages.length; i++) await page.waitForTimeout(100);
  assert.match(messages.at(-1).content[0].text, /tell me about .*Ada Lovelace/);

  await inView(() => window.__reteView.cy.$(":selected").unselect());
  await view.locator("#fit").click();
  await shot("");

  // Charles's statements, so his box has a property to list.
  await inView(() => window.__reteView.cy.getElementById("<https://example.org/charles>").emit("dbltap"));
  await until(() => window.__reteView.nodes.get("<https://example.org/charles>").props.length > 0, null, "Charles's properties");

  // Boxes: each entity a box with its properties, in layers, with square lines.
  await view.locator("#opt-diagram").selectOption("boxes");
  await until(() => window.__reteView.cy.nodes(".card").length === window.__reteView.nodes.size, null, "a box per entity");
  assert.equal(await view.locator("#opt-layout").inputValue(), "layered");
  assert.equal(await view.locator("#opt-edges").inputValue(), "square");
  assert.equal(await inView(() => window.__reteView.cy.edges()[0].style("curve-style")), "round-taxi");
  const charles = await inView(() => {
    const el = window.__reteView.cy.getElementById("<https://example.org/charles>");
    return { h: el.height(), svg: decodeURIComponent(el.data("svg")) };
  });
  assert.match(charles.svg, />Charles Babbage</);
  assert.match(charles.svg, />born<[\s\S]*>1791</);
  assert.ok(charles.h > 40, `a box with a property is taller than a name: ${charles.h}`);
  // Laid out in layers: Ada knows Mary, so Mary sits below her (once the layout has animated).
  await page.waitForTimeout(500);
  const [ada, mary] = await inView(() => ["ada", "mary"].map((k) => window.__reteView.cy.getElementById(`<https://example.org/${k}>`).position("y")));
  assert.ok(mary > ada, `Mary (${mary}) below Ada (${ada})`);
  await shot("-boxes");

  // Classes: a box per type with its members' properties, the links between classes counted.
  await view.locator("#opt-diagram").selectOption("classes");
  await until(() => window.__reteView.classBoxes.size === 3, null, "a box per class");
  const links = await inView(() => window.__reteView.cy.edges().map((e) => e.data("label")));
  assert.ok(links.includes("knows ×3"), links.join(", "));
  await shot("-classes");
  // A class leads to its members.
  await inView(() => window.__reteView.cy.getElementById("class:https://example.org/Person").select());
  await until(() => window.__reteView.selection[0]?.class_members === 3, null, "the class selected");
  await view.locator('#details [data-act="members"]').click();
  await until(() => window.__reteView.options.diagram === "boxes" && window.__reteView.highlight.size === 3, null, "its members");
});

test("a right-click on a node offers what to do with it; light or dark", { skip }, async (t) => {
  const { client, page, view, inView, until, messages, viewId, shot } = await host(t, {
    dataset: "people",
    query: "CONSTRUCT { ?s ?p ?o } WHERE { ?s ?p ?o FILTER(isIRI(?o)) }",
  });
  const P = (k) => `<https://example.org/${k}>`;
  const nodes = async () => JSON.parse((await client.callTool({ name: "graph_view_state", arguments: { view_id: viewId } })).content[0].text).nodes;
  await until((id) => window.__reteView?.cy?.getElementById(id).data("label") === "Analytical Engine", P("engine"), "the nodes, named");

  // A real right-click, where the node is drawn — once the layout has stopped moving it.
  const settled = async () => {
    let last = "";
    for (let i = 0; i < 60; i++) {
      const now = await inView(() => {
        const cy = window.__reteView.cy;
        return JSON.stringify([cy.zoom(), cy.pan(), cy.nodes().map((n) => n.position())]);
      });
      if (now === last) return;
      last = now;
      await page.waitForTimeout(150);
    }
  };
  const frame = await page.locator("#f").boundingBox();
  const at = async (id) => {
    await settled();
    const p = await inView((id) => {
      const r = document.getElementById("graph").getBoundingClientRect();
      const q = window.__reteView.cy.getElementById(id).renderedPosition();
      return { x: r.left + q.x, y: r.top + q.y };
    }, id);
    return { x: frame.x + p.x, y: frame.y + p.y };
  };
  const click = async (id, button = "left") => {
    const p = await at(id);
    await page.mouse.click(p.x, p.y, { button });
  };
  const item = (text) => view.locator("#ctx button", { hasText: text });
  const menuOpen = () => until(() => !document.getElementById("ctx").hidden, null, "the menu");

  // The engine: what can be done with it.
  await click(P("engine"), "right");
  await menuOpen();
  assert.equal(await view.locator("#ctx .ctx-title").textContent(), "Analytical Engine");
  for (const label of ["Expand", "Compress", "Select neighbours", "Search its name", "Send to chat", "Select several…", "Hide", "Remove", "Open in browser"]) {
    assert.equal(await item(label).count(), 1, label);
  }
  await shot("-menu");
  // Compress: the lab, linked to the engine alone, goes.
  await item("Compress").click();
  await until((id) => !window.__reteView.nodes.has(id), P("lab"), "the lab compressed away");
  assert.ok(await view.locator("#ctx").isHidden());
  assert.equal(await nodes(), 4);

  // Aggregate every Person into one node; the links to it merge, counted.
  await click(P("ada"), "right");
  await menuOpen();
  await item("Aggregate all Person").click();
  await until(() => window.__reteView.cy.getElementById("agg:1").nonempty(), null, "the aggregate drawn");
  assert.ok(await inView((id) => window.__reteView.cy.getElementById(id).empty(), P("ada")), "Ada is inside it");
  assert.ok((await inView(() => window.__reteView.cy.edges().map((e) => e.data("label")))).includes("worksFor ×2"));
  await click("agg:1", "right");
  await menuOpen();
  await item("Ungroup").click();
  await until((id) => window.__reteView.aggregates.size === 0 && window.__reteView.cy.getElementById(id).nonempty(), P("ada"), "the people again");

  // Select several: clicks add; the menu then acts on all of them.
  await click(P("charles"), "right");
  await menuOpen();
  await item("Select several…").click();
  assert.ok(await view.locator("#selecting").isVisible());
  await click(P("mary"));
  await until(() => window.__reteView.selection.length === 2, null, "two selected").catch(async (error) => {
    const why = await inView(() => {
      const cy = window.__reteView.cy;
      return JSON.stringify({
        selection: window.__reteView.selection.map((s) => s.id),
        selected: cy.$(":selected").map((n) => n.id()),
        type: cy.selectionType(),
        at: cy.nodes().map((n) => [n.id(), n.renderedPosition(), n.renderedWidth()]),
      });
    });
    throw new Error(`${error.message} — ${why}`);
  });
  assert.equal(await view.locator("#selecting-count").textContent(), "2");
  // A stray click on the background keeps them, while selecting several.
  await page.mouse.click(frame.x + 12, frame.y + 120);
  await page.waitForTimeout(200);
  assert.equal(await inView(() => window.__reteView.cy.$(":selected").length), 2);
  await click(P("mary"), "right");
  await menuOpen();
  assert.equal(await view.locator("#ctx .ctx-title").textContent(), "2 nodes");
  await item("Send 2 to chat").click();
  for (let i = 0; i < 30 && !messages.length; i++) await page.waitForTimeout(100);
  const sent = messages.at(-1).content[0].text;
  assert.match(sent, /^From graph view v\d+ on people, 2 nodes:\n/);
  assert.match(sent, /- Charles Babbage <https:\/\/example\.org\/charles> \(Person\)/);
  assert.match(sent, /- Mary Somerville <https:\/\/example\.org\/mary> \(Person\)/);
  await page.keyboard.press("Escape");
  assert.ok(await view.locator("#selecting").isHidden());

  // Remove takes a node out of the view, on the server too.
  await inView(() => window.__reteView.cy.$(":selected").unselect());
  await click(P("mary"), "right");
  await menuOpen();
  await item("Remove").click();
  await until((id) => !window.__reteView.nodes.has(id), P("mary"), "Mary removed");
  assert.equal(await nodes(), 3);

  // On the background: the view's own menu.
  await page.mouse.click(frame.x + 12, frame.y + 120, { button: "right" });
  await menuOpen();
  assert.equal(await item("Fit to view").count(), 1);
  await page.keyboard.press("Escape");

  // Light or dark, whatever Claude's theme: auto, light, dark.
  assert.equal(await view.locator("#theme").textContent(), "◐");
  const line = () => inView(() => window.__reteView.cy.edges()[0].style("line-color"));
  const lightLine = await line();
  await view.locator("#theme").click();
  await view.locator("#theme").click();
  assert.equal(await inView(() => document.documentElement.dataset.theme), "dark");
  assert.equal(await view.locator("#theme").textContent(), "☾");
  assert.notEqual(await line(), lightLine);
  await view.locator("#opt-diagram").selectOption("boxes");
  await until(() => window.__reteView.cy.nodes(".card").length === 3, null, "boxes, dark");
  await shot("-dark");
});

test("the view says which version it is, with credits and links", { skip }, async (t) => {
  const { view, page, until, links, shot } = await host(t, { dataset: "people", schema: true });
  await until(() => window.__reteView.cy?.nodes(".card").length === 3, null, "the view");
  const version = JSON.parse(await (await import("node:fs/promises")).readFile(join(here, "..", "build", "manifest.json"), "utf8")).version;
  assert.equal(await view.locator("#about-open").textContent(), `rete ${version}`);
  await view.locator("#about-open").click();
  const about = view.locator("#about");
  assert.ok(await about.isVisible());
  const text = await about.textContent();
  assert.match(text, /By Carlos Vivar Rios · Apache-2\.0/);
  assert.match(text, /cytoscape \d+\.\d+\.\d+ \(MIT\)/);
  await shot("-about");
  // Links open through the host: the sandbox navigates nowhere itself.
  await about.locator("a", { hasText: "Documentation" }).click();
  for (let i = 0; i < 30 && !links.length; i++) await page.waitForTimeout(100);
  assert.deepEqual(links, ["https://caviri.github.io/rete/"]);
  await page.keyboard.press("Escape");
  assert.ok(await about.isHidden());
});

test("a dataset's schema, drawn as class boxes with their properties", { skip }, async (t) => {
  const { view, inView, until, shot } = await host(t, { dataset: "people", schema: true });
  await until(() => window.__reteView.cy?.nodes(".card").length === 3, null, "a box per class");
  assert.equal(await view.locator("#opt-diagram").inputValue(), "boxes");
  const person = await inView(() => decodeURIComponent(window.__reteView.cy.getElementById("<https://example.org/Person>").data("svg")));
  assert.match(person, />3 instances</);
  assert.match(person, />label</);
  await shot("-schema");
});

test("a file attached to the chat opens when the user chooses it in the view", { skip }, async (t) => {
  const { dir, page, view, inView, until, toModel, messages, shot } = await host(t, {
    dataset: "/mnt/user-data/uploads/talks.rete",
  });

  // Claude's copy is out of reach: the view asks the user for the file.
  await until(() => !document.getElementById("awaiting").hidden, null, "the request for the file");
  assert.equal(await view.locator("#awaiting-file").textContent(), "talks.rete");
  assert.match(await view.locator("#status-main").textContent(), /waiting for talks.rete/);
  await shot("-attachment");

  // The user chooses it on their computer (here: the fixture's people.rete).
  const bytes = await readFile(join(dir, "people.rete"));
  await view.locator("#file-input").setInputFiles({ name: "talks.rete", mimeType: "application/octet-stream", buffer: bytes });
  await until(() => window.__reteView.dataset === "chat:talks" && window.__reteView.cy.nodes().length > 0, null, "the file drawn");
  assert.ok(await view.locator("#awaiting").isHidden());
  // Claude asked for it, so the view tells Claude, which carries on.
  for (let i = 0; i < 30 && !messages.length; i++) await page.waitForTimeout(100);
  assert.match(messages.at(-1).content[0].text, /^I opened talks.rete in graph view v\d+: it is dataset "chat:talks" \(17 statements\)/);

  // A file dropped on the view opens there too; Claude learns of it quietly.
  await inView(() => {
    const files = new DataTransfer();
    files.items.add(new File(["@prefix ex: <https://example.org/> .\nex:x ex:p ex:y .\n"], "dropped.ttl"));
    for (const type of ["dragenter", "dragover", "drop"]) {
      document.dispatchEvent(new DragEvent(type, { dataTransfer: files, bubbles: true, cancelable: true }));
    }
  });
  await until(() => window.__reteView.dataset === "chat:dropped" && window.__reteView.cy.nodes().length === 2, null, "the dropped file");
  assert.ok(await view.locator("#dropzone").isHidden());
  for (let i = 0; i < 30 && !toModel.length; i++) await page.waitForTimeout(100);
  assert.match(toModel.at(-1).content[0].text, /the user opened dropped.ttl: the view now shows dataset "chat:dropped"/);
  assert.equal(messages.length, 1, "no message for a file Claude did not ask for");
});

// --- the card view and the hive layout ------------------------------------------------

const PNG = Buffer.from("iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mP8z8BQDwAEhQGAhKmMIQAAAABJRU5ErkJggg==", "base64");

/** A one-triangle glTF binary, for a 3D card to show. */
function triangleGlb() {
  const positions = Buffer.alloc(36);
  [0, 0, 0, 1, 0, 0, 0, 1, 0].forEach((v, i) => positions.writeFloatLE(v, i * 4));
  const json = Buffer.from(
    JSON.stringify({
      asset: { version: "2.0" },
      scene: 0,
      scenes: [{ nodes: [0] }],
      nodes: [{ mesh: 0 }],
      meshes: [{ primitives: [{ attributes: { POSITION: 0 } }] }],
      accessors: [{ bufferView: 0, componentType: 5126, count: 3, type: "VEC3", min: [0, 0, 0], max: [1, 1, 0] }],
      bufferViews: [{ buffer: 0, byteLength: 36 }],
      buffers: [{ byteLength: 36 }],
    }),
  );
  const pad = Buffer.alloc((4 - (json.length % 4)) % 4, 0x20);
  const jsonChunk = Buffer.concat([json, pad]);
  const total = 12 + 8 + jsonChunk.length + 8 + positions.length;
  const out = Buffer.alloc(total);
  out.writeUInt32LE(0x46546c67, 0);
  out.writeUInt32LE(2, 4);
  out.writeUInt32LE(total, 8);
  out.writeUInt32LE(jsonChunk.length, 12);
  out.writeUInt32LE(0x4e4f534a, 16);
  jsonChunk.copy(out, 20);
  const at = 20 + jsonChunk.length;
  out.writeUInt32LE(positions.length, at);
  out.writeUInt32LE(0x004e4942, at + 4);
  positions.copy(out, at + 8);
  return out;
}

// 17 statements twice over: 289 rows, each with a picture at its own address, a clip, a sound, a model,
// a picture that is not there, a name that is markup, and an address that is a script.
const CARDS_QUERY = `PREFIX ex: <https://example.org/>
SELECT ?s ?p ?o ?photo ?clip ?sound ?model ?missing ?evil ?js ?n WHERE {
  ?s ?p ?o . ?x ?q ?z
  BIND(CONCAT("https://media.example/img-", STR(STRLEN(STR(?s))), "-", STR(STRLEN(STR(?p))), "-", STR(STRLEN(STR(?o))), "-", STR(STRLEN(STR(?x))), "-", STR(STRLEN(STR(?q))), "-", STR(STRLEN(STR(?z))), ".png") AS ?photo)
  BIND("https://media.example/clip.webm" AS ?clip)
  BIND("https://media.example/sound.mp3" AS ?sound)
  BIND("https://media.example/triangle.glb" AS ?model)
  BIND("https://media.example/missing.jpg" AS ?missing)
  BIND("<img src=x onerror=window.__pwned=1>" AS ?evil)
  BIND("javascript:window.__pwned=2" AS ?js)
  BIND(STRLEN(STR(?o)) AS ?n)
}`;

async function mediaHost(page, requests) {
  await page.route("https://media.example/**", async (route) => {
    const url = route.request().url();
    requests.push(url);
    const cors = { "access-control-allow-origin": "*" };
    if (url.endsWith(".png")) return route.fulfill({ status: 200, contentType: "image/png", body: PNG, headers: cors });
    if (url.endsWith(".glb")) return route.fulfill({ status: 200, contentType: "model/gltf-binary", body: triangleGlb(), headers: cors });
    if (url.endsWith("missing.jpg")) return route.fulfill({ status: 404, body: "no", headers: cors });
    // a clip and a sound that never arrive: the players stay, with their controls
    return new Promise(() => {});
  });
}

test("cards: one per row, a masonry, lazy media, fields that can be set, nothing injected", { skip }, async (t) => {
  const requests = [];
  const { page, view, inView, until, links, shot } = await host(t, { dataset: "people", query: CARDS_QUERY, title: "everything" }, { route: (p) => mediaHost(p, requests) });
  await until(() => window.__reteView?.table?.rows?.length > 100, null, "the rows");

  await view.locator('.tabs button[data-tab="cards"]').click();
  assert.equal(await view.locator('.tabs button[data-tab="cards"]').getAttribute("aria-selected"), "true");
  assert.ok(await view.locator("#table").isHidden() && await view.locator("#graph").isHidden());
  // One card per row, a screenful at a time.
  assert.equal(await view.locator("#cards-grid .rcard").count(), 24);
  assert.match(await view.locator("#cards-count").textContent(), /^289 of 289 cards$/);
  assert.match(await view.locator("#cards-more").textContent(), /^Show 48 more \(265 hidden\)$/);

  // Fields are detected from the values, nothing about the dataset is known.
  const kinds = await inView(() => [...document.querySelectorAll("#cards-grid .rcard:first-child .media-slot")].map((s) => s.dataset.kind));
  assert.deepEqual(kinds, ["image", "video", "audio", "model3d", "image"]);
  assert.equal(await view.locator("#cards-grid .rcard:first-child .cf-k").first().textContent(), "S");

  // A masonry on a wide page: more than one column.
  const lefts = () => inView(() => new Set([...document.querySelectorAll("#cards-grid .rcard")].map((c) => Math.round(c.getBoundingClientRect().left))).size);
  assert.ok((await lefts()) >= 3, "columns side by side");

  // Nothing off screen is fetched: pictures load when their card nears the screen.
  await page.waitForTimeout(500);
  const first = await inView(() => [
    [...document.querySelectorAll("#cards-grid .media-slot[data-kind=image]")].filter((s) => s.firstElementChild).length,
    document.querySelectorAll("#cards-grid .media-slot[data-kind=image]").length,
  ]);
  assert.ok(first[0] > 0 && first[0] < first[1], `some pictures loaded (${first[0]}) but not all ${first[1]}`);
  const pngs = () => new Set(requests.filter((u) => u.endsWith(".png"))).size;
  assert.ok(pngs() <= first[0], `${pngs()} requests for ${first[0]} pictures near the screen`);
  // the cards grow as their pictures arrive, so the end is where the scrolling gets to
  for (let y = 0, end = 1; y <= end + 400; y += 400) {
    end = await inView(() => document.querySelector(".cards-wrap").scrollHeight);
    await inView((y) => document.querySelector(".cards-wrap").scrollTo(0, y), y);
    await page.waitForTimeout(60);
  }
  await until(() => [...document.querySelectorAll("#cards-grid .media-slot[data-kind=image]")].every((s) => s.firstElementChild), null, "every picture loaded once scrolled to");
  await inView(() => document.querySelector(".cards-wrap").scrollTo(0, 0));

  // The picture, the missing one, the model, the players.
  await until(() => document.querySelector("#cards-grid .rcard:first-child .media-slot[data-kind=image].done img"), null, "a picture");
  await until(() => document.querySelector("#cards-grid .rcard:first-child .media-fallback"), null, "the missing picture said so");
  const fallback = await inView(() => {
    const f = document.querySelector("#cards-grid .rcard:first-child .media-fallback");
    return { text: f.textContent, open: f.querySelector("a").dataset.open };
  });
  assert.match(fallback.text, /did not load here/);
  assert.equal(fallback.open, "https://media.example/missing.jpg");
  await view.locator("#cards-grid .rcard:first-child .media-fallback a").click();
  for (let i = 0; i < 30 && !links.length; i++) await page.waitForTimeout(100);
  assert.equal(links.at(-1), "https://media.example/missing.jpg", "opens through the host, not in the sandbox");
  const players = await inView(() => ({
    video: document.querySelector("#cards-grid .rcard:first-child video")?.controls,
    audio: document.querySelector("#cards-grid .rcard:first-child audio")?.controls,
    videoSrc: document.querySelector("#cards-grid .rcard:first-child video")?.getAttribute("src"),
  }));
  assert.deepEqual(players, { video: true, audio: true, videoSrc: "https://media.example/clip.webm" });
  await until(
    () => {
      const mv = document.querySelector("#cards-grid .rcard:first-child model-viewer");
      return mv && customElements.get("model-viewer") && mv.loaded;
    },
    null,
    "the 3D model loaded in model-viewer (WebGL)",
  );
  assert.ok(await inView(() => document.querySelector("#cards-grid .rcard:first-child .media-slot[data-kind=model3d]").classList.contains("done")));
  await shot("-cards");

  // Nothing injected: a value that is markup is text, an address that is a script is not a link.
  const evil = await inView(() => {
    const c = document.querySelector("#cards-grid .rcard:first-child");
    const fields = Object.fromEntries([...c.querySelectorAll(".cf")].map((f) => [f.querySelector(".cf-k").textContent, f.querySelector(".cf-v")]));
    return {
      text: fields.Evil.textContent,
      imgInEvil: !!fields.Evil.querySelector("img"),
      js: fields.Js.textContent,
      jsLink: !!fields.Js.querySelector("a"),
      anyScriptHref: !!document.querySelector('#cards-grid a[href^="javascript"]'),
      pwned: window.__pwned ?? null,
    };
  });
  assert.deepEqual(evil, { text: "<img src=x onerror=window.__pwned=1>", imgInEvil: false, js: "javascript:window.__pwned=2", jsLink: false, anyScriptHref: false, pwned: null });

  // The Fields menu: the same types as the playground's; a field's type is set per column.
  await view.locator("#cards-fields-menu summary").click();
  const opts = await view.locator('#cards-fields select[data-field="photo"] option').allTextContents();
  assert.deepEqual(opts, ["Auto", "Text", "Link", "Button", "Image", "Audio", "Video", "3D", "PDF", "Number"]);
  await view.locator('#cards-fields select[data-field="photo"]').selectOption("link");
  await until(() => window.__reteView.options.card_fields.photo === "link", null, "the setting kept");
  assert.equal(await inView(() => document.querySelectorAll("#cards-grid .rcard:first-child .media-slot[data-kind=image]").length), 1, "only the picture that is not there is left");
  assert.equal(await view.locator('#cards-grid .rcard:first-child .cf a[href^="https://media.example/img-"]').count(), 1);
  await view.locator('#cards-fields select[data-field="clip"]').selectOption("text");
  assert.equal(await inView(() => document.querySelectorAll("#cards-grid .rcard:first-child video").length), 0);
  await view.locator('#cards-fields select[data-field="n"]').selectOption("number");
  assert.equal(await view.locator("#cards-grid .rcard:first-child .cf-v.num").count(), 1);
  await view.locator('#cards-fields select[data-field="photo"]').selectOption("auto");
  await until(() => !("photo" in window.__reteView.options.card_fields), null, "auto goes back to detection");
  // The setting is what Claude reads with graph_view_state, and sets with graph_view_update.
  await inView(() => (document.querySelector("#cards-fields-menu").open = false));

  // More cards, then a filter, then none.
  await view.locator("#cards-more").click();
  assert.equal(await view.locator("#cards-grid .rcard").count(), 72);
  assert.match(await view.locator("#cards-more").textContent(), /^Show 48 more \(217 hidden\)$/);
  await view.locator("#cards-filter").fill("zzz-not-there");
  await until(() => document.querySelectorAll("#cards-grid .rcard").length === 0 && !document.getElementById("cards-empty").hidden, null, "no matches");
  await view.locator("#cards-filter").fill("");
  await until(() => document.querySelectorAll("#cards-grid .rcard").length === 24, null, "a filter starts the paging again");

  // A card is a row: selecting it selects its entity in the graph, with the keyboard too.
  await view.locator("#cards-grid .rcard:first-child").focus();
  await page.keyboard.press("Enter");
  await until(() => window.__reteView.selection.length === 1, null, "the card's entity selected");
  assert.ok(await view.locator("#cards-grid .rcard:first-child").evaluate((c) => c.classList.contains("on")));

  // One column on a narrow page.
  await page.locator("#f").evaluate((f) => (f.style.width = "380px"));
  await page.waitForTimeout(400);
  assert.equal(await lefts(), 1, "a single column");
  await page.locator("#f").evaluate((f) => (f.style.width = "1080px"));
  await shot("-cards-narrow");

  // Back on the graph, nothing of the cards runs.
  await view.locator('.tabs button[data-tab="graph"]').click();
  assert.ok(await view.locator("#cards").isHidden());
});

test("cards: a host that lists what it allows gets no request for the rest; the page says why", { skip }, async (t) => {
  const requests = [];
  const { page, view, until, links } = await host(
    t,
    { dataset: "people", query: CARDS_QUERY, title: "everything", options: { view: "cards" } },
    { caps: { sandbox: { csp: { resourceDomains: ["https://allowed.example"], connectDomains: [] } } }, route: (p) => mediaHost(p, requests) },
  );
  await until(() => window.__reteView?.table?.rows?.length > 100 && !document.getElementById("cards").hidden, null, "the cards");
  await until(() => document.querySelector("#cards-grid .rcard:first-child .media-fallback"), null, "a card told what is blocked");
  await page.waitForTimeout(500);
  assert.deepEqual(requests, [], "nothing was asked of a site the host does not allow");
  const text = await view.locator("#cards-grid .rcard:first-child .media-fallback").first().textContent();
  assert.match(text, /media\.example is not on this host's list of allowed sites/);
  // And the link still works: the host opens it.
  await view.locator("#cards-grid .rcard:first-child .media-fallback a").first().click();
  for (let i = 0; i < 30 && !links.length; i++) await page.waitForTimeout(100);
  assert.match(links.at(-1), /^https:\/\/media\.example\//);
});

test("the hive layout: nodes on axes by class, ordered by links, edges bent between the axes", { skip }, async (t) => {
  const { page, view, inView, until, shot } = await host(t, {
    dataset: "people",
    query: "CONSTRUCT { ?s ?p ?o } WHERE { ?s ?p ?o FILTER(isIRI(?o)) }",
    options: { layout: "hive" },
  });
  await until(() => window.__reteView?.cy?.nodes().length === 5 && window.__reteView.hive, null, "the hive drawn");
  await until(() => window.__reteView.cy.getElementById("<https://example.org/ada>").data("label") === "Ada Lovelace", null, "labels");
  await view.locator("#opt-layout").selectOption("circle");
  await until(() => !window.__reteView.hive, null, "the hive taken down");
  assert.equal(await inView(() => window.__reteView.cy.edges()[0].style("curve-style")), "bezier", "the other layouts' lines are back");
  assert.ok(await view.locator("#hive-axes").isHidden());
  assert.ok(await view.locator("#opt-hive-axes").isHidden());

  await view.locator("#opt-layout").selectOption("hive");
  await until(() => window.__reteView.hive?.axes.length >= 2, null, "axes");
  assert.ok(await view.locator("#opt-hive-axes").isVisible(), "the axis and order controls appear");
  // The axes are the classes (Person, then the rest), the people on one axis.
  const plan = await inView(() => ({ by: window.__reteView.hive.by, axes: window.__reteView.hive.axes.map((a) => [a.key, a.count]) }));
  assert.equal(plan.by, "type");
  assert.deepEqual(plan.axes[0], ["Person", 3]);
  await page.waitForTimeout(600); // the layout animates
  const placed = await inView(() => {
    const S = window.__reteView;
    return S.cy.nodes().map((n) => [n.position("x") - S.hive.pos[n.id()].x, n.position("y") - S.hive.pos[n.id()].y]);
  });
  for (const [dx, dy] of placed) assert.ok(Math.abs(dx) < 1 && Math.abs(dy) < 1, "every node is where the plan put it");
  assert.equal(await inView(() => window.__reteView.cy.edges()[0].style("curve-style")), "unbundled-bezier");
  assert.equal(await view.locator("#hive-axes line").count(), plan.axes.length);
  assert.ok(await view.locator("#hive-axes").isVisible(), "the axes are drawn over the graph");
  assert.match(await view.locator("#hive-axes text").first().textContent(), /^Person \(3\)$/);
  await shot("-hive");

  // Still a graph: select, hover and pan work as in the other layouts.
  await inView(() => window.__reteView.cy.getElementById("<https://example.org/ada>").select());
  await until(() => window.__reteView.selection.length === 1, null, "a selection");
  await inView(() => window.__reteView.cy.panBy({ x: 40, y: 0 }));
  const moved = await inView(() => {
    const line = document.querySelector("#hive-axes line");
    return line.getAttribute("x1");
  });
  await inView(() => window.__reteView.cy.panBy({ x: 40, y: 0 }));
  assert.notEqual(await inView(() => document.querySelector("#hive-axes line").getAttribute("x1")), moved, "the axes follow the pan");

  // Another axis and order, from the controls.
  await view.locator(".tabs button[data-tab=cards]").click();
  assert.ok(await view.locator("#hive-axes").isHidden(), "not over the cards");
  await view.locator(".tabs button[data-tab=graph]").click();
  await until(() => !document.getElementById("hive-axes").hasAttribute("hidden"), null, "the axes back with the graph");
  await view.locator("#opt-hive-axes").selectOption("role");
  await until(() => window.__reteView.hive.by === "role", null, "axes by role");
  assert.deepEqual(await inView(() => window.__reteView.hive.axes.map((a) => a.key)), ["sources", "both", "sinks"]);
  await view.locator("#opt-hive-order").selectOption("name");
  await until(() => window.__reteView.options.hive_order === "name", null, "ordered by name");
  await shot("-hive-role");
});

test("cards under a content security policy like Claude's: what is declared loads, the rest says so", { skip }, async (t) => {
  const requests = [];
  const policy = "default-src 'none'; script-src 'unsafe-inline' 'wasm-unsafe-eval'; style-src 'unsafe-inline'; img-src data: https://media.example; media-src https://media.example; connect-src https://media.example";
  const { view, inView, until } = await host(
    t,
    {
      dataset: "people",
      title: "policy",
      query: `SELECT ?a ?other ?model WHERE { BIND("https://media.example/ok.png" AS ?a) BIND("https://elsewhere.example/blocked.png" AS ?other) BIND("https://media.example/triangle.glb" AS ?model) }`,
      options: { view: "cards" },
    },
    { csp: policy, route: (p) => mediaHost(p, requests) },
  );
  await until(() => document.querySelectorAll("#cards-grid .rcard").length === 1, null, "the card");
  // The declared site: the picture loads, and so does the model (the viewer starts from an inline script, which the policy allows).
  await until(() => document.querySelector(".media-slot[data-kind=image].done img"), null, "the picture from the declared site");
  // A model loads when it is in view (model-viewer's own laziness, on top of the page's).
  await inView(() => document.querySelector("model-viewer").scrollIntoView({ block: "center" }));
  await until(() => document.querySelector("model-viewer")?.loaded, null, "the model from the declared site");
  // The other: refused by the policy, so the card says so and links to it, and the status line names the site.
  await until(() => document.querySelector(".media-fallback"), null, "the refused picture");
  assert.match(await view.locator(".media-fallback").first().textContent(), /did not load here \(the host may block elsewhere\.example\)/);
  await until(() => /elsewhere\.example/.test(document.getElementById("status-note").textContent), null, "the status line names the blocked site");
  assert.deepEqual(requests.filter((u) => u.includes("elsewhere")), []);
  assert.equal(await inView(() => window.__reteView.options.view), "cards");
});

test("a 3D model the policy will not let the viewer fetch, or that never arrives, ends as a link, not a blank", { skip }, async (t) => {
  const requests = [];
  const policy = "default-src 'none'; script-src 'unsafe-inline' 'wasm-unsafe-eval'; style-src 'unsafe-inline'; img-src data:; connect-src 'none'";
  const asked = {
    dataset: "people",
    title: "strict",
    query: `SELECT ?model WHERE { BIND("https://media.example/triangle.glb" AS ?model) }`,
    options: { view: "cards" },
  };
  // connect-src does not list the site: the fetch is refused, the viewer says so, and the card links to the file.
  const refused = await host(t, asked, { csp: policy, route: (p) => mediaHost(p, requests) });
  await refused.until(() => document.querySelector(".media-fallback"), null, "the refused model");
  assert.match(await refused.view.locator(".media-fallback").first().textContent(), /3D model did not load here \(the host may block media\.example\)/);
  assert.equal(await refused.view.locator(".media-fallback a").first().getAttribute("data-open"), "https://media.example/triangle.glb");
  await refused.until(() => /media\.example/.test(document.getElementById("status-note").textContent), null, "the status line names the site");

  // A model that never arrives: after a while on screen, unloaded, it becomes a link too.
  const hung = await host(
    t,
    { ...asked, query: `SELECT ?model WHERE { BIND("https://media.example/hang.bin" AS ?model) }`, options: { view: "cards", card_fields: { model: "model3d" } } },
    { route: (p) => mediaHost(p, requests) },
  );
  await hung.until(() => window.__reteView?.cards && document.querySelector("model-viewer"), null, "the viewer");
  await hung.inView(() => (window.__reteView.cards.viewerTimeout = 1500));
  await hung.until(() => document.querySelector(".media-fallback"), null, "the model given up on");
  assert.match(await hung.view.locator(".media-fallback").first().textContent(), /3D model did not finish loading here/);
});
