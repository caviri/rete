// A deep link must reproduce the VIEW, not just the graph and the query.
//
// updateHash() used to emit dataset/url, endpoint, load, mode and q/ex — and
// nothing else. Everything in the toolbar beside them was dropped, so someone
// could flip ⛁ All graphs, get an answer, press Share, and the recipient opened
// STANDARD SPARQL SEMANTICS and saw different results from the same link. Same
// class of defect as #148 (the link named a catalog dataset while an off-catalog
// file was open): a link that claims to reproduce a view it does not.
//
// What this asserts, and the distinction it is built around:
//
//   ANSWER-AFFECTING (union, reason, strategy, round, fed) — a dropped one makes
//   the link LIE. So for the two that can be proved cheaply and locally, this
//   check does not merely compare checkbox states: it asserts THE RESULTS
//   ACTUALLY DIFFER, because that is the entire point.
//     · union   — named-graphs-only fixture: 0 rows standard, 6 rows union.
//     · reason  — embedded `causal` ontology: `?x a ex:Factor` is 0 rows without
//                 OWL QL rewriting and 30 with it (nothing is typed ex:Factor
//                 directly; every instance reaches it through subClassOf).
//   plus strategy/round/fed state round-trips.
//
//   PRESENTATIONAL (view, labels) — cannot make a link lie about data; asserted
//   as state round-trips only.
//
// And three things that are easy to break while adding parameters:
//   · the DEFAULT-view hash must be byte-for-byte what it was (a hash carrying
//     six "=0"s would make the common case worse than before);
//   · the ex=N short link must still win over the full q= form;
//   · a generated share page (docs/d/…, docs/q/…) forwards to a hash built from
//     the CATALOG alone, so it cannot carry view state — with any view-state
//     param present, Share must hand out the DEEP link instead.
import { createServer } from "node:http";
import { readFile } from "node:fs/promises";
import { launchBrowser } from "./_browser.mjs";

const listen = (server) => new Promise((resolve) => server.listen(0, "127.0.0.1", () => resolve(server.address().port)));

const PLAIN_Q = "SELECT ?s ?p ?o WHERE { ?s ?p ?o }";
// Nothing in causal.nt is typed ex:Factor; RiskFactor / Condition / Treatment /
// Symptom / Outcome are its subclasses (and Disease a subclass of Condition), so
// this is exactly 0 without reasoning and > 0 with it.
const FACTOR_Q = "SELECT ?x WHERE { ?x a <http://ex/Factor> }";

// The parameters under test — also the list shareableUrl() refuses to hand to a
// generated share page.
const VIEW_STATE_PARAMS = ["union", "reason", "strategy", "round", "fed", "view", "labels"];

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
  const fixtureUrl = `http://127.0.0.1:${port}/union.rete`;

  const PGPORT = process.env.PGPORT || "8090";
  const browser = await launchBrowser();
  const failures = [];
  const pageErrors = [];
  const evidence = {};

  // #run is never disabled in the markup, so "run is enabled" says nothing about
  // boot. `load=` only appears in the hash once a dataset is actually open, and
  // applyViewState() runs immediately after — so wait for that, then for the
  // specific restored state each case cares about.
  const open = async (hash) => {
    const page = await browser.newPage();
    page.on("pageerror", (e) => pageErrors.push(String(e).slice(0, 200)));
    await page.goto(`http://localhost:${PGPORT}/playground.html${hash}`, { waitUntil: "domcontentloaded" });
    await page.waitForFunction(
      () => window.PlaygroundEditor && /[#&]load=/.test(location.hash),
      undefined,
      { timeout: 90000 },
    );
    return page;
  };

  const settledHash = (page) => page.evaluate(() => location.hash);

  const run = async (page, q, { timeout = 90000, settleMs = 400 } = {}) => {
    await page.evaluate((query) => {
      document.getElementById("qmeta").textContent = "";
      if (query != null) window.PlaygroundEditor.setText("q", query);
      document.getElementById("run").click();
    }, q);
    await page.waitForFunction(
      () => /row\(s\)|ASK/.test((document.getElementById("qmeta") || {}).textContent || "") ||
            document.querySelector("#out .error-box"),
      undefined,
      { timeout },
    );
    await page.waitForTimeout(settleMs);
    return page.evaluate(() => {
      const qmeta = (document.getElementById("qmeta") || {}).textContent || "";
      const m = qmeta.match(/(\d+)\s+row/);
      return {
        qmeta,
        rows: m ? Number(m[1]) : -1,
        error: !!document.querySelector("#out .error-box"),
        outText: ((document.getElementById("out") || {}).textContent || "").slice(0, 200),
      };
    });
  };

  const toolbar = (page) => page.evaluate(() => {
    const el = (id) => document.getElementById(id);
    return {
      union: !!(el("unionGraphs") && el("unionGraphs").checked),
      reason: !!(el("owlReason") && el("owlReason").checked),
      labels: !!(el("decodeToggle") && el("decodeToggle").checked),
      strategy: (el("strategy") || {}).value || "",
      round: (el("round") || {}).value || "",
      view: (el("fmt") || {}).value || "",
      roundVisible: !!(el("roundWrap") && !el("roundWrap").classList.contains("hidden")),
      unionNote: !!document.querySelector("#out .union-note"),
      fedChips: Array.from(document.querySelectorAll("#fedChips .fed-chip:not(.fed-self) .fed-chip-name")).map((n) => n.textContent.trim()),
      runLabel: ((el("run") || {}).textContent || "").trim(),
    };
  });

  // ── 1. the default-view hash is byte-for-byte what it was ──────────────────
  // These strings were captured from the build BEFORE the view-state parameters
  // existed and are asserted verbatim: a hash carrying six "=0"s would make the
  // ordinary link — by far the common case — worse than it was, so "only emit a
  // non-default" has to be a tested property, not an intention. A failure here
  // means a new parameter started leaking into a plain view.
  const DEFAULT_LINKS = [
    ["", "#dataset=scholar&load=bundled&mode=sparql&ex=0"],
    ["#dataset=causal&mode=sparql", "#dataset=causal&load=bundled&mode=sparql&ex=0"],
    ["#dataset=causal&mode=sparql&ex=1", "#dataset=causal&load=bundled&mode=sparql&ex=1"],
    ["#dataset=scholar&mode=shacl", "#dataset=scholar&load=bundled&mode=shacl&ex=0"],
  ];
  evidence.defaultHashes = [];
  for (const [link, want] of DEFAULT_LINKS) {
    const page = await open(link);
    await page.waitForTimeout(600);           // let any late boot step re-stamp it
    const got = await settledHash(page);
    evidence.defaultHashes.push({ link, got });
    if (got !== want) failures.push(`default link ${JSON.stringify(link)} settled on ${JSON.stringify(got)}, expected ${JSON.stringify(want)}`);
    await page.close();
  }

  // ── 2. ⛁ union: the results differ, and the link carries the difference ─────
  // (a) the plain link: standard semantics, the correct 0 on a file whose quads
  //     all live in named graphs.
  const uBase = `#url=${encodeURIComponent(fixtureUrl)}`;
  const uPage = await open(uBase);
  const uOff = await run(uPage, PLAIN_Q);
  if (uOff.rows !== 0) failures.push(`union: plain link expected 0 rows, got ${uOff.rows} ("${uOff.qmeta.slice(0, 90)}")`);
  const uOffHash = await settledHash(uPage);
  if (/[#&]union=/.test(uOffHash)) failures.push(`union: an OFF toggle emitted a union param: ${uOffHash.slice(0, 120)}`);

  // (b) flip it, re-run, and read the link the page now offers.
  await uPage.check("#unionGraphs");
  const uOn = await run(uPage, PLAIN_Q);
  if (uOn.rows !== 6) failures.push(`union: expected 6 union rows in the source view, got ${uOn.rows} ("${uOn.qmeta.slice(0, 90)}")`);
  const uOnHash = await settledHash(uPage);
  if (!/[#&]union=1(&|$)/.test(uOnHash)) failures.push(`union: the shared link does not carry union=1: ${uOnHash.slice(0, 160)}`);
  await uPage.close();

  // (c) the recipient's page: the toggle comes back, the non-standard mounting is
  //     ANNOUNCED (they did not throw the switch), and the SAME query answers 6
  //     where the plain link answered 0.
  const uRestored = await open(uOnHash);
  await uRestored.waitForFunction(
    () => !!(document.getElementById("unionGraphs") || {}).checked, undefined, { timeout: 30000 },
  ).catch(() => failures.push("union: the toggle did not come back from the link"));
  const uState = await toolbar(uRestored);
  if (!uState.union) failures.push("union: restored page has the toggle OFF");
  if (!uState.unionNote) failures.push("union: restoring a non-standard mounting posted no announcement");
  const uAgain = await run(uRestored, PLAIN_Q);
  if (uAgain.rows !== 6) failures.push(`union: the restored link answered ${uAgain.rows} rows, expected 6 (the plain link answers 0)`);
  if (!/union default graph/i.test(uAgain.qmeta)) failures.push("union: the restored run does not stamp the result meta");
  await uRestored.close();
  evidence.union = { plainLinkRows: uOff.rows, sourceViewRows: uOn.rows, restoredLinkRows: uAgain.rows, hash: uOnHash };

  // ── 3. 🧠 reason: same proof on the embedded causal ontology (no network) ────
  const rBase = `#dataset=causal&mode=sparql&q=${encodeURIComponent(FACTOR_Q)}`;
  const rPage = await open(rBase);
  const rOff = await run(rPage, null);
  if (rOff.rows !== 0) failures.push(`reason: plain link expected 0 rows, got ${rOff.rows} ("${rOff.qmeta.slice(0, 90)}")`);
  const rOffHash = await settledHash(rPage);
  if (/[#&]reason=/.test(rOffHash)) failures.push(`reason: an OFF toggle emitted a reason param: ${rOffHash.slice(0, 120)}`);

  await rPage.check("#owlReason");
  const rOn = await run(rPage, null);
  if (!(rOn.rows > 0)) failures.push(`reason: expected entailed rows in the source view, got ${rOn.rows} ("${rOn.qmeta.slice(0, 90)}")`);
  const rOnHash = await settledHash(rPage);
  if (!/[#&]reason=1(&|$)/.test(rOnHash)) failures.push(`reason: the shared link does not carry reason=1: ${rOnHash.slice(0, 160)}`);
  await rPage.close();

  const rRestored = await open(rOnHash);
  await rRestored.waitForFunction(
    () => !!(document.getElementById("owlReason") || {}).checked, undefined, { timeout: 30000 },
  ).catch(() => failures.push("reason: the toggle did not come back from the link"));
  const rAgain = await run(rRestored, null);
  if (rAgain.rows !== rOn.rows) failures.push(`reason: the restored link answered ${rAgain.rows} rows, the view it was copied from answered ${rOn.rows}`);
  if (rAgain.rows === rOff.rows) failures.push(`reason: the restored link answered the SAME ${rAgain.rows} rows as the un-reasoned link — the parameter changed nothing`);
  await rRestored.close();
  evidence.reason = { plainLinkRows: rOff.rows, sourceViewRows: rOn.rows, restoredLinkRows: rAgain.rows, hash: rOnHash };

  // ── 4. strategy + round ─────────────────────────────────────────────────────
  // Answer-affecting: `progressive` answers from the pyramid summary and is
  // approximate BY CONTRACT, so a link that drops it hands over exact-looking
  // numbers computed a different way.
  const sPage = await open("#dataset=causal&mode=sparql");
  await sPage.selectOption("#strategy", "progressive");
  const sHash = await settledHash(sPage);
  if (!/[#&]strategy=progressive(&|$)/.test(sHash)) failures.push(`strategy: not carried after selecting progressive: ${sHash.slice(0, 160)}`);
  if (/[#&]round=/.test(sHash)) failures.push(`strategy: a round rode along with a non-community strategy: ${sHash.slice(0, 160)}`);
  // community + a round: both must travel, and only together.
  await sPage.selectOption("#strategy", "community");
  await sPage.fill("#round", "2");
  await sPage.waitForTimeout(300);
  const cHash = await settledHash(sPage);
  if (!/[#&]strategy=community(&|$)/.test(cHash)) failures.push(`strategy: community not carried: ${cHash.slice(0, 160)}`);
  if (!/[#&]round=2(&|$)/.test(cHash)) failures.push(`strategy: the community round is not carried: ${cHash.slice(0, 160)}`);
  await sPage.close();

  const sRestored = await open(cHash);
  await sRestored.waitForFunction(
    () => (document.getElementById("strategy") || {}).value === "community", undefined, { timeout: 30000 },
  ).catch(() => failures.push("strategy: community did not come back from the link"));
  const sState = await toolbar(sRestored);
  if (sState.strategy !== "community") failures.push(`strategy: restored "${sState.strategy}", expected community`);
  if (sState.round !== "2") failures.push(`strategy: restored round "${sState.round}", expected 2`);
  if (!sState.roundVisible) failures.push("strategy: the Round input stayed hidden after restoring the community strategy");
  await sRestored.close();
  evidence.strategy = { hash: cHash, restored: { strategy: sState.strategy, round: sState.round } };

  // ── 5. federation: catalog keys travel, and the restored view really is
  //      federated (the Run button is the observable proof). ──────────────────
  const fPage = await open("#dataset=causal&mode=sparql");
  await fPage.evaluate(() => document.getElementById("fedAdd").click());
  await fPage.selectOption("#fedCatalog", "scholar");
  await fPage.uncheck("#fedCatalogLazy");   // in-memory: no network in --local
  await fPage.evaluate(() => document.getElementById("fedAddConfirm").click());
  await fPage.waitForTimeout(300);
  const fHash = await settledHash(fPage);
  if (!/[#&]fed=scholar(&|$)/.test(fHash)) failures.push(`fed: a catalog partner is not carried: ${fHash.slice(0, 160)}`);
  await fPage.close();

  const fRestored = await open(fHash);
  await fRestored.waitForFunction(
    () => /Run federated/.test((document.getElementById("run") || {}).textContent || ""), undefined, { timeout: 30000 },
  ).catch(() => failures.push("fed: the restored page is not in federated mode"));
  const fState = await toolbar(fRestored);
  if (fState.fedChips.length !== 1) failures.push(`fed: restored ${fState.fedChips.length} partner chips, expected 1 (${JSON.stringify(fState.fedChips)})`);
  await fRestored.close();
  evidence.fed = { hash: fHash, chips: fState.fedChips, runLabel: fState.runLabel };

  // ── 6. an AD-HOC federation address is deliberately NOT in the link, and
  //      Share says so rather than handing out a quietly-narrower view. ───────
  const SECRET = "https://internal.example.invalid/staging.rete?token=hunter2";
  const aPage = await open("#dataset=causal&mode=sparql");
  await aPage.evaluate(() => document.getElementById("fedAdd").click());
  await aPage.evaluate(() => document.querySelector('#fedModes button[data-fedmode="link"]').click());
  await aPage.fill("#fedLinkUrl", SECRET);
  await aPage.evaluate(() => document.getElementById("fedAddConfirm").click());
  await aPage.context().grantPermissions(["clipboard-read", "clipboard-write"], { origin: `http://localhost:${PGPORT}` });
  await aPage.evaluate(() => document.getElementById("shareBtn").click());
  await aPage.waitForTimeout(400);
  const adhoc = await aPage.evaluate(() => ({
    hash: location.hash,
    qmeta: (document.getElementById("qmeta") || {}).textContent || "",
  }));
  if (adhoc.hash.includes("internal.example.invalid") || adhoc.hash.includes("hunter2")) {
    failures.push(`fed: a pasted address LEAKED into the share link: ${adhoc.hash.slice(0, 160)}`);
  }
  if (!/WITHOUT/.test(adhoc.qmeta) || !/staging/.test(adhoc.qmeta)) {
    failures.push(`fed: Share did not say the pasted source is missing from the link: "${adhoc.qmeta.slice(0, 160)}"`);
  }
  await aPage.close();
  evidence.adHocFed = adhoc;

  // ── 7. presentational round-trips: view + labels ────────────────────────────
  const pPage = await open("#dataset=causal&mode=sparql&ex=0");
  await pPage.selectOption("#fmt", "cards");
  await pPage.uncheck("#decodeToggle");
  await pPage.waitForTimeout(500);
  const pHash = await settledHash(pPage);
  if (!/[#&]view=cards(&|$)/.test(pHash)) failures.push(`view: not carried: ${pHash.slice(0, 160)}`);
  if (!/[#&]labels=0(&|$)/.test(pHash)) failures.push(`labels: OFF not carried: ${pHash.slice(0, 160)}`);
  // The short form must survive the extra parameters.
  if (!/[#&]ex=0(&|$)/.test(pHash)) failures.push(`view: the ex=N short link was lost once view state was added: ${pHash.slice(0, 160)}`);
  if (/[#&]q=/.test(pHash)) failures.push(`view: an unedited example fell back to the full q= form: ${pHash.slice(0, 160)}`);
  await pPage.close();

  const pRestored = await open(pHash);
  await pRestored.waitForFunction(
    () => (document.getElementById("fmt") || {}).value === "cards", undefined, { timeout: 30000 },
  ).catch(() => failures.push("view: cards did not come back from the link"));
  const pState = await toolbar(pRestored);
  if (pState.view !== "cards") failures.push(`view: restored "${pState.view}", expected cards`);
  if (pState.labels) failures.push("labels: restored ON, the link said OFF");
  // Restoring a view must not have run the query behind the user's back (#fmt's
  // change handler runs it; applyViewState assigns the value instead).
  const ran = await pRestored.evaluate(() => /row\(s\)|ASK/.test((document.getElementById("qmeta") || {}).textContent || ""));
  if (ran) failures.push("view: restoring the output type auto-ran the query");
  await pRestored.close();
  evidence.presentation = { hash: pHash, restored: { view: pState.view, labels: pState.labels } };

  // ── 8. share pages: still preferred for a plain view, refused for one that
  //      carries view state (their forwarding hash comes from the catalog). ───
  const shPage = await open("#dataset=causal&mode=sparql&ex=0");
  await shPage.context().grantPermissions(["clipboard-read", "clipboard-write"], { origin: `http://localhost:${PGPORT}` });
  await shPage.evaluate(() => document.getElementById("shareBtn").click());
  await shPage.waitForTimeout(400);
  const plainShare = await shPage.evaluate(() => navigator.clipboard.readText().catch(() => "READ_FAILED"));
  if (!/\/q\/causal-0\.html$/.test(plainShare)) failures.push(`share page: a plain example view no longer shares its preview page (got "${plainShare.slice(0, 120)}")`);
  await shPage.check("#unionGraphs");
  await shPage.evaluate(() => document.getElementById("shareBtn").click());
  await shPage.waitForTimeout(400);
  const stateShare = await shPage.evaluate(() => navigator.clipboard.readText().catch(() => "READ_FAILED"));
  if (!/playground\.html#/.test(stateShare) || !/union=1/.test(stateShare)) {
    failures.push(`share page: a view carrying union was shared as a preview page that cannot express it (got "${stateShare.slice(0, 160)}")`);
  }
  await shPage.close();
  evidence.sharePage = { plainShare, stateShare };

  // ── 9. an unknown / hostile parameter value must be ignored, not applied ────
  const hostile = await open("#dataset=causal&mode=sparql&strategy=../../etc&view=<script>&round=99x&union=maybe");
  const hState = await toolbar(hostile);
  if (hState.strategy !== "whole") failures.push(`hostile: strategy became "${hState.strategy}"`);
  if (hState.view !== "table") failures.push(`hostile: view became "${hState.view}"`);
  if (hState.round !== "") failures.push(`hostile: round became "${hState.round}"`);
  if (hState.union) failures.push("hostile: union=maybe turned the toggle on");
  await hostile.close();

  if (pageErrors.length) failures.push(`page errors: ${pageErrors.slice(0, 2).join(" | ")}`);

  await browser.close();
  server.close();

  const pass = failures.length === 0;
  console.log(JSON.stringify({
    verdict: pass ? "PASS" : "FAIL",
    note: "deep links carry view state: union/reason change the ANSWER and round-trip (0→6 and 0→N, proved on both ends); strategy+round, catalog federation, view and labels round-trip; ad-hoc federation addresses stay out of the link and Share says so; the default-view hash is unchanged; share pages refused for a view they cannot express",
    evidence,
    failures,
  }, null, 2));
  process.exit(pass ? 0 : 1);
};

main().catch((e) => {
  console.log(JSON.stringify({ verdict: "FAIL", error: String(e && e.stack || e).slice(0, 500) }, null, 2));
  process.exit(1);
});
