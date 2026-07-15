// Required Pages post-deploy check. It verifies the exact staged commit and a
// real lazy R2 query, and treats browser console/page errors as deployment bugs.
import { launchBrowser } from "./_browser.mjs";

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

  await page.evaluate(() => document.getElementById("run").click());
  let out = { rows: 0, errBlock: false, qmeta: "", errText: "" };
  for (let i = 0; i < 60; i++) {
    await page.waitForTimeout(1000);
    out = await page.evaluate(() => {
      const qmeta = (document.getElementById("qmeta") || {}).textContent || "";
      const match = qmeta.match(/(\d+)\s+row/);
      return {
        rows:
          document.querySelectorAll("#out table tbody tr, #out .cards .card").length ||
          (match ? Number(match[1]) : 0),
        errBlock: Boolean(document.querySelector("#out .error-box")),
        qmeta,
        errText:
          (document.querySelector("#out .err-tech-body") || {}).textContent?.slice(0, 200) || "",
      };
    });
    if (out.rows > 0 || out.errBlock) break;
  }

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
