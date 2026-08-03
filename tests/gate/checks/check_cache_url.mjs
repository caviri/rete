// Caching an OFF-CATALOG .rete by URL — the Load modal's "Download & cache"
// route and the #url=…&load=cache deep link. Pinned here:
//
//   1. The file's TRUE size is shown BEFORE the download starts (read from the
//      file's own header over 1–2 range reads — the #95 probe), with a way out.
//   2. The download happens ONCE (exactly one full GET), persists in
//      IndexedDB, and after a reload the SECOND session answers queries with
//      ZERO requests to the file's host — the property that makes cached mode
//      worth having, asserted from the server's own request counters.
//   3. The view names the file that is actually open (its filename or its own
//      Dataset Card title) on every surface — chip, header, SOURCES chip with
//      an in-memory badge — and the share hash carries url=…&load=cache, not a
//      catalog key. (The "scholar.rete" mislabeling bug had three doors; the
//      cached-by-URL path must not be a fourth.)
//   4. The deep link honors load=cache: already-cached → opens with zero
//      network; not-yet-cached → the same consent step, and backing out falls
//      back to lazy instead of a dead console.
import { createServer } from "node:http";
import { readFile } from "node:fs/promises";
import { launchBrowser } from "./_browser.mjs";
import { runWithRetry } from "./_util.mjs";

const listen = (server) => new Promise((resolve) => server.listen(0, "127.0.0.1", () => resolve(server.address().port)));

// Same off-catalog fixture strategy as check_url_param / check_load_modal: a
// bundled dataset's bytes served under a name no catalog entry uses, so a pass
// cannot come from the key quietly resolving to a bundled dataset.
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

// Mirror of the app's formatBytes — the consent button must show THIS number.
const fmtBytes = (v) => {
  if (v < 1024) return v + " B";
  if (v < 1024 * 1024) return (v / 1024).toFixed(1) + " KB";
  if (v < 1024 * 1024 * 1024) return (v / 1024 / 1024).toFixed(1) + " MB";
  return (v / 1024 / 1024 / 1024).toFixed(2) + " GB";
};

const main = async () => {
  const fixture = await embeddedFixture("causal");
  const PATH = "/off-catalog-cached.rete";
  // full = whole-body GETs (the download under test), range = partial reads
  // (the size probe / lazy reads), other = OPTIONS preflights + HEAD — counted
  // apart so neither can masquerade as a download in the assertions.
  const traffic = { full: 0, range: 0, other: 0 };
  const server = createServer((req, res) => {
    if (req.url?.split("?")[0] !== PATH) { res.writeHead(404); res.end("not found"); return; }
    const common = {
      "Access-Control-Allow-Origin": "*",
      "Access-Control-Expose-Headers": "Content-Range,Content-Length,Accept-Ranges",
      "Accept-Ranges": "bytes",
    };
    if (req.method === "OPTIONS") {
      traffic.other++;
      res.writeHead(204, { ...common, "Access-Control-Allow-Headers": "*", "Access-Control-Allow-Methods": "GET,HEAD,OPTIONS" });
      res.end();
      return;
    }
    if (req.method === "HEAD") {
      traffic.other++;
      res.writeHead(200, { ...common, "Content-Type": "application/octet-stream", "Content-Length": fixture.length });
      res.end();
      return;
    }
    const range = req.headers.range && /bytes=(\d+)-(\d*)/.exec(req.headers.range);
    if (range) {
      traffic.range++;
      const start = Number(range[1]);
      const end = range[2] ? Math.min(Number(range[2]), fixture.length - 1) : fixture.length - 1;
      const body = fixture.subarray(start, end + 1);
      res.writeHead(206, { ...common, "Content-Type": "application/octet-stream", "Content-Range": `bytes ${start}-${end}/${fixture.length}`, "Content-Length": body.length });
      res.end(body);
      return;
    }
    traffic.full++;
    res.writeHead(200, { ...common, "Content-Type": "application/octet-stream", "Content-Length": fixture.length });
    res.end(fixture);
  });
  const port = await listen(server);
  const fixtureUrl = `http://127.0.0.1:${port}${PATH}`;

  const PGPORT = process.env.PGPORT || "8090";
  const browser = await launchBrowser();
  const failures = [];
  const pageErrors = [];
  const FILE_LABEL = "off-catalog-cached.rete";
  const CARD_TITLE = "cardiometabolic causal model (confounders, mediators, colliders, loops)";

  const claimed = (page) => page.evaluate(() => ({
    dsName: ((document.getElementById("dsName") || {}).textContent || "").trim(),
    dsTitle: ((document.getElementById("dsTitle") || {}).textContent || "").trim(),
    pill: ((document.getElementById("sourcePill") || {}).textContent || "").trim(),
    fedName: ((document.querySelector("#fedChips .fed-self .fed-chip-name") || {}).textContent || "").trim(),
    fedKind: ((document.querySelector("#fedChips .fed-self .fed-chip-kind") || {}).textContent || "").trim(),
    hash: location.hash,
  }));
  const assertCachedClaims = async (page, where) => {
    const c = await claimed(page);
    if (c.dsName !== FILE_LABEL && c.dsName !== CARD_TITLE) failures.push(`${where}: dataset chip claims "${c.dsName.slice(0, 90)}"`);
    if (c.dsTitle !== FILE_LABEL && c.dsTitle !== CARD_TITLE) failures.push(`${where}: header title claims "${c.dsTitle.slice(0, 90)}"`);
    if (c.pill !== "remote (cached)") failures.push(`${where}: source pill says "${c.pill}"`);
    if (c.fedName !== FILE_LABEL && c.fedName !== CARD_TITLE) failures.push(`${where}: SOURCES chip claims "${c.fedName.slice(0, 90)}"`);
    if (c.fedKind !== "in-memory") failures.push(`${where}: SOURCES chip kind says "${c.fedKind}"`);
    if (!c.hash.includes(`url=${encodeURIComponent(fixtureUrl)}`)) failures.push(`${where}: hash lacks the address: ${c.hash.slice(0, 120)}`);
    if (!/[#&]load=cache/.test(c.hash)) failures.push(`${where}: hash lacks load=cache: ${c.hash.slice(0, 120)}`);
    if (/[#&]dataset=/.test(c.hash)) failures.push(`${where}: hash still names a catalog dataset: ${c.hash.slice(0, 120)}`);
    return c;
  };
  const cachedReady = (page) => page.waitForFunction(
    () => /remote \(cached\)/i.test((document.getElementById("sourcePill") || {}).textContent || "")
      && document.getElementById("cacheModal")?.classList.contains("hidden"),
    undefined,
    { timeout: 90000 },
  );

  // ---- route 1: the Load modal, size-first consent, download once ----------
  const ctx = await browser.newContext();
  const page = await ctx.newPage();
  page.on("pageerror", (e) => pageErrors.push(String(e).slice(0, 200)));
  await page.goto(`http://localhost:${PGPORT}/playground.html#dataset=scholar&mode=sparql`, { waitUntil: "domcontentloaded" });
  await page.waitForFunction(() => document.getElementById("run") && !document.getElementById("run").disabled, undefined, { timeout: 90000 });

  await page.click("#loadBtn");
  await page.fill("#loadUrl", fixtureUrl);
  await page.click("#loadUrlCache");
  // The consent step must appear with the file's TRUE size — before ANY full
  // download. The probe itself may only use small range reads.
  await page.waitForFunction(
    () => !document.getElementById("cacheConfirm").classList.contains("hidden"),
    undefined,
    { timeout: 60000 },
  ).catch(() => failures.push("consent step never appeared"));
  const confirmState = await page.evaluate(() => ({
    goText: ((document.getElementById("cacheGo") || {}).textContent || "").trim(),
    sub: ((document.getElementById("cacheSub") || {}).textContent || "").trim(),
  }));
  const wantSize = fmtBytes(fixture.length);
  if (confirmState.goText !== `Download ${wantSize}`) {
    failures.push(`consent button says "${confirmState.goText}", expected "Download ${wantSize}"`);
  }
  if (!confirmState.sub.includes(wantSize)) failures.push(`consent text lacks the size: "${confirmState.sub.slice(0, 120)}"`);
  if (traffic.full !== 0) failures.push(`the size probe downloaded the file (${traffic.full} full GETs before consent)`);
  const probeRange = traffic.range;

  await page.click("#cacheGo");
  await cachedReady(page).catch(() => failures.push("cached mode never became ready after Download"));
  await assertCachedClaims(page, "modal route");
  await page.waitForFunction(() => window.PlaygroundEditor, undefined, { timeout: 60000 });
  await page.evaluate(() => window.PlaygroundEditor.setText("q", "SELECT ?s ?p ?o WHERE { ?s ?p ?o } LIMIT 5"));
  const res1 = await runWithRetry(page, { steps: 30 });
  if (res1.rows < 1) failures.push(`query over the cached URL returned ${res1.rows} rows (qmeta: ${res1.qmeta.slice(0, 60)})`);
  const firstTraffic = { ...traffic };
  if (firstTraffic.full !== 1) failures.push(`expected exactly 1 full download, saw ${firstTraffic.full}`);
  if (firstTraffic.range > probeRange) failures.push(`range reads AFTER the probe: ${firstTraffic.range - probeRange} (cached queries must not touch the host)`);

  // ---- the property that makes cached mode worth having: reload, then ------
  // ---- query, with ZERO requests to the file's host ------------------------
  await page.reload({ waitUntil: "domcontentloaded" });
  await cachedReady(page).catch(() => failures.push("cached mode did not restore after reload"));
  const afterReload = { ...traffic };
  if (afterReload.full !== firstTraffic.full || afterReload.range !== firstTraffic.range || afterReload.other !== firstTraffic.other) {
    failures.push(`reload touched the host (full +${afterReload.full - firstTraffic.full}, range +${afterReload.range - firstTraffic.range}, other +${afterReload.other - firstTraffic.other})`);
  }
  await assertCachedClaims(page, "after reload");
  await page.waitForFunction(() => window.PlaygroundEditor, undefined, { timeout: 60000 });
  await page.evaluate(() => window.PlaygroundEditor.setText("q", "SELECT ?s ?p ?o WHERE { ?s ?p ?o } LIMIT 5"));
  const res2 = await runWithRetry(page, { steps: 30 });
  if (res2.rows < 1) failures.push(`query after reload returned ${res2.rows} rows`);
  const secondTraffic = { full: traffic.full - afterReload.full, range: traffic.range - afterReload.range, other: traffic.other - afterReload.other };
  if (secondTraffic.full !== 0 || secondTraffic.range !== 0 || secondTraffic.other !== 0) {
    failures.push(`second-session query touched the host (full ${secondTraffic.full}, range ${secondTraffic.range}, other ${secondTraffic.other})`);
  }

  // ---- route 2: the deep link, already cached → zero network ---------------
  const deep = await ctx.newPage();
  deep.on("pageerror", (e) => pageErrors.push(String(e).slice(0, 200)));
  const beforeDeep = { ...traffic };
  await deep.goto(`http://localhost:${PGPORT}/playground.html#url=${encodeURIComponent(fixtureUrl)}&load=cache&mode=sparql`, { waitUntil: "domcontentloaded" });
  await cachedReady(deep).catch(() => failures.push("deep link did not restore cached mode"));
  await assertCachedClaims(deep, "deep link (cached)");
  await deep.waitForFunction(() => window.PlaygroundEditor, undefined, { timeout: 60000 });
  await deep.evaluate(() => window.PlaygroundEditor.setText("q", "SELECT ?s ?p ?o WHERE { ?s ?p ?o } LIMIT 5"));
  const res3 = await runWithRetry(deep, { steps: 30 });
  if (res3.rows < 1) failures.push(`deep-link query returned ${res3.rows} rows`);
  const deepTraffic = { full: traffic.full - beforeDeep.full, range: traffic.range - beforeDeep.range, other: traffic.other - beforeDeep.other };
  if (deepTraffic.full !== 0 || deepTraffic.range !== 0 || deepTraffic.other !== 0) {
    failures.push(`cached deep link touched the host (full ${deepTraffic.full}, range ${deepTraffic.range}, other ${deepTraffic.other})`);
  }
  await deep.close();
  await page.close();
  await ctx.close();

  // ---- route 3: fresh browser profile, deep link NOT yet cached ------------
  // The consent step must gate the download here too, and backing out must
  // fall back to lazy — never a dead console, never a byte of payload.
  const ctx2 = await browser.newContext();
  const fresh = await ctx2.newPage();
  fresh.on("pageerror", (e) => pageErrors.push(String(e).slice(0, 200)));
  const beforeFresh = { ...traffic };
  await fresh.goto(`http://localhost:${PGPORT}/playground.html#url=${encodeURIComponent(fixtureUrl)}&load=cache&mode=sparql`, { waitUntil: "domcontentloaded" });
  await fresh.waitForFunction(
    () => !document.getElementById("cacheConfirm").classList.contains("hidden"),
    undefined,
    { timeout: 90000 },
  ).catch(() => failures.push("uncached deep link skipped the consent step"));
  const freshGo = await fresh.evaluate(() => ((document.getElementById("cacheGo") || {}).textContent || "").trim());
  if (freshGo !== `Download ${wantSize}`) failures.push(`uncached deep link consent says "${freshGo}"`);
  await fresh.click("#cacheCancel");
  await fresh.waitForFunction(
    () => /remote \(lazy\)/i.test((document.getElementById("sourcePill") || {}).textContent || ""),
    undefined,
    { timeout: 30000 },
  ).catch(() => failures.push("backing out did not fall back to lazy"));
  const freshTraffic = { full: traffic.full - beforeFresh.full, range: traffic.range - beforeFresh.range };
  if (freshTraffic.full !== 0) failures.push(`backing out still downloaded the file (${freshTraffic.full} full GETs)`);
  await fresh.close();
  await ctx2.close();

  if (pageErrors.length) failures.push(`page errors: ${pageErrors.slice(0, 3).join(" | ")}`);

  await browser.close();
  await new Promise((resolve) => server.close(resolve));

  const pass = failures.length === 0;
  console.log(JSON.stringify({
    verdict: pass ? "PASS" : "FAIL",
    note: "cache an off-catalog URL: size-first consent, one download, zero-network reload + deep link, honest labels",
    fixtureBytes: fixture.length,
    probeRangeReads: probeRange,
    firstTraffic,
    secondTraffic,
    deepTraffic,
    rows: { first: res1.rows, afterReload: res2.rows, deepLink: res3.rows },
    failures,
  }, null, 2));
  process.exit(pass ? 0 : 1);
};

main().catch((e) => {
  console.log(JSON.stringify({ verdict: "FAIL", error: String((e && e.stack) || (e && e.message) || e).slice(0, 900) }, null, 2));
  process.exit(1);
});
