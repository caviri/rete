// A host can report a length that is WRONG and hide the one that is right.
//
// GitHub Pages does exactly this (issue #95): browsers send Accept-Encoding:
// gzip, so HEAD answers with the COMPRESSED size (58,083,308 for a 71,237,191
// byte file), and `Content-Range` — which carries the true total — is invisible
// to JS because no Access-Control-Expose-Headers is sent. `Content-Encoding` is
// not CORS-safelisted either, so a page cannot even tell that the number it got
// is a compressed one. The reader believed it, and every read past that point
// died as "io: range out of bounds".
//
// The fix derives the length from the .rete itself. This check pins it against a
// server that reproduces the pathology exactly, so the regression cannot come
// back through a host we do not control.
import { createServer } from "node:http";
import { readFile } from "node:fs/promises";
import { launchBrowser } from "./_browser.mjs";
import { runWithRetry } from "./_util.mjs";

const listen = (s) => new Promise((r) => s.listen(0, "127.0.0.1", () => r(s.address().port)));

const main = async () => {
  const fixture = await readFile("/work/tests/gate/.cache/card-fixture.rete");
  // The lie: 60% of the real size, as a compressed length would be. Every read
  // past it used to be rejected before a single byte was misparsed.
  const LIE = Math.floor(fixture.length * 0.6);
  const seen = { head: 0, range: 0 };

  const server = createServer((req, res) => {
    if (req.url?.split("?")[0] !== "/hostile.rete") { res.writeHead(404); res.end(); return; }
    // Deliberately NO Access-Control-Expose-Headers: Content-Range goes on the
    // wire but stays unreadable from JS, exactly as on GitHub Pages.
    const cors = { "Access-Control-Allow-Origin": "*", "Accept-Ranges": "bytes" };
    if ((req.method || "GET").toUpperCase() === "HEAD") {
      seen.head++;
      res.writeHead(200, { ...cors, "Content-Length": LIE });
      res.end();
      return;
    }
    const m = req.headers.range && /bytes=(\d+)-(\d*)/.exec(req.headers.range);
    if (m) {
      seen.range++;
      const start = Number(m[1]);
      if (start >= fixture.length) { res.writeHead(416, cors); res.end(); return; }
      const end = m[2] ? Math.min(Number(m[2]), fixture.length - 1) : fixture.length - 1;
      const body = fixture.subarray(start, end + 1);
      res.writeHead(206, { ...cors, "Content-Range": `bytes ${start}-${end}/${fixture.length}`, "Content-Length": body.length });
      res.end(body);
      return;
    }
    res.writeHead(200, { ...cors, "Content-Length": fixture.length });
    res.end(fixture);
  });
  const port = await listen(server);

  const browser = await launchBrowser();
  const failures = [];
  const page = await browser.newPage();
  const errs = [];
  page.on("pageerror", (e) => errs.push(String(e).slice(0, 160)));

  // PLAYGROUND_BASE lets this run against a DEPLOYED page — how the check itself
  // was validated: pointed at the release that still had the old probe, it fails
  // exactly as #95 did. A regression test that passes before and after the fix
  // would prove nothing.
  const base = process.env.PLAYGROUND_BASE || `http://localhost:${process.env.PGPORT || 8090}`;
  await page.goto(
    `${base}/playground.html` +
      `#url=${encodeURIComponent(`http://127.0.0.1:${port}/hostile.rete`)}&mode=sparql`,
    { waitUntil: "domcontentloaded" },
  );
  await page.waitForFunction(
    () => document.getElementById("run") && !document.getElementById("run").disabled,
    { timeout: 90000 },
  );
  await page.waitForFunction(() => window.PlaygroundEditor, { timeout: 60000 });
  await page.evaluate(() => window.PlaygroundEditor.setText("q", "SELECT ?s ?p ?o WHERE { ?s ?p ?o } LIMIT 5"));

  const res = await runWithRetry(page, { steps: 60 });
  if (res.errBlock) failures.push(`query failed against the lying host: ${res.errText.slice(0, 160)}`);
  if (res.rows < 1) failures.push(`got ${res.rows} rows from a host whose HEAD under-reports the length`);
  // The specific way it used to break — worth naming so a future regression is
  // recognised rather than merely counted.
  if (/range out of bounds/i.test(res.errText)) failures.push("still trusting the reported length: 'range out of bounds'");
  if (errs.length) failures.push(`page errors: ${errs.slice(0, 2).join(" | ")}`);

  await browser.close();
  server.close();

  const pass = failures.length === 0;
  console.log(JSON.stringify({
    verdict: pass ? "PASS" : "FAIL",
    note: "length probe survives a host that under-reports via HEAD and hides Content-Range",
    realBytes: fixture.length,
    headClaimed: LIE,
    rows: res.rows,
    requests: seen,
    failures,
  }, null, 2));
  process.exit(pass ? 0 : 1);
};

main().catch((e) => {
  console.log(JSON.stringify({ verdict: "FAIL", error: String(e && e.message).slice(0, 300) }, null, 2));
  process.exit(1);
});
