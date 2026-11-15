// Regression for the "stale scholar schema" bug: opening a dataset that has NO
// schema pyramid (crossref ships without one) must show an honest "no ontology
// schema" message AND must NOT leave the previously-viewed dataset's classes,
// relations or ontology diagram in the schema panels.
// Usage: node check_schema_empty.mjs
import { launchBrowser } from "./_browser.mjs";

const main = async () => {
  const browser = await launchBrowser();
  const page = await browser.newPage();
  const errs = [];
  page.on("pageerror", (e) => errs.push(String(e).slice(0, 200)));
  const PORT = process.env.PGPORT || "8090";

  // crossref (format 0x05, built --no-pyramid) has no schema. dblp was the
  // example until its 2026-07-01 file: convert to 0x08 writes the schema section,
  // so this needs a fixture of its own once crossref is converted. Opening the Schema
  // tab must show the honest "no ontology schema" message, with every schema
  // panel — summary, classes, relations, ontology diagram — empty. Before the
  // fix, the no-pyramid path only rewrote schemaOut, leaving the previously
  // rendered dataset's classes and ontology diagram sitting in the panels.
  await page.goto(`http://localhost:${PORT}/playground.html#dataset=crossref&load=lazy&mode=schema`,
    { waitUntil: "domcontentloaded" });
  await page.waitForFunction(() => window.PlaygroundEditor && document.getElementById("schemaOut"),
    { timeout: 60000 });
  // wait until the schema read resolves into an honest note (either "no ontology
  // schema" for a genuine no-pyramid file, or the async-reader limitation note)
  // or timeout. Crucially it must NOT be the generic 🐞 engine-error card and
  // must NOT leave a previous dataset's classes/diagram behind.
  for (let i = 0; i < 20; i++) {
    await page.waitForTimeout(1000);
    const out = await page.evaluate(() => (document.getElementById("schemaOut") || {}).textContent || "");
    if (/no ontology schema|no schema pyramid|async.*reader|fast \(async\)/i.test(out)) break;
  }

  const state = await page.evaluate(() => ({
    out: (document.getElementById("schemaOut") || {}).textContent || "",
    classes: (document.getElementById("schemaClasses") || {}).textContent || "",
    relations: (document.getElementById("schemaRelations") || {}).textContent || "",
    diagram: (document.getElementById("ontologyDiagram") || {}).textContent || "",
  }));

  const honestNote = /no ontology schema|no schema pyramid|async.*reader|fast \(async\)/i.test(state.out);
  const panelsEmpty = !state.classes.trim() && !state.relations.trim() && !state.diagram.trim();
  const notGenericError = !/engine tripped|🐞/i.test(state.out);

  const pass = honestNote && panelsEmpty && notGenericError && errs.length === 0;
  console.log(JSON.stringify({
    verdict: pass ? "PASS" : "FAIL",
    honestNote, panelsEmpty, notGenericError,
    schemaOut: state.out.slice(0, 160),
    leftoverClasses: state.classes.slice(0, 80),
    errs: errs.slice(0, 3),
  }, null, 2));
  await browser.close();
  process.exit(pass ? 0 : 1);
};
main();
