// Informational: run the worldcup2026 squad example lazily against the LIVE
// deployed GitHub Pages playground (catches deploy/cache skew vs the local build).
// Can lag a push by a minute or two — the gate reports it but doesn't fail on it.
import { chromium } from "playwright";

const main = async () => {
  const browser = await chromium.launch();
  const page = await browser.newPage();
  const errs = [];
  page.on("pageerror", (e) => errs.push(String(e).slice(0, 200)));
  await page.goto("https://caviri.github.io/rete/playground.html#dataset=worldcup2026&load=lazy&mode=sparql&ex=0", { waitUntil: "domcontentloaded" });
  await page.waitForFunction(() => window.PlaygroundEditor && document.getElementById("run"), { timeout: 60000 });
  await page.waitForTimeout(4000);
  const build = await page.evaluate(() => window.RETE_BUILD || "");
  await page.evaluate(() => document.getElementById("run").click());
  let out = { rows: 0, errBlock: false, qmeta: "" };
  for (let i = 0; i < 60; i++) {
    await page.waitForTimeout(1000);
    out = await page.evaluate(() => {
      const qm = (document.getElementById("qmeta") || {}).textContent || "";
      const m = qm.match(/(\d+)\s+row/);
      return {
        rows: document.querySelectorAll("#out table tbody tr, #out .cards .card").length || (m ? Number(m[1]) : 0),
        errBlock: !!document.querySelector("#out .error-box"),
        qmeta: qm,
        errText: (document.querySelector("#out .err-tech-body") || {}).textContent?.slice(0, 200) || "",
      };
    });
    if (out.rows > 0 || out.errBlock) break;
  }
  const pass = out.rows > 0 && !out.errBlock && errs.length === 0;
  console.log(JSON.stringify({ verdict: pass ? "PASS" : "FAIL", deployedBuild: build, ...out, errs: errs.slice(0, 3) }, null, 2));
  await browser.close();
  process.exit(pass ? 0 : 1);
};
main();
