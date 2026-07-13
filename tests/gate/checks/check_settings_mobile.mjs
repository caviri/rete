// Phone-viewport check of the reworked Settings modal: no horizontal overflow,
// the model input fits, and the Storage + Session sections render. Runs a query
// first so the session log has a row. Usage: node check_settings_mobile.mjs
import { chromium } from "playwright";

const main = async () => {
  const browser = await chromium.launch();
  const page = await browser.newPage({ viewport: { width: 390, height: 844 }, deviceScaleFactor: 3, isMobile: true, hasTouch: true });
  const errs = [];
  page.on("pageerror", (e) => errs.push("page: " + String(e).slice(0, 200)));
  page.on("console", (m) => { if (m.type() === "error") errs.push("console: " + m.text().slice(0, 200)); });
  const PORT = process.env.PGPORT || "8090";
  await page.goto(`http://localhost:${PORT}/playground.html`, { waitUntil: "domcontentloaded" });
  await page.waitForFunction(() => window.PlaygroundEditor && document.getElementById("run"), { timeout: 60000 });
  await page.waitForTimeout(2500);

  // Run a query so the session log has an entry.
  await page.evaluate(() => window.PlaygroundEditor.setText("q", "SELECT * WHERE { ?s ?p ?o } LIMIT 3"));
  await page.evaluate(() => document.getElementById("run").click());
  await page.waitForTimeout(2500);

  // Open Settings (button may be relocated on phones — click it directly).
  await page.evaluate(() => { const b = document.getElementById("settingsBtn"); if (b) b.click(); });
  await page.waitForTimeout(400);
  await page.evaluate(() => document.getElementById("settingsModal").classList.remove("hidden"));
  await page.waitForTimeout(1200); // storage estimate + renders

  const r = await page.evaluate(() => {
    const q = (id) => document.getElementById(id);
    const card = document.querySelector("#settingsModal .modal-card");
    const input = q("aiModelId");
    const vw = window.innerWidth;
    // Open the Advanced fold so its controls are laid out, then check EVERY
    // control's right edge — a child wider than a card with overflow:hidden won't
    // grow scrollWidth (clipped), so page/card overflow alone can miss it.
    document.querySelectorAll("#settingsModal .rc-adv").forEach((d) => { d.open = true; });
    const clipped = [];
    document.querySelectorAll("#settingsModal input, #settingsModal button, #settingsModal .rc-input, #settingsModal .stg-row, #settingsModal .set-h").forEach((el) => {
      const rect = el.getBoundingClientRect();
      if (rect.width > 0 && rect.right > vw + 1) clipped.push((el.id || el.className || el.tagName) + "@" + Math.round(rect.right));
    });
    return {
      pageOverflow: document.documentElement.scrollWidth - vw,       // >0 == horizontal scroll (bad)
      cardOverflow: card ? card.scrollWidth - card.clientWidth : -1, // >0 == content wider than card (bad)
      clippedControls: clipped,                                      // controls whose right edge exceeds the viewport
      inputRight: input ? Math.round(input.getBoundingClientRect().right) : -1,
      vw,
      inputFits: input ? input.getBoundingClientRect().right <= vw + 1 : false,
      storageInfo: (q("storageInfo") || {}).textContent || "",
      breakdownRows: document.querySelectorAll("#storageBreakdown .stg-row").length,
      hasClearAll: !!q("clearCacheAll"), hasClearModels: !!q("clearModelsBtn"),
      hasRefresh: !!q("refreshSessionBtn"),
      sessionRows: document.querySelectorAll("#sessionLog .stg-logrow").length,
      sessionInfo: (q("sessionInfo") || {}).textContent || "",
      advPresent: !!document.querySelector(".rc-adv"),
    };
  });

  const pass =
    r.pageOverflow <= 1 && r.cardOverflow <= 1 && r.inputFits && r.clippedControls.length === 0 &&
    r.hasClearAll && r.hasClearModels && r.hasRefresh &&
    r.breakdownRows >= 2 && r.sessionRows >= 1 && errs.length === 0;
  console.log(JSON.stringify({ verdict: pass ? "PASS" : "FAIL", ...r, errs: errs.slice(0, 4) }, null, 2));
  await browser.close();
  process.exit(pass ? 0 : 1);
};
main();
