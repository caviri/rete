// Required Pages post-deploy check. It verifies the exact staged commit and a
// real lazy R2 query, and treats browser console/page errors as deployment bugs.
import { launchBrowser } from "./_browser.mjs";
import { runWithRetry } from "./_util.mjs";

const base = process.env.DEPLOYED_URL || "https://caviri.github.io/rete/";
const expected = process.env.EXPECTED_SHA || "";
const target = new URL(
  "playground.html#dataset=worldcup2026&load=lazy&mode=sparql&ex=0",
  base.endsWith("/") ? base : `${base}/`,
);

const main = async () => {
  const browser = await launchBrowser();
  const page = await browser.newPage();
  const errs = [];
  page.on("pageerror", (error) => errs.push(`page: ${String(error).slice(0, 200)}`));
  page.on("console", (message) => {
    if (message.type() === "error") errs.push(`console: ${message.text().slice(0, 200)}`);
  });

  let build = "";
  // Pages deployment can be followed briefly by a stale CDN response. Retry
  // with a cache-buster, but only accept the exact expected build stamp.
  for (let attempt = 0; attempt < 12; attempt++) {
    const url = new URL(target);
    url.searchParams.set("deploy", `${expected}-${attempt}`);
    await page.goto(url.toString(), { waitUntil: "domcontentloaded" });
    await page.waitForFunction(
      () => window.PlaygroundEditor && document.getElementById("run"),
      { timeout: 60000 },
    );
    build = await page.evaluate(() => window.RETE_BUILD || "");
    if (!expected || build === expected) break;
    await page.waitForTimeout(5000);
  }

  await page.waitForFunction(
    () => {
      const run = document.getElementById("run");
      const query = window.PlaygroundEditor?.getText
        ? window.PlaygroundEditor.getText("q")
        : (document.getElementById("q") || {}).value;
      return run && !run.disabled && query && query.trim().length > 0;
    },
    { timeout: 60000 },
  );
  await page.waitForTimeout(4000); // remote dataset open + selected example load
  const out = await runWithRetry(page, { tries: 3, steps: 60, stepMs: 1000 });

  const exactBuild = !expected || build === expected;
  const pass = exactBuild && out.rows > 0 && !out.errBlock && errs.length === 0;
  console.log(
    JSON.stringify(
      {
        verdict: pass ? "PASS" : "FAIL",
        url: target.toString(),
        expectedBuild: expected,
        deployedBuild: build,
        exactBuild,
        ...out,
        tries: out.tries,
        errs: errs.slice(0, 5),
      },
      null,
      2,
    ),
  );
  await browser.close();
  process.exit(pass ? 0 : 1);
};

main().catch((error) => {
  console.error(error);
  process.exit(1);
});
