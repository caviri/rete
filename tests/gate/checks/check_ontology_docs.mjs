// The ReSpec-style ontology reference: opening the Schema tab on a dataset with
// an embedded OWL ontology renders a documentation of its classes and
// object/datatype properties WITH their definitions — and it works over the
// remote async reader, independent of the schema pyramid. c4dt has a rich hand-
// written TBox; dblp ships --no-pyramid but still carries dblp.ttl, so its
// ontology must document even though the schema diagram is empty.
// Usage: node check_ontology_docs.mjs
import { launchBrowser } from "./_browser.mjs";

const readDocs = async (page, ds) => {
  await page.goto(`http://localhost:${process.env.PGPORT || "8090"}/playground.html#dataset=${ds}&load=lazy&mode=schema`,
    { waitUntil: "domcontentloaded" });
  await page.waitForFunction(() => window.PlaygroundEditor && document.getElementById("ontologyDocs"),
    { timeout: 60000 });
  for (let i = 0; i < 25; i++) {
    await page.waitForTimeout(1000);
    const t = await page.evaluate(() => (document.getElementById("ontologyDocs") || {}).textContent || "");
    if (!/reading the embedded/i.test(t) && t.length > 40) break;
  }
  return page.evaluate(() => {
    const el = document.getElementById("ontologyDocs");
    return {
      terms: el.querySelectorAll(".onto-class").length,          // class cards
      props: el.querySelectorAll(".onto-prop").length,           // property rows
      defs: el.querySelectorAll(".onto-def, .onto-prop-def").length,
      toc: el.querySelectorAll(".onto-toc a").length,
      links: el.querySelectorAll(".onto-range-link").length,     // class-to-class connections
      ttlBtn: el.querySelectorAll('[data-onto-view="ttl"]').length,
      txt: (el.textContent || "").replace(/\s+/g, " ").slice(0, 100),
    };
  });
};

const main = async () => {
  const browser = await launchBrowser();
  const page = await browser.newPage();
  const errs = [];
  page.on("pageerror", (e) => errs.push(String(e).slice(0, 160)));

  const c4dt = await readDocs(page, "c4dt");     // rich TBox with definitions (+ pyramid)
  const dblp = await readDocs(page, "dblp");     // --no-pyramid, but carries dblp.ttl
  const worldcup = await readDocs(page, "worldcup"); // NO formal ontology — effective schema from data

  // class-centric explorer: classes, per-class properties, class-to-class range
  // links (connections), definitions, and the Turtle toggle
  const c4dtOk = c4dt.terms >= 10 && c4dt.props >= 10 && c4dt.defs >= 5 &&
    c4dt.links >= 1 && c4dt.ttlBtn >= 1;
  const dblpOk = dblp.terms >= 5 && dblp.props >= 5;   // ontology docs despite no pyramid
  // "available for ALL datasets": a dataset with no formal OWL ontology still
  // gets a reference derived from its rdf:type classes + predicate usage
  const worldcupOk = worldcup.terms >= 5 && worldcup.props >= 5;
  const pass = c4dtOk && dblpOk && worldcupOk && errs.length === 0;

  console.log(JSON.stringify({
    verdict: pass ? "PASS" : "FAIL",
    c4dt, dblp, worldcup, c4dtOk, dblpOk, worldcupOk, errs: errs.slice(0, 3),
  }, null, 2));
  await browser.close();
  process.exit(pass ? 0 : 1);
};
main();
