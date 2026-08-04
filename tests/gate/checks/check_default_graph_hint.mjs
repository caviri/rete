// The empty-default-graph explainer. A real user ran
// SELECT (COUNT(*) AS ?triples) WHERE { ?s ?p ?o } on nkod.rete — whose 2.28M
// quads ALL live in named graphs — got the CORRECT answer 0, and twice
// concluded the page was broken. When (and only when) the loaded file's
// default graph is verifiably empty while named graphs hold data, the result
// panel adds one informational line pointing at GRAPH ?g { … }.
//
// Asserted here, all three fact sources:
//  - resident graph (built in-browser from named-graph-only N-Quads): note shown;
//  - remote lazy file whose own Dataset Card reports triple_count 0 with
//    named_graph_count > 0 (the gate's named-graphs-only fixture): note shown;
//  - remote lazy file WITHOUT a card (named-graphs-only-nocard fixture): the
//    fallback asks the file itself with two first-match ASKs — a cardless
//    remote used to show nothing here, the one gap in the explainer's paths;
// every note must point at BOTH escapes (GRAPH ?g and the ⛁ All graphs
// toggle — the owner reported the note not naming the toggle);
// and the two ways it must NOT fire:
//  - an ordinary file (scholar) where a query legitimately counts 0 — no note;
//  - a query that already names GRAPH on the named-graphs file — no note.
import { createServer } from "node:http";
import { readFile } from "node:fs/promises";
import { launchBrowser } from "./_browser.mjs";

const listen = (server) => new Promise((resolve) => server.listen(0, "127.0.0.1", () => resolve(server.address().port)));

const NQ = `<http://example.test/a> <http://example.test/p> "in graph 1" <http://example.test/g1> .
<http://example.test/b> <http://example.test/p> "in graph 2" <http://example.test/g2> .`;

const COUNT_Q = "SELECT (COUNT(*) AS ?triples) WHERE { ?s ?p ?o }";
const GRAPH_Q = "SELECT ?s WHERE { GRAPH <http://example.test/graph/999> { ?s ?p ?o } }";

const main = async () => {
  // Built by scripts/build_wasm.sh (like card-fixture.rete). Missing = red with
  // a note that says how to produce it, not a silent skip.
  let remoteFixture = null, nocardFixture = null;
  try {
    remoteFixture = await readFile("/work/tests/gate/.cache/named-graphs-only.rete");
    nocardFixture = await readFile("/work/tests/gate/.cache/named-graphs-only-nocard.rete");
  } catch (e) {
    console.log(JSON.stringify({ verdict: "FAIL", error: "tests/gate/.cache/named-graphs-only(.rete|-nocard.rete) missing — run scripts/build_wasm.sh" }));
    process.exit(1);
  }
  const server = createServer((req, res) => {
    const fixture = /nocard/.test(req.url || "") ? nocardFixture : remoteFixture;
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

  // Run a query and wait for ITS result (qmeta repaints per run), then report
  // whether the explainer note is under it. `settleMs` gives the async fact
  // read (ASK / card) time to land — presence waits for it, absence outwaits it.
  const runAndProbe = async (page, q, { expectNote, settleMs = 2500, timeout = 60000 }) => {
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
    if (expectNote) {
      await page.waitForSelector("#out .empty-default-note", { timeout: settleMs + 27500 }).catch(() => {});
    } else {
      await page.waitForTimeout(settleMs);
    }
    return page.evaluate(() => ({
      note: (document.querySelector("#out .empty-default-note") || {}).textContent || "",
      qmeta: (document.getElementById("qmeta") || {}).textContent || "",
      error: !!document.querySelector("#out .error-box"),
      outText: ((document.getElementById("out") || {}).textContent || "").slice(0, 200),
    }));
  };

  // --- resident-graph fact source: build named-graph-only quads in-browser ---
  const mem = await open("#dataset=scholar&mode=sparql");
  await mem.click("#buildBtn");
  await mem.selectOption("#buildFormat", "nq");
  await mem.evaluate((text) => window.PlaygroundEditor.setText("buildText", text), NQ);
  await mem.fill("#cardTitle", "Named graphs only");
  await mem.fill("#cardKey", "named-graphs-only-hint");
  await mem.click("#buildRun");
  await mem.waitForFunction(() => /Saved|Built/.test((document.getElementById("buildOut") || {}).textContent || ""), undefined, { timeout: 30000 });
  await mem.click("#buildOpen");
  await mem.waitForFunction(() => /named graphs only/i.test((document.getElementById("dsName") || {}).textContent || ""), undefined, { timeout: 15000 });

  const memHit = await runAndProbe(mem, COUNT_Q, { expectNote: true });
  if (memHit.error) failures.push(`in-memory count errored: ${memHit.outText}`);
  if (!/default graph is empty/i.test(memHit.note)) {
    failures.push(`in-memory: no explainer over a named-graph-only file (out: "${memHit.outText}")`);
  }
  if (memHit.note && !/GRAPH \?g/.test(memHit.note)) failures.push("in-memory: explainer does not point at GRAPH ?g { … }");
  if (memHit.note && !/All graphs/.test(memHit.note)) failures.push("in-memory: explainer does not point at the ⛁ All graphs toggle");

  // A query that already names GRAPH must NOT get the note, even on this file.
  const memGraphQ = await runAndProbe(mem, GRAPH_Q, { expectNote: false });
  if (memGraphQ.note) failures.push(`in-memory: explainer fired on a GRAPH query: "${memGraphQ.note.slice(0, 80)}"`);

  // Cleanup: the builder saved the fixture dataset into this browser profile;
  // the page is closed with the context, nothing persists across checks.
  await mem.close();

  // --- negative control: ordinary file, legitimately-zero count --------------
  const plain = await open("#dataset=scholar&mode=sparql");
  const plainZero = await runAndProbe(plain, "SELECT (COUNT(*) AS ?n) WHERE { ?s <http://no.such/predicate> ?o }", { expectNote: false });
  if (plainZero.error) failures.push(`scholar zero-count errored: ${plainZero.outText}`);
  if (plainZero.note) failures.push(`scholar: explainer fired on an ordinary file: "${plainZero.note.slice(0, 80)}"`);
  await plain.close();

  // --- remote fact source: the file's own Dataset Card -----------------------
  const remote = await open(`#url=${encodeURIComponent(`http://127.0.0.1:${port}/named-graphs-only.rete`)}`);
  const remoteHit = await runAndProbe(remote, COUNT_Q, { expectNote: true, timeout: 90000 });
  if (remoteHit.error) failures.push(`remote count errored: ${remoteHit.outText}`);
  if (!/default graph is empty/i.test(remoteHit.note)) {
    failures.push(`remote: no explainer over the carded named-graphs-only fixture (out: "${remoteHit.outText}")`);
  }
  if (remoteHit.note && !/3 named graphs/.test(remoteHit.note)) {
    failures.push(`remote: explainer does not carry the card's named-graph count: "${remoteHit.note.slice(0, 120)}"`);
  }
  if (remoteHit.note && !/All graphs/.test(remoteHit.note)) failures.push("remote: explainer does not point at the ⛁ All graphs toggle");
  await remote.close();

  // --- remote WITHOUT a card: the ASK-probe fallback -------------------------
  // The same shape minus the Dataset Card. The explainer must still appear —
  // the fact now comes from two first-match ASKs on the open worker session
  // (a cardless remote used to show nothing at all here). No card also means
  // no count, so the note says "named graphs" without a number.
  const nocard = await open(`#url=${encodeURIComponent(`http://127.0.0.1:${port}/named-graphs-only-nocard.rete`)}`);
  const nocardHit = await runAndProbe(nocard, COUNT_Q, { expectNote: true, timeout: 90000 });
  if (nocardHit.error) failures.push(`nocard remote count errored: ${nocardHit.outText}`);
  if (!/default graph is empty/i.test(nocardHit.note)) {
    failures.push(`nocard remote: no explainer over the CARDLESS named-graphs-only fixture (out: "${nocardHit.outText}")`);
  }
  if (nocardHit.note && !/named graphs/i.test(nocardHit.note)) failures.push(`nocard remote: explainer lost the named-graphs wording: "${nocardHit.note.slice(0, 120)}"`);
  if (nocardHit.note && !/All graphs/.test(nocardHit.note)) failures.push("nocard remote: explainer does not point at the ⛁ All graphs toggle");
  await nocard.close();

  if (pageErrors.length) failures.push(`page errors: ${pageErrors.slice(0, 2).join(" | ")}`);

  await browser.close();
  server.close();

  const pass = failures.length === 0;
  console.log(JSON.stringify({
    verdict: pass ? "PASS" : "FAIL",
    note: "empty-default-graph explainer: shown for named-graph-only files (resident ASK + remote card + cardless-remote ASK fallback), points at GRAPH ?g AND ⛁ All graphs, absent for ordinary zero results and GRAPH queries",
    memNote: memHit.note.slice(0, 100),
    remoteNote: remoteHit.note.slice(0, 100),
    nocardNote: nocardHit.note.slice(0, 100),
    failures,
  }, null, 2));
  process.exit(pass ? 0 : 1);
};

main().catch((e) => {
  console.log(JSON.stringify({ verdict: "FAIL", error: String(e && e.message).slice(0, 300) }, null, 2));
  process.exit(1);
});
