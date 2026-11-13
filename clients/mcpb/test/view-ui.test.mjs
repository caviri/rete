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
async function host(t, args) {
  const { chromium } = await import("playwright");
  const { build } = await import("esbuild");

  const dir = await fixture();
  const client = new Client({ name: "ui-host", version: "0" });
  await client.connect(new StdioClientTransport({ command: process.execPath, args: [SERVER, dir] }));
  t.after(() => client.close());

  const html = (await client.readResource({ uri: "ui://rete/graph-view.html" })).contents[0].text;
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
    async ({ html, args, result }) => {
      const iframe = document.getElementById("f");
      const bridge = new window.AppBridge(
        null,
        { name: "test host", version: "0" },
        { openLinks: {}, serverTools: {}, updateModelContext: { text: {} }, message: { text: {} }, downloadFile: {} },
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
    { html, args, result },
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
