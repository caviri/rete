// The EPFL ENAC open-science dataset over live R2: load it lazily and run the
// example that is the whole point of the graph — every ENAC repository with the
// lab that owns it. Asserts rows, no error block, no page errors, and that the
// repository column really carries GitHub/GitLab IRIs (the Open Pulse-shared
// nodes), so a silently-empty or mis-shaped result cannot pass.
// Usage: node check_enac.mjs
import { launchBrowser } from "./_browser.mjs";
import { runWithRetry } from "./_util.mjs";

const main = async () => {
  const browser = await launchBrowser();
  const page = await browser.newPage();
  const errs = [];
  page.on("pageerror", (e) => errs.push(String(e).slice(0, 200)));
  if (process.env.RETE_FORCE_SYNC === "1") {
    await page.addInitScript(() => localStorage.setItem("asyncReadsOn", "0"));
  }
  const PORT = process.env.PGPORT || "8090";
  // ex=1 is "Every ENAC repository, by lab".
  await page.goto(
    `http://localhost:${PORT}/playground.html#dataset=enac-it4research&load=lazy&mode=sparql&ex=1`,
    { waitUntil: "domcontentloaded" },
  );
  await page.waitForFunction(() => window.PlaygroundEditor && document.getElementById("run"), { timeout: 60000 });
  await page.waitForTimeout(4000);

  const res = await runWithRetry(page, { steps: 60 });

  // Shape check: the result must actually contain platform repository IRIs.
  const body = await page.evaluate(() =>
    (document.querySelector("#out table") || {}).textContent || "");
  const hasRepoIri = /github\.com\/|gitlab\.(?:com|epfl\.ch)\/|c4science\.ch\//.test(body);
  const hasLabCode = /\b(CNPA|LSMS|RESSLAB|VITA|IBOIS|DISAL|LTE|SXL|CEAT)\b/.test(body);

  const pass = res.rows > 0 && !res.errBlock && hasRepoIri && hasLabCode && errs.length === 0;
  console.log(JSON.stringify({
    verdict: pass ? "PASS" : "FAIL",
    rows: res.rows, qmeta: res.qmeta, errText: res.errText,
    hasRepoIri, hasLabCode,
    forceSync: process.env.RETE_FORCE_SYNC === "1",
    tries: res.tries, errs: errs.slice(0, 3),
  }, null, 2));
  await browser.close();
  process.exit(pass ? 0 : 1);
};
main();
