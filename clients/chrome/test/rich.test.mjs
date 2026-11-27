// What the playground's cells, PMTiles and the page reader do in the extension, checked in a real Chromium:
//   - table cells: images, a IIIF manifest (paged thumbnail), WKT geometries (locator maps), a column shown as
//     Markdown from the column menu;
//   - cards: picture first, fields label over value, the focus view moves with the arrow keys;
//   - column statistics in the table header (histogram, values; a click on a bar filters the column);
//   - the Map view of a result with geometry, over the basemap set in the popup (here the test's own archive; tiles
//     embedded in a .rete exist only in format 0x05 files, written by
//     scripts/embed_tiles.py; they are checked by hand on geoadmin-tiles.rete, see the README);
//   - a .pmtiles URL opens the map viewer with the archive's layers;
//   - "Read this page's data" on a page with JSON-LD, microdata and Open Graph tags: the reader shows the recipe in
//     order, the graph answers a query, the .rete can be downloaded.
// Hermetic: every fixture is made in memory (test/fixtures.mjs) and served from 127.0.0.1. The build is copied and
// given host access to 127.0.0.1, which a user grants by choosing "Read this page's data" (activeTab); automation
// cannot click a context menu.
//   node clients/chrome/build.mjs && xvfb-run -a node --test clients/chrome/test/*.test.mjs
import { test, before, after } from "node:test";
import assert from "node:assert/strict";
import { createServer } from "node:http";
import { cpSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";
import { chromium } from "playwright";
import { png, pmtiles } from "./fixtures.mjs";

const here = dirname(fileURLToPath(import.meta.url));
const build = join(here, "..", "build");
const repo = join(here, "..", "..", "..");

let server, base, ctx, profile, extDir, extId, sw;
const files = new Map(); // path -> {body, type}
const errors = [];
const requested = []; // paths the test server was asked for

function serve(req, res) {
  const cors = { "Access-Control-Allow-Origin": "*", "Access-Control-Allow-Headers": "Range", "Access-Control-Expose-Headers": "Content-Range, Content-Length, Accept-Ranges", "Accept-Ranges": "bytes" };
  if (req.method === "OPTIONS") { res.writeHead(204, cors); return res.end(); }
  requested.push(req.url.split("?")[0]);
  const f = files.get(req.url.split("?")[0]);
  if (!f) { res.writeHead(404, cors); return res.end(); }
  const m = /^bytes=(\d+)-(\d*)$/.exec(req.headers.range || "");
  if (!m) { res.writeHead(200, { ...cors, "Content-Type": f.type, "Content-Length": f.body.length }); return res.end(f.body); }
  const a = +m[1], b = Math.min(m[2] ? +m[2] : f.body.length - 1, f.body.length - 1);
  if (a >= f.body.length) { res.writeHead(416, { ...cors, "Content-Range": `bytes */${f.body.length}` }); return res.end(); }
  res.writeHead(206, { ...cors, "Content-Type": f.type, "Content-Range": `bytes ${a}-${b}/${f.body.length}`, "Content-Length": b - a + 1 });
  res.end(f.body.subarray(a, b + 1));
}

before(async () => {
  server = createServer(serve);
  await new Promise((r) => server.listen(0, "127.0.0.1", r));
  base = `http://127.0.0.1:${server.address().port}`;
  const rete = await import(pathToFileURL(join(repo, "clients", "js", "dist", "index.js")).href);
  const pm = pmtiles();
  const ttl = `@prefix ex: <http://example.org/> . @prefix schema: <http://schema.org/> . @prefix geo: <http://www.opengis.net/ont/geosparql#> .
ex:a schema:name "Alpha" ; schema:image <${base}/img/a.png> ; ex:manifest <${base}/iiif/manifest.json> ; geo:asWKT "POINT(6.63 46.52)"^^geo:wktLiteral ; ex:note "**Bold** and a [link](https://example.org/)" ; ex:place "Testland" .
ex:b schema:name "Beta" ; schema:image <${base}/img/b.png> ; geo:asWKT "POLYGON((-10 -10, 10 -10, 10 10, -10 10, -10 -10))"^^geo:wktLiteral ; ex:note "plain" ; ex:place "Elsewhere" .`;
  const built = Buffer.from(await rete.build(ttl, "ttl"));
  files.set("/rich.rete", { body: built, type: "application/octet-stream" });
  const otherNt = [
    '<http://example.org/x> <http://schema.org/name> "Other" .',
    '<http://example.org/y> <http://schema.org/name> "Another" .',
    "<http://example.org/x> <http://example.org/knows> <http://example.org/y> .",
  ].join("\n") + "\n";
  files.set("/other.rete", { body: Buffer.from(await rete.build(otherNt, "nt")), type: "application/octet-stream" });
  files.set("/test.pmtiles", { body: pm, type: "application/octet-stream" });
  files.set("/img/a.png", { body: png(64, 48, [42, 111, 176]), type: "image/png" });
  files.set("/img/b.png", { body: png(48, 64, [200, 100, 30]), type: "image/png" });
  files.set("/iiif/manifest.json", { type: "application/json", body: Buffer.from(JSON.stringify({
    "@context": "http://iiif.io/api/presentation/3/context.json", id: `${base}/iiif/manifest.json`, type: "Manifest", label: { en: ["A test book"] },
    metadata: [{ label: { en: ["Date"] }, value: { en: ["1450"] } }],
    items: ["a", "b"].map((n, i) => ({ id: `${base}/iiif/canvas/${i}`, type: "Canvas", label: { none: [`p. ${i + 1}`] },
      items: [{ type: "AnnotationPage", items: [{ type: "Annotation", motivation: "painting", body: { id: `${base}/img/${n}.png`, type: "Image", format: "image/png" } }] }] })),
  })) });
  files.set("/page.html", { type: "text/html; charset=utf-8", body: Buffer.from(`<!doctype html><html lang="en"><head><title>Tomato soup | Example kitchen</title>
<meta property="og:title" content="Tomato soup"><meta property="og:image" content="${base}/img/a.png"><meta name="description" content="A soup.">
<script type="application/ld+json">
{"@context":"http://schema.org","@type":["Recipe"],"@id":"#recipe","name":"Tomato soup","description":"A plain tomato soup.",
 "image":{"@type":"ImageObject","url":"${base}/img/a.png"},"author":[{"@type":"Person","name":"A. Cook"}],
 "prepTime":"PT10M","cookTime":"PT45M","totalTime":"PT55M","recipeYield":"4",
 "recipeIngredient":["4 tomatoes","1 onion","2 cups stock","salt"],
 "recipeInstructions":[{"@type":"HowToStep","text":"Chop the onion and the tomatoes."},{"@type":"HowToStep","text":"Simmer in the stock for 40 minutes."},{"@type":"HowToStep","text":"Blend and season."}],
 "aggregateRating":{"@type":"AggregateRating","ratingValue":"4.5","ratingCount":"12"},
 "nutrition":{"@type":"NutritionInformation","calories":"120 kcal","fatContent":"3 g"},
 "publisher":{"@type":"Organization","name":"Example kitchen","url":"https://example.org/"}}
</script>
<script type="application/ld+json">{"@context":"https://schema.org","@type":"BreadcrumbList","itemListElement":[{"@type":"ListItem","position":1,"item":{"@id":"${base}/","name":"Home"}}]}</script>
</head><body><p>A long story about soup, then ads.</p>
<div itemscope itemtype="https://schema.org/Event"><span itemprop="name">Soup festival</span> <time itemprop="startDate" datetime="2026-12-01">1 Dec</time> <a itemprop="url" href="/festival">more</a></div>
</body></html>`) });

  // the build, with host access to the test server (a user's activeTab grant)
  extDir = mkdtempSync(join(tmpdir(), "retelens-ext-"));
  cpSync(build, extDir, { recursive: true });
  const man = JSON.parse(readFileSync(join(extDir, "manifest.json"), "utf8"));
  man.host_permissions.push("http://127.0.0.1/*");
  writeFileSync(join(extDir, "manifest.json"), JSON.stringify(man, null, 2));
  profile = mkdtempSync(join(tmpdir(), "retelens-"));
  ctx = await chromium.launchPersistentContext(profile, {
    headless: false, viewport: { width: 1280, height: 860 },
    args: [`--disable-extensions-except=${extDir}`, `--load-extension=${extDir}`],
  });
  sw = ctx.serviceWorkers()[0] || (await ctx.waitForEvent("serviceworker", { timeout: 30000 }));
  extId = new URL(sw.url()).host;
});

after(async () => {
  await ctx?.close();
  server?.close();
  for (const d of [profile, extDir]) if (d) rmSync(d, { recursive: true, force: true });
});

const shots = process.env.RETELENS_SHOTS; // a folder: save screenshots there to look at
const shot = async (page, name) => { if (shots) await page.screenshot({ path: join(shots, name + ".png") }); };

// waitForFunction evaluates a string, which an extension page's policy refuses: poll with evaluate instead
async function until(page, fn, ms = 60000, what = "the page") {
  const end = Date.now() + ms;
  while (Date.now() < end) { if (await page.evaluate(fn).catch(() => false)) return; await page.waitForTimeout(150); }
  throw new Error("timed out waiting for " + what);
}
async function openViewer(params) {
  const page = await ctx.newPage();
  page.on("pageerror", (e) => errors.push(String(e)));
  await page.goto(`chrome-extension://${extId}/viewer.html?` + new URLSearchParams(params));
  return page;
}
async function runQuery(page, q) {
  await page.locator('button[data-tab="console"]').click();
  await page.fill('textarea[aria-label="SPARQL query"]', q);
  await page.locator("button", { hasText: "Run" }).first().click();
}

test("table cells: images, IIIF, geometries; a column shown as Markdown", async () => {
  const page = await openViewer({ src: `${base}/rich.rete`, name: "rich.rete", host: "url" });
  await page.waitForSelector("table td", { timeout: 90000 });
  await runQuery(page, "PREFIX schema: <http://schema.org/> PREFIX ex: <http://example.org/> PREFIX geo: <http://www.opengis.net/ont/geosparql#>\nSELECT ?name ?image ?manifest ?wkt ?note ?place WHERE { ?s schema:name ?name ; schema:image ?image ; geo:asWKT ?wkt ; ex:place ?place ; ex:note ?note . OPTIONAL { ?s ex:manifest ?manifest } } ORDER BY ?name");
  await page.locator("img.rl-thumb").first().waitFor({ timeout: 30000 });
  // (no waitForFunction: it needs unsafe-eval, which the extension pages' CSP does not allow)
  let loaded = 0;
  for (let i = 0; i < 60 && loaded < 2; i++) {
    loaded = await page.locator("img.rl-thumb").evaluateAll((els) => els.filter((im) => im.complete && im.naturalWidth > 0).length);
    if (loaded < 2) await page.waitForTimeout(250);
  }
  assert.ok(loaded >= 2, `two thumbnails loaded, got ${loaded}`);
  await page.locator(".rl-iiif-frame img").first().waitFor({ timeout: 30000 });
  assert.match(await page.locator(".rl-iiif-nav").first().innerText(), /1 \/ 2/);
  assert.equal(await page.locator(".rl-geo svg").count(), 2);
  assert.match(await page.locator("thead").innerText(), /Name[\s\S]*Image[\s\S]*Manifest/);
  // the column menu: show ?note as Markdown
  await page.locator('[data-rl="colfilter"][data-col="note"]').click();
  await page.locator('[data-rl="colmenu"] select[aria-label="Show as"]').selectOption("markdown");
  await page.locator(".rl-md strong", { hasText: "Bold" }).waitFor({ timeout: 10000 });
  await page.keyboard.press("Escape");
  await shot(page, "table");
  // a IIIF thumbnail opens the paged viewer with the manifest's metadata
  await page.locator(".rl-iiif-frame").first().click();
  await page.locator(".rl-iiif-modal .rl-iiif-title", { hasText: "A test book" }).waitFor({ timeout: 10000 });
  await page.keyboard.press("ArrowRight");
  assert.match(await page.locator(".rl-iiif-modal .rl-iiif-count").innerText(), /page 2 \/ 2/);
  await shot(page, "iiif");
  await page.keyboard.press("Escape");
  assert.equal(await page.locator(".rl-modal").count(), 0);

  // cards: picture first, fields label over value; the focus view moves with the arrow keys
  await page.locator('button[data-view="cards"]').first().click();
  await page.locator('[data-rl="card"] .rl-card-media img').first().waitFor({ timeout: 10000 });
  assert.equal(await page.locator('[data-rl="card"]').count(), 2);
  assert.match(await page.locator('[data-rl="card"]').first().innerText(), /Alpha[\s\S]*Place[\s\S]*Testland/);
  await shot(page, "cards");
  await page.locator('[data-rl="card"] [data-rl="card-title"]').first().evaluate((el) => el.closest("[data-rl=card]").querySelector(".rl-fields dt").click());
  await page.locator(".rl-focus .rl-focus-count", { hasText: "1 / 2" }).waitFor({ timeout: 10000 });
  await page.keyboard.press("ArrowRight");
  await page.locator(".rl-focus .rl-focus-count", { hasText: "2 / 2" }).waitFor({ timeout: 10000 });
  await shot(page, "focus");
  await page.keyboard.press("Escape");

  // the Map view: both rows, over the basemap the popup sets (the default, data.graphplaza.com, is not used here)
  await sw.evaluate((u) => chrome.storage.local.set({ basemapUrl: u }), `${base}/test.pmtiles`);
  await page.locator('button[data-view="map"]').first().click();
  await page.locator(".rl-mapbar", { hasText: "2 places" }).waitFor({ timeout: 30000 });
  for (let i = 0; i < 40 && !requested.includes("/test.pmtiles"); i++) await page.waitForTimeout(250);
  assert.ok(requested.includes("/test.pmtiles"), "the basemap archive was read");
  assert.equal(await page.locator(".rl-mapbar .rl-err").count(), 0);
  await page.locator(".rl-map canvas").first().waitFor({ state: "attached", timeout: 30000 });
  await page.waitForTimeout(800);
  await shot(page, "map");
  assert.deepEqual(errors, []);
  await page.close();
});

test("column statistics: a histogram, the values, a click filters", async () => {
  const page = await openViewer({ src: `${base}/rich.rete`, name: "rich.rete", host: "url" });
  await page.waitForSelector("table td", { timeout: 90000 });
  const colors = ["red", "green", "blue"];
  const values = Array.from({ length: 30 }, (_, i) => `(${i * i} "${colors[i % 3]}")`).join(" ");
  await runQuery(page, `SELECT ?n ?colour WHERE { VALUES (?n ?colour) { ${values} } }`);
  await page.locator('[data-rl="stat"][data-kind="number"]').waitFor({ timeout: 30000 });
  const num = page.locator('[data-rl="stat"][data-kind="number"]');
  assert.ok((await num.locator(".rl-stat-g").count()) >= 5, "a histogram of several bins");
  assert.match(await num.locator(".rl-stat-cap").innerText(), /^0\s+841$/);
  const cls = page.locator('[data-rl="stat"][data-kind="classes"]');
  assert.match(await cls.locator(".rl-stat-cap").innerText(), /3 values/);
  await cls.locator(".rl-stat-g").first().hover();
  assert.match(await cls.locator(".rl-stat-cap").innerText(), /(red|green|blue)\s+10 · 33%/);
  await shot(page, "stats");
  await cls.locator(".rl-stat-g").first().click();
  await page.getByText("10 of 30 rows").first().waitFor({ timeout: 10000 });
  assert.equal(await page.locator('[data-rl="stat"][data-kind="classes"] .rl-on').count(), 1, "the chosen value is marked");
  await page.locator('[data-rl="stat"][data-kind="classes"] .rl-on').click();
  await page.getByText("30 of 30 rows").first().waitFor({ timeout: 10000 });
  assert.deepEqual(errors, []);
  await page.close();
});

test("query hints pick the view and name the columns; a .pmtiles column is a map layer", async () => {
  const page = await openViewer({ src: `${base}/rich.rete`, name: "rich.rete", host: "url" });
  await page.waitForSelector("table td", { timeout: 90000 });
  await runQuery(page, `PREFIX schema: <http://schema.org/> PREFIX geo: <http://www.opengis.net/ont/geosparql#>
# Places with a layer
# view: map
# column ?name: Nombre — the place's name
# column ?layer: Capa — vector tiles drawn under the places
SELECT ?name ?shape ?layer WHERE { ?s schema:name ?name ; geo:asWKT ?shape . BIND(<${base}/test.pmtiles> AS ?layer) }`);
  // the hint chose the Map view, and the layer column is drawn under the result
  await page.locator('[data-rl="seg-item"][data-view="map"][aria-pressed="true"]').waitFor({ timeout: 30000 });
  await page.locator(".rl-mapbar", { hasText: "Layer: Test shapes" }).waitFor({ timeout: 30000 });
  assert.match(await page.locator('[data-rl="qcols"]').innerText(), /Nombre[\s\S]*the place's name[\s\S]*Capa/);
  // the table names the columns as the hints say, and shows the layer as a button
  await page.locator('button[data-view="table"]').first().click();
  await page.locator("thead", { hasText: "Nombre" }).waitFor({ timeout: 10000 });
  assert.ok(await page.locator("td .rl-btnlink", { hasText: "Map layer" }).count(), "a Map layer button in the table");
  assert.deepEqual(errors, []);
  await page.close();
});

test("a .pmtiles URL opens the map viewer with its layers", async () => {
  const page = await openViewer({ src: `${base}/test.pmtiles`, name: "test.pmtiles", host: "url" });
  await page.locator(".rl-pmside", { hasText: "Test shapes" }).waitFor({ timeout: 30000 });
  const side = await page.locator(".rl-pmside").innerText();
  assert.match(side, /vector \(MVT\)/);
  assert.match(side, /shapes/);
  assert.match(side, /test fixture/);
  await page.locator(".rl-map canvas").first().waitFor({ state: "attached", timeout: 30000 });
  await page.waitForTimeout(800);
  // a click on the square lists its properties
  const box = await page.locator(".rl-map").boundingBox();
  await page.mouse.click(box.x + box.width / 2 + 40, box.y + box.height / 2 - 30);
  await page.locator(".rl-popup", { hasText: "Testland" }).waitFor({ timeout: 10000 });
  await shot(page, "pmtiles");
  assert.deepEqual(errors, []);
  await page.close();
});

test("dark mode on a host whose colours are oklch (Tailwind v4, Hugging Face): the graph labels stay light", async () => {
  const page = await openViewer({ src: `${base}/rich.rete`, name: "rich.rete", host: "url" });
  await page.waitForSelector("table td", { timeout: 90000 });
  // what HF computes now: Tailwind v4 colours as oklch()
  await page.addStyleTag({ content: ".dark .dark\\:text-gray-100{color:oklch(0.967 0.003 264.542)!important}.dark .dark\\:bg-gray-950{background-color:oklch(0.13 0.028 261.692)!important}.text-gray-900{color:oklch(0.21 0.034 264.665)!important}" });
  await page.evaluate(() => document.documentElement.classList.add("dark"));
  assert.match(await page.evaluate(() => window.RetelensTheme.toRgb("oklch(0.967 0.003 264.542)")), /^rgb\(2[34]\d, 2[34]\d, 2[34]\d\)$/, "oklch read as rgb");
  // the probe classes compute to oklch now, as on HF: the tokens the graph gets are rgb all the same
  assert.match(await page.evaluate(() => window.RetelensTheme.tokens(document.body).text), /^rgb\(/);
  await page.locator('[data-rl="seg-item"][data-view="graph"]').first().click();
  await until(page, () => !!(window.__retelensKit && window.__retelensKit.cy && window.__retelensKit.cy.nodes().length), 60000, "the graph");
  await page.waitForTimeout(800);
  const c = await page.evaluate(() => window.__retelensKit.cy.nodes()[0].style("color"));
  const m = /rgb\((\d+),\s*(\d+),\s*(\d+)\)/.exec(c);
  assert.ok(m, "a colour cytoscape reads: " + c);
  const lum = (0.2126 * +m[1] + 0.7152 * +m[2] + 0.0722 * +m[3]) / 255;
  assert.ok(lum > 0.7, `light labels in dark mode (${c})`);
  await page.close();
});

test("the query list filters by words and by view", async () => {
  const page = await openViewer({ src: `${base}/rich.rete`, name: "rich.rete", host: "url" });
  await page.waitForSelector("table td", { timeout: 90000 });
  const ex = await page.evaluate(() => window.__retelens.session.examples.map((e) => ({ title: e.title, view: e.view })));
  // the Query pill opens the list
  await page.locator('button[aria-haspopup="listbox"]', { hasText: /Query|Starter queries/ }).first().click();
  const list = page.locator("#rl-qlist");
  await list.waitFor({ timeout: 10000 });
  const opts = () => page.locator("#rl-qlist [role=option]").count();
  assert.equal(await opts(), ex.length);
  // the view buttons: All, then the views the queries suit, each with its count
  const views = await page.locator("[data-rl=qviews] button").allTextContents();
  assert.match(views[0], new RegExp("^All " + ex.length));
  const word = ex[0].title.split(/s+/)[0];
  await page.fill("input[aria-label=\"Filter queries\"]", word);
  assert.equal(await opts(), ex.filter((e) => e.title.toLowerCase().includes(word.toLowerCase())).length || 0);
  await page.fill("input[aria-label=\"Filter queries\"]", "");
  const v = ex[ex.length - 1].view;
  await page.locator(`[data-rl=qviews] button[data-view="${v}"]`).click();
  assert.equal(await opts(), ex.filter((e) => e.view === v).length);
  await page.close();
});

test("the query list opens in an inline box too (a Hugging Face or Zenodo page has no side panel)", async () => {
  const page = await openViewer({ src: `${base}/rich.rete`, name: "rich.rete", host: "url" });
  const mine = [];
  page.on("pageerror", (e) => mine.push(String(e)));
  await page.waitForSelector("table td", { timeout: 90000 });
  // the Dataset Viewer box of a Hugging Face page: an Explorer in box mode over the same open file
  await page.evaluate(() => {
    const { Explorer } = window.RetelensExplorer;
    const box = new Explorer({ session: window.__retelens.session, mode: "box", urlKeys: [] });
    const host = document.createElement("div");
    host.id = "boxhost"; host.setAttribute("data-retelens", "");
    host.append(box.el);
    document.body.prepend(host);
    box.start();
  });
  await until(page, () => !!document.querySelector("#boxhost table td"), 60000, "the box's table");
  await page.locator('#boxhost button[aria-haspopup="listbox"]', { hasText: /Query|Starter queries/ }).first().click();
  await until(page, () => document.querySelectorAll("#rl-qlist [role=option]").length > 0, 10000, "the query list");
  const r = await page.evaluate(() => { const b = document.getElementById("rl-qlist").closest("[role=dialog]").getBoundingClientRect(); return { w: b.width, h: b.height, top: b.top }; });
  assert.ok(r.w > 100 && r.h > 50, `the list is placed on screen (${JSON.stringify(r)})`);
  // pick one: the box runs it
  await page.locator("#rl-qlist [role=option]").last().click();
  assert.equal(await page.locator("#rl-qlist").count(), 0, "the list closes");
  assert.deepEqual(mine, [], "no error in the page");
  await page.close();
});

test("the SPARQL console colours the query and numbers its lines", async () => {
  const page = await openViewer({ src: `${base}/rich.rete`, name: "rich.rete", host: "url" });
  await page.waitForSelector("table td", { timeout: 90000 });
  await page.locator('button[data-tab="console"]').click();
  const q = "PREFIX schema: <http://schema.org/>\n# names\nSELECT ?s ?name WHERE {\n  ?s schema:name ?name .\n  FILTER(?name != \"x\")\n}\nLIMIT 5";
  await page.fill('textarea[aria-label="SPARQL query"]', q);
  const ed = page.locator("[data-rl=editor]");
  assert.equal((await ed.locator(".rl-ed-gutter").textContent()).split("\n").length, 7, "one number per line");
  const spans = await ed.locator(".rl-ed-hl span").evaluateAll((els) => els.map((e) => [e.textContent, e.className]));
  const cls = (t) => (spans.find(([x]) => x === t) || [])[1] || "";
  assert.match(cls("SELECT"), /text-purple-700/, "a keyword");
  assert.match(cls("<http://schema.org/>"), /text-green-700/, "an IRI");
  assert.match(cls("?name"), /text-blue-700/, "a variable");
  assert.match(cls("# names"), /text-gray-400/, "a comment");
  assert.match(cls('"x"'), /text-amber-700/, "a literal");
  // the copy under the textarea is exactly its text, so the colours sit under the right characters
  assert.equal(await ed.locator(".rl-ed-hl code").evaluate((c) => c.textContent), q + "\n");
  // a value set from code (here an example chosen) redraws the copy and the numbers
  await page.evaluate(() => { const t = document.querySelector('textarea[aria-label="SPARQL query"]'); t.value = "ASK { ?s ?p ?o }"; });
  assert.equal(await ed.locator(".rl-ed-gutter").textContent(), "1");
  assert.equal(await ed.locator(".rl-ed-hl code").evaluate((c) => c.textContent), "ASK { ?s ?p ?o }\n");
  await shot(page, "console");
  await page.close();
});

test("several files: nothing opens until the user picks one; About reads a file's card", async () => {
  const page = await ctx.newPage();
  page.on("pageerror", (e) => errors.push(String(e)));
  const q = new URLSearchParams([["src", `${base}/rich.rete`], ["name", "rich.rete"], ["src", `${base}/other.rete`], ["name", "other.rete"], ["host", "url"]]);
  await page.goto(`chrome-extension://${extId}/viewer.html?` + q);
  await page.waitForSelector("[data-rl=chooser]", { timeout: 60000 });
  assert.match(await page.locator(".rl-choose-q").textContent(), /has 2 graphs. Which one do you want to open?/);
  assert.equal(await page.locator("[data-rl=choice]").count(), 2);
  assert.deepEqual(await page.evaluate(() => [window.__retelens.session.idx, window.__retelens.session.meta]), [-1, null], "no file opened");
  // About reads only the card: the second file has 3 statements
  await page.locator("[data-rl=choice][data-name='other.rete']").locator("[data-rl=about]").click();
  await page.locator("[data-rl=choice][data-name='other.rete']").locator("[data-rl=choice-info]", { hasText: "statements" }).waitFor({ timeout: 30000 });
  assert.match(await page.locator("[data-rl=choice][data-name='other.rete']").locator("[data-rl=choice-info]").textContent(), /3 statements/);
  assert.deepEqual(await page.evaluate(() => [window.__retelens.session.idx, window.__retelens.session.meta]), [-1, null], "still nothing opened");
  await shot(page, "chooser");
  // Open: that file, its first rows
  await page.locator("[data-rl=choice][data-name='other.rete']").locator("[data-rl=open-choice]").click();
  await until(page, () => window.__retelens.session.idx === 1 && !!window.__retelens.session.meta, 60000, "the chosen file");
  await page.waitForSelector("table td", { timeout: 60000 });
  assert.equal(await page.locator("[data-rl=chooser]").count(), 0);
  assert.match(await page.locator("body").textContent(), /other.rete/);
  await page.close();
});

test("Read this page's data: the reader, the graph, the .rete", async () => {
  const page = await ctx.newPage();
  await page.goto(`${base}/page.html`);
  const opened = ctx.waitForEvent("page", { timeout: 30000 });
  await sw.evaluate(async (url) => { const [t] = await chrome.tabs.query({ url }); await globalThis.retelensReadPage(t); }, `${base}/page.html`);
  const viewer = await opened;
  viewer.on("pageerror", (e) => errors.push(String(e)));
  await viewer.locator(".rl-r-title", { hasText: "Tomato soup" }).first().waitFor({ timeout: 60000 });
  const reader = await viewer.locator(".rl-reader").innerText();
  assert.match(reader, /Ingredients[\s\S]*4 tomatoes[\s\S]*1 onion[\s\S]*2 cups stock[\s\S]*salt/, "ingredients, in the page's order");
  assert.match(reader, /Chop the onion[\s\S]*Simmer[\s\S]*Blend/, "steps, in order");
  assert.match(reader, /Cook time\s*45 min/);
  assert.match(reader, /4\.5 ★ · 12 ratings/);
  assert.match(reader, /Soup festival/, "the microdata event");
  assert.equal(await viewer.locator(".rl-r-steps li").count(), 3);
  const report = await viewer.locator(".rl-r-report").innerText();
  assert.match(report, /JSON-LD block 1: 1 thing/);
  assert.match(report, /microdata: 1 thing/);
  assert.match(report, /meta tags/);
  await viewer.locator(".rl-tabs", { hasText: "Explore" }).waitFor();
  assert.ok(await viewer.locator('[data-rl="download-rete"]').count(), "a Download .rete link");
  await viewer.locator("[data-retelens-topbar]", { hasText: "statements" }).waitFor({ timeout: 30000 });
  await shot(viewer, "reader");
  // the graph built from the page answers the first starter query
  await viewer.locator('.rl-tab[data-tab="explore"]').click();
  await viewer.waitForSelector("table td", { timeout: 60000 });
  const cells = await viewer.locator("table td").allInnerTexts();
  assert.ok(cells.some((c) => /Recipe/.test(c)), `a Recipe row in ${JSON.stringify(cells.slice(0, 12))}`);
  assert.ok(cells.some((c) => /Tomato soup/.test(c)));
  await shot(viewer, "explore");
  assert.deepEqual(errors, []);
  await viewer.close();
  await page.close();
});
