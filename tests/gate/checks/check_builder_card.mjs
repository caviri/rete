// The in-browser builder must be able to write the Dataset Card the file will
// CARRY — not just a catalog listing — and the round trip has to close: author
// a card in step 3, build, open the result, and read it back in the 🏷 Card
// modal that the same page renders.
//
// It also pins the two boundaries that keep the feature honest:
//   * validation is the ENGINE's, so a card the CLI would refuse is refused
//     here with the CLI's own words (a free-text `theme`, a stray top-level key);
//   * a browser build derives no profile and records no build conditions, so
//     those must render as ABSENT — never as empty lists or zeroed measurements.
//
// No network is involved.
import { launchBrowser } from "./_browser.mjs";

const FIXTURE = `<http://example.test/alice> <http://example.test/knows> <http://example.test/bob> .
<http://example.test/bob> <http://example.test/name> "Bob"@en .
<http://example.test/alice> <http://example.test/name> "Alice"@en .`;

// Every curated field, so the round trip covers the whole surface the viewer
// now renders — including the two the CLI validates specially.
const CARD = {
  title: "Browser-authored graph",
  description: "Built in the page, card and all.",
  license: "CC0-1.0",
  source: "https://example.org/browser-source",
  version: "1.2.3",
  created: "2026-08-04",
  source_date: "2026-08-01",
  creators: [{ name: "Grace Hopper", orcid: "https://orcid.org/0000-0001-2345-6789" }],
  publisher: { name: "Browser Publishing", ror: "https://ror.org/02mhbdp94" },
  canonical_url: "https://example.org/browser.rete",
  sparql_endpoint: "https://example.org/browser/sparql",
  derived_from: ["https://example.org/browser-input.nt"],
  doi: "https://doi.org/10.5281/zenodo.1234567",
  cite_as: "Hopper, G. (2026). Browser-authored graph.",
  keywords: ["zeta", "alpha"],
  theme: ["http://publications.europa.eu/resource/authority/data-theme/TECH"],
  extra: { internal_id: "BR-1", review: { status: "draft" } },
};

const main = async () => {
  const browser = await launchBrowser();
  const page = await browser.newPage();
  const errs = [];
  const failures = [];
  page.on("pageerror", (e) => errs.push(String(e).slice(0, 240)));
  page.on("console", (m) => { if (m.type() === "error") errs.push("console: " + m.text().slice(0, 200)); });
  const PORT = process.env.PGPORT || "8090";
  await page.goto(`http://localhost:${PORT}/playground.html#dataset=scholar&mode=sparql`, { waitUntil: "domcontentloaded" });
  await page.waitForFunction(() => window.PlaygroundEditor && document.getElementById("buildBtn"), { timeout: 60000 });
  await page.click("#buildBtn");
  await page.evaluate((text) => window.PlaygroundEditor.setText("buildText", text), FIXTURE);
  await page.fill("#cardKey", "browser-card-fixture");

  const typeCard = async (obj) => {
    await page.fill("#cardCode", typeof obj === "string" ? obj : JSON.stringify(obj, null, 2));
    // `fill` fires `input`, which is what drives validation.
    return page.evaluate(() => (document.getElementById("cardCodeMsg") || {}).textContent || "");
  };

  // ---- the engine's validation, surfaced in the editor ----------------------
  // The messages must be the CLI's, not a re-statement: a re-statement is how
  // the browser would start accepting cards `rete build --card-file` refuses.
  for (const [label, doc, mustSay] of [
    ["a free-text theme", { theme: ["physics"] }, /not an IRI/],
    ["a theme, pointing at keywords", { theme: ["physics"] }, /keywords/],
    ["a stray top-level key", { title: "T", region: "CH" }, /unknown field .?region/],
    ["an over-deep extra value", { extra: { a: { b: { c: { d: 1 } } } } }, /level cap/],
  ]) {
    const msg = await typeCard(doc);
    if (!mustSay.test(msg)) failures.push(`editor did not report ${label} the way the CLI does: "${msg}"`);
  }
  // Unparseable JSON is reported as such, and does not silently build.
  const broken = await typeCard("{ not json");
  if (!/invalid|not JSON/i.test(broken)) failures.push(`broken JSON was not flagged: "${broken}"`);

  // A rejected card must block the build rather than quietly dropping the card.
  await page.click("#buildRun");
  await page.waitForFunction(() => /rejected/i.test((document.getElementById("buildOut") || {}).textContent || ""), { timeout: 20000 })
    .catch(() => failures.push("a build with an invalid card was not stopped"));

  // ---- the real card --------------------------------------------------------
  const okMsg = await typeCard(CARD);
  if (!/valid card/i.test(okMsg)) failures.push(`a valid card was not accepted: "${okMsg}"`);
  // The form and the JSON are one document: the title typed on the left has to
  // be the title in the card, and editing the JSON must not have wiped the form.
  const titleField = await page.inputValue("#cardTitle");
  if (titleField !== CARD.title) failures.push(`the form did not pick up the card's title: "${titleField}"`);
  await page.fill("#cardLicense", "CC-BY-4.0");
  const patched = await page.evaluate(() => JSON.parse(document.getElementById("cardCode").value));
  if (patched.license !== "CC-BY-4.0") failures.push("editing the form did not patch the card");
  if (!patched.creators || patched.creators.length !== 1) {
    failures.push("editing the form dropped a curated field the form does not own");
  }

  await page.click("#buildRun");
  await page.waitForFunction(() => /Saved|Built/.test((document.getElementById("buildOut") || {}).textContent || ""), { timeout: 30000 });
  const built = await page.evaluate(() => ({
    out: (document.getElementById("buildOut") || {}).textContent || "",
    canOpen: !(document.getElementById("buildOpen") || {}).disabled,
  }));
  if (!built.canOpen) failures.push("the build produced nothing to open");
  // The result must say what a browser build cannot do, rather than leaving the
  // missing profile looking like a defect in the file.
  if (!/carries your Dataset Card/i.test(built.out)) failures.push("the build did not report that the card was written");
  if (!/derived profile/i.test(built.out) || !/build record/i.test(built.out)) {
    failures.push("the build did not say the derived profile and build record are CLI-only");
  }

  // ---- read it back in the viewer ------------------------------------------
  await page.click("#buildOpen");
  await page.waitForFunction(() => /browser-authored graph/i.test((document.getElementById("dsName") || {}).textContent || ""), { timeout: 20000 });
  await page.click("#cardBtn");
  await page.waitForFunction(
    () => !document.getElementById("cardModal").classList.contains("hidden") &&
          !/Reading the card/.test(document.getElementById("cardBody").textContent),
    { timeout: 30000 },
  );
  const shown = await page.evaluate(() => {
    const body = document.getElementById("cardBody");
    return {
      title: document.getElementById("cardModalTitle").textContent,
      text: body.textContent,
      hrefs: [...body.querySelectorAll("a[href]")].map((a) => a.getAttribute("href")),
      chips: [...body.querySelectorAll(".card-chip")].map((e) => e.textContent.trim()),
      extraKeys: [...body.querySelectorAll(".card-extra > tbody > tr > td.card-x-key")].map((e) => e.textContent.trim()),
      stats: [...body.querySelectorAll(".card-stat b")].map((e) => e.textContent),
      build: (body.querySelector(".card-build") || {}).textContent || "",
      costs: body.querySelectorAll(".card-cost").length,
      sections: [...body.querySelectorAll(".card-sec > summary")].map((e) => e.textContent.trim()),
    };
  });

  if (!/Browser-authored graph/.test(shown.title)) failures.push(`viewer title: ${shown.title}`);
  for (const [label, re] of [
    ["description", /Built in the page/],
    ["version", /1\.2\.3/],
    ["creator", /Grace Hopper/],
    ["publisher", /Browser Publishing/],
    ["cite_as", /Hopper, G\. \(2026\)/],
  ]) {
    if (!re.test(shown.text)) failures.push(`the authored ${label} did not survive the round trip`);
  }
  for (const href of ["https://orcid.org/0000-0001-2345-6789", "https://ror.org/02mhbdp94",
                      "https://doi.org/10.5281/zenodo.1234567",
                      "http://publications.europa.eu/resource/authority/data-theme/TECH"]) {
    if (!shown.hrefs.includes(href)) failures.push(`${href} did not round-trip as a link`);
  }
  if (!shown.chips.some((c) => /alpha/.test(c))) failures.push("keywords did not round-trip");
  if (!shown.chips.some((c) => /EU Data Themes/.test(c))) failures.push("theme scheme not shown after the round trip");
  for (const k of ["internal_id", "review"]) {
    if (!shown.extraKeys.includes(k)) failures.push(`extra key ${k} did not round-trip`);
  }
  // The counts are the build's own measurement, so they are real.
  if (!shown.stats.length) failures.push("the browser-written card carries no counts");

  // The honest boundary: no derived profile, no build record — as ABSENCE.
  for (const derived of ["Predicates", "Classes", "Vocabularies", "Example queries", "Signals"]) {
    if (shown.sections.some((s) => s.startsWith(derived))) {
      failures.push(`a browser build rendered a derived section it never computed: ${derived}`);
    }
  }
  if (shown.costs) failures.push("a browser build showed per-query costs it never measured");
  if (!/no build record/i.test(shown.build)) {
    failures.push(`a file without build info did not say so: "${shown.build.slice(0, 140)}"`);
  }
  if (/\b0 ms\b/.test(shown.build) || /Built at\s*$/.test(shown.build)) {
    failures.push("the absent build record rendered as zeros/blanks instead of absence");
  }

  if (errs.length) failures.push(`page errors: ${errs.slice(0, 2).join(" | ")}`);
  const pass = failures.length === 0;
  console.log(JSON.stringify({
    verdict: pass ? "PASS" : "FAIL",
    note: "in-browser build writes the file's Dataset Card; CLI-identical validation; absent derived profile + build record render as absence",
    sections: shown.sections,
    failures,
  }, null, 2));
  await browser.close();
  process.exit(pass ? 0 : 1);
};

main().catch((e) => {
  console.log(JSON.stringify({ verdict: "FAIL", error: String(e && e.message).slice(0, 300) }, null, 2));
  process.exit(1);
});
