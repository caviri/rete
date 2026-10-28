// The graph view in a real browser, behind a host built on the MCP Apps SDK's
// own AppBridge — the host side Claude Desktop implements: the page runs in a
// sandboxed iframe, gets the tool's input and result, and its tool calls go
// through the bridge to a real server process. Needs Chromium, so it runs only
// with RETE_UI_TEST=1 (the Playwright image has it; see clients/mcpb/README.md).
// RETE_UI_SHOT=<path> also saves a screenshot.
import assert from "node:assert/strict";
import { dirname, join } from "node:path";
import test from "node:test";
import { fileURLToPath } from "node:url";

import { Client } from "@modelcontextprotocol/sdk/client/index.js";
import { StdioClientTransport } from "@modelcontextprotocol/sdk/client/stdio.js";

import { fixture } from "./view-fixture.mjs";

const here = dirname(fileURLToPath(import.meta.url));
const SERVER = join(here, "..", "build", "server", "index.mjs");
const enabled = process.env.RETE_UI_TEST === "1";

const KNOWS =
  "PREFIX ex: <https://example.org/> CONSTRUCT { ?a ex:knows ?b . ?a a ?t } WHERE { ?a ex:knows ?b ; a ?t }";

test("the graph view works in a sandboxed iframe behind an MCP Apps host", { skip: !enabled && "set RETE_UI_TEST=1 (needs Chromium)" }, async (t) => {
  const { chromium } = await import("playwright");
  const { build } = await import("esbuild");

  const client = new Client({ name: "ui-host", version: "0" });
  await client.connect(new StdioClientTransport({ command: process.execPath, args: [SERVER, await fixture()] }));
  t.after(() => client.close());

  const html = (await client.readResource({ uri: "ui://rete/graph-view.html" })).contents[0].text;
  const args = { dataset: "people", query: KNOWS, title: "who knows whom" };
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
  await page.exposeFunction("hostCallTool", async (params) => client.callTool(params));
  await page.exposeFunction("hostSaw", (kind, params) => (kind === "context" ? toModel : messages).push(params));
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

  // It loads its view, then names the nodes.
  await until(() => window.__reteView?.cy?.nodes().length === 3, null, "3 nodes drawn");
  await until(() => window.__reteView.cy.getElementById("<https://example.org/ada>").data("label") === "Ada Lovelace", null, "labels");
  assert.equal(await inView(() => window.__reteView.viewId), viewId);

  // Labels off and on.
  await view.locator("#opt-labels").uncheck();
  assert.equal(await inView(() => window.__reteView.cy.nodes()[0].style("label")), "");
  await view.locator("#opt-labels").check();

  // Layers: by type; hiding one hides its nodes; boxes draw them.
  // Charles's type comes with the labels the view asks for: then all three are people.
  await until(() => window.__reteView.groups.get("Person")?.count === 3, null, "a Person layer of 3");
  await view.locator('#layers input[data-layer="Person"]').uncheck();
  assert.equal(await inView(() => window.__reteView.cy.nodes(".hidden").length), 3);
  await view.locator('#layers input[data-layer="Person"]').check();
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

  if (process.env.RETE_UI_SHOT) {
    await inView(() => window.__reteView.cy.$(":selected").unselect());
    await view.locator("#fit").click();
    await page.waitForTimeout(600);
    await page.locator("#f").screenshot({ path: process.env.RETE_UI_SHOT });
  }
});
