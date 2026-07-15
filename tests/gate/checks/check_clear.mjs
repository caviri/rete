// Prove "Clear everything" actually frees storage: seed the Cache API (where AI
// model weights live — the thing that was NEVER cleared) AND all four rete
// IndexedDB stores, click Clear everything, then assert everything is gone.
import { launchBrowser } from "./_browser.mjs";

const main = async () => {
  const browser = await launchBrowser();
  const page = await browser.newPage();
  const errs = [];
  page.on("pageerror", (e) => errs.push(String(e).slice(0, 200)));
  const PORT = process.env.PGPORT || "8090";
  await page.goto(`http://localhost:${PORT}/playground.html`, { waitUntil: "domcontentloaded" });
  await page.waitForFunction(() => window.PlaygroundEditor && document.getElementById("run"), { timeout: 60000 });
  await page.waitForTimeout(1500);

  // Seed a fake model cache (Cache API) + a row in each rete store.
  const seeded = await page.evaluate(async () => {
    // Cache API — simulate a downloaded model file (~1 MB).
    const c = await caches.open("transformers-cache");
    await c.put("https://hf/model.onnx", new Response(new Uint8Array(1024 * 1024)));
    // rete cache DB: put one key in every store.
    const put = () => new Promise((res) => {
      const r = indexedDB.open("playgroundCache", 2);
      r.onupgradeneeded = () => { const db = r.result; ["files", "meta", "ranges", "rangeMeta"].forEach((s) => { if (!db.objectStoreNames.contains(s)) db.createObjectStore(s); }); };
      r.onsuccess = () => {
        const db = r.result;
        const t = db.transaction(["files", "meta", "ranges", "rangeMeta"], "readwrite");
        t.objectStore("files").put(new Uint8Array(2048), "k::duckdb");
        t.objectStore("meta").put({ size: 2048 }, "k::duckdb");
        t.objectStore("ranges").put(new Uint8Array(4096), "u#0");
        t.objectStore("rangeMeta").put({ bytes: 4096, total: 8192, blocks: [0] }, "u");
        t.oncomplete = () => res(true); t.onerror = () => res(false);
      };
      r.onerror = () => res(false);
    });
    await put();
    const cacheKeys = await caches.keys();
    return { cacheKeys };
  });

  const count = () => page.evaluate(() => new Promise((res) => {
    Promise.all([
      caches.keys(),
      new Promise((r) => { const q = indexedDB.open("playgroundCache", 2); q.onsuccess = () => { const db = q.result; const out = {}; let n = 0; const stores = ["files", "meta", "ranges", "rangeMeta"]; stores.forEach((s) => { const cr = db.transaction(s).objectStore(s).count(); cr.onsuccess = () => { out[s] = cr.result; if (++n === stores.length) r(out); }; cr.onerror = () => { out[s] = -1; if (++n === stores.length) r(out); }; }); }; q.onerror = () => r({ err: 1 }); }),
    ]).then(([ck, stores]) => res({ caches: ck.length, stores }));
  }));

  const before = await count();

  // Open settings and click Clear everything.
  await page.evaluate(() => { const b = document.getElementById("settingsBtn"); if (b) b.click(); document.getElementById("settingsModal").classList.remove("hidden"); });
  await page.waitForTimeout(600);
  await page.evaluate(() => document.getElementById("clearCacheAll").click());
  await page.waitForTimeout(2500);

  const after = await count();
  const freedText = await page.evaluate(() => (document.getElementById("storageFreed") || {}).textContent || "");

  const storesEmpty = (s) => s && s.files === 0 && s.meta === 0 && s.ranges === 0 && s.rangeMeta === 0;
  const pass =
    before.caches >= 1 && Object.values(before.stores).every((v) => v >= 1) && // seeded
    after.caches === 0 && storesEmpty(after.stores) &&                          // cleared
    errs.length === 0;
  console.log(JSON.stringify({ verdict: pass ? "PASS" : "FAIL", before, after, freedText, errs: errs.slice(0, 3) }, null, 2));
  await browser.close();
  process.exit(pass ? 0 : 1);
};
main();
