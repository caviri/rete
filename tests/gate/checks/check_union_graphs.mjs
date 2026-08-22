// The ⛁ All graphs (union default graph) toggle.
//
// SPARQL says a pattern outside GRAPH matches the DEFAULT graph; on a file
// whose quads all live in named graphs, 0 rows is the CORRECT standard answer
// (the W3C conformance suite runs with the toggle off and must stay green).
// The toggle opts into the union-default-graph mode Virtuoso / GraphDB / Jena
// TDB offer: the file is mounted as if the default graph were the union of the
// default graph and every named graph. Non-negotiables asserted here:
//  - OFF BY DEFAULT: the fresh page has the switch unchecked and answers 0;
//  - CHANGES RESULTS when on: the same query answers the union (6 rows on the
//    named-graphs-only fixture), over BOTH engines — the remote lazy worker
//    and the resident in-memory graph;
//  - ANNOUNCED: flipping it posts a note, and every run under it stamps the
//    result meta ("union default graph");
//  - the empty-default-graph explainer NEVER shows while the toggle is on
//    (with union semantics that message would be wrong);
//  - GRAPH ?g still enumerates named graphs with the toggle on;
//  - turning it off restores the standard 0;
//  - TRANSFER FIGURES stay truthful on the remote path (reported: "the bytes
//    fetched log doesn't get updated which makes it looks like an error"):
//    the first query's final line counts the session OPEN's fetches instead of
//    contradicting the live counter with "0 range req", and any 0-request run
//    names the session cache it ran on, so a zero reads as the cache working;
//  - the ⛁ All graphs and 🏷 Labels switches each have a ? help affordance
//    opening their modal (same conventions as the Strategy/Reason modals:
//    × close, backdrop, Escape);
//  - the ⊞ Range-requests inspector carries byte offsets on EVERY row (the
//    column used to render "—" always — offsets were dropped at the progress
//    hook) and its Copy button puts a self-sufficient debug report (offsets +
//    query + file) on the clipboard.
import { createServer } from "node:http";
import { readFile } from "node:fs/promises";
import { launchBrowser } from "./_browser.mjs";

const listen = (server) => new Promise((resolve) => server.listen(0, "127.0.0.1", () => resolve(server.address().port)));

const PLAIN_Q = "SELECT ?s ?p ?o WHERE { ?s ?p ?o }";
const GRAPH_Q = "SELECT ?g ?s WHERE { GRAPH ?g { ?s ?p ?o } }";

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

  // Run a query and wait for its qmeta; settleMs lets the (async) explainer
  // fact-read land so its absence is meaningful.
  const run = async (page, q, { settleMs = 2500, timeout = 90000 } = {}) => {
    await page.evaluate((query) => {
      document.getElementById("qmeta").textContent = "";
      window.PlaygroundEditor.setText("q", query);
      document.getElementById("run").click();
    }, q);
    await page.waitForFunction(
      () => /row\(s\)|ASK/.test((document.getElementById("qmeta") || {}).textContent || "") ||
            document.querySelector("#out .error-box"),
      undefined,
      { timeout },
    );
    await page.waitForTimeout(settleMs);
    return page.evaluate(() => ({
      qmeta: (document.getElementById("qmeta") || {}).textContent || "",
      error: !!document.querySelector("#out .error-box"),
      outText: ((document.getElementById("out") || {}).textContent || "").slice(0, 300),
      explainer: !!document.querySelector("#out .empty-default-note"),
    }));
  };

  // Remote transfer figures must never contradict themselves: a "0 range req"
  // line is honest only when it names the session cache the run answered from
  // (otherwise a run that visibly fetched — or merged for seconds — reads as a
  // fetch that never happened, the reported "looks like an error").
  const auditTransfer = (label, qmeta) => {
    if (!/range req/.test(qmeta)) { failures.push(`${label}: remote result meta has no transfer figures: "${qmeta.slice(0, 100)}"`); return; }
    if (/\b0 range req/.test(qmeta) && !/session's cache \(/.test(qmeta)) {
      failures.push(`${label}: a 0-request run does not name the session cache it ran on: "${qmeta.slice(0, 140)}"`);
    }
  };

  const exercise = async (page, label) => {
    const isRemote = label === "remote";
    // Off by default — a semantics switch must never arrive flipped.
    const def = await page.evaluate(() => {
      const u = document.getElementById("unionGraphs");
      return { present: !!u, checked: !!(u && u.checked), visible: !!(u && u.closest("label")) };
    });
    if (!def.present) { failures.push(`${label}: no #unionGraphs switch in the page`); return; }
    if (def.checked) failures.push(`${label}: the union toggle is ON by default`);

    // Standard semantics: the empty default graph answers 0.
    const off = await run(page, PLAIN_Q);
    if (off.error) failures.push(`${label}: baseline query errored: ${off.outText}`);
    if (!/\b0 row/.test(off.qmeta)) failures.push(`${label}: expected the standard 0 rows with the toggle off, qmeta: "${off.qmeta.slice(0, 80)}"`);
    if (/union default graph/i.test(off.qmeta)) failures.push(`${label}: the OFF run claims union semantics`);
    if (isRemote) {
      // FIRST query of a fresh worker session: opening the file fetched real
      // ranges (the live counter showed them), so the final line must count
      // them too — it used to say "0 range req … served from cache" here.
      auditTransfer(`${label} (baseline)`, off.qmeta);
      if (!/[1-9]\d* range req/.test(off.qmeta) || !/incl\. opening the file/.test(off.qmeta)) {
        failures.push(`${label}: the first query's final line hides the session open's fetches: "${off.qmeta.slice(0, 140)}"`);
      }
      // …and its fetch log must carry byte offsets + a copyable debug report
      // (audited here, adjacent to the run that produced the log).
      await auditReqLog(page, label);
    }

    // Flip it on: announced immediately…
    await page.check("#unionGraphs");
    const announced = await page.evaluate(() => !!document.querySelector("#out .union-note"));
    if (!announced) failures.push(`${label}: flipping the toggle posted no announcement note`);

    // …and the SAME query now answers the union (6 quads across 3 graphs),
    // stamped as non-standard in the result meta, with the empty-default-graph
    // explainer suppressed (it would be wrong under union semantics).
    const on = await run(page, PLAIN_Q);
    if (on.error) failures.push(`${label}: union run errored: ${on.outText}`);
    if (!/\b6 row/.test(on.qmeta)) failures.push(`${label}: expected 6 union rows, qmeta: "${on.qmeta.slice(0, 100)}"`);
    if (!/union default graph/i.test(on.qmeta)) failures.push(`${label}: the union run does not announce itself in the result meta`);
    if (on.explainer) failures.push(`${label}: the empty-default-graph explainer showed WHILE union was on`);
    if (isRemote) auditTransfer(`${label} (union)`, on.qmeta);

    // A SECOND union run answers entirely from the warm session (the tiny
    // fixture is fully cached by now): the transfer figures must say so
    // rather than sit at a bare unexplained zero.
    if (isRemote) {
      const warm = await run(page, PLAIN_Q, { settleMs: 300 });
      if (!/\b6 row/.test(warm.qmeta)) failures.push(`${label}: warm union re-run lost the union rows, qmeta: "${warm.qmeta.slice(0, 100)}"`);
      auditTransfer(`${label} (warm union)`, warm.qmeta);
      if (!/0 new bytes/.test(warm.qmeta) || !/session's cache \(/.test(warm.qmeta)) {
        failures.push(`${label}: warm union re-run does not explain its zero transfer: "${warm.qmeta.slice(0, 140)}"`);
      }
    }

    // GRAPH ?g must still see the named graphs with the toggle on.
    const g = await run(page, GRAPH_Q, { settleMs: 300 });
    if (!/\b6 row/.test(g.qmeta)) failures.push(`${label}: GRAPH ?g broke under the toggle, qmeta: "${g.qmeta.slice(0, 80)}"`);

    // Off again: the standard 0 comes back (no sticky state).
    await page.uncheck("#unionGraphs");
    const back = await run(page, PLAIN_Q, { settleMs: 300 });
    if (!/\b0 row/.test(back.qmeta)) failures.push(`${label}: toggling off did not restore standard semantics, qmeta: "${back.qmeta.slice(0, 80)}"`);
  };

  // A ? help affordance beside a toolbar switch — same conventions as the
  // Strategy/Reason modals: opens the modal, closes on ×, Escape, backdrop.
  // Shared by ⛁ All graphs (#unionHelp/#unionModal) and 🏷 Labels
  // (#labelsHelp/#labelsModal).
  const exerciseHelp = async (page, label, { name, helpId, modalId, closeId, musts }) => {
    const present = await page.evaluate(
      ([h, m]) => !!document.getElementById(h) && !!document.getElementById(m), [helpId, modalId]);
    if (!present) { failures.push(`${label}: no #${helpId} button / #${modalId} in the page`); return; }
    const openState = async () => page.evaluate((m) => !document.getElementById(m).classList.contains("hidden"), modalId);
    await page.click(`#${helpId}`);
    if (!(await openState())) { failures.push(`${label}: clicking ? did not open the ${name} modal`); return; }
    const text = await page.evaluate((m) => document.getElementById(m).textContent || "", modalId);
    for (const must of musts) {
      if (!text.includes(must)) failures.push(`${label}: ${name} modal is missing "${must}"`);
    }
    await page.click(`#${closeId}`);
    if (await openState()) failures.push(`${label}: × did not close the ${name} modal`);
    await page.click(`#${helpId}`);
    await page.keyboard.press("Escape");
    if (await openState()) failures.push(`${label}: Escape did not close the ${name} modal`);
    await page.click(`#${helpId}`);
    // Same technique as check_load_modal: a click whose target IS the modal
    // element (the backdrop), not a descendant — deterministic, no hit-testing.
    await page.evaluate((m) => document.getElementById(m).dispatchEvent(
      new MouseEvent("click", { bubbles: true })), modalId);
    if (await openState()) failures.push(`${label}: backdrop click did not close the ${name} modal`);
  };

  const HELP_MODALS = [
    { name: "All graphs", helpId: "unionHelp", modalId: "unionModal", closeId: "unionModalClose",
      musts: ["union of the default graph and every named graph", "Virtuoso", "standard SPARQL",
              "FROM", "live SPARQL endpoints", "materializes the merged index"] },
    { name: "Labels", helpId: "labelsHelp", modalId: "labelsModal", closeId: "labelsModalClose",
      musts: ["editor decorations", "rdfs:label", "Label property", "range requests"] },
  ];

  // The ⊞ requests inspector after a remote run: every row must carry its byte
  // offsets (the column used to render "—" on every row — the offsets were
  // dropped at the progress hook), and the Copy affordance must put a
  // self-sufficient debug report (offsets + query + file) on the clipboard.
  const auditReqLog = async (page, label) => {
    const btnVisible = await page.evaluate(() => {
      const b = document.getElementById("reqLogBtn");
      return !!b && !b.classList.contains("hidden");
    });
    if (!btnVisible) { failures.push(`${label}: no ⊞ requests button after a remote run`); return; }
    await page.click("#reqLogBtn");
    const modal = await page.evaluate(() => ({
      open: !document.getElementById("reqModal").classList.contains("hidden"),
      cells: Array.from(document.querySelectorAll("#reqLogBody tbody td.mono")).map((c) => c.textContent || ""),
      copyBtn: !!document.querySelector("#reqLogBody .err-copy"),
      report: (document.querySelector("#reqLogBody .err-tech-body") || {}).textContent || "",
    }));
    if (!modal.open) { failures.push(`${label}: ⊞ requests did not open the modal`); return; }
    if (!modal.cells.length) failures.push(`${label}: request log table has no rows`);
    const missing = modal.cells.filter((c) => !/\d+-\d+/.test(c));
    if (missing.length) {
      failures.push(`${label}: ${missing.length} of ${modal.cells.length} log rows carry NO byte offsets (the "—" column bug): ${JSON.stringify(modal.cells.slice(0, 3))}`);
    }
    if (!modal.copyBtn) failures.push(`${label}: request log has no Copy affordance`);
    if (!/remote fetch log/.test(modal.report)) failures.push(`${label}: debug report missing its header`);
    if (!/\d+-\d+/.test(modal.report)) failures.push(`${label}: debug report carries no byte offsets`);
    if (!/SELECT \?s \?p \?o/i.test(modal.report)) failures.push(`${label}: debug report does not carry the query`);
    if (!/127\.0\.0\.1/.test(modal.report)) failures.push(`${label}: debug report does not name the file`);
    // Copy actually lands on the clipboard — the SAME shared helper (and
    // fallback) the error box uses, so only the landing needs asserting here.
    await page.context().grantPermissions(["clipboard-read", "clipboard-write"], { origin: `http://localhost:${PGPORT}` });
    await page.click("#reqLogBody .err-copy");
    await page.waitForTimeout(400);
    const clip = await page.evaluate(() => navigator.clipboard.readText().catch(() => "READ_FAILED"));
    if (!/remote fetch log/.test(clip) || !/\d+-\d+/.test(clip)) {
      failures.push(`${label}: clipboard does not hold the fetch log (got: "${String(clip).slice(0, 60)}")`);
    }
    await page.keyboard.press("Escape"); // close the modal before moving on
  };

  // ---- remote lazy path (the worker engine — asyncify default in Chromium) --
  const remote = await open(`#url=${encodeURIComponent(`http://127.0.0.1:${port}/union.rete`)}`);
  await exercise(remote, "remote");
  for (const spec of HELP_MODALS) await exerciseHelp(remote, "remote", spec);
  await remote.close();

  // ---- resident path (Graph.query_opts on the in-memory engine) -------------
  const local = await open("#dataset=scholar&mode=sparql");
  await local.setInputFiles("#loadFileInput", "/work/tests/gate/.cache/named-graphs-only.rete");
  await local.waitForFunction(
    () => /custom file/.test((document.getElementById("meta") || {}).textContent || "") &&
          /Local file/i.test((document.getElementById("dsName") || {}).textContent || ""),
    undefined,
    { timeout: 30000 },
  );
  await exercise(local, "resident");
  await local.close();

  if (pageErrors.length) failures.push(`page errors: ${pageErrors.slice(0, 2).join(" | ")}`);

  await browser.close();
  server.close();

  const pass = failures.length === 0;
  console.log(JSON.stringify({
    verdict: pass ? "PASS" : "FAIL",
    note: "union default graph toggle: off by default, 0→6 rows when on (remote + resident), announced, explainer suppressed, GRAPH intact, reversible; remote transfer figures truthful (open counted, zero runs name the cache); req-log rows carry offsets + copyable report; ⛁/🏷 help modals open/close",
    failures,
  }, null, 2));
  process.exit(pass ? 0 : 1);
};

main().catch((e) => {
  console.log(JSON.stringify({ verdict: "FAIL", error: String(e && e.message).slice(0, 300) }, null, 2));
  process.exit(1);
});
