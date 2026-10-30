// Two hosts that used to keep a .rete out of the browser, reproduced exactly.
//
// 1. ZENODO. Its file API (https://zenodo.org/api/records/<id>/files/<name>/content)
//    answers a range request with 206 and Access-Control-Allow-Origin: *, and
//    allows Range in the preflight — but its Access-Control-Expose-Headers lists
//    only Content-Type, ETag, Link and the rate-limit headers, so JavaScript
//    cannot read Content-Range. HEAD's Content-Length is CORS-safelisted and
//    always readable. The URL does not end in the file name, either.
// 2. A host that IGNORES Range: 200 and the whole body to every range request,
//    with Cache-Control: no-store so the browser's HTTP cache cannot quietly
//    turn later requests into 206s (Chromium does that for a cacheable body,
//    which is how this case hid). The readers keep that body and serve every
//    read from it instead of failing with a "connection hiccup".
//
// Each host is opened by #url= in both engines (the sync reader and the
// asyncify one) and once more with the persistent range cache on. Zenodo itself
// is not contacted: these are its headers, served locally.
import { createServer } from "node:http";
import { readFile } from "node:fs/promises";
import { launchBrowser } from "./_browser.mjs";
import { runWithRetry } from "./_util.mjs";

const listen = (s) => new Promise((r) => s.listen(0, "127.0.0.1", () => r(s.address().port)));
const NAME = "card-fixture.rete";

const main = async () => {
  const fixture = await readFile(`/work/tests/gate/.cache/${NAME}`);
  const seen = { zenodo: { range: 0, head: 0, preflight: 0 }, norange: { full: 0, head: 0, preflight: 0 } };

  const server = createServer((req, res) => {
    const path = (req.url || "").split("?")[0];
    const method = (req.method || "GET").toUpperCase();
    const zenodo = path === `/api/records/21168821/files/${NAME}/content`;
    const norange = path === `/norange/${NAME}`;
    if (!zenodo && !norange) { res.writeHead(404); res.end(); return; }
    const host = zenodo ? seen.zenodo : seen.norange;
    if (method === "OPTIONS") {
      host.preflight++;
      res.writeHead(204, {
        "Access-Control-Allow-Origin": "*",
        "Access-Control-Allow-Methods": "GET, HEAD, OPTIONS",
        "Access-Control-Allow-Headers": "Range, Content-Type, Authorization, Accept",
        "Access-Control-Max-Age": "21600",
      });
      res.end();
      return;
    }
    // Zenodo's exposed set: Content-Range is sent but NOT listed.
    const cors = {
      "Access-Control-Allow-Origin": "*",
      "Access-Control-Expose-Headers": "Content-Type, ETag, Link, X-RateLimit-Limit, X-RateLimit-Remaining, X-RateLimit-Reset",
      "Content-Type": "application/octet-stream",
      ETag: '"md5:0123456789abcdef"',
    };
    if (method === "HEAD") {
      host.head++;
      res.writeHead(200, { ...cors, "Content-Length": fixture.length });
      res.end();
      return;
    }
    const m = req.headers.range && /^bytes=(\d+)-(\d*)$/.exec(req.headers.range);
    if (zenodo && m) {
      host.range++;
      const start = Number(m[1]);
      if (start >= fixture.length) { res.writeHead(416, cors); res.end(); return; }
      const end = m[2] ? Math.min(Number(m[2]), fixture.length - 1) : fixture.length - 1;
      const body = fixture.subarray(start, end + 1);
      res.writeHead(206, { ...cors, "Accept-Ranges": "bytes", "Content-Range": `bytes ${start}-${end}/${fixture.length}`, "Content-Length": body.length });
      res.end(body);
      return;
    }
    if (norange) host.full++;
    res.writeHead(200, { ...cors, "Content-Length": fixture.length, ...(norange ? { "Cache-Control": "no-store" } : {}) });
    res.end(fixture);
  });
  const port = await listen(server);
  const urls = {
    zenodo: `http://127.0.0.1:${port}/api/records/21168821/files/${NAME}/content`,
    norange: `http://127.0.0.1:${port}/norange/${NAME}`,
  };

  const base = process.env.PLAYGROUND_BASE || `http://localhost:${process.env.PGPORT || 8090}`;
  const browser = await launchBrowser();
  const failures = [];
  const runs = [];
  const cases = [
    ["zenodo", "sync", false], ["zenodo", "async", false], ["zenodo", "sync", true],
    ["norange", "sync", false], ["norange", "async", false], ["norange", "sync", true],
  ];
  for (const [hostName, engine, rangeCache] of cases) {
    const ctx = await browser.newContext();
    const page = await ctx.newPage();
    const errs = [];
    page.on("pageerror", (e) => errs.push(String(e).slice(0, 160)));
    await page.addInitScript(([a, rc]) => {
      try { localStorage.setItem("asyncReadsOn", a); localStorage.setItem("rangeCacheOn", rc); } catch (e) {}
    }, [engine === "async" ? "1" : "0", rangeCache ? "1" : "0"]);
    const before = hostName === "norange" ? seen.norange.full : seen.zenodo.range;
    await page.goto(`${base}/playground.html#url=${encodeURIComponent(urls[hostName])}&mode=sparql`, { waitUntil: "domcontentloaded" });
    const tag = `${hostName}/${engine}${rangeCache ? "+range-cache" : ""}`;
    try {
      await page.waitForFunction(() => document.getElementById("run") && !document.getElementById("run").disabled && window.PlaygroundEditor, { timeout: 90000 });
      await page.evaluate(() => window.PlaygroundEditor.setText("q", "SELECT ?s ?p ?o WHERE { ?s ?p ?o } LIMIT 5"));
      const res = await runWithRetry(page, { steps: 60, tries: 2 });
      const label = await page.evaluate(() => (document.getElementById("dsName") || {}).textContent || "");
      const after = hostName === "norange" ? seen.norange.full : seen.zenodo.range;
      runs.push({ case: tag, rows: res.rows, tries: res.tries, label: label.slice(0, 60), requests: after - before });
      if (res.errBlock || res.rows < 1) failures.push(`${tag}: ${res.rows} rows${res.errText ? " — " + res.errText.slice(0, 140) : ""}`);
      // The off-catalog label is the file name, never Zenodo's trailing "content".
      if (hostName === "zenodo" && /^content\b/i.test(label.trim())) failures.push(`${tag}: labelled "${label.trim()}" instead of ${NAME}`);
      // A range-ignoring host's body is kept: a few whole downloads per page
      // (each reader the page opens probes once), not one per read.
      if (hostName === "norange" && after - before > 6) failures.push(`${tag}: ${after - before} whole-body downloads for one query`);
    } catch (e) {
      failures.push(`${tag}: ${String(e && e.message).slice(0, 160)}`);
    }
    if (errs.length) failures.push(`${tag}: page errors: ${errs.slice(0, 2).join(" | ")}`);
    await ctx.close();
  }
  await browser.close();
  server.close();

  // The ranged path must actually have run against the mock, else this proves
  // less than it says.
  if (!seen.zenodo.range) failures.push("the Zenodo mock saw no range request");
  const pass = failures.length === 0;
  console.log(JSON.stringify({
    verdict: pass ? "PASS" : "FAIL",
    note: "Zenodo's header set (Content-Range hidden) and a Range-ignoring no-store host both open and answer",
    runs, requests: seen, failures,
  }, null, 2));
  process.exit(pass ? 0 : 1);
};

main().catch((e) => {
  console.log(JSON.stringify({ verdict: "FAIL", error: String(e && e.message).slice(0, 300) }, null, 2));
  process.exit(1);
});
