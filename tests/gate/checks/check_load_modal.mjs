// The "Load" button beside Build opens ONE pre-modal offering every way in:
// drop/pick a local .rete, paste a URL (opened lazily, like #url=), or hand
// off to the dataset catalog. Before it, the routes were scattered — the
// dataset chip opened the catalog, drag-and-drop and the URL field hid under
// the catalog's "Advanced" fold — and a phone user had no discoverable way to
// open their own file at all.
//
// Asserted here: all three routes are reachable; the URL route opens an
// OFF-CATALOG file end to end (served from a local range server — the claim
// under test is the modal, not the public internet); the file route ingests
// real bytes through the picker (the path a phone must use); Escape and a
// click on the backdrop both dismiss; and the button + modal work at phone
// width.
import { createServer } from "node:http";
import { readFile, writeFile } from "node:fs/promises";
import { launchBrowser } from "./_browser.mjs";
import { runWithRetry } from "./_util.mjs";

const listen = (server) => new Promise((resolve) => server.listen(0, "127.0.0.1", () => resolve(server.address().port)));

// Same off-catalog fixture strategy as check_url_param: a bundled dataset's
// bytes served under a name no catalog entry uses.
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
  const fixture = await embeddedFixture("causal");
  const PATH = "/load-modal-off-catalog.rete";
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

  const open = async (viewport) => {
    const page = await browser.newPage(viewport ? { viewport } : {});
    page.on("pageerror", (e) => pageErrors.push(String(e).slice(0, 200)));
    // A dataset hash keeps the boot catalog modal out of the way — the thing
    // under test is the Load modal, not the boot flow.
    await page.goto(`http://localhost:${PGPORT}/playground.html#dataset=scholar&mode=sparql`, { waitUntil: "domcontentloaded" });
    await page.waitForFunction(
      () => document.getElementById("run") && !document.getElementById("run").disabled,
      undefined,
      { timeout: 90000 },
    );
    return page;
  };

  const modalHidden = (page) => page.evaluate(() => document.getElementById("loadModal").classList.contains("hidden"));

  const page = await open();

  // --- open: the button beside Build ----------------------------------------
  await page.click("#loadBtn");
  if (await modalHidden(page)) failures.push("Load button did not open the modal");

  // --- all three routes are present and visible ------------------------------
  const routes = await page.evaluate(() => {
    const vis = (el) => !!el && el.offsetParent !== null;
    return {
      drop: vis(document.getElementById("loadDropZone")),
      file: !!document.getElementById("loadFileInput"),
      url: vis(document.getElementById("loadUrl")) && vis(document.getElementById("loadUrlGo")),
      examples: vis(document.getElementById("loadExamplesBtn")),
    };
  });
  if (!routes.drop || !routes.file) failures.push(`file route missing (drop ${routes.drop}, input ${routes.file})`);
  if (!routes.url) failures.push("URL route missing");
  if (!routes.examples) failures.push("examples route missing");

  // --- Escape closes ---------------------------------------------------------
  await page.keyboard.press("Escape");
  if (!(await modalHidden(page))) failures.push("Escape did not close the Load modal");

  // --- click on the backdrop closes ------------------------------------------
  await page.click("#loadBtn");
  await page.evaluate(() => document.getElementById("loadModal").dispatchEvent(
    new MouseEvent("click", { bubbles: true })));
  if (!(await modalHidden(page))) failures.push("backdrop click did not close the Load modal");

  // --- examples route hands off to the EXISTING catalog browser --------------
  await page.click("#loadBtn");
  await page.click("#loadExamplesBtn");
  const handoff = await page.evaluate(() => ({
    loadHidden: document.getElementById("loadModal").classList.contains("hidden"),
    catalogShown: !document.getElementById("sourceModal").classList.contains("hidden"),
    hasSidebar: document.querySelectorAll("#dsSidebar .ds-side-item").length > 0,
  }));
  if (!handoff.loadHidden || !handoff.catalogShown) failures.push(`examples route: load hidden ${handoff.loadHidden}, catalog shown ${handoff.catalogShown}`);
  if (!handoff.hasSidebar) failures.push("examples route opened an empty catalog");
  await page.keyboard.press("Escape");

  // --- URL route: a bad address is refused IN the modal ----------------------
  await page.click("#loadBtn");
  await page.fill("#loadUrl", "javascript:alert(1)");
  await page.click("#loadUrlGo");
  const refused = await page.evaluate(() => ({
    err: (document.getElementById("loadUrlErr") || {}).textContent || "",
    stillOpen: !document.getElementById("loadModal").classList.contains("hidden"),
  }));
  if (!/http/i.test(refused.err) || !refused.stillOpen) {
    failures.push(`bad URL not refused inline (err: "${refused.err.slice(0, 80)}", open: ${refused.stillOpen})`);
  }

  // --- URL route end to end: an OFF-CATALOG file opens lazily ----------------
  await page.fill("#loadUrl", fixtureUrl);
  await page.click("#loadUrlGo");
  if (!(await modalHidden(page))) failures.push("URL route did not close the modal on success");
  const FILE_LABEL = "load-modal-off-catalog.rete";
  const CARD_TITLE = "cardiometabolic causal model (confounders, mediators, colliders, loops)";
  await page.waitForFunction(
    ([a, b]) => {
      const t = ((document.getElementById("dsName") || {}).textContent || "").trim();
      return t === a || t === b;
    },
    [FILE_LABEL, CARD_TITLE],
    { timeout: 60000 },
  ).catch(() => { /* reported below */ });
  const claims = await page.evaluate(() => ({
    dsName: ((document.getElementById("dsName") || {}).textContent || "").trim(),
    pill: ((document.getElementById("sourcePill") || {}).textContent || "").trim(),
    field: ((document.getElementById("remoteUrl") || {}).value || ""),
    fedName: ((document.querySelector("#fedChips .fed-self .fed-chip-name") || {}).textContent || "").trim(),
    fedKind: ((document.querySelector("#fedChips .fed-self .fed-chip-kind") || {}).textContent || "").trim(),
  }));
  if (claims.dsName !== FILE_LABEL && claims.dsName !== CARD_TITLE) {
    failures.push(`URL route: dataset chip claims "${claims.dsName.slice(0, 90)}"`);
  }
  if (claims.pill !== "remote (lazy)") failures.push(`URL route: pill says "${claims.pill}"`);
  if (claims.field !== fixtureUrl) failures.push(`URL route: remote field holds "${claims.field.slice(0, 90)}"`);
  if (claims.fedName !== FILE_LABEL && claims.fedName !== CARD_TITLE) {
    failures.push(`URL route: SOURCES chip claims "${claims.fedName.slice(0, 90)}"`);
  }
  if (claims.fedKind !== "lazy") failures.push(`URL route: SOURCES chip kind "${claims.fedKind}"`);

  await page.waitForFunction(() => window.PlaygroundEditor, undefined, { timeout: 60000 });
  await page.evaluate(() => window.PlaygroundEditor.setText("q", "SELECT ?s ?p ?o WHERE { ?s ?p ?o } LIMIT 5"));
  const res = await runWithRetry(page, { steps: 60 });
  if (res.errBlock) failures.push(`query over the URL route errored: ${res.errText.slice(0, 160)}`);
  if (res.rows < 1) failures.push(`query over the URL route returned ${res.rows} rows (qmeta: ${res.qmeta.slice(0, 60)})`);

  // --- file route: the picker ingests real bytes (the phone path) ------------
  const tmp = "/tmp/load-modal-local.rete";
  await writeFile(tmp, fixture);
  await page.click("#loadBtn");
  await page.setInputFiles("#loadFileInput", tmp);
  await page.waitForFunction(
    () => ((document.getElementById("dsName") || {}).textContent || "").trim() === "Local file",
    undefined,
    { timeout: 30000 },
  ).catch(() => failures.push("file route: dataset chip did not switch to 'Local file'"));
  const fileClaims = await page.evaluate(() => ({
    hidden: document.getElementById("loadModal").classList.contains("hidden"),
    pill: ((document.getElementById("sourcePill") || {}).textContent || "").trim(),
    fedName: ((document.querySelector("#fedChips .fed-self .fed-chip-name") || {}).textContent || "").trim(),
    fedKind: ((document.querySelector("#fedChips .fed-self .fed-chip-kind") || {}).textContent || "").trim(),
  }));
  if (!fileClaims.hidden) failures.push("file route did not close the modal");
  if (fileClaims.pill !== "local file") failures.push(`file route: pill says "${fileClaims.pill}"`);
  if (fileClaims.fedName !== "Local file") failures.push(`file route: SOURCES chip claims "${fileClaims.fedName.slice(0, 90)}"`);
  if (fileClaims.fedKind !== "in-memory") failures.push(`file route: SOURCES chip kind "${fileClaims.fedKind}"`);
  await page.evaluate(() => window.PlaygroundEditor.setText("q", "SELECT ?s ?p ?o WHERE { ?s ?p ?o } LIMIT 5"));
  const fileRes = await runWithRetry(page, { steps: 30 });
  if (fileRes.rows < 1) failures.push(`file route query returned ${fileRes.rows} rows`);
  await page.close();

  // --- phone width: button visible, modal opens and fits ---------------------
  const phone = await open({ width: 390, height: 844 });
  const phoneState = await phone.evaluate(() => {
    const b = document.getElementById("loadBtn");
    const vis = !!b && b.offsetParent !== null && b.getBoundingClientRect().width > 0;
    return { vis, right: b ? Math.round(b.getBoundingClientRect().right) : -1 };
  });
  if (!phoneState.vis) failures.push("phone: Load button not visible");
  if (phoneState.right > 390) failures.push(`phone: Load button overflows the viewport (right=${phoneState.right})`);
  await phone.click("#loadBtn");
  const phoneModal = await phone.evaluate(() => {
    const m = document.getElementById("loadModal");
    const card = m.querySelector(".modal-card");
    const r = card.getBoundingClientRect();
    const vis = (el) => !!el && el.offsetParent !== null;
    return {
      open: !m.classList.contains("hidden"),
      fits: r.width <= 390 && r.left >= 0,
      routes: vis(document.getElementById("loadDropZone")) && vis(document.getElementById("loadUrl")) && vis(document.getElementById("loadExamplesBtn")),
    };
  });
  if (!phoneModal.open) failures.push("phone: Load modal did not open");
  if (!phoneModal.fits) failures.push("phone: Load modal card overflows the viewport");
  if (!phoneModal.routes) failures.push("phone: not all three routes visible");
  await phone.close();

  if (pageErrors.length) failures.push(`page errors: ${pageErrors.slice(0, 2).join(" | ")}`);

  await browser.close();
  server.close();

  const pass = failures.length === 0;
  console.log(JSON.stringify({
    verdict: pass ? "PASS" : "FAIL",
    note: "Load pre-modal: three routes reachable; URL route opens an off-catalog .rete end to end; file picker ingests bytes; Escape/backdrop close; phone width",
    urlRows: res.rows,
    fileRows: fileRes.rows,
    failures,
  }, null, 2));
  process.exit(pass ? 0 : 1);
};

main().catch((e) => {
  console.log(JSON.stringify({ verdict: "FAIL", error: String((e && e.stack) || (e && e.message) || e).slice(0, 900) }, null, 2));
  process.exit(1);
});
