// SPARQL type-error warnings in the result panel. FILTER(CONTAINS(?s, "/"))
// on an IRI is a type error, which drops every row without a word, so an
// empty result looked like "CONTAINS is unsupported". The engine has counted
// these since #279 and the wasm envelope has carried them since #280; this
// asserts the playground SHOWS them:
//  - a query whose FILTER errors on every row: the banner names the function
//    and the kind, and the run summary counts the warnings;
//  - arithmetic on a non-number (an operator, not a function) is named too;
//  - the same question asked correctly (STR(?s)): rows, and no banner;
//  - the rows are what the engine returns either way (the banner is extra).
import { launchBrowser } from "./_browser.mjs";

const BAD_Q = 'SELECT ?s WHERE { ?s ?p ?o FILTER(CONTAINS(?s, "/")) } LIMIT 5';
const ARITH_Q = 'SELECT ?s WHERE { ?s ?p ?o FILTER(?s + 1 > 0) } LIMIT 5';
const GOOD_Q = 'SELECT ?s WHERE { ?s ?p ?o FILTER(CONTAINS(STR(?s), "/")) } LIMIT 5';

const main = async () => {
  const PGPORT = process.env.PGPORT || "8090";
  const browser = await launchBrowser();
  const failures = [];
  const pageErrors = [];

  const page = await browser.newPage();
  page.on("pageerror", (e) => pageErrors.push(String(e).slice(0, 200)));
  await page.goto(`http://localhost:${PGPORT}/playground.html#dataset=scholar&mode=sparql`, { waitUntil: "domcontentloaded" });
  await page.waitForFunction(
    () => window.PlaygroundEditor && document.getElementById("run") && !document.getElementById("run").disabled,
    undefined,
    { timeout: 90000 },
  );

  const run = async (q) => {
    await page.evaluate((query) => {
      document.getElementById("qmeta").textContent = "";
      window.PlaygroundEditor.setText("q", query);
      document.getElementById("run").click();
    }, q);
    await page.waitForFunction(
      () => /row\(s\)/.test((document.getElementById("qmeta") || {}).textContent || "") ||
            document.querySelector("#out .error-box"),
      undefined,
      { timeout: 60000 },
    );
    return page.evaluate(() => ({
      banner: (document.querySelector("#out .query-warnings") || {}).textContent || "",
      qmeta: (document.getElementById("qmeta") || {}).textContent || "",
      rows: document.querySelectorAll("#out table tbody tr").length,
      error: !!document.querySelector("#out .error-box"),
    }));
  };

  const bad = await run(BAD_Q);
  if (bad.error) failures.push("the type-error query errored instead of returning a result");
  if (!/CONTAINS/.test(bad.banner)) failures.push(`no warning banner naming CONTAINS (banner: "${bad.banner.slice(0, 160)}")`);
  if (!/an IRI as argument 1/.test(bad.banner)) failures.push(`banner does not say what was wrong: "${bad.banner.slice(0, 200)}"`);
  if (!/STR\(\)/.test(bad.banner)) failures.push("banner lost the STR() hint");
  if (!/0 row\(s\).*⚠ 1 warning/.test(bad.qmeta)) failures.push(`summary does not count the warning: "${bad.qmeta}"`);

  const arith = await run(ARITH_Q);
  if (!/Operator \+ received an IRI as the left operand/.test(arith.banner)) {
    failures.push(`arithmetic warning missing or misworded: "${arith.banner.slice(0, 200)}"`);
  }

  const good = await run(GOOD_Q);
  if (good.banner) failures.push(`a clean query shows a warning banner: "${good.banner.slice(0, 120)}"`);
  if (/warning/.test(good.qmeta)) failures.push(`a clean query's summary mentions warnings: "${good.qmeta}"`);
  if (good.rows < 1) failures.push("the correct STR() query returned no rows on scholar");

  if (pageErrors.length) failures.push(`page errors: ${pageErrors.slice(0, 2).join(" | ")}`);
  await browser.close();

  const pass = failures.length === 0;
  console.log(JSON.stringify({
    verdict: pass ? "PASS" : "FAIL",
    note: "type-error warnings: banner above the result naming the function/operator + hint, counted in the summary; none on a clean query",
    badBanner: bad.banner.slice(0, 160),
    badQmeta: bad.qmeta,
    goodRows: good.rows,
    failures,
  }, null, 2));
  process.exit(pass ? 0 : 1);
};

main().catch((e) => {
  console.log(JSON.stringify({ verdict: "FAIL", error: String(e && e.message).slice(0, 300) }, null, 2));
  process.exit(1);
});
