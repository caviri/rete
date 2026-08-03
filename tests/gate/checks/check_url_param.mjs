// #url=<address of a .rete> must open a file that is NOT in the catalog.
//
// Until this parameter existed a deep link could only name a catalog key, so
// anyone hosting their own .rete had no shareable link to it — they had to be
// told to paste the address into the field by hand, and one stray character in
// that paste surfaced only as a bare "Error: open" from the range reader.
//
// The fixture is served from a local range-capable server rather than a real
// third-party host: the parameter is what's under test, and a check that reaches
// the public internet goes red for reasons that have nothing to do with it.
import { createServer } from "node:http";
import { readFile } from "node:fs/promises";
import { launchBrowser } from "./_browser.mjs";
import { runWithRetry } from "./_util.mjs";

const listen = (server) => new Promise((resolve) => server.listen(0, "127.0.0.1", () => resolve(server.address().port)));

const embeddedFixture = async (name) => {
  const html = await readFile("/work/docs/playground.html", "utf8");
  const marker = "const RETE_DATASETS_B64 = ";
  const start = html.indexOf(marker);
  if (start < 0) throw new Error("tracked playground has no embedded dataset map");
  const jsonStart = start + marker.length;
  const lineEnd = html.indexOf("\n", jsonStart);
  const expression = html.slice(jsonStart, lineEnd).trim().replace(/;$/, "");
  const encoded = JSON.parse(expression)[name];
  if (!encoded) throw new Error(`tracked playground has no embedded ${name} dataset`);
  return Buffer.from(encoded, "base64");
};

const main = async () => {
  // Served under a name no catalog entry uses, so a pass cannot come from the
  // key quietly resolving to a bundled dataset instead.
  const fixture = await embeddedFixture("causal");
  const PATH = "/not-in-the-catalog.rete";
  const server = createServer((req, res) => {
    if (req.url?.split("?")[0] !== PATH) { res.writeHead(404); res.end("not found"); return; }
    const common = {
      "Access-Control-Allow-Origin": "*",
      "Access-Control-Expose-Headers": "Content-Range,Content-Length,Accept-Ranges",
      "Accept-Ranges": "bytes",
    };
    const range = req.headers.range && /bytes=(\d+)-(\d*)/.exec(req.headers.range);
    if (range) {
      const start = Number(range[1]);
      const end = range[2] ? Math.min(Number(range[2]), fixture.length - 1) : fixture.length - 1;
      const body = fixture.subarray(start, end + 1);
      res.writeHead(206, { ...common, "Content-Type": "application/octet-stream", "Content-Range": `bytes ${start}-${end}/${fixture.length}`, "Content-Length": body.length });
      res.end(body);
      return;
    }
    res.writeHead(200, { ...common, "Content-Type": "application/octet-stream", "Content-Length": fixture.length });
    res.end(fixture);
  });
  const port = await listen(server);
  const fixtureUrl = `http://127.0.0.1:${port}${PATH}`;

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

  // 1. Happy path: the deep link alone opens the off-catalog file and a query
  //    answers from it.
  const page = await open(`#url=${encodeURIComponent(fixtureUrl)}`);
  await page.waitForFunction(
    (u) => (document.getElementById("remoteUrl") || {}).value === u,
    fixtureUrl,
    { timeout: 30000 },
  ).catch(() => failures.push("#url= did not populate the remote URL field"));

  await page.waitForFunction(() => window.PlaygroundEditor, { timeout: 60000 });
  await page.evaluate(() => window.PlaygroundEditor.setText("q", "SELECT ?s ?p ?o WHERE { ?s ?p ?o } LIMIT 5"));
  const res = await runWithRetry(page, { steps: 60 });
  if (res.errBlock) failures.push(`query over #url= errored: ${res.errText.slice(0, 160)}`);
  if (res.rows < 1) failures.push(`query over #url= returned ${res.rows} rows (qmeta: ${res.qmeta.slice(0, 60)})`);

  // 2. Share must round-trip. updateHash() used to emit dataset=<key> for every
  //    view, so sharing an off-catalog remote handed out a link to whatever
  //    dataset happened to be loaded before it — a different graph than the one
  //    on screen.
  const shared = await page.evaluate(() => {
    document.getElementById("shareBtn").click();
    return location.hash;
  });
  if (!shared.includes(`url=${encodeURIComponent(fixtureUrl)}`)) {
    failures.push(`share link does not carry the remote address: ${shared.slice(0, 120)}`);
  }
  if (/[#&]dataset=/.test(shared)) {
    failures.push(`share link still names a catalog dataset: ${shared.slice(0, 120)}`);
  }

  // 3. A non-http scheme must be refused rather than handed to the reader. This
  //    value arrives from the address bar, so this guard is what keeps a
  //    javascript: URL out of it.
  const bad = await open(`#url=${encodeURIComponent("javascript:alert(1)")}`);
  const refusal = await bad.evaluate(() => (document.getElementById("out") || {}).textContent || "");
  if (!/needs an http/i.test(refusal)) failures.push(`javascript: URL was not refused (#out: "${refusal.slice(0, 100)}")`);

  if (pageErrors.length) failures.push(`page errors: ${pageErrors.slice(0, 2).join(" | ")}`);

  await browser.close();
  server.close();

  if (failures.length) {
    console.error(`FAIL url_param\n  - ${failures.join("\n  - ")}`);
    process.exit(1);
  }
  console.log(`PASS url_param — #url= opened an off-catalog .rete (${res.rows} rows); javascript: refused`);
};

main().catch((e) => { console.error(`FAIL url_param — ${e && e.message}`); process.exit(1); });
