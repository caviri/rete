// B1 regression: if the engine wasm fails to instantiate in the worker, the query
// must SURFACE AN ERROR — not hang forever ("querying…", no rows, no error). We
// corrupt the async wasm response so wasm_bindgen rejects on init, then assert an
// error box appears within the watchdog window. Usage: node check_worker_init.mjs
import { launchBrowser } from "./_browser.mjs";

const main = async () => {
  const browser = await launchBrowser();
  const page = await browser.newPage();
  const PORT = process.env.PGPORT || "8090";
  // Serve garbage for the async wasm → CompileError in the worker's wasm_bindgen.
  await page.route(/rete_wasm_async\.wasm/, (route) =>
    route.fulfill({ status: 200, headers: { "content-type": "application/wasm" }, body: Buffer.from([0, 1, 2, 3, 4, 5, 6, 7, 8, 9]) }));
  await page.addInitScript(() => { try { localStorage.setItem("asyncReadsOn", "1"); } catch (e) {} }); // force the async path
  await page.goto(`http://localhost:${PORT}/playground.html#dataset=worldcup2026&load=lazy&mode=sparql&ex=0`, { waitUntil: "domcontentloaded" });
  await page.waitForFunction(() => window.PlaygroundEditor && document.getElementById("run"), { timeout: 60000 });
  await page.waitForTimeout(3000);

  await page.evaluate(() => document.getElementById("run").click());

  let s = { errBox: false, rows: 0, qmeta: "" };
  let surfacedBy = -1;
  for (let i = 0; i < 40; i++) { // must surface well within the 30 s watchdog + fetch
    await page.waitForTimeout(1000);
    s = await page.evaluate(() => ({
      errBox: !!document.querySelector("#out .error-box, #out .note"),
      rows: document.querySelectorAll("#out table tbody tr, #out .cards .card").length,
      qmeta: (document.getElementById("qmeta") || {}).textContent || "",
      outText: (document.getElementById("out") || {}).textContent?.slice(0, 140) || "",
    }));
    if (s.errBox) { surfacedBy = i + 1; break; }
  }

  // PASS = an error surfaced (didn't hang). It must NOT silently show rows.
  const pass = s.errBox && s.rows === 0;
  console.log(JSON.stringify({ verdict: pass ? "PASS" : "FAIL", surfacedAfterSec: surfacedBy, ...s }, null, 2));
  await browser.close();
  process.exit(pass ? 0 : 1);
};
main();
