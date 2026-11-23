// Browser check of site/query-like-a-child: drag and drop with a real pointer,
// the generic sketch, and a live file (Linked Jazz over HTTP range requests).
//
//   bash tests/query-like-a-child/run.sh
//
// Serves the repository the way scripts/stage_pages.sh lays it out (site/ at
// the root, docs/ under /docs/), then drives headless Chromium.
const http = require("http");
const fs = require("fs");
const path = require("path");
const { chromium } = require("playwright-core");

const ROOT = path.resolve(__dirname, "../..");
const SHOTS = process.env.SHOTS || path.join(__dirname, "shots");
const TYPES = { ".html": "text/html", ".js": "text/javascript", ".mjs": "text/javascript", ".css": "text/css", ".wasm": "application/wasm", ".json": "application/json", ".svg": "image/svg+xml" };

const server = http.createServer((req, res) => {
  let p = decodeURIComponent(req.url.split("?")[0]);
  if (p.endsWith("/")) p += "index.html";
  const file = p.startsWith("/docs/") ? path.join(ROOT, p) : path.join(ROOT, "site", p);
  if (!file.startsWith(ROOT) || !fs.existsSync(file) || fs.statSync(file).isDirectory()) { res.writeHead(404); res.end("not found"); return; }
  res.writeHead(200, { "content-type": TYPES[path.extname(file)] || "application/octet-stream" });
  fs.createReadStream(file).pipe(res);
});

let failed = 0;
let lastQuery = "";
const ok = (cond, what) => { console.log((cond ? "ok   " : "FAIL ") + what); if (!cond) { failed++; console.log(lastQuery.replace(/^/gm, "     | ")); } };

async function dragTo(page, from, to, opts = {}) {
  await to.scrollIntoViewIfNeeded();
  await from.scrollIntoViewIfNeeded();
  const a = await from.boundingBox(), b = await to.boundingBox();
  const sx = a.x + Math.min(12, a.width / 2), sy = a.y + a.height / 2;
  const tx = b.x + (opts.dx ?? b.width / 2), ty = b.y + (opts.dy ?? b.height / 2);
  await page.mouse.move(sx, sy);
  await page.mouse.down();
  await page.mouse.move(sx + 8, sy + 8, { steps: 2 });
  await page.mouse.move(tx, ty, { steps: 8 });
  if (opts.shot) await page.screenshot({ path: path.join(SHOTS, opts.shot) });
  await page.mouse.up();
}

(async () => {
  fs.mkdirSync(SHOTS, { recursive: true });
  await new Promise((r) => server.listen(0, "127.0.0.1", r));
  const base = `http://127.0.0.1:${server.address().port}/query-like-a-child/`;
  const browser = await chromium.launch({ executablePath: process.env.CHROMIUM || undefined });
  const page = await browser.newPage({ viewport: { width: 1440, height: 900 } });
  const errors = [];
  page.on("pageerror", (e) => errors.push(String(e)));
  page.on("console", (m) => { if (m.type() === "error" && !/Failed to load resource/.test(m.text())) errors.push(m.text()); });
  // A missing file of ours is an error; a dead image link in the data is not.
  page.on("response", (r) => { if (r.status() >= 400 && r.url().startsWith("http://127.0.0.1")) errors.push(`${r.status()} ${r.url()}`); });
  const settled = () => page.waitForFunction(() => !window.__sketch.busy(), null, { timeout: 60000 });
  const query = async () => (lastQuery = await page.evaluate(() => window.__sketch.query()));

  // ── without a file ──
  await page.goto(base);
  await page.waitForSelector(".pal");
  ok((await query()).includes("?thing ?property ?value ."), "empty sketch is the match-everything query");
  ok(await page.locator("#run").isDisabled(), "Run is off without a file");
  await page.screenshot({ path: path.join(SHOTS, "01-empty.png") });

  await dragTo(page, page.locator(".pal", { hasText: "is a kind of thing" }), page.locator("#canvas"), { shot: "02-dragging.png" });
  ok(await page.locator("#canvas .block").count() === 1, "a palette block dragged into the sketch");
  ok((await query()).includes("?thing a ?kind ."), "open slot stands for a variable");
  ok(await page.locator("#canvas .slot.sh-class").count() === 1 && await page.locator("#canvas .slot.sh-thing").count() === 1 && await page.locator(".item.k-class").count() > 0, "slots and pieces carry shapes");
  {
    const item = page.locator(".item.k-class", { hasText: "Person" }).first();
    await item.scrollIntoViewIfNeeded();
    const a = await item.boundingBox();
    await page.mouse.move(a.x + 12, a.y + a.height / 2);
    await page.mouse.down();
    await page.mouse.move(a.x + 60, a.y - 40, { steps: 4 });
    ok(await page.locator("#canvas .slot.can").count() === 1 && await page.locator('#canvas .slot.can[data-role="class"]').count() === 1, "while a kind is dragged, only the kind slot lights up");
    await page.screenshot({ path: path.join(SHOTS, "02b-matching.png") });
    await page.mouse.move(5, 5);
    await page.mouse.up();
  }

  await dragTo(page, page.locator(".item.k-class", { hasText: "Person" }).first(), page.locator('.slot[data-role="class"]'));
  ok((await query()).includes("?thing a foaf:Person ."), "a kind dropped on the kind slot");

  await dragTo(page, page.locator(".pal", { hasText: "has a name" }), page.locator("#canvas .block"), { dy: 60 });
  ok(/\?thing a foaf:Person \.\n\s+\?thing rdfs:label\|/.test(await query()), "second block inserted after the first");

  await dragTo(page, page.locator(".pal", { hasText: "if there is" }), page.locator("#canvas .block").last(), { dy: 60 });
  await dragTo(page, page.locator("#canvas > .zone > .block .handle").nth(1), page.locator(".kids .zone"));
  ok(/OPTIONAL \{\n\s+\?thing rdfs:label/.test(await query()), "a block moved into a group");

  await dragTo(page, page.locator("#vars .var", { hasText: "?name" }), page.locator('.dropvars[data-area="show"]'));
  ok((await query()).includes("SELECT ?name\n"), "a variable dragged into the answer");

  await dragTo(page, page.locator(".item.k-prop", { hasText: "creator" }), page.locator("#canvas > .zone"), { dy: 4 });
  ok(/WHERE \{\n\s+\?thing dcterms:creator \?creator \./.test(await query()), "a property dropped at the top makes a link block");

  await dragTo(page, page.locator('.slot[data-f="o"]', { hasText: "?creator" }), page.locator(".kids .slot[data-f=s]"));
  ok(/OPTIONAL \{\n\s+\?creator rdfs:label/.test(await query()), "a variable dragged from slot to slot connects them");

  await dragTo(page, page.locator("#canvas > .zone > .block .handle").first(), page.locator("#palette"));
  ok(!(await query()).includes("dcterms:creator"), "a block dragged back to the rail is removed");
  await page.click("#undo");
  ok((await query()).includes("dcterms:creator"), "undo brings it back");

  await page.locator('.slot[data-f="o"]', { hasText: "?creator" }).click();
  await page.locator("#pop input[placeholder='a value']").fill("1984");
  await page.locator("#pop .pop-row", { has: page.locator("input[placeholder='a value']") }).locator("button").click();
  ok(/dcterms:creator "1984-01-01"\^\^xsd:date/.test(await query()), "the picker sets a typed value");
  await page.screenshot({ path: path.join(SHOTS, "03-generic.png"), fullPage: true });

  const link = await page.evaluate(() => location.href);
  await page.goto("about:blank");
  await page.goto(link);
  await page.waitForSelector("#canvas .block");
  ok((await query()).includes("dcterms:creator"), "the link restores the sketch");

  // ── with a file ──
  await page.click("#clear");
  await page.locator(".sample", { hasText: "Linked Jazz" }).click();
  await page.waitForSelector(".data-name", { timeout: 60000 });
  ok(await page.locator(".item.k-class").count() >= 1 && await page.locator(".item.k-prop").count() >= 10, "the shelf lists the file's kinds and properties");
  await page.locator(".recipe", { hasText: "List things of a kind" }).click();
  await page.waitForSelector("#out tbody tr", { timeout: 60000 });
  ok((await query()).includes("?thing foaf:name ?name ."), "the recipe uses the file's own label property");
  ok(await page.locator("#out tbody tr").count() === 50, "live result: 50 rows");
  await page.screenshot({ path: path.join(SHOTS, "04-file.png") });

  await page.locator('#canvas .slot[data-role="var"]', { hasText: "?name" }).first().click();
  await page.screenshot({ path: path.join(SHOTS, "05-picker.png") });
  await page.keyboard.press("Escape");

  await page.locator(".pal", { hasText: "is linked to" }).click();
  await page.locator('#canvas .slot[data-role="prop"]').click();
  ok(await page.locator("#pop .opt.fits").count() >= 1, "properties that fit the kind come first");
  await page.locator("#pop .opt", { hasText: "knows of" }).first().click();
  await page.locator('#canvas .slot[data-role="any"]').click();
  await page.locator("#pop .opt", { hasText: "Find a thing by name" }).click();
  await page.waitForSelector("#finder h4:has-text('A sample of what “knows of” points to')", { timeout: 60000 });
  ok(await page.locator("#finder .opt").count() >= 10, "the finder opens with a quick sample that fits the slot");
  await page.screenshot({ path: path.join(SHOTS, "06a-sample.png") });
  await page.locator("#finder input").fill("Mary Lou");
  await page.waitForSelector("#finder .opt .sub", { timeout: 60000 });
  await page.screenshot({ path: path.join(SHOTS, "06-finder.png") });
  await page.locator("#finder .opt", { hasText: "Mary Lou Williams" }).first().click();
  ok((await query()).includes("rel:knowsOf dbr:Mary_Lou_Williams ."), "the finder fills a slot with a thing from the file");
  await settled();
  const n = await page.locator("#out tbody tr").count();
  ok(n > 0 && n < 50, `people who know of Mary Lou Williams: ${n} rows`);

  await page.locator(".seg button", { hasText: "Count" }).click();
  await settled();
  ok((await query()).includes("SELECT (COUNT(*) AS ?count)") && await page.locator("#out tbody tr").count() === 1, "count mode");
  await page.locator(".seg button", { hasText: "List" }).click();
  await settled();

  await dragTo(page, page.locator("#out .cell-iri").first(), page.locator("#canvas > .zone"), { dy: 4 });
  ok(/VALUES \?thing \{ \S+ \}/.test(await query()), "a result dragged into the sketch pins it");
  await settled();
  ok(await page.locator("#out tbody tr").count() <= 1, "and the result follows");
  await page.screenshot({ path: path.join(SHOTS, "07-pinned.png") });

  await page.locator(".linkish", { hasText: "Edit as text" }).click();
  await page.locator("#query textarea").fill("ASK { ?s ?p ?o }");
  await page.waitForSelector("#out .ask", { timeout: 30000 });
  ok((await page.locator("#out .ask").textContent()) === "Yes", "a query written by hand runs too");

  // ── a phone ──
  await page.setViewportSize({ width: 390, height: 800 });
  await page.locator(".linkish", { hasText: "Back to the blocks" }).click();
  await settled();
  const wide = await page.evaluate(() => document.documentElement.scrollWidth - document.documentElement.clientWidth);
  ok(wide <= 1, `no sideways scroll at phone width (${wide}px over)`);
  await page.screenshot({ path: path.join(SHOTS, "08-phone.png"), fullPage: true });

  ok(errors.length === 0, "no page errors" + (errors.length ? ": " + errors.join(" | ") : ""));
  await browser.close();
  server.close();
  console.log(failed ? `\n${failed} FAILED` : "\nall passed");
  process.exit(failed ? 1 : 0);
})().catch((e) => { console.error(e); process.exit(2); });
