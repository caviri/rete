// Regression for the "stale scholar schema" bug: opening a file that has NO
// schema pyramid must show an honest "no ontology schema" message AND must NOT
// leave the previously-viewed dataset's classes, relations or ontology diagram
// in the schema panels.
//
// The file is the worldcup2026 gate fixture, which has no rdf:type statement.
// A catalog dataset used to stand in (dblp, built --no-pyramid), but converting
// a file to format 0x08 writes its schema, so no catalog file stays without one.
// Usage: node check_schema_empty.mjs
import { createServer } from "node:http";
import { readFile } from "node:fs/promises";
import { launchBrowser } from "./_browser.mjs";

const listen = (server) => new Promise((resolve) => server.listen(0, "127.0.0.1", () => resolve(server.address().port)));

const main = async () => {
  let fixture = null;
  try {
    fixture = await readFile("/work/tests/gate/.cache/worldcup2026.rete");
  } catch (e) {
    console.log(JSON.stringify({ verdict: "FAIL", error: "tests/gate/.cache/worldcup2026.rete missing — run tests/gate/fixtures.sh" }));
    process.exit(1);
  }
  const server = createServer((req, res) => {
    const common = {
      "Access-Control-Allow-Origin": "*",
      "Access-Control-Expose-Headers": "Content-Range,Content-Length,Accept-Ranges",
      "Accept-Ranges": "bytes",
    };
    const range = req.headers.range && /bytes=(\d+)-(\d*)/.exec(req.headers.range);
    if (range) {
      const start = Number(range[1]);
      const end = range[2] ? Math.min(Number(range[2]), fixture.length - 1) : fixture.length - 1;
      const body = fixture.subarray(start, end + 1);
      res.writeHead(206, { ...common, "Content-Type": "application/octet-stream", "Content-Range": `bytes ${start}-${end}/${fixture.length}`, "Content-Length": body.length });
      res.end(body);
      return;
    }
    res.writeHead(200, { ...common, "Content-Type": "application/octet-stream", "Content-Length": fixture.length });
    res.end(req.method === "HEAD" ? undefined : fixture);
  });
  const fixturePort = await listen(server);
  const browser = await launchBrowser();
  const page = await browser.newPage();
  const errs = [];
  page.on("pageerror", (e) => errs.push(String(e.stack || e).slice(0, 900)));
  const PORT = process.env.PGPORT || "8090";

  // Opening the Schema tab must show the honest "no ontology schema" message,
  // with every schema panel — summary, classes, relations, ontology diagram —
  // empty. Before the fix, the no-pyramid path only rewrote schemaOut, leaving
  // the previously rendered dataset's classes and ontology diagram in the panels.
  await page.goto(`http://localhost:${PORT}/playground.html#url=${encodeURIComponent(`http://127.0.0.1:${fixturePort}/worldcup2026.rete`)}&load=lazy&mode=schema`,
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
  server.close();
  process.exit(pass ? 0 : 1);
};
main();
