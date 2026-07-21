// Regression lock for the async-reader Schema/ontology-preview fix (commit
// 1b747dc): a remote dataset that HAS a schema pyramid must render its classes,
// class-level relations and the ontology diagram on the ASYNC reader (the
// desktop default) — not trap with "null function" as it did before the raw
// *_url driver was wired in. Pairs with check_schema_empty (the no-pyramid case).
// Usage: node check_schema_render.mjs
import { launchBrowser } from "./_browser.mjs";

const main = async () => {
  const browser = await launchBrowser();
  const page = await browser.newPage();
  const errs = [];
  page.on("pageerror", (e) => errs.push(String(e).slice(0, 200)));
  const PORT = process.env.PGPORT || "8090";

  // worldcup is remote-lazy WITH a schema pyramid. Do NOT force sync — this must
  // work on the async default reader.
  await page.goto(`http://localhost:${PORT}/playground.html#dataset=worldcup&load=lazy&mode=schema`,
    { waitUntil: "domcontentloaded" });
  await page.waitForFunction(() => window.PlaygroundEditor && document.getElementById("schemaOut"),
    { timeout: 60000 });
  for (let i = 0; i < 20; i++) {
    await page.waitForTimeout(1000);
    const out = await page.evaluate(() => (document.getElementById("schemaOut") || {}).textContent || "");
    if (/classes and .* relation/i.test(out) || /null function|🐞|fast \(async\)/i.test(out)) break;
  }

  const s = await page.evaluate(() => ({
    out: (document.getElementById("schemaOut") || {}).textContent || "",
    classes: (document.getElementById("schemaClasses") || {}).textContent || "",
    relations: (document.getElementById("schemaRelations") || {}).textContent || "",
    diagram: (document.getElementById("ontologyDiagram") || {}).textContent || "",
  }));

  const rendered = /\d+ classes and \d+ class-level relations/i.test(s.out);
  const classesPopulated = /schema\.org|w3id\.org|wikidata|Q\d|Person|Team/i.test(s.classes) && s.classes.trim().length > 20;
  const diagramDrawn = s.diagram.trim().length > 50;
  const notTrapped = !/null function|engine tripped|🐞|fast \(async\)|no schema pyramid|no ontology schema/i.test(s.out);

  const pass = rendered && classesPopulated && diagramDrawn && notTrapped && errs.length === 0;
  console.log(JSON.stringify({
    verdict: pass ? "PASS" : "FAIL",
    rendered, classesPopulated, diagramDrawn, notTrapped,
    schemaOut: s.out.slice(0, 120),
    classes: s.classes.replace(/\s+/g, " ").slice(0, 90),
    diagramChars: s.diagram.length,
    errs: errs.slice(0, 3),
  }, null, 2));
  await browser.close();
  process.exit(pass ? 0 : 1);
};
main();
