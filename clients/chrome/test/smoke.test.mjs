// Loads build/ into Chromium the way a user does ("Load unpacked") and checks what a user sees:
//   - the service worker starts, named Retelens, with the workspace version;
//   - the viewer opens a .rete file over HTTP Range and shows the first example query's answer,
//     with the engine this commit built (the status line names its version);
//   - the Graph view draws (graph view bundle and layout worker under the extension's CSP).
// Hermetic: the file is the repo's docs/plaza/data/linked-jazz.rete on a local Range server.
// Chrome loads extensions only in a headed browser, so CI runs this under xvfb-run.
//
//   node clients/chrome/build.mjs && xvfb-run -a node --test clients/chrome/test/*.test.mjs
import { test, before, after } from "node:test";
import assert from "node:assert/strict";
import { createServer } from "node:http";
import { mkdtempSync, readFileSync, rmSync, statSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { chromium } from "playwright";

const here = dirname(fileURLToPath(import.meta.url));
const ext = join(here, "..", "build");
const repo = join(here, "..", "..", "..");
const FILE = join(repo, "docs", "plaza", "data", "linked-jazz.rete");
const version = readFileSync(join(repo, "Cargo.toml"), "utf8").match(/^\[workspace\.package\][\s\S]*?^version = "([^"]+)"/m)[1];

let server, base, ctx, profile, extId;
const errors = [];

before(async () => {
  statSync(join(ext, "manifest.json")); // build first: node clients/chrome/build.mjs
  const bytes = readFileSync(FILE);
  server = createServer((req, res) => {
    const cors = {
      "Access-Control-Allow-Origin": "*",
      "Access-Control-Allow-Headers": "Range",
      "Access-Control-Expose-Headers": "Content-Range, Content-Length, Accept-Ranges",
      "Accept-Ranges": "bytes",
    };
    if (req.method === "OPTIONS") { res.writeHead(204, cors); return res.end(); }
    const m = /^bytes=(\d+)-(\d*)$/.exec(req.headers.range || "");
    if (!m) { res.writeHead(200, { ...cors, "Content-Length": bytes.length }); return res.end(req.method === "HEAD" ? undefined : bytes); }
    const a = +m[1], b = Math.min(m[2] ? +m[2] : bytes.length - 1, bytes.length - 1);
    if (a >= bytes.length) { res.writeHead(416, { ...cors, "Content-Range": `bytes */${bytes.length}` }); return res.end(); }
    res.writeHead(206, { ...cors, "Content-Range": `bytes ${a}-${b}/${bytes.length}`, "Content-Length": b - a + 1 });
    res.end(req.method === "HEAD" ? undefined : bytes.subarray(a, b + 1));
  });
  await new Promise((r) => server.listen(0, "127.0.0.1", r));
  base = `http://127.0.0.1:${server.address().port}`;
  profile = mkdtempSync(join(tmpdir(), "retelens-"));
  ctx = await chromium.launchPersistentContext(profile, {
    headless: false,
    viewport: { width: 1280, height: 800 },
    args: [`--disable-extensions-except=${ext}`, `--load-extension=${ext}`],
  });
});

after(async () => {
  await ctx?.close();
  server?.close();
  if (profile) rmSync(profile, { recursive: true, force: true });
});

test("the service worker starts as Retelens with the workspace version", async () => {
  const sw = ctx.serviceWorkers()[0] || (await ctx.waitForEvent("serviceworker", { timeout: 30000 }));
  extId = new URL(sw.url()).host;
  // the worker can be listed a moment before the extension APIs are bound in it: ask until they are
  let m = null;
  for (let i = 0; i < 40 && !m; i++) {
    m = await sw.evaluate(() => (globalThis.chrome && chrome.runtime ? chrome.runtime.getManifest() : null)).catch(() => null);
    if (!m) await new Promise((r) => setTimeout(r, 250));
  }
  assert.ok(m, "the service worker never exposed chrome.runtime");
  assert.equal(m.name, "Retelens");
  assert.equal(m.version, version.match(/^\d+\.\d+\.\d+/)[0]);
});

test("the viewer answers the file's first example query, then draws the graph", async () => {
  const page = await ctx.newPage();
  page.on("pageerror", (e) => errors.push(String(e)));
  const src = `${base}/linked-jazz.rete`;
  await page.goto(`chrome-extension://${extId}/viewer.html?` + new URLSearchParams({ src, name: "linked-jazz.rete", host: "url" }));
  await page.waitForSelector("table td", { timeout: 90000 });
  const cells = await page.locator("table td").allInnerTexts();
  assert.ok(cells.some((t) => /^11,?471$/.test(t.trim())), `expected the statement count 11471 in ${JSON.stringify(cells.slice(0, 8))}`);
  // locator waits only: the extension pages' CSP has no unsafe-eval, which waitForFunction needs
  await page.getByText(`rete-graph ${version}`).first().waitFor({ timeout: 30000 });

  // the musicians' relationships, from the SPARQL console, drawn as a graph
  await page.locator('button[data-tab="console"]').click();
  await page.fill('textarea[aria-label="SPARQL query"]',
    "SELECT ?s ?p ?o WHERE { ?s ?p ?o . FILTER(STRSTARTS(STR(?p), \"http://purl.org/vocab/relationship/\")) } LIMIT 200");
  await page.locator("button", { hasText: "Run" }).first().click();
  await page.locator("tbody tr").nth(20).waitFor({ timeout: 60000 });
  await page.locator('button[data-view="graph"]').first().click();
  await page.locator('[data-rl="graph"] canvas').first().waitFor({ state: "attached", timeout: 60000 });
  const note = await page.locator('[data-rl="graph-note"]').innerText();
  assert.match(note, /\b\d+ nodes, \d+ edges/);
  assert.deepEqual(errors, []);
  await page.close();
});
