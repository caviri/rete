// The playground's in-browser builder ingests RDF 1.2 Turtle and TriG.
//
// Through the real UI: pick the format, set "Quoted triples: RDF 1.2", paste,
// build, open the bytes, and query the reifiers. Also asserts the RDF-star
// default still refuses the same Turtle, naming the choice, so the new select
// changes nothing for anyone who leaves it alone. No network is involved.
import { launchBrowser } from "./_browser.mjs";

const TTL = `@prefix ex: <http://example.test/> .
ex:alice ex:says <<( ex:bob ex:knows ex:carol )>> .
<< ex:bob ex:knows ex:dave >> ex:source ex:wiki .
ex:erin ex:knows ex:frank {| ex:since "2020" |} .
ex:gina ex:name "Gina"@en--ltr .`;

const TRIG = `@prefix ex: <http://example.test/> .
ex:g {
  ex:alice ex:says <<( ex:bob ex:knows ex:carol )>> .
  << ex:bob ex:knows ex:dave >> ex:source ex:wiki .
}`;

const QUERY = `SELECT ?r ?t WHERE { ?r <http://www.w3.org/1999/02/22-rdf-syntax-ns#reifies> ?t }`;

async function build(page, fmt, syntax, text, key) {
  await page.selectOption("#buildFormat", fmt);
  await page.selectOption("#buildQtSyntax", syntax);
  await page.evaluate((t) => window.PlaygroundEditor.setText("buildText", t), text);
  await page.fill("#cardTitle", `RDF 1.2 ${fmt}`);
  await page.fill("#cardKey", key);
  await page.evaluate(() => { const o = document.getElementById("buildOut"); if (o) o.textContent = ""; });
  await page.click("#buildRun");
  await page.waitForFunction(() => /Saved|Built|failed/i.test((document.getElementById("buildOut") || {}).textContent || ""), { timeout: 30000 });
  return page.evaluate(() => ({
    meta: (document.getElementById("buildMeta") || {}).textContent || "",
    out: (document.getElementById("buildOut") || {}).textContent || "",
    canOpen: !(document.getElementById("buildOpen") || {}).disabled,
  }));
}

const main = async () => {
  const browser = await launchBrowser();
  const page = await browser.newPage();
  const errs = [];
  page.on("pageerror", (e) => errs.push(String(e).slice(0, 240)));
  page.on("console", (m) => { if (m.type() === "error") errs.push("console: " + m.text().slice(0, 200)); });
  const PORT = process.env.PGPORT || "8090";
  await page.goto(`http://localhost:${PORT}/playground.html#dataset=scholar&mode=sparql`, { waitUntil: "domcontentloaded" });
  await page.waitForFunction(() => window.PlaygroundEditor && document.getElementById("buildBtn"), { timeout: 60000 });
  await page.click("#buildBtn");

  // The default surface must refuse RDF 1.2 Turtle, and say what to choose.
  const refused = await build(page, "ttl", "rdf-star", TTL, "rdf12-refused");
  const trig = await build(page, "trig", "rdf12", TRIG, "rdf12-trig");
  const ttl = await build(page, "ttl", "rdf12", TTL, "rdf12-ttl");

  await page.click("#buildOpen");
  await page.waitForFunction(() => /RDF 1\.2 ttl/i.test((document.getElementById("dsName") || {}).textContent || ""), { timeout: 15000 });
  await page.evaluate((q) => {
    const strategy = document.getElementById("strategy");
    if (strategy) { strategy.value = "whole"; strategy.dispatchEvent(new Event("change")); }
    window.PlaygroundEditor.setText("q", q);
    document.getElementById("run").click();
  }, QUERY);
  await page.waitForFunction(() => document.querySelectorAll("#out table tbody tr").length > 0 || document.querySelector("#out .error-box"), { timeout: 30000 });
  const query = await page.evaluate(() => ({
    rows: document.querySelectorAll("#out table tbody tr").length,
    text: (document.getElementById("out") || {}).textContent || "",
    error: !!document.querySelector("#out .error-box"),
  }));

  const checks = {
    rdfStarRefusesNamingRdf12: /failed/i.test(refused.out) && /set Quoted triples to RDF 1\.2/.test(refused.out) && !refused.canOpen,
    trigBuilt: trig.canOpen && /\b3 triples\b/.test(trig.meta),
    turtleBuilt: ttl.canOpen && /\b7 triples\b/.test(ttl.meta),
    twoReifiersQueried: query.rows === 2 && !query.error && /example\.test\/dave/.test(query.text) && /example\.test\/frank/.test(query.text),
    noPageErrors: errs.length === 0,
  };
  const pass = Object.values(checks).every(Boolean);
  console.log(JSON.stringify({
    verdict: pass ? "PASS" : "FAIL",
    checks,
    refused: refused.out.slice(0, 200), trigMeta: trig.meta, ttlMeta: ttl.meta,
    rows: query.rows, errs: errs.slice(0, 4),
  }, null, 2));
  await browser.close();
  process.exit(pass ? 0 : 1);
};
main();
