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

  // What the UI may CLAIM to have open for this off-catalog file: its own
  // file name, or — once the async card read lands — the Dataset Card title
  // embedded in the fixture (the causal demo file). Anything else means the
  // page is asserting a different dataset is open than the one that is: the
  // key fallback used to resolve unknown keys to the FIRST catalog entry, so
  // an nkod.rete URL sat under a "hugging-face.rete — …" header, and a real
  // report chased the wrong dataset because of it.
  const FILE_LABEL = "not-in-the-catalog.rete";
  const CARD_TITLE = "cardiometabolic causal model (confounders, mediators, colliders, loops)";
  const claimed = (page) => page.evaluate(() => ({
    dsName: ((document.getElementById("dsName") || {}).textContent || "").trim(),
    dsTitle: ((document.getElementById("dsTitle") || {}).textContent || "").trim(),
    pill: ((document.getElementById("sourcePill") || {}).textContent || "").trim(),
  }));
  const assertClaims = async (page, where) => {
    // The filename label paints synchronously; the card title may replace it
    // once the worker has read the card. Wait for either accepted value, then
    // reject everything else — asserting on what the UI claims, not merely
    // that it did not crash.
    await page.waitForFunction(
      ([a, b]) => {
        const t = ((document.getElementById("dsName") || {}).textContent || "").trim();
        return t === a || t === b;
      },
      [FILE_LABEL, CARD_TITLE],
      { timeout: 60000 },
    ).catch(() => { /* fall through to the explicit report below */ });
    const c = await claimed(page);
    if (c.dsName !== FILE_LABEL && c.dsName !== CARD_TITLE) {
      failures.push(`${where}: dataset chip claims "${c.dsName.slice(0, 90)}" for ${FILE_LABEL}`);
    }
    if (c.dsTitle !== FILE_LABEL && c.dsTitle !== CARD_TITLE) {
      failures.push(`${where}: header title claims "${c.dsTitle.slice(0, 90)}" for ${FILE_LABEL}`);
    }
    if (c.pill !== "remote (lazy)") {
      failures.push(`${where}: source pill says "${c.pill}", expected "remote (lazy)"`);
    }
    return c;
  };

  // 1. Happy path: the deep link alone opens the off-catalog file and a query
  //    answers from it.
  const page = await open(`#url=${encodeURIComponent(fixtureUrl)}`);
  await page.waitForFunction(
    (u) => (document.getElementById("remoteUrl") || {}).value === u,
    fixtureUrl,
    { timeout: 30000 },
  ).catch(() => failures.push("#url= did not populate the remote URL field"));

  // 1a. The page must name what it actually loaded (chip + header + pill).
  await assertClaims(page, "#url= open");

  await page.waitForFunction(() => window.PlaygroundEditor, { timeout: 60000 });
  await page.evaluate(() => window.PlaygroundEditor.setText("q", "SELECT ?s ?p ?o WHERE { ?s ?p ?o } LIMIT 5"));
  const res = await runWithRetry(page, { steps: 60 });
  if (res.errBlock) failures.push(`query over #url= errored: ${res.errText.slice(0, 160)}`);
  if (res.rows < 1) failures.push(`query over #url= returned ${res.rows} rows (qmeta: ${res.qmeta.slice(0, 60)})`);

  // 1c. Running a query must not relabel the view either.
  const afterQuery = await assertClaims(page, "after query");

  // 1d. A remote connected BY HAND (Build → advanced → paste a URL → Connect,
  //     which passes datasetKey = null) used to keep the PREVIOUS dataset's
  //     name on the chip — same wrong claim, different door.
  const manual = await open("");
  const before = await claimed(manual);
  await manual.evaluate((u) => {
    document.getElementById("remoteUrl").value = u;
    document.getElementById("remoteConnect").click();
  }, fixtureUrl);
  const manualClaim = await assertClaims(manual, "manual Connect");
  if (manualClaim.dsName === before.dsName) {
    failures.push(`manual Connect kept the previous dataset's name: "${before.dsName.slice(0, 90)}"`);
  }
  await manual.close();

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

  // 3. A scheme-LESS address must work, read as https like an address bar reads
  //    it. Pasted links routinely arrive that way, and refusing them produced a
  //    bare "Error: open" from the range reader that named no cause — the
  //    report that prompted this.
  const noScheme = await open(`#url=${encodeURIComponent(`127.0.0.1:${port}${PATH}`)}`);
  const normalized = await noScheme.evaluate(
    () => (document.getElementById("remoteUrl") || {}).value || "",
  );
  if (normalized !== `https://127.0.0.1:${port}${PATH}`) {
    failures.push(`scheme-less #url= was not normalized to https: "${normalized}"`);
  }

  // 4. A scheme that is NOT http(s) must still be refused rather than handed to
  //    the reader — this value arrives from the address bar, so that guard is
  //    what keeps a javascript: URL out of it. It must not be "fixed" by
  //    prefixing https:// either.
  const bad = await open(`#url=${encodeURIComponent("javascript:alert(1)")}`);
  const refusal = await bad.evaluate(() => ({
    out: (document.getElementById("out") || {}).textContent || "",
    field: (document.getElementById("remoteUrl") || {}).value || "",
  }));
  if (!/needs an http/i.test(refusal.out)) failures.push(`javascript: URL was not refused (#out: "${refusal.out.slice(0, 100)}")`);
  if (/javascript:/i.test(refusal.field)) failures.push("a javascript: URL reached the remote URL field");

  if (pageErrors.length) failures.push(`page errors: ${pageErrors.slice(0, 2).join(" | ")}`);

  await browser.close();
  server.close();

  // The runner reads the LAST JSON object on stdout and requires
  // verdict === "PASS" — a zero exit alone is not a pass.
  const pass = failures.length === 0;
  console.log(JSON.stringify({
    verdict: pass ? "PASS" : "FAIL",
    note: "#url= opens an off-catalog .rete; UI names the actual file (chip/header/pill); share round-trips; javascript: refused",
    rows: res.rows,
    claimedAfterQuery: afterQuery,
    fixture: fixtureUrl,
    failures,
  }, null, 2));
  process.exit(pass ? 0 : 1);
};

main().catch((e) => {
  console.log(JSON.stringify({ verdict: "FAIL", error: String(e && e.message).slice(0, 300) }, null, 2));
  process.exit(1);
});
