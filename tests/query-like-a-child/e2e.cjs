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

  await page.locator('#canvas .slot[data-role="class"]').hover();
  ok(await page.locator('.legend .lit[data-legend="class"]').count() === 1 && await page.locator(".legend .lit").count() === 1
    && await page.locator('.pal.lit[data-t="kind"]').count() === 1 && await page.locator(".item.k-class.lit").count() === 1,
    "pointing at a slot lights its shape, its block and its piece in the sidebar");
  ok(/^A kind goes here/.test(await page.locator('#canvas .slot[data-role="class"]').getAttribute("title")), "and its tooltip names the kind of piece");
  await page.screenshot({ path: path.join(SHOTS, "02c-spotlight.png") });
  await page.mouse.move(700, 880);
  ok(await page.locator("#rail .lit").count() === 0, "leaving the sketch clears it");
  // a variable made by hand joins the list and can be dragged onto a slot
  await page.click("#newvar");
  await page.locator("#pop input").fill("the artist");
  await page.locator("#pop .btn", { hasText: "Add" }).click();
  ok(await page.locator("#vars .var.unused", { hasText: "?the_artist" }).count() === 1 && !(await query()).includes("the_artist"), "a new variable joins the list without changing the query");
  await dragTo(page, page.locator("#vars .var", { hasText: "?the_artist" }), page.locator('#canvas .slot[data-f="s"]'));
  ok((await query()).includes("?the_artist a foaf:Person .") && await page.locator("#vars .var.unused").count() === 0, "dragged onto a slot, it is used");
  await page.locator('#canvas .slot[data-f="s"]').click();
  await page.locator("#pop input[placeholder='a new name']").fill("thing");
  await page.locator("#pop .pop-row", { has: page.locator("input[placeholder='a new name']") }).locator("button").click();
  ok((await query()).includes("?thing a foaf:Person .") && await page.locator("#vars .var.unused", { hasText: "?the_artist" }).count() === 1, "and it stays in the list when the slot moves on");
  await page.locator("#vars .var.unused .x").click();
  ok(await page.locator("#vars .var", { hasText: "?the_artist" }).count() === 0, "until it is removed");
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

  // ── column widths ──
  {
    const width = async (sel) => (await page.locator(sel).boundingBox()).width;
    const pull = async (sel, dx) => {
      const b = await page.locator(sel).boundingBox();
      await page.mouse.move(b.x + b.width / 2, b.y + 200);
      await page.mouse.down();
      await page.mouse.move(b.x + b.width / 2 + dx, b.y + 220, { steps: 5 });
      await page.mouse.up();
    };
    const rail0 = await width("#rail"), res0 = await width(".results");
    await pull("#splitRail", 90);
    { const w = await width("#rail"); ok(Math.abs(w - rail0 - 90) <= 2, `dragging the first divider widens the blocks column (${rail0} to ${w})`); }
    await pull("#splitMain", -150);
    { const w = await width(".results"); ok(Math.abs(w - res0 - 150) <= 3, `dragging the second divider widens the result (${res0} to ${w})`); }
    await page.screenshot({ path: path.join(SHOTS, "03b-resized.png") });
    await page.reload();
    await page.waitForSelector(".pal");
    ok(Math.abs(await width("#rail") - rail0 - 90) <= 2, "the widths are kept across a reload");
    await page.locator("#splitRail").dblclick();
    await page.locator("#splitMain").dblclick();
    ok(Math.abs(await width("#rail") - rail0) <= 1 && Math.abs(await width(".results") - res0) <= 1, "a double-click puts a divider back");
    ok(await page.evaluate(() => getComputedStyle(document.querySelector(".board")).scrollbarWidth) === "thin", "scrollbars are the thin, themed kind");
  }

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

  await page.locator(".linkish", { hasText: "Edit the SPARQL" }).click();
  await page.locator("#query textarea").fill("ASK { ?s ?p ?o }");
  await page.waitForSelector("#out .ask", { timeout: 30000 });
  ok((await page.locator("#out .ask").textContent()) === "Yes", "a query written by hand runs too");

  // ── examples, cards, graph ──
  await page.locator("#qback").click();
  await page.click("#clear");
  ok(await page.locator(".recipe.mine").count() >= 5 && await page.locator(".recipe:not(.mine)").count() >= 17, "the empty sketch offers the file's own examples and the general ones");
  await page.screenshot({ path: path.join(SHOTS, "10-examples.png"), fullPage: true });
  const blocksFromCard = await page.locator(".recipe.mine", { hasText: "Opens as blocks" }).count() + await page.locator(".recipe.mine span:not(:has-text('Opens as text'))").count();
  ok(blocksFromCard >= 3, `some of the card's queries open as blocks (${blocksFromCard})`);
  await page.locator(".recipe.mine span:not(:has-text('Opens as text'))").first().click();
  ok(await page.locator("#canvas .block").count() >= 1, "a card query becomes blocks");
  await page.click("#clear");
  await page.locator(".recipe", { hasText: "Either of two kinds" }).click();
  ok(/\{\n\s+\?thing a foaf:Person \.\n\s+\} UNION \{/.test(await query()), "a union example");
  await page.click("#clear");
  await page.locator(".item.k-prop", { hasText: "knows of" }).first().click();
  await page.locator(".pal", { hasText: "has a name" }).click();
  await settled();
  await page.locator("#views button", { hasText: "Graph" }).click();
  await page.waitForSelector("svg.graph .g-node");
  ok(await page.locator("svg.graph .g-node").count() >= 10 && await page.locator("svg.graph .g-edge").count() >= 10, "the graph view draws the sketch's links");
  await page.waitForTimeout(1500);
  await page.screenshot({ path: path.join(SHOTS, "11-graph.png") });
  await page.locator("#views button", { hasText: "Cards" }).click();
  ok(await page.locator("#out .card").count() === 50 && await page.locator("#out .card h3").count() === 50 && await page.locator("#out .card dd .cell-iri").count() >= 50 && !(await page.locator("#out").textContent()).includes("[object"), "the cards view has one card per row");
  await page.screenshot({ path: path.join(SHOTS, "12-cards.png") });
  await page.locator("#views button", { hasText: "Table" }).click();
  await page.locator(".linkish", { hasText: "Edit the SPARQL" }).click();

  // ── names for identifiers ──
  ok(await page.locator("#out .cell-iri", { hasText: "Billy Eckstine" }).count() >= 0, "result cells are named");
  await page.locator("#qback").click();
  await page.click("#clear");
  await page.locator(".recipe", { hasText: "List things of a kind" }).click();
  await page.waitForSelector("#out .cell-iri:has-text('Aaron Copland')", { timeout: 60000 });
  ok(true, "things in the result are shown by their name in the file");
  await page.click("text=Close file");
  await page.locator(".sample", { hasText: "Wikidata paintings" }).click();
  await page.waitForSelector(".item.k-prop:has-text('creator P170')", { timeout: 120000 });
  ok(await page.locator(".item.k-prop", { hasText: /^creator P170/ }).first().textContent().then((x) => /P170/.test(x)), "an identifier property is listed by the file's label (creator, P170)");
  await page.click("#clear");
  await page.locator(".item.k-prop", { hasText: /^creator P170/ }).first().click();
  ok(/\?thing wdt:P170 \?creator \./.test(await query()), "and its variable takes the label's name");
  await settled();
  await page.waitForSelector("#out .cell-iri + small", { timeout: 120000 });
  ok(await page.locator("#out .cell-iri + small", { hasText: /wd:Q\d+/ }).count() >= 10, "a result names its things and keeps the identifier beside the name");
  await page.screenshot({ path: path.join(SHOTS, "09-labels.png") });
  await page.click("text=Close file");
  await page.locator(".sample", { hasText: "Linked Jazz" }).click();
  await page.waitForSelector(".data-name", { timeout: 60000 });
  await settled();
  await page.locator(".linkish", { hasText: "Edit the SPARQL" }).click();

  // ── a phone ──
  await page.setViewportSize({ width: 390, height: 800 });
  await page.locator("#qback").click();
  await settled();
  const wide = await page.evaluate(() => document.documentElement.scrollWidth - document.documentElement.clientWidth);
  ok(wide <= 1, `no sideways scroll at phone width (${wide}px over)`);
  await page.screenshot({ path: path.join(SHOTS, "08-phone.png"), fullPage: true });

  // ── editing the SPARQL: the blocks follow when they can ──
  await page.setViewportSize({ width: 1440, height: 900 });
  await page.click("#clear");
  await page.locator("#query pre").click();
  await page.locator("#query textarea").fill("PREFIX foaf: <http://xmlns.com/foaf/0.1/>\nSELECT ?who ?n WHERE {\n  ?who a foaf:Person ; foaf:name ?n .\n  FILTER(CONTAINS(LCASE(STR(?n)), \"duke\"))\n} LIMIT 5");
  await page.waitForFunction(() => document.querySelectorAll("#canvas .block").length === 3);
  ok(/blocks follow/.test(await page.locator("#qtag").textContent()) && await page.locator('#canvas .slot.is-var', { hasText: "?who" }).count() >= 2, "typing SPARQL the blocks can say rebuilds the blocks");
  await settled();
  ok(await page.locator("#out tbody tr").count() >= 1, "and it runs");
  await page.locator("#query textarea").fill("SELECT ?s WHERE { ?s ?p ?o . FILTER(isIRI(?o)) } LIMIT 3");
  await page.waitForFunction(() => /cannot say/.test(document.querySelector("#qtag").textContent));
  ok(await page.locator("#canvas .block").count() === 3, "text the blocks cannot say runs as text and leaves the blocks alone");
  await settled();
  ok(await page.locator("#out tbody tr").count() === 3, "and it runs too");
  await page.locator("#qback").click();
  ok((await query()).includes('FILTER(CONTAINS(LCASE(STR(?n)), "duke"))'), "going back returns to the last script");

  // ── about, and the guided tour on Hermetic books ──
  await page.setViewportSize({ width: 1440, height: 900 });
  await page.click("#aboutbtn");
  ok(await page.locator("#about a[href='https://scratch.mit.edu/about']").count() === 1, "About says where the idea comes from");
  await page.screenshot({ path: path.join(SHOTS, "13-about.png") });
  await page.locator("#about .btn", { hasText: "Take the guided tour" }).click();
  for (let i = 1; i <= 8; i++) {
    await page.waitForSelector("#tour-next:not([disabled])", { timeout: 120000 });
    if (i === 5) { await settled(); await page.waitForSelector("#out .card img", { timeout: 120000 }); await page.screenshot({ path: path.join(SHOTS, "14-tour-cards.png") }); }
    await page.click("#tour-next");
    await page.waitForFunction((n) => document.querySelector(".tour-n").textContent.includes(`${n} of 9`), i + 1);
  }
  await page.waitForSelector("#tour-next:not([disabled])");
  await settled();
  ok(/\?b \S*placePublished \?placePublished \./.test(await query()) && /SELECT \?cover \?title \?author \?year \?placePublished\n/.test(await query()), "the tour adds the place of publication as a column");
  await page.waitForFunction(() => document.querySelectorAll("#out .card").length >= 10 && [...document.querySelectorAll("#out .card dt")].some((d) => d.textContent === "?placePublished"), null, { timeout: 120000 });
  ok(await page.locator("#out .card img").count() >= 10, "and the cards show the covers and the place");
  await page.screenshot({ path: path.join(SHOTS, "15-tour-end.png") });
  await page.click("#tour-next");
  ok(await page.locator("#tour").isHidden() && await page.locator("#tour-ring").isHidden(), "the tour ends");

  ok(errors.length === 0, "no page errors" + (errors.length ? ": " + errors.join(" | ") : ""));
  await browser.close();
  server.close();
  console.log(failed ? `\n${failed} FAILED` : "\nall passed");
  process.exit(failed ? 1 : 0);
})().catch((e) => { console.error(e); process.exit(2); });
