// The examples panel offers the LOADED FILE's own Dataset Card queries.
//
// The reported bug: an off-catalog .rete showed NO examples at all even though
// its card ships some — examplesForDataset() read only CATALOG.examples[key],
// and an off-catalog key has no entry. The fix supplements the curated catalog
// examples with the card's queries (both card shapes), deduplicated by a
// normalizing fingerprint (comments/PREFIX/vars/LIMIT-number/case/whitespace),
// provenance-labelled (family "Card" + a separator), and KEPT even when a
// query returns 0 rows — a zero-row query is still a starting point to edit.
//
// Asserted here:
//  - off-catalog remote (named-graphs-only fixture): card examples appear;
//    the near-duplicate example_queries entry is deduped; the zero-row
//    default-graph query is present, runs to 0 rows, and STAYS listed;
//  - local FILE load (card-fixture): card examples appear, and the previous
//    dataset's catalog examples do NOT leak over the local file;
//  - catalog dataset (embedded causal): catalog examples stay first and
//    unchanged; the card's curated example_queries (byte-identical to the
//    catalog's) are deduplicated away — no "Card query N" rows — while the
//    card's auto-generated titled queries appear under the Card separator.
import { createServer } from "node:http";
import { readFile } from "node:fs/promises";
import { launchBrowser } from "./_browser.mjs";

const listen = (server) => new Promise((resolve) => server.listen(0, "127.0.0.1", () => resolve(server.address().port)));

const ZERO_ROW_Q = "SELECT ?s ?p ?o WHERE { ?s ?p ?o } LIMIT 10";

const main = async () => {
  let fixture = null;
  try {
    fixture = await readFile("/work/tests/gate/.cache/named-graphs-only.rete");
  } catch (e) {
    console.log(JSON.stringify({ verdict: "FAIL", error: "tests/gate/.cache/named-graphs-only.rete missing — run scripts/build_wasm.sh" }));
    process.exit(1);
  }
  const server = createServer((req, res) => {
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
    res.end(req.method === "HEAD" ? undefined : fixture);
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
      () => window.PlaygroundEditor && document.getElementById("run") && !document.getElementById("run").disabled,
      undefined,
      { timeout: 90000 },
    );
    return page;
  };

  // The library list (#examples) re-renders whether or not the modal is open —
  // wait for the async card read to land as Card-family rows.
  const waitForCardRows = (page, timeout = 90000) => page.waitForFunction(
    () => document.querySelectorAll('#examples article[data-family="Card"]').length > 0,
    undefined,
    { timeout },
  );

  // NB: first-child — the button holds the label span AND (sometimes) a
  // perf-badge span; selecting every span double-counts labelled examples.
  const libraryShape = (page) => page.evaluate(() => ({
    cardLabels: [...document.querySelectorAll('#examples article[data-family="Card"] .example-button')].map((e) => e.querySelector("span").textContent),
    otherLabels: [...document.querySelectorAll('#examples article:not([data-family="Card"]) .example-button')].map((e) => e.querySelector("span").textContent),
    sep: (document.querySelector("#examples .ex-card-sep") || {}).textContent || "",
    families: [...document.querySelectorAll("#familyFilters button")].map((b) => b.textContent),
    // The separator must sit BEFORE the first Card row and AFTER every non-Card row.
    order: [...document.querySelectorAll("#examples article, #examples .ex-card-sep")]
      .map((el) => (el.classList.contains("ex-card-sep") ? "SEP" : el.dataset.family === "Card" ? "card" : "cat")),
  }));

  // ---- 1. off-catalog remote: the reported bug's exact shape ----------------
  const remote = await open(`#url=${encodeURIComponent(`http://127.0.0.1:${port}/named-graphs-only.rete`)}`);
  await waitForCardRows(remote);
  const r = await libraryShape(remote);
  if (!r.cardLabels.length) failures.push("remote: no card examples at all");
  if (r.otherLabels.length) {
    failures.push(`remote: an off-catalog key showed catalog examples: ${r.otherLabels.slice(0, 3).join(" | ")}`);
  }
  // The card's curated strings are untitled → positional labels. Entry 3 is a
  // near-duplicate of entry 2 (prefix label, case, variable names and the
  // LIMIT number all differ) and must be deduplicated away.
  if (!r.cardLabels.includes("Card query 1")) failures.push(`remote: zero-row curated query missing (labels: ${r.cardLabels.join(" | ")})`);
  if (!r.cardLabels.includes("Card query 2")) failures.push("remote: titled-graph curated query missing");
  if (r.cardLabels.includes("Card query 3")) failures.push("remote: the near-duplicate example_queries entry was NOT deduped");
  if (!/From this file's Dataset Card/.test(r.sep)) failures.push(`remote: no provenance separator (got: "${r.sep.slice(0, 60)}")`);
  if (!r.families.includes("Card")) failures.push("remote: no Card family filter chip");
  // The connect note must not keep claiming there is no library.
  const note = await remote.evaluate(() => (document.querySelector("#out .note") || {}).textContent || "");
  if (/No example library for a custom URL/.test(note)) failures.push("remote: the connect note still claims there is no example library");

  // ---- 2. the zero-row query: selectable, runs to 0 rows, STAYS listed ------
  const zi = await remote.evaluate((q) => {
    const btns = [...document.querySelectorAll("#examples article .example-button")];
    const hit = btns.find((b) => b.querySelector("span").textContent === "Card query 1");
    if (!hit) return { ok: false };
    hit.click();
    return { ok: true, editor: (document.getElementById("q") || {}).value || "" };
  }, ZERO_ROW_Q);
  if (!zi.ok) failures.push("remote: could not select Card query 1");
  else if (zi.editor.trim() !== ZERO_ROW_Q) failures.push(`remote: selecting Card query 1 loaded the wrong query: "${zi.editor.slice(0, 60)}"`);
  await remote.evaluate(() => { document.getElementById("qmeta").textContent = ""; document.getElementById("run").click(); });
  await remote.waitForFunction(
    () => /row\(s\)/.test((document.getElementById("qmeta") || {}).textContent || "") || document.querySelector("#out .error-box"),
    undefined,
    { timeout: 90000 },
  );
  const zr = await remote.evaluate(() => ({
    qmeta: (document.getElementById("qmeta") || {}).textContent || "",
    error: !!document.querySelector("#out .error-box"),
    stillListed: [...document.querySelectorAll('#examples article[data-family="Card"] .example-button span')].some((e) => e.textContent === "Card query 1"),
  }));
  if (zr.error) failures.push("remote: the zero-row card query errored");
  if (!/\b0 row/.test(zr.qmeta)) failures.push(`remote: expected 0 rows on the empty default graph, qmeta: "${zr.qmeta.slice(0, 80)}"`);
  if (!zr.stillListed) failures.push("remote: the zero-row query vanished from the panel after running");
  await remote.close();

  // ---- 3. local FILE load: card examples, no stale catalog leak -------------
  // Load starts from scholar (which shows its OWN catalog + card examples), so
  // both kinds of stale row would be visible. Wait for the file to actually be
  // the open dataset — scholar's card rows already satisfy a bare "any card
  // row" wait — then for the scholar rows to be gone.
  const local = await open("#dataset=scholar&mode=sparql");
  await local.setInputFiles("#loadFileInput", "/work/tests/gate/.cache/card-fixture.rete");
  await local.waitForFunction(
    () => (document.getElementById("dsName") || {}).textContent === "Local file" &&
          document.querySelectorAll('#examples article:not([data-family="Card"])').length === 0 &&
          document.querySelectorAll('#examples article[data-family="Card"]').length > 0,
    undefined,
    { timeout: 90000 },
  ).catch(() => {});
  const l = await libraryShape(local);
  if (!l.cardLabels.length) failures.push("local file: no card examples");
  if (l.otherLabels.length) failures.push(`local file: the PREVIOUS dataset's catalog examples leaked over a local file: ${l.otherLabels.slice(0, 3).join(" | ")}`);
  await local.close();

  // ---- 4. catalog dataset: supplement + dedupe against the curated library --
  // The embedded causal card carries 8 example_queries BYTE-IDENTICAL to 8 of
  // the 10 catalog examples (they were baked from the catalog) plus ~19
  // auto-generated titled queries. So: 10 catalog rows first, a Card block
  // after, and NOT ONE positional "Card query N" row (all 8 deduped).
  const cat = await open("#dataset=causal&load=bundled&mode=sparql");
  await waitForCardRows(cat);
  const c = await libraryShape(cat);
  if (c.otherLabels.length !== 10) failures.push(`causal: expected the 10 catalog examples unchanged, got ${c.otherLabels.length}`);
  if (!c.cardLabels.length) failures.push("causal: the card's auto-generated queries did not supplement the catalog");
  if (c.cardLabels.some((t) => /^Card query \d+$/.test(t))) {
    failures.push(`causal: a byte-identical curated card query escaped dedupe: ${c.cardLabels.filter((t) => /^Card query \d+$/.test(t)).join(" | ")}`);
  }
  const sepAt = c.order.indexOf("SEP");
  if (sepAt < 0 || c.order.slice(0, sepAt).some((k) => k === "card") || c.order.slice(sepAt + 1).some((k) => k === "cat")) {
    failures.push(`causal: catalog and card examples interleave (order: ${c.order.join(",")})`);
  }
  await cat.close();

  if (pageErrors.length) failures.push(`page errors: ${pageErrors.slice(0, 2).join(" | ")}`);

  await browser.close();
  server.close();

  const pass = failures.length === 0;
  console.log(JSON.stringify({
    verdict: pass ? "PASS" : "FAIL",
    note: "card examples: off-catalog remote + local file + catalog supplement, dedupe, zero-row kept",
    remoteCardLabels: r.cardLabels,
    causalCardCount: c.cardLabels.length,
    failures,
  }, null, 2));
  process.exit(pass ? 0 : 1);
};

main().catch((e) => {
  console.log(JSON.stringify({ verdict: "FAIL", error: String(e && e.message).slice(0, 300) }, null, 2));
  process.exit(1);
});
