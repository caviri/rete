import assert from "node:assert/strict";
import { launchBrowser } from "./_browser.mjs";

const PORT = process.env.PGPORT || "8090";
const FIX = "https://fixture.test";
const FIXTURE = `<${FIX}/row/1> <${FIX}/title> "First card" .
<${FIX}/row/1> <${FIX}/image> <${FIX}/media/one.jpg> .
<${FIX}/row/1> <${FIX}/pdf> <${FIX}/media/book.pdf> .
<${FIX}/row/1> <${FIX}/audio> <${FIX}/media/sound.mp3> .
<${FIX}/row/1> <${FIX}/video> <${FIX}/media/movie.mp4> .
<${FIX}/row/1> <${FIX}/spin> <${FIX}/item-spin/one.webm> .
<${FIX}/row/1> <${FIX}/model> <${FIX}/media/model.glb> .
<${FIX}/row/1> <${FIX}/iiif> <${FIX}/manifest.json> .
<${FIX}/row/1> <${FIX}/page> <${FIX}/page/one> .
<${FIX}/row/1> <${FIX}/markdown> "# Heading\\n\\n- one\\n- two\\n\\n**bold** [safe](https://example.test/) [bad](javascript:alert(1)) <script>window.__markdownPwned=1</script>"@en .
<${FIX}/row/2> <${FIX}/title> "Second card" .
<${FIX}/row/2> <${FIX}/image> <${FIX}/media/two.jpg> .
<${FIX}/row/3> <${FIX}/title> "Third card" .
<${FIX}/row/3> <${FIX}/image> <${FIX}/media/three.jpg> .`;

const TABLE_QUERY = `SELECT ?title ?image ?pdf ?audio ?video ?spin ?model ?iiif ?page ?markdown WHERE {
  ?row <${FIX}/title> ?title ; <${FIX}/image> ?image .
  OPTIONAL { ?row <${FIX}/pdf> ?pdf }
  OPTIONAL { ?row <${FIX}/audio> ?audio }
  OPTIONAL { ?row <${FIX}/video> ?video }
  OPTIONAL { ?row <${FIX}/spin> ?spin }
  OPTIONAL { ?row <${FIX}/model> ?model }
  OPTIONAL { ?row <${FIX}/iiif> ?iiif }
  OPTIONAL { ?row <${FIX}/page> ?page }
  OPTIONAL { ?row <${FIX}/markdown> ?markdown }
} ORDER BY ?title`;

const IMAGE = `<svg xmlns="http://www.w3.org/2000/svg" width="960" height="720" viewBox="0 0 960 720">
  <rect width="960" height="720" fill="#d9e5df"/><path d="M0 600L320 260l180 190 130-150 330 420" fill="#5b7f70"/>
</svg>`;

const PDF_STUB = `
export const GlobalWorkerOptions = {};
export function getDocument() {
  window.__pdfOpenCount = (window.__pdfOpenCount || 0) + 1;
  const doc = {
    numPages: 3,
    getPage(n) {
      return Promise.resolve({
        getViewport({ scale }) { return { width: 240 * scale, height: 320 * scale }; },
        render({ canvasContext }) {
          canvasContext.fillStyle = n % 2 ? "#d9e5df" : "#adc8bc";
          canvasContext.fillRect(0, 0, 20, 20);
          return { promise: Promise.resolve() };
        },
      });
    },
  };
  return { promise: Promise.resolve(doc) };
}`;

const MODEL_VIEWER_STUB = `
if (!customElements.get("model-viewer")) {
  customElements.define("model-viewer", class extends HTMLElement {
    getDimensions() { return { x: 1, y: 2, z: 3 }; }
  });
}`;

async function routeFixtures(page) {
  await page.route("https://api.github.com/repos/caviri/rete/pulls?*", (route) =>
    route.fulfill({ contentType: "application/json", body: "[]" }));
  await page.route("https://cdn.jsdelivr.net/npm/pdfjs-dist@4.7.76/build/pdf.min.mjs", (route) =>
    route.fulfill({ contentType: "application/javascript", body: PDF_STUB }));
  await page.route("https://cdn.jsdelivr.net/npm/@google/model-viewer@3.5.0/dist/model-viewer.min.js", (route) =>
    route.fulfill({ contentType: "application/javascript", body: MODEL_VIEWER_STUB }));
  await page.route(`${FIX}/**`, (route) => {
    const url = route.request().url();
    if (url.endsWith("/manifest.json")) {
      return route.fulfill({
        contentType: "application/json",
        body: JSON.stringify({
          id: `${FIX}/manifest.json`, type: "Manifest", label: { en: ["Fixture manifest"] },
          items: [{
            id: `${FIX}/canvas/1`, type: "Canvas", label: { en: ["Page 1"] },
            items: [{ items: [{ body: { id: `${FIX}/media/one.jpg`, type: "Image" } }] }],
          }],
        }),
      });
    }
    if (/\/media\/(one|two|three)\.jpg$/.test(url)) {
      return route.fulfill({ contentType: "image/svg+xml", body: IMAGE });
    }
    if (url.endsWith("/page/one")) {
      return route.fulfill({
        contentType: "text/html",
        body: "<!doctype html><title>Fixture page</title><h1>Embedded fixture</h1>",
      });
    }
    if (url.endsWith(".mp3")) return route.fulfill({ contentType: "audio/mpeg", body: "" });
    if (url.endsWith(".mp4")) return route.fulfill({ contentType: "video/mp4", body: "" });
    if (url.endsWith(".webm")) return route.fulfill({ contentType: "video/webm", body: "" });
    if (url.endsWith(".glb")) return route.fulfill({ contentType: "model/gltf-binary", body: "" });
    return route.fulfill({ status: 404, body: "fixture not found" });
  });
}

async function buildFixture(page) {
  await page.click("#buildBtn");
  await page.selectOption("#buildFormat", "nq");
  await page.evaluate((text) => window.PlaygroundEditor.setText("buildText", text), FIXTURE);
  await page.fill("#cardTitle", "Rich media fixture");
  await page.fill("#cardKey", "rich-media-fixture");
  await page.click("#buildRun");
  await page.waitForFunction(
    () => /Saved|Built/.test((document.getElementById("buildOut") || {}).textContent || ""),
    undefined,
    { timeout: 30000 },
  );
  await page.click("#buildOpen");
  await page.waitForFunction(
    () => /rich media fixture/i.test((document.getElementById("dsName") || {}).textContent || ""),
    undefined,
    { timeout: 15000 },
  );
}

async function runQuery(page, query, view = "table") {
  await page.selectOption("#fmt", view);
  await page.evaluate((q) => {
    const strategy = document.getElementById("strategy");
    if (strategy) {
      strategy.value = "whole";
      strategy.dispatchEvent(new Event("change"));
    }
    window.PlaygroundEditor.setText("q", q);
    document.getElementById("run").click();
  }, query);
  await page.waitForFunction(
    (kind) => kind === "cards"
      ? document.querySelectorAll("#out .cards .rcard").length === 3
      : document.querySelectorAll("#out table tbody tr").length === 3,
    view,
    { timeout: 30000 },
  );
}

async function setType(page, column, type) {
  await page.selectOption(`select.coltype[data-col="${column}"]`, type);
  await page.waitForFunction(
    ({ column, type }) => document.querySelector(`select.coltype[data-col="${column}"]`)?.value === type,
    { column, type },
  );
}

async function checkRenderers(page) {
  const forced = {
    image: "image", audio: "audio", video: "video", spin: "spin",
    model: "model3d", iiif: "iiif", page: "page", markdown: "markdown", pdf: "pdf",
  };
  for (const [column, type] of Object.entries(forced)) await setType(page, column, type);

  await page.locator(".page-preview-cell").first().scrollIntoViewIfNeeded();
  await page.waitForSelector(".page-preview-cell iframe", { timeout: 10000 });
  await page.waitForSelector(".pdfview-pg:text-is('1 / 3')", { timeout: 10000 });
  await page.waitForSelector(".iiif-cell.iiif-ready", { timeout: 10000 });

  const labels = await page.locator("#out .media-source").allTextContents();
  for (const label of [
    "Open image ↗", "Open PDF ↗", "Open audio ↗", "Open video ↗",
    "Open 3D ↗", "Open manifest ↗", "Open page ↗",
  ]) assert.ok(labels.includes(label), `missing media source label: ${label}`);

  const links = await page.locator("#out .media-source").evaluateAll((nodes) =>
    nodes.map((a) => ({ target: a.target, rel: a.rel, href: a.href })));
  assert.ok(links.every((x) => x.target === "_blank" && /noopener/.test(x.rel) && /noreferrer/.test(x.rel)));

  assert.equal(await page.locator(".markdown-body h1").first().textContent(), "Heading");
  assert.equal(await page.locator(".markdown-body li").count(), 2);
  assert.equal(await page.locator(".markdown-body strong").first().textContent(), "bold");
  assert.equal(await page.locator(".markdown-body script").count(), 0);
  assert.equal(await page.locator('.markdown-body a[href^="javascript:"]').count(), 0);
  assert.equal(await page.evaluate(() => window.__markdownPwned), undefined);
  const safe = page.locator('.markdown-body a[href="https://example.test/"]').first();
  assert.equal(await safe.getAttribute("target"), "_blank");
  assert.match(await safe.getAttribute("rel"), /noopener/);

  const sandbox = await page.locator(".page-preview-cell iframe").first().getAttribute("sandbox");
  assert.equal(sandbox, "allow-scripts");
  assert.equal(await page.locator(".page-preview-cell iframe").first().getAttribute("referrerpolicy"), "no-referrer");

  await page.locator(".pdfview-stage").first().click();
  await page.waitForSelector(".pdf-modal:not(.hidden)");
  assert.equal(await page.evaluate(() => window.__pdfOpenCount), 1);
  assert.match(await page.locator(".pdf-modal-page").textContent(), /1\s*\/\s*3/);
  await page.click(".pdf-modal-next");
  await page.waitForFunction(() => /2\s*\/\s*3/.test(document.querySelector(".pdf-modal-page")?.textContent || ""));
  await page.keyboard.press("Escape");
  await page.waitForSelector(".pdf-modal.hidden", { state: "attached" });
  assert.equal(await page.evaluate(() => window.__pdfOpenCount), 1);
}

async function checkCards(page) {
  await page.selectOption("#fmt", "cards");
  await page.waitForSelector("#out .cards .rcard");
  await page.waitForFunction(() => document.querySelectorAll("#out .cards .img-wrap.img-done").length === 3);

  const gridImage = page.locator("#out .cards .cell-thumb").first();
  await gridImage.hover();
  await page.waitForSelector(".thumb-zoom:not(.hidden)");
  const bounds = await page.evaluate(() => {
    const source = document.querySelector("#out .cards .cell-thumb").getBoundingClientRect();
    const zoom = document.querySelector(".thumb-zoom").getBoundingClientRect();
    return {
      source: { width: source.width, height: source.height },
      zoom: { left: zoom.left, top: zoom.top, right: zoom.right, bottom: zoom.bottom, width: zoom.width, height: zoom.height },
      viewport: { width: innerWidth, height: innerHeight },
    };
  });
  assert.ok(bounds.zoom.width > bounds.source.width || bounds.zoom.height > bounds.source.height);
  assert.ok(bounds.zoom.left >= 7 && bounds.zoom.top >= 7);
  assert.ok(bounds.zoom.right <= bounds.viewport.width - 7 && bounds.zoom.bottom <= bounds.viewport.height - 7);

  await page.locator("#out .cards .rcard").first().click({ position: { x: 24, y: 24 } });
  await page.waitForSelector("#cardFocusModal:not(.hidden)");
  const currentImage = page.locator(".cardfocus-slide.is-current .cell-thumb").first();
  await currentImage.hover();
  await page.waitForTimeout(350);
  assert.ok(await page.locator(".thumb-zoom").evaluate((el) => el.classList.contains("hidden")));

  const peeks = await page.evaluate(() => {
    const track = document.getElementById("cardFocusTrack").getBoundingClientRect();
    const slides = [...document.querySelectorAll(".cardfocus-slide")].map((el) => el.getBoundingClientRect());
    const visible = (r) => Math.max(0, Math.min(track.right, r.right) - Math.max(track.left, r.left));
    return { widths: slides.map(visible), trackWidth: track.width };
  });
  assert.ok(peeks.widths[0] > peeks.trackWidth * 0.55);
  assert.ok(peeks.widths[1] > 60, "next card should visibly peek into the desktop modal");

  const track = page.locator("#cardFocusTrack");
  await track.hover({ position: { x: 700, y: 50 } });
  await page.keyboard.down("Shift");
  await page.mouse.wheel(0, 700);
  await page.keyboard.up("Shift");
  await page.waitForFunction(() => /^2\s*\//.test(document.getElementById("cardFocusCount")?.textContent || ""));

  const box = await page.locator(".cardfocus-slide.is-current").boundingBox();
  assert.ok(box);
  const dragY = box.y + box.height - 14; // blank card area, not its image/link
  await page.mouse.move(box.x + box.width * 0.7, dragY);
  await page.mouse.down();
  await page.mouse.move(box.x + box.width * 0.1, dragY, { steps: 8 });
  await page.mouse.up();
  await page.waitForFunction(() => /^3\s*\//.test(document.getElementById("cardFocusCount")?.textContent || ""));
}

const browser = await launchBrowser();
const page = await browser.newPage({ viewport: { width: 1440, height: 1000 } });
const errors = [];
let failure = "";
page.on("pageerror", (error) => errors.push(String(error).slice(0, 240)));
page.on("console", (message) => {
  if (message.type() === "error") errors.push("console: " + message.text().slice(0, 220));
});

try {
  await routeFixtures(page);
  await page.goto(`http://localhost:${PORT}/playground.html#dataset=scholar&mode=sparql`, {
    waitUntil: "domcontentloaded",
  });
  await page.waitForFunction(() => window.PlaygroundEditor && document.getElementById("buildBtn"), undefined, { timeout: 60000 });
  await buildFixture(page);
  await runQuery(page, TABLE_QUERY);
  await checkRenderers(page);
  await checkCards(page);
  assert.deepEqual(errors, []);
} catch (error) {
  failure = error && error.stack ? error.stack : String(error);
}

console.log(JSON.stringify({
  verdict: failure ? "FAIL" : "PASS",
  failure: failure.slice(0, 1200),
  errors: errors.slice(0, 6),
}, null, 2));
await browser.close();
process.exit(failure ? 1 : 0);
