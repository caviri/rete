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

// Click Run and poll; retry on ANY no-rows outcome (error box OR empty) up to
// `tries` times. Rationale: classifying "is this error transient?" from the UI
// text is unreliable (a live-R2 range blip can surface as several different
// messages), so instead we lean on determinism — a REAL regression fails on every
// retry and the check still goes red; only a transient recovers. All callers here
// assert rows>0, so retrying a genuinely-empty result just costs a little time.
export async function runWithRetry(page, opts = {}) {
  const tries = opts.tries || 3;
  let out = { rows: 0, errBlock: false, errText: "", qmeta: "" };
  for (let k = 0; k < tries; k++) {
    await page.evaluate(() => document.getElementById("run").click());
    out = await pollOut(page, opts);
    if (out.rows > 0) return { ...out, tries: k + 1 };
    await page.waitForTimeout(1500); // no rows (error or empty) — back off and retry
  }
  return { ...out, tries };
}
