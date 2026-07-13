// B3 regression: the Settings "↻ Refresh session" button must actually RELOAD the
// document (the old fragment-only navigation did nothing). We stamp a marker on
// window, click Refresh, and assert the marker is gone (a real reload wiped the JS
// context) while the dataset is preserved in the hash. Usage: node check_refresh_session.mjs
import { chromium } from "playwright";

const main = async () => {
  const browser = await chromium.launch();
  const page = await browser.newPage();
  const errs = [];
  page.on("pageerror", (e) => errs.push(String(e).slice(0, 200)));
  const PORT = process.env.PGPORT || "8090";
  await page.goto(`http://localhost:${PORT}/playground.html#dataset=scholar&mode=sparql`, { waitUntil: "domcontentloaded" });
  await page.waitForFunction(() => window.PlaygroundEditor && document.getElementById("run"), { timeout: 60000 });
  await page.waitForTimeout(2000);

  // Stamp a marker that only survives if the page does NOT reload.
  await page.evaluate(() => { window.__refreshMarker = "present"; });
  const before = await page.evaluate(() => window.__refreshMarker);

  // Open Settings and click Refresh session.
  await page.evaluate(() => { const b = document.getElementById("settingsBtn"); if (b) b.click(); document.getElementById("settingsModal").classList.remove("hidden"); });
  await page.waitForTimeout(400);
  await page.evaluate(() => document.getElementById("refreshSessionBtn").click());

  // Wait for the reload to complete (fresh editor).
  await page.waitForTimeout(3000);
  await page.waitForFunction(() => window.PlaygroundEditor && document.getElementById("run"), { timeout: 30000 });

  const after = await page.evaluate(() => ({
    markerGone: typeof window.__refreshMarker === "undefined",
    hash: location.hash,
    modalHidden: document.getElementById("settingsModal").classList.contains("hidden"),
  }));

  const pass = before === "present" && after.markerGone && /dataset=scholar/.test(after.hash) && errs.length === 0;
  console.log(JSON.stringify({ verdict: pass ? "PASS" : "FAIL", markerBefore: before, reloaded: after.markerGone, hash: after.hash, modalHidden: after.modalHidden, errs: errs.slice(0, 3) }, null, 2));
  await browser.close();
  process.exit(pass ? 0 : 1);
};
main();
