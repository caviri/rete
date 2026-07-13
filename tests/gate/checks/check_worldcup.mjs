// Verify the worldcup2026 ex=0 squad query now runs (parse error fixed) against
// the live remote dataset. Usage: node check_worldcup.mjs
import { chromium } from "playwright";

const main = async () => {
  const browser = await chromium.launch();
  const page = await browser.newPage();
  const errs = [];
  page.on("pageerror", (e) => errs.push(String(e).slice(0, 200)));
  const PORT = process.env.PGPORT || "8090";
  await page.goto(`http://localhost:${PORT}/playground.html#dataset=worldcup2026&load=lazy&mode=sparql&ex=0`, { waitUntil: "domcontentloaded" });
  await page.waitForFunction(() => window.PlaygroundEditor && document.getElementById("run"), { timeout: 60000 });
  await page.waitForTimeout(4000); // dataset open + example load

  const q = await page.evaluate(() => (window.PlaygroundEditor.getText ? window.PlaygroundEditor.getText("q") : (document.getElementById("q") || {}).value) || "");
  await page.evaluate(() => document.getElementById("run").click());

  let out = { rows: 0, err: "", qmeta: "" };
  for (let i = 0; i < 45; i++) {
    await page.waitForTimeout(1000);
    out = await page.evaluate(() => ({
      rows: document.querySelectorAll("#out table tbody tr").length,
      err: (document.querySelector("#out .err-tech-body") || document.querySelector("#out .err-advice") || {}).textContent || "",
      errBlock: !!document.querySelector("#out .error-box"),
      qmeta: (document.getElementById("qmeta") || {}).textContent || "",
    }));
    if (out.rows > 0 || out.errBlock) break;
  }

  const hasXsd = /PREFIX xsd:/i.test(q);
  const pass = hasXsd && out.rows > 0 && !out.errBlock && errs.length === 0;
  console.log(JSON.stringify({
    verdict: pass ? "PASS" : "FAIL",
    queryHasXsdPrefix: hasXsd,
    rows: out.rows, qmeta: out.qmeta,
    errText: out.err.slice(0, 160),
    errs: errs.slice(0, 3),
  }, null, 2));
  await browser.close();
  process.exit(pass ? 0 : 1);
};
main();
