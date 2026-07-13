// With an iPhone UA and NO override, the playground must default to the sync
// reader: it should NOT fetch the async wasm, the remote query must still run,
// and the Settings toggle must reflect "off" with iPhone-specific advice.
import { chromium } from "playwright";
import { runWithRetry } from "./_util.mjs";

const IPHONE_UA = "Mozilla/5.0 (iPhone; CPU iPhone OS 18_7 like Mac OS X) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/26.5 Mobile/15E148 Safari/604.1";
const Q = `PREFIX wc: <https://w3id.org/rete/worldcup#>
PREFIX sc: <http://schema.org/>
PREFIX xsd: <http://www.w3.org/2001/XMLSchema#>
SELECT ?num ?player WHERE {
  <https://w3id.org/rete/worldcup/2026/team/Argentina> wc:squadPlayer ?p .
  ?p sc:name ?player .
  OPTIONAL { ?p wc:shirtNumber ?num }
} ORDER BY xsd:integer(?num)`;

const main = async () => {
  const browser = await chromium.launch();
  const ctx = await browser.newContext({ userAgent: IPHONE_UA, viewport: { width: 390, height: 844 }, isMobile: true, hasTouch: true });
  const page = await ctx.newPage();
  const asyncWasmReqs = [];
  const errs = [];
  page.on("request", (r) => { if (/rete_wasm_async/.test(r.url())) asyncWasmReqs.push(r.url().split("/").pop()); });
  page.on("pageerror", (e) => errs.push(String(e).slice(0, 200)));
  const PORT = process.env.PGPORT || "8090";
  await page.addInitScript(() => { try { localStorage.removeItem("asyncReadsOn"); } catch (e) {} });
  await page.goto(`http://localhost:${PORT}/playground.html#dataset=worldcup2026&load=lazy&mode=sparql`, { waitUntil: "domcontentloaded" });
  await page.waitForFunction(() => window.PlaygroundEditor && document.getElementById("run"), { timeout: 60000 });
  await page.waitForTimeout(3500);

  await page.evaluate((q) => window.PlaygroundEditor.setText("q", q), Q);
  // runWithRetry counts cards (mobile default view) and retries a transient R2 blip.
  const out = await runWithRetry(page);

  // Inspect the Settings toggle.
  await page.evaluate(() => { const b = document.getElementById("settingsBtn"); if (b) b.click(); document.getElementById("settingsModal").classList.remove("hidden"); });
  await page.waitForTimeout(600);
  const toggle = await page.evaluate(() => {
    const t = document.getElementById("asyncReadsToggle");
    const info = document.getElementById("asyncReadsInfo");
    return { present: !!t, checked: t ? t.checked : null, info: info ? info.textContent : "" };
  });

  const pass =
    out.rows > 0 && !out.errBlock &&
    asyncWasmReqs.length === 0 &&          // never fetched the async wasm → sync path chosen
    toggle.present && toggle.checked === false &&
    /iphone|ipad/i.test(toggle.info) &&
    errs.length === 0;
  console.log(JSON.stringify({
    verdict: pass ? "PASS" : "FAIL",
    rows: out.rows, errBlock: out.errBlock,
    asyncWasmFetched: asyncWasmReqs,
    toggleChecked: toggle.checked, toggleInfo: toggle.info.slice(0, 120),
    errs: errs.slice(0, 3),
  }, null, 2));
  await browser.close();
  process.exit(pass ? 0 : 1);
};
main();
