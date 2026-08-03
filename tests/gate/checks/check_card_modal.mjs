// The Dataset Card button must open the card that travels inside the .rete,
// in both the rendered and the JSON view — and over BOTH read paths, which are
// different code: a resident graph answers from memory, a remote one routes
// through the worker because card_url does synchronous range XHR.
//
// The remote path is served from a local range server carrying a REAL card, so
// the check also pins the property that makes the card useful: reading it costs
// a couple of small ranged reads, not the whole file.
import { createServer } from "node:http";
import { readFile } from "node:fs/promises";
import { launchBrowser } from "./_browser.mjs";

const listen = (server) => new Promise((resolve) => server.listen(0, "127.0.0.1", () => resolve(server.address().port)));

const main = async () => {
  // Built by the gate's own setup step, and built WITH a card (see run.mjs).
  const fixture = await readFile("/work/tests/gate/.cache/card-fixture.rete");
  const traffic = { full: 0, head: 0, range: 0, bytes: 0 };
  const server = createServer((req, res) => {
    if (req.url?.split("?")[0] !== "/carded.rete") { res.writeHead(404); res.end("nope"); return; }
    const common = {
      "Access-Control-Allow-Origin": "*",
      "Access-Control-Expose-Headers": "Content-Range,Content-Length,Accept-Ranges",
      "Accept-Ranges": "bytes",
    };
    const range = req.headers.range && /bytes=(\d+)-(\d*)/.exec(req.headers.range);
    if (range) {
      traffic.range++;
      const start = Number(range[1]);
      const end = range[2] ? Math.min(Number(range[2]), fixture.length - 1) : fixture.length - 1;
      const body = fixture.subarray(start, end + 1);
      traffic.bytes += body.length;
      res.writeHead(206, { ...common, "Content-Type": "application/octet-stream", "Content-Range": `bytes ${start}-${end}/${fixture.length}`, "Content-Length": body.length });
      res.end(body);
      return;
    }
    // A HEAD carries no body — the reader uses one to learn Content-Length
    // before it can ask for a range. Counting it as a "download" would be a
    // measurement bug, so it is tracked separately.
    if ((req.method || "GET").toUpperCase() === "HEAD") {
      traffic.head++;
      res.writeHead(200, { ...common, "Content-Type": "application/octet-stream", "Content-Length": fixture.length });
      res.end();
      return;
    }
    traffic.full++;
    res.writeHead(200, { ...common, "Content-Type": "application/octet-stream", "Content-Length": fixture.length });
    res.end(fixture);
  });
  const port = await listen(server);

  const PGPORT = process.env.PGPORT || "8090";
  const browser = await launchBrowser();
  const failures = [];
  const pageErrors = [];

  const open = async (hash) => {
    const page = await browser.newPage();
    page.on("pageerror", (e) => pageErrors.push(String(e).slice(0, 200)));
    await page.goto(`http://localhost:${PGPORT}/playground.html${hash}`, { waitUntil: "domcontentloaded" });
    await page.waitForFunction(
      () => document.getElementById("run") && !document.getElementById("run").disabled,
      { timeout: 90000 },
    );
    return page;
  };

  const openCard = async (page) => {
    await page.click("#cardBtn");
    await page.waitForFunction(
      () => !document.getElementById("cardModal").classList.contains("hidden") &&
            !/Reading the card/.test(document.getElementById("cardBody").textContent),
      { timeout: 60000 },
    );
  };

  // ---- remote path: card_url through the worker -----------------------------
  const remote = await open(`#url=${encodeURIComponent(`http://127.0.0.1:${port}/carded.rete`)}`);
  // Measure the CARD READ, not the session: opening the graph is separate
  // traffic, and charging it to the card would be an attribution error.
  const before = { ...traffic };
  await openCard(remote);
  const cardRead = {
    full: traffic.full - before.full,
    head: traffic.head - before.head,
    range: traffic.range - before.range,
    bytes: traffic.bytes - before.bytes,
  };

  const rendered = await remote.evaluate(() => ({
    title: document.getElementById("cardModalTitle").textContent,
    body: document.getElementById("cardBody").textContent.slice(0, 4000),
    foot: document.getElementById("cardFootNote").textContent,
    stats: [...document.querySelectorAll("#cardBody .card-stat b")].map((e) => e.textContent),
  }));
  if (!/Gate Card Fixture/.test(rendered.title)) failures.push(`rendered title missing the card's title: ${rendered.title}`);
  if (!/a fixture card/i.test(rendered.body)) failures.push("rendered view does not show the card description");
  if (!rendered.stats.length) failures.push("rendered view shows no counts");

  // The point of the CARD tier: reading the card is a couple of small ranged
  // reads, never a whole-file GET. At this fixture's size the card is most of
  // the file, so the BYTE ratio proves little here — the assertion that carries
  // weight is that no unranged download happened, and that the request count
  // stays in single digits however big the file gets.
  if (cardRead.full > 0) failures.push(`reading the card pulled the WHOLE file ${cardRead.full}×`);
  if (cardRead.range === 0) failures.push("reading the card issued no range request at all");
  if (cardRead.range > 6) failures.push(`card read took ${cardRead.range} range requests — expected ~2`);

  // ---- JSON view: coloured, and the card's own bytes -------------------------
  const json = await remote.evaluate(() => {
    document.getElementById("cardTabJson").click();
    const pre = document.querySelector("#cardBody pre.card-json");
    return {
      text: pre ? pre.textContent.slice(0, 3000) : "",
      keys: pre ? pre.querySelectorAll("span.k").length : 0,
      nums: pre ? pre.querySelectorAll("span.n").length : 0,
      // A raw < in the card must not have become a real element.
      injected: !!(pre && pre.querySelector("script, img")),
    };
  });
  if (!json.text.trim().startsWith("{")) failures.push("JSON tab does not show a JSON object");
  if (json.keys < 3) failures.push(`JSON view has ${json.keys} coloured keys — not highlighted`);
  if (json.nums < 1) failures.push("JSON view coloured no numbers");
  if (json.injected) failures.push("card text was injected as live HTML");
  if (!/"Gate Card Fixture"/.test(json.text)) failures.push("JSON view is not this file's card");

  // ---- a card query loads into the editor ------------------------------------
  const used = await remote.evaluate(() => {
    document.getElementById("cardTabView").click();
    const b = document.querySelector("#cardBody .card-q-use");
    if (!b) return { ok: false, why: "no Use button on any card query" };
    b.click();
    return {
      ok: true,
      hidden: document.getElementById("cardModal").classList.contains("hidden"),
      // setText mirrors into the textarea, which is the only readable handle —
      // PlaygroundEditor exposes no getter.
      q: (document.getElementById("q") || {}).value || "",
    };
  });
  if (!used.ok) failures.push(used.why);
  else {
    if (!used.hidden) failures.push("using a card query left the modal open");
    if (!/SELECT/i.test(used.q)) failures.push(`card query did not reach the editor: "${used.q.slice(0, 60)}"`);
  }

  // ---- resident path: Rete.card() from memory --------------------------------
  // A bundled demo file carries NO card, and the modal must say so plainly
  // rather than showing an empty shell.
  const bundled = await open("#dataset=causal&load=bundled");
  await openCard(bundled);
  const none = await bundled.evaluate(() => document.getElementById("cardBody").textContent);
  if (!/carries no Dataset Card/i.test(none)) {
    failures.push(`bundled file without a card did not say so: "${none.slice(0, 120)}"`);
  }

  if (pageErrors.length) failures.push(`page errors: ${pageErrors.slice(0, 2).join(" | ")}`);

  await browser.close();
  server.close();

  const pass = failures.length === 0;
  console.log(JSON.stringify({
    verdict: pass ? "PASS" : "FAIL",
    note: "Dataset Card modal: rendered + coloured JSON, remote (card-tier read) and resident paths",
    cardRead,
    fileBytes: fixture.length,
    sessionTraffic: traffic,
    failures,
  }, null, 2));
  process.exit(pass ? 0 : 1);
};

main().catch((e) => {
  console.log(JSON.stringify({ verdict: "FAIL", error: String(e && e.message).slice(0, 300) }, null, 2));
  process.exit(1);
});
