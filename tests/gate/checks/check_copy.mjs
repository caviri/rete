// Verify (1) a parse/syntax error now shows the Copy-log button, and (2) the copy
// buttons ACTUALLY put text on the clipboard (the iOS-safe execCommand-first path).
// Reads the clipboard back to confirm. Usage: node check_copy.mjs
import { chromium } from "playwright";

const main = async () => {
  const browser = await chromium.launch();
  const ctx = await browser.newContext();
  const PORT = process.env.PGPORT || "8090";
  await ctx.grantPermissions(["clipboard-read", "clipboard-write"], { origin: `http://localhost:${PORT}` });
  const page = await ctx.newPage();
  const errs = [];
  page.on("pageerror", (e) => errs.push(String(e).slice(0, 200)));
  await page.goto(`http://localhost:${PORT}/playground.html`, { waitUntil: "domcontentloaded" });
  await page.waitForFunction(() => window.PlaygroundEditor && document.getElementById("run"), { timeout: 60000 });
  await page.waitForTimeout(2500);

  // A malformed query → parse error (the "user"-tone case that had no button).
  await page.evaluate(() => window.PlaygroundEditor.setText("q", "SELECT ?x WHERE { ?x ?p ?o "));
  await page.evaluate(() => document.getElementById("run").click());

  let btn = false, report = "";
  for (let i = 0; i < 16; i++) {
    await page.waitForTimeout(400);
    const r = await page.evaluate(() => {
      const b = document.querySelector("#out .err-copy");
      const pre = document.querySelector("#out .err-tech-body");
      return { btn: !!b, report: pre ? pre.textContent : "", headline: (document.querySelector("#out .err-headline") || {}).textContent || "" };
    });
    btn = r.btn; report = r.report;
    if (btn) break;
  }

  // Click the copy button, then read the clipboard back.
  await page.evaluate(() => { const b = document.querySelector("#out .err-copy"); if (b) b.click(); });
  await page.waitForTimeout(400);
  const errClip = await page.evaluate(() => navigator.clipboard.readText().catch(() => "READ_FAILED"));

  // Now the share button (#shareBtn) → clipboard should hold the page URL.
  let shareClip = "(no shareBtn)";
  const hasShare = await page.evaluate(() => !!document.getElementById("shareBtn"));
  if (hasShare) {
    await page.evaluate(() => document.getElementById("shareBtn").click());
    await page.waitForTimeout(400);
    shareClip = await page.evaluate(() => navigator.clipboard.readText().catch(() => "READ_FAILED"));
  }

  const parseErrShown = /parse|expected|adjust the query/i.test(report) || report.includes("error:");
  const errCopied = errClip.includes("rete playground — error report");
  const shareCopied = !hasShare || shareClip.includes("playground.html");
  const pass = btn && parseErrShown && errCopied && shareCopied && errs.length === 0;
  console.log(JSON.stringify({
    verdict: pass ? "PASS" : "FAIL",
    parseErrorHasButton: btn,
    errCopied, errClipSample: errClip.slice(0, 80),
    shareCopied, shareClipSample: String(shareClip).slice(0, 80),
    errs: errs.slice(0, 3),
  }, null, 2));
  await browser.close();
  process.exit(pass ? 0 : 1);
};
main();
