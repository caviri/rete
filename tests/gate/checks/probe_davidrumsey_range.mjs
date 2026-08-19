// Reproduce the reported "range fetch failed mid-query" on davidrumsey in a
// FRESH browser context (no cache), capturing every request to the .rete so we
// can see which range failed and with what status.
//
//   docker run --rm -v "$PWD:/work" -w /work/tests/gate \
//     mcr.microsoft.com/playwright:v1.49.0-noble node checks/probe_davidrumsey_range.mjs
//
// PAGE env overrides the target (default: the deployed playground).
import { chromium } from "playwright";

const PAGE = process.env.PAGE ||
  "https://caviri.github.io/rete/playground.html#dataset=davidrumsey&load=lazy&mode=sparql&ex=1";
const RETE = "davidrumsey.rete";

const browser = await chromium.launch();
const ctx = await browser.newContext({ viewport: { width: 1400, height: 900 } });
const page = await ctx.newPage();

const reqs = [];
page.on("request", (r) => {
  if (r.url().includes(RETE)) {
    reqs.push({ n: reqs.length + 1, range: r.headers()["range"] || "(none)", status: null, err: null });
  }
});
page.on("response", async (r) => {
  if (!r.url().includes(RETE)) return;
  const range = r.request().headers()["range"] || "(none)";
  const hit = reqs.find((x) => x.range === range && x.status === null);
  if (hit) {
    hit.status = r.status();
    hit.cr = r.headers()["content-range"] || "";
  }
});
page.on("requestfailed", (r) => {
  if (!r.url().includes(RETE)) return;
  const range = r.request().headers()["range"] || "(none)";
  const hit = reqs.find((x) => x.range === range && x.status === null);
  if (hit) hit.err = r.failure()?.errorText || "failed";
});
const consoleErrs = [];
page.on("console", (m) => { if (m.type() === "error") consoleErrs.push(m.text().slice(0, 200)); });
page.on("pageerror", (e) => consoleErrs.push("pageerror: " + e.message.slice(0, 200)));

await page.goto(PAGE, { waitUntil: "domcontentloaded" });
// let the page settle, then run whatever example the fragment selected
await page.waitForTimeout(6000);
const runBtn = page.locator("#run, button:has-text('Run')").first();
if (await runBtn.count()) {
  await runBtn.click().catch(() => {});
}
// wait for either rows or an error surface
await page.waitForTimeout(45000);

const bodyText = (await page.locator("body").innerText().catch(() => "")) || "";
const failedMsg = /range fetch failed|Remote query failed|refusing to return/i.test(bodyText);

console.log(`page: ${PAGE}`);
console.log(`.rete requests: ${reqs.length}`);
for (const r of reqs) {
  console.log(`  #${r.n} range=${r.range} -> status=${r.status ?? "-"} ` +
              `${r.cr ? "cr=" + r.cr : ""} ${r.err ? "ERR=" + r.err : ""}`);
}
const bad = reqs.filter((r) => r.err || (r.status && r.status !== 206 && r.status !== 200));
console.log(`\nFAILED/NON-206 requests: ${bad.length}`);
bad.forEach((r) => console.log(`  range=${r.range} status=${r.status} err=${r.err}`));
console.log(`page shows a range-failure message: ${failedMsg}`);
if (consoleErrs.length) {
  console.log("console errors:");
  consoleErrs.slice(0, 6).forEach((e) => console.log("  " + e));
}
await browser.close();
