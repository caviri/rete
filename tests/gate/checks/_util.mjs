// Shared helpers for the browser checks. The point: distinguish a REAL regression
// (fail immediately) from a live-R2 network blip (retry a couple of times), so the
// gate doesn't go red on transient CDN latency — the false-red the audit flagged.

// Poll #out until rows appear or an error box shows; returns a snapshot. Counts
// table rows OR cards (mobile default view) OR the qmeta "N row(s)" tally.
export async function pollOut(page, { steps = 45, stepMs = 1000 } = {}) {
  let out = { rows: 0, errBlock: false, errText: "", qmeta: "" };
  for (let i = 0; i < steps; i++) {
    await page.waitForTimeout(stepMs);
    out = await page.evaluate(() => {
      const qm = (document.getElementById("qmeta") || {}).textContent || "";
      const m = qm.match(/(\d+)\s+row/);
      return {
        rows: document.querySelectorAll("#out table tbody tr, #out .cards .card").length || (m ? Number(m[1]) : 0),
        errBlock: !!document.querySelector("#out .error-box"),
        errText: (document.querySelector("#out .err-advice, #out .err-tech-body") || {}).textContent || "",
        qmeta: qm,
      };
    });
    if (out.rows > 0 || out.errBlock) break;
  }
  return out;
}

// Transient = network / host hiccup (mirrors the app's own classifyError transient
// bucket). A wasm trap, parse error, wrong rows, etc. are NOT transient.
export const isTransient = (t) =>
  /hiccup|could not determine length|short range|failed to fetch|networkerror|network error|load failed|status\s*0\b|status\s*5\d\d|timeout|connection|err_|range req|ignored Range/i.test(String(t || ""));

// Click Run and poll; on a TRANSIENT error (or an empty no-result) wait and retry
// up to `tries` times. A non-transient error box returns immediately (real bug).
export async function runWithRetry(page, opts = {}) {
  const tries = opts.tries || 3;
  let out = { rows: 0, errBlock: false, errText: "", qmeta: "" };
  for (let k = 0; k < tries; k++) {
    await page.evaluate(() => document.getElementById("run").click());
    out = await pollOut(page, opts);
    if (out.rows > 0) return { ...out, tries: k + 1 };
    if (out.errBlock && !isTransient(out.errText)) return { ...out, tries: k + 1 }; // real error — stop
    await page.waitForTimeout(1500); // transient / empty — back off and retry
  }
  return { ...out, tries };
}
