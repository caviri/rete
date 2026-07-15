// GeoSPARQL -> Tiles output with a deterministic local PMTiles fixture. Leaflet
// and the tile renderer are stubbed at their public boundary so the release gate
// never talks to a public tile server, while still proving source selection,
// layer wiring, result bounds, and non-empty canvas rendering.
import { createServer } from "node:http";
import { launchBrowser } from "./_browser.mjs";

const listen = (server) => new Promise((resolve) => server.listen(0, "127.0.0.1", () => resolve(server.address().port)));

const QUERY = `PREFIX geo: <http://www.opengis.net/ont/geosparql#>
PREFIX geof: <http://www.opengis.net/def/function/geosparql/>
PREFIX ex: <http://ex/>
PREFIX rdfs: <http://www.w3.org/2000/01/rdf-schema#>
SELECT ?territory ?w WHERE {
  ?t ex:year 1914 ; rdfs:label ?territory ; geo:hasGeometry/geo:asWKT ?w .
  FILTER(geof:sfContains(?w, "POINT(2.35 48.85)"^^geo:wktLiteral))
}`;

const main = async () => {
  const fixture = Buffer.from("PMTiles release-gate fixture v1");
  let pmRequests = 0;
  const server = createServer((req, res) => {
    if (req.url?.split("?")[0] !== "/fixture.pmtiles") { res.writeHead(404); res.end("not found"); return; }
    pmRequests++;
    const range = req.headers.range && /bytes=(\d+)-(\d*)/.exec(req.headers.range);
    const start = range ? Number(range[1]) : 0;
    const end = range && range[2] ? Math.min(Number(range[2]), fixture.length - 1) : fixture.length - 1;
    const body = fixture.subarray(start, end + 1);
    res.writeHead(range ? 206 : 200, {
      "Content-Type": "application/octet-stream", "Content-Length": body.length,
      "Accept-Ranges": "bytes", "Content-Range": `bytes ${start}-${end}/${fixture.length}`,
      "Access-Control-Allow-Origin": "*", "Access-Control-Expose-Headers": "Content-Range,Content-Length,Accept-Ranges",
    });
    res.end(body);
  });
  const fixturePort = await listen(server);
  const fixtureUrl = `http://127.0.0.1:${fixturePort}/fixture.pmtiles`;

  const browser = await launchBrowser();
  const page = await browser.newPage();
  const errs = [];
  page.on("pageerror", (e) => errs.push(String(e).slice(0, 240)));
  page.on("console", (m) => { if (m.type() === "error") errs.push("console: " + m.text().slice(0, 200)); });
  await page.addInitScript((pmUrl) => {
    Object.defineProperty(window, "RETE_PLAYGROUND_CATALOG", {
      configurable: true,
      set(value) {
        value.pmtiles = value.pmtiles || {};
        value.pmtiles.history = { url: pmUrl, label: "local gate fixture", size: "28 B", layers: { countries: "shapeName" } };
        Object.defineProperty(window, "RETE_PLAYGROUND_CATALOG", { configurable: true, writable: true, value });
      },
    });
    window.__gateTileMap = null;
    class GateMap {
      constructor(el) { this.el = el; this.layers = []; this.bounds = null; window.__gateTileMap = this; }
      setView(center, zoom) { this.center = center; this.zoom = zoom; return this; }
      fitBounds(bounds) { this.bounds = bounds; return this; }
      invalidateSize() { return this; }
      remove() { this.el.innerHTML = ""; }
    }
    window.L = { map: (el) => new GateMap(el) };
    class Symbolizer { constructor(options) { this.options = options; } }
    window.protomapsL = {
      PolygonSymbolizer: Symbolizer,
      CircleSymbolizer: Symbolizer,
      leafletLayer(options) {
        return { addTo(map) {
          map.layers.push(this);
          fetch(options.url, { headers: { Range: "bytes=0-15" } }).then((r) => r.arrayBuffer()).then(() => {
            const canvas = document.createElement("canvas"); canvas.width = 32; canvas.height = 32;
            const ctx = canvas.getContext("2d"); ctx.fillStyle = "#147d69"; ctx.fillRect(0, 0, 32, 32);
            map.el.appendChild(canvas);
          });
          return this;
        } };
      },
    };
  }, fixtureUrl);
  await page.route("https://unpkg.com/leaflet@1.9.4/dist/leaflet.css", (route) => route.fulfill({ status: 200, contentType: "text/css", body: "" }));
  await page.route("https://unpkg.com/leaflet@1.9.4/dist/leaflet.js", (route) => route.fulfill({ status: 200, contentType: "text/javascript", body: "/* local gate stub; window.L installed by init script */" }));

  const PORT = process.env.PGPORT || "8090";
  await page.goto(`http://localhost:${PORT}/playground.html#dataset=history&mode=sparql`, { waitUntil: "domcontentloaded" });
  await page.waitForFunction(() => window.PlaygroundEditor && document.getElementById("run"), { timeout: 60000 });
  await page.waitForTimeout(2000);
  await page.evaluate((q) => {
    const strategy = document.getElementById("strategy");
    if (strategy) { strategy.value = "whole"; strategy.dispatchEvent(new Event("change")); }
    window.PlaygroundEditor.setText("q", q);
    document.getElementById("run").click();
  }, QUERY);
  await page.waitForFunction(() => {
    const meta = (document.getElementById("qmeta") || {}).textContent || "";
    return (/mapped feature|row/.test(meta) && !/running/i.test(meta)) || document.querySelector("#out .error-box");
  }, { timeout: 30000 });
  await page.selectOption("#fmt", "tiles");
  await page.waitForFunction(() => document.querySelector("#tilesMap canvas") || document.querySelector("#out .note"), { timeout: 15000 });
  const result = await page.evaluate(() => {
    const canvas = document.querySelector("#tilesMap canvas");
    let nonEmpty = false;
    if (canvas) nonEmpty = [...canvas.getContext("2d").getImageData(0, 0, canvas.width, canvas.height).data].some((v) => v !== 0);
    const map = window.__gateTileMap;
    const b = map && map.bounds;
    const containsParis = !!b && b[0][1] <= 2.35 && b[1][1] >= 2.35 && b[0][0] <= 48.85 && b[1][0] >= 48.85;
    return { nonEmpty, layers: map ? map.layers.length : 0, bounds: b, containsParis, cap: (document.getElementById("tilesCap") || {}).textContent || "", error: !!document.querySelector("#out .error-box") };
  });
  const pass = result.nonEmpty && result.layers === 1 && result.containsParis && pmRequests > 0 && !result.error && errs.length === 0;
  console.log(JSON.stringify({ verdict: pass ? "PASS" : "FAIL", fixtureUrl, pmRequests, ...result, errs: errs.slice(0, 4) }, null, 2));
  await browser.close();
  await new Promise((resolve) => server.close(resolve));
  process.exit(pass ? 0 : 1);
};
main();
