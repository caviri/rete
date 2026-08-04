// The Dataset Card button must open the card that travels inside the .rete,
// in both the rendered and the JSON view — and over BOTH read paths, which are
// different code: a resident graph answers from memory, a remote one routes
// through the worker because card_url does synchronous range XHR.
//
// The remote path is served from a local range server carrying a REAL card, so
// the check also pins the property that makes the card useful: reading it costs
// a couple of small ranged reads, not the whole file.
import { createServer } from "node:http";
import { readFile } from "node:fs/promises";
import { launchBrowser } from "./_browser.mjs";

const listen = (server) => new Promise((resolve) => server.listen(0, "127.0.0.1", () => resolve(server.address().port)));

const main = async () => {
  // Built by the gate's own setup step, and built WITH a card (see run.mjs).
  const fixture = await readFile("/work/tests/gate/.cache/card-fixture.rete");
  const traffic = { full: 0, head: 0, range: 0, bytes: 0 };
  const server = createServer((req, res) => {
    if (req.url?.split("?")[0] !== "/carded.rete") { res.writeHead(404); res.end("nope"); return; }
    const common = {
      "Access-Control-Allow-Origin": "*",
      "Access-Control-Expose-Headers": "Content-Range,Content-Length,Accept-Ranges",
      "Accept-Ranges": "bytes",
    };
    const range = req.headers.range && /bytes=(\d+)-(\d*)/.exec(req.headers.range);
    if (range) {
      traffic.range++;
      const start = Number(range[1]);
      const end = range[2] ? Math.min(Number(range[2]), fixture.length - 1) : fixture.length - 1;
      const body = fixture.subarray(start, end + 1);
      traffic.bytes += body.length;
      res.writeHead(206, { ...common, "Content-Type": "application/octet-stream", "Content-Range": `bytes ${start}-${end}/${fixture.length}`, "Content-Length": body.length });
      res.end(body);
      return;
    }
    // A HEAD carries no body — the reader uses one to learn Content-Length
    // before it can ask for a range. Counting it as a "download" would be a
    // measurement bug, so it is tracked separately.
    if ((req.method || "GET").toUpperCase() === "HEAD") {
      traffic.head++;
      res.writeHead(200, { ...common, "Content-Type": "application/octet-stream", "Content-Length": fixture.length });
      res.end();
      return;
    }
    traffic.full++;
    res.writeHead(200, { ...common, "Content-Type": "application/octet-stream", "Content-Length": fixture.length });
    res.end(fixture);
  });
  const port = await listen(server);

  const PGPORT = process.env.PGPORT || "8090";
  const browser = await launchBrowser();
  const failures = [];
  const pageErrors = [];

  const open = async (hash) => {
    const page = await browser.newPage();
    page.on("pageerror", (e) => pageErrors.push(String(e).slice(0, 200)));
    await page.goto(`http://localhost:${PGPORT}/playground.html${hash}`, { waitUntil: "domcontentloaded" });
    await page.waitForFunction(
      () => document.getElementById("run") && !document.getElementById("run").disabled,
      { timeout: 90000 },
    );
    return page;
  };

  const openCard = async (page) => {
    await page.click("#cardBtn");
    await page.waitForFunction(
      () => !document.getElementById("cardModal").classList.contains("hidden") &&
            !/Reading the card/.test(document.getElementById("cardBody").textContent),
      { timeout: 60000 },
    );
  };

  // ---- remote path: card_url through the worker -----------------------------
  const remote = await open(`#url=${encodeURIComponent(`http://127.0.0.1:${port}/carded.rete`)}`);
  // Measure the CARD READ, not the session: opening the graph is separate
  // traffic, and charging it to the card would be an attribution error.
  const before = { ...traffic };
  await openCard(remote);
  const cardRead = {
    full: traffic.full - before.full,
    head: traffic.head - before.head,
    range: traffic.range - before.range,
    bytes: traffic.bytes - before.bytes,
  };

  const rendered = await remote.evaluate(() => ({
    title: document.getElementById("cardModalTitle").textContent,
    body: document.getElementById("cardBody").textContent.slice(0, 4000),
    foot: document.getElementById("cardFootNote").textContent,
    stats: [...document.querySelectorAll("#cardBody .card-stat b")].map((e) => e.textContent),
  }));
  if (!/one header \+ one coalesced range/.test(rendered.foot)) {
    failures.push(`remote footnote does not state the CARD tier's budget: "${rendered.foot}"`);
  }
  if (!/build record/.test(rendered.foot)) {
    failures.push(`the footnote does not mention the build record this file carries: "${rendered.foot}"`);
  }
  if (!/Gate Card Fixture/.test(rendered.title)) failures.push(`rendered title missing the card's title: ${rendered.title}`);
  if (!/a fixture card/i.test(rendered.body)) failures.push("rendered view does not show the card description");
  if (!rendered.stats.length) failures.push("rendered view shows no counts");

  // ---- the description is Markdown, and ONLY Markdown ------------------------
  // The fixture's description carries every construct the viewer supports, plus
  // a <script>, an <img onerror=> and a javascript: link. Both halves matter:
  // the formatting must APPEAR, and none of the markup may have become an
  // element. Raw HTML is not a supported description format — that is the whole
  // reason Markdown is (see docs/dataset-cards.md).
  const desc = await remote.evaluate(() => {
    const d = document.querySelector("#cardBody .card-desc");
    if (!d) return null;
    const tags = (sel) => [...d.querySelectorAll(sel)].map((e) => e.textContent.trim());
    return {
      // Headings are shifted under the modal's own <h3>: a card never emits h1/h2/h3.
      h1h2h3: d.querySelectorAll("h1, h2, h3").length,
      h4: tags("h4"), h5: tags("h5"), h6: tags("h6"),
      uls: d.querySelectorAll("ul").length,
      lis: tags("ul > li"),
      nested: tags("li > ul > li"),
      ols: d.querySelectorAll("ol").length,
      olis: tags("ol > li"),
      quote: tags("blockquote").join(" "),
      hrs: d.querySelectorAll("hr").length,
      pre: tags("pre code").join(" "),
      strong: tags("strong"), code: tags("code"),
      hrefs: [...d.querySelectorAll("a[href]")].map((a) => a.getAttribute("href")),
      // Nothing from the description may have become live markup.
      injected: d.querySelectorAll("script, img, iframe, object, embed").length,
      inlineHandlers: [...d.querySelectorAll("*")].filter((e) =>
        [...e.attributes].some((a) => /^on/i.test(a.name))).length,
      // …and the raw markup must still be READABLE as text, not vanished.
      text: d.textContent,
      pwned: window.__cardDescPwned,
    };
  });
  if (!desc) {
    failures.push("the card description did not render a .card-desc block");
  } else {
    if (desc.h1h2h3) failures.push(`a card description emitted ${desc.h1h2h3} h1/h2/h3 — headings must sit under the modal's own <h3>`);
    if (!desc.h4.some((t) => /A level-one heading/.test(t))) failures.push(`"# " did not become an <h4>: ${JSON.stringify(desc.h4)}`);
    if (!desc.h5.some((t) => /A level-two heading/.test(t))) failures.push(`"## " did not become an <h5>: ${JSON.stringify(desc.h5)}`);
    if (!desc.lis.includes("a bullet")) failures.push(`bullets did not become list items: ${JSON.stringify(desc.lis)}`);
    if (!desc.nested.includes("a nested bullet")) failures.push(`an indented bullet did not nest: ${JSON.stringify(desc.nested)}`);
    if (!desc.ols) failures.push("a numbered list did not become an <ol>");
    if (!desc.olis.includes("first")) failures.push(`ordered items missing: ${JSON.stringify(desc.olis)}`);
    if (!/A quoted line/.test(desc.quote)) failures.push(`"> " did not become a blockquote: "${desc.quote}"`);
    if (!desc.hrs) failures.push('"---" did not become a rule');
    if (!/SELECT \* WHERE/.test(desc.pre)) failures.push(`a fenced block did not become <pre><code>: "${desc.pre}"`);
    if (!desc.strong.includes("bold")) failures.push("**bold** did not render");
    if (!desc.code.some((c) => /rete build/.test(c))) failures.push(`\`code\` did not render: ${JSON.stringify(desc.code)}`);
    if (!desc.hrefs.includes("https://example.org/gate-desc-link")) {
      failures.push(`a markdown link is not a link: ${JSON.stringify(desc.hrefs)}`);
    }
    // --- injection: the half that must NOT have happened ---
    if (desc.injected) failures.push(`the description created ${desc.injected} script/img/frame elements`);
    if (desc.inlineHandlers) failures.push(`the description created ${desc.inlineHandlers} elements with an on* handler`);
    if (desc.pwned !== undefined) failures.push("a <script> inside the description EXECUTED");
    if (desc.hrefs.some((h) => /^javascript:/i.test(h))) failures.push(`a javascript: link survived: ${JSON.stringify(desc.hrefs)}`);
    if (!/<script>/.test(desc.text)) failures.push("the escaped <script> is not readable as text — it was swallowed, not escaped");
  }

  // The sidebar description is a <p>: it must stay flat. A <ul>/<h4> in there
  // would be re-parented by the HTML parser and tear the layout apart — which is
  // why the block renderer is scoped to the modal.
  const sidebar = await remote.evaluate(() => {
    const p = document.getElementById("dsDesc");
    return {
      blocks: p.querySelectorAll("h1,h2,h3,h4,h5,h6,ul,ol,li,blockquote,hr,pre,p,div").length,
      injected: p.querySelectorAll("script, img").length,
      text: p.textContent,
      tagline: (document.getElementById("dsTagline") || {}).textContent || "",
    };
  });
  if (sidebar.blocks) failures.push(`#dsDesc rendered ${sidebar.blocks} block elements inside a <p>`);
  if (sidebar.injected) failures.push("#dsDesc turned card text into live markup");
  // Flattened, not stripped: the heading's WORDS survive, its "## " does not,
  // and bullets read as bullets instead of as stray hyphens.
  if (!/A level-one heading/.test(sidebar.text)) failures.push("#dsDesc dropped the heading text instead of flattening it");
  if (/#+\s*A level-one heading/.test(sidebar.text)) failures.push(`#dsDesc shows a raw heading marker: "${sidebar.text.slice(0, 120)}"`);
  if (!/•\s*a bullet/.test(sidebar.text)) failures.push(`#dsDesc did not flatten bullets: "${sidebar.text.slice(0, 200)}"`);
  if (/[#*`>]/.test(sidebar.tagline)) failures.push(`the header tagline shows raw markdown: "${sidebar.tagline}"`);

  // The point of the CARD tier: reading the card is a couple of small ranged
  // reads, never a whole-file GET. At this fixture's size the card is most of
  // the file, so the BYTE ratio proves little here — the assertion that carries
  // weight is that no unranged download happened, and that the request count
  // stays in single digits however big the file gets.
  if (cardRead.full > 0) failures.push(`reading the card pulled the WHOLE file ${cardRead.full}×`);
  if (cardRead.range === 0) failures.push("reading the card issued no range request at all");
  if (cardRead.range > 6) failures.push(`card read took ${cardRead.range} range requests — expected ~2`);

  // ---- JSON view: coloured, and the card's own bytes -------------------------
  const json = await remote.evaluate(() => {
    document.getElementById("cardTabJson").click();
    const pre = document.querySelector("#cardBody pre.card-json");
    return {
      text: pre ? pre.textContent.slice(0, 3000) : "",
      keys: pre ? pre.querySelectorAll("span.k").length : 0,
      nums: pre ? pre.querySelectorAll("span.n").length : 0,
      // A raw < in the card must not have become a real element.
      injected: !!(pre && pre.querySelector("script, img")),
    };
  });
  if (!json.text.trim().startsWith("{")) failures.push("JSON tab does not show a JSON object");
  if (json.keys < 3) failures.push(`JSON view has ${json.keys} coloured keys — not highlighted`);
  if (json.nums < 1) failures.push("JSON view coloured no numbers");
  if (json.injected) failures.push("card text was injected as live HTML");
  if (!/"Gate Card Fixture"/.test(json.text)) failures.push("JSON view is not this file's card");

  // ---- a card query loads into the editor ------------------------------------
  const used = await remote.evaluate(() => {
    document.getElementById("cardTabView").click();
    const b = document.querySelector("#cardBody .card-q-use");
    if (!b) return { ok: false, why: "no Use button on any card query" };
    b.click();
    return {
      ok: true,
      hidden: document.getElementById("cardModal").classList.contains("hidden"),
      // setText mirrors into the textarea, which is the only readable handle —
      // PlaygroundEditor exposes no getter.
      q: (document.getElementById("q") || {}).value || "",
    };
  });
  if (!used.ok) failures.push(used.why);
  else {
    if (!used.hidden) failures.push("using a card query left the modal open");
    if (!/SELECT/i.test(used.q)) failures.push(`card query did not reach the editor: "${used.q.slice(0, 60)}"`);
  }

  // ---- the CURATED fields, and the build record ------------------------------
  // Two fixtures, same triples, different cards: card-full carries every curated
  // field, card-fixture carries none of them. Rendering only the first would
  // prove the fields appear; rendering BOTH is what proves a missing field
  // renders as absence rather than as an empty row that reads like a measurement.
  const fullBytes = await readFile("/work/tests/gate/.cache/card-full.rete");
  const fullTraffic = { full: 0, head: 0, range: 0 };
  const fullServer = createServer((req, res) => {
    const common = { "Access-Control-Allow-Origin": "*", "Access-Control-Expose-Headers": "Content-Range,Content-Length,Accept-Ranges", "Accept-Ranges": "bytes" };
    const r = req.headers.range && /bytes=(\d+)-(\d*)/.exec(req.headers.range);
    if (r) {
      fullTraffic.range++;
      const s = Number(r[1]);
      const e = r[2] ? Math.min(Number(r[2]), fullBytes.length - 1) : fullBytes.length - 1;
      const body = fullBytes.subarray(s, e + 1);
      res.writeHead(206, { ...common, "Content-Range": `bytes ${s}-${e}/${fullBytes.length}`, "Content-Length": body.length });
      res.end(body); return;
    }
    if ((req.method || "GET").toUpperCase() === "HEAD") {
      fullTraffic.head++;
      res.writeHead(200, { ...common, "Content-Length": fullBytes.length }); res.end(); return;
    }
    fullTraffic.full++;
    res.writeHead(200, { ...common, "Content-Length": fullBytes.length }); res.end(fullBytes);
  });
  const fullPort = await listen(fullServer);
  const full = await open(`#url=${encodeURIComponent(`http://127.0.0.1:${fullPort}/full.rete`)}`);
  const fullBefore = { ...fullTraffic };
  await openCard(full);
  const fullRead = { full: fullTraffic.full - fullBefore.full, range: fullTraffic.range - fullBefore.range };

  const rich = await full.evaluate(() => {
    const body = document.getElementById("cardBody");
    // Sections are <details>; their text is in the DOM whether open or not.
    return {
      text: body.textContent,
      html: body.innerHTML,
      chips: [...body.querySelectorAll(".card-chip")].map((e) => e.textContent.trim()),
      // Every link the rendered card offers, so identifier links can be asserted
      // as links rather than as text that happens to contain a URL.
      hrefs: [...body.querySelectorAll("a[href]")].map((a) => a.getAttribute("href")),
      metaLabels: [...body.querySelectorAll(".card-meta td.card-k")].map((e) => e.textContent.trim()),
      extraKeys: [...body.querySelectorAll(".card-extra > tbody > tr > td.card-x-key")].map((e) => e.textContent.trim()),
      hasExtraSub: !!body.querySelector(".card-x-sub"),
      buildBlock: (body.querySelector(".card-build") || {}).textContent || "",
      buildRows: [...document.querySelectorAll(".card-build .card-meta td.card-k")].map((e) => e.textContent.trim()),
      costs: [...body.querySelectorAll(".card-cost")].map((e) => e.textContent.trim()),
      citeBtn: !!body.querySelector(".card-cite-copy"),
    };
  });

  // Reading the card AND the build record must stay inside the CARD tier's
  // budget: the writer lays the build-info section immediately after the card so
  // one coalesced range covers both. `rete-core` pins that at the reader; what
  // this pins is the BROWSER path — that the playground did not "add build info"
  // by making a second call. Measured on the SAME file, card-only against
  // card+build, so the comparison isolates the extra section and nothing else.
  //
  // Each engine call opens its own reader with a cold block cache, so a modal
  // that fetched the build record with a SECOND call would show up here as
  // roughly double the requests. Staying level with the card-only fixture's read
  // is what says it is still one call, one header, one coalesced range. (The
  // reader-level property — that the two sections come back in a single range —
  // is pinned in rete-core by
  // `card_and_build_info_ranged_is_one_header_plus_one_range`; the block cache
  // rounds offsets to blocks, so it is not observable from here.)
  if (fullRead.full > 0) failures.push(`card+build read pulled the WHOLE file ${fullRead.full}×`);
  if (fullRead.range === 0) failures.push("card+build read issued no range request");
  if (fullRead.range > cardRead.range) {
    failures.push(
      `reading card+build cost ${fullRead.range} range requests against ${cardRead.range} for the ` +
      `sibling fixture — the modal is making more than one engine call`,
    );
  }

  // -- the curated fields are RENDERED (not merely present in the JSON tab) --
  const wanted = [
    ["version", /2026\.08/],
    ["source_date", /2026-07-15/],
    ["creators", /Ada Lovelace/],
    ["creator without an ORCID", /A Creator Without An Identifier/],
    ["publisher", /Gate Fixtures Institute/],
    ["cite_as", /Lovelace, A\. \(2026\)/],
  ];
  for (const [label, re] of wanted) {
    if (!re.test(rich.text)) failures.push(`rendered view does not show ${label}`);
  }
  for (const label of ["Version", "Creators", "Publisher", "DOI", "Canonical copy", "SPARQL endpoint", "Derived from", "Cite as", "Source date"]) {
    if (!rich.metaLabels.includes(label)) failures.push(`identity table has no "${label}" row`);
  }
  // Identifiers are rendered as LINKS to the identifier — that is the whole
  // value of the card asking for an ORCID/ROR IRI rather than a string.
  for (const href of [
    "https://orcid.org/0000-0002-1825-0097",
    "https://ror.org/01ggx4157",
    "https://doi.org/10.5281/zenodo.9999999",
    "https://example.org/gate/card-full.rete",
    "https://example.org/gate/sparql",
    "https://example.org/gate/upstream.rete",
  ]) {
    if (!rich.hrefs.includes(href)) failures.push(`${href} is not rendered as a link`);
  }
  if (!rich.citeBtn) failures.push("cite_as has no copy button");

  // keywords → chips; theme → chips naming the SCHEME (read from the IRI, never
  // fetched) and linking the concept.
  if (!rich.chips.some((c) => /alpha-keyword/.test(c))) failures.push("keywords are not rendered");
  if (!rich.chips.some((c) => /EU Data Themes/.test(c))) failures.push("theme does not name its concept scheme");
  if (!rich.chips.some((c) => /Wikidata/.test(c))) failures.push("second theme scheme not recognized");
  if (!rich.hrefs.includes("http://publications.europa.eu/resource/authority/data-theme/EDUC")) {
    failures.push("theme IRI is not a link to the concept");
  }

  // The extra bag: publisher-defined, no agreed meaning — shown, and SAID to be
  // the publisher's own.
  for (const k of ["internal_id", "pipeline_stage", "review", "mirrors"]) {
    if (!rich.extraKeys.includes(k)) failures.push(`extra bag key ${k} is not rendered`);
  }
  if (!rich.hasExtraSub) failures.push("a nested extra value did not get its own key/value table");
  if (!/publisher's own/i.test(rich.text) || !/no meaning/i.test(rich.text)) {
    failures.push("the extra bag is not labelled as the publisher's own, meaning-free fields");
  }

  // The build record: present, separated, and its cost figures sit with the
  // queries they describe.
  if (!/Build record/.test(rich.buildBlock)) failures.push("no build record section");
  for (const label of ["Built at", "Builder"]) {
    if (!rich.buildRows.includes(label)) failures.push(`build record has no "${label}" row`);
  }
  if (!/rete-cli/.test(rich.buildBlock)) failures.push("build record does not name the builder");
  if (!rich.costs.length) failures.push("no per-query cost line next to any query");
  if (!rich.costs.some((c) => /range request/.test(c) && /row/.test(c))) {
    failures.push(`cost line does not carry the portable figures: ${rich.costs[0]}`);
  }
  if (!rich.costs.some((c) => /on the build machine/.test(c))) {
    failures.push("debug_ms is shown without saying it is one machine's reference");
  }

  // -- ABSENT, not empty: the plain fixture carries none of the above ---------
  const plain = await remote.evaluate(() => {
    const body = document.getElementById("cardBody");
    return {
      text: body.textContent,
      metaLabels: [...body.querySelectorAll(".card-meta td.card-k")].map((e) => e.textContent.trim()),
      chips: body.querySelectorAll(".card-chip").length,
      extra: body.querySelectorAll(".card-extra").length,
      emptyCells: [...body.querySelectorAll(".card-meta td:nth-child(2)")]
        .filter((td) => !td.textContent.trim()).length,
      buildBlock: (body.querySelector(".card-build") || {}).textContent || "",
    };
  });
  await remote.evaluate(() => document.getElementById("cardTabView").click());
  if (plain.chips) failures.push(`a card without keywords/theme rendered ${plain.chips} chips`);
  if (plain.extra) failures.push("a card without an extra bag rendered the extra section");
  if (plain.emptyCells) failures.push(`${plain.emptyCells} identity rows rendered with an empty value`);
  for (const label of ["Creators", "Publisher", "DOI", "Cite as", "Version"]) {
    if (plain.metaLabels.includes(label)) failures.push(`"${label}" row rendered for a card that has no such field`);
  }
  // This fixture IS built with a card, so it does have a build record; the
  // absent-build-record wording is asserted on the cardless file further down.
  if (!/Build record/.test(plain.buildBlock)) failures.push("carded fixture shows no build record");

  await full.close();
  fullServer.close();

  // ---- resident path: Rete.card() from memory --------------------------------
  // Different code from the remote path: no worker, no IO — the bytes are
  // already in wasm. Every embedded dataset now ships a card, so this asserts a
  // real one renders rather than merely not crashing.
  const bundled = await open("#dataset=causal&load=bundled");
  await openCard(bundled);
  const resident = await bundled.evaluate(() => ({
    title: document.getElementById("cardModalTitle").textContent,
    body: document.getElementById("cardBody").textContent.slice(0, 2000),
    stats: [...document.querySelectorAll("#cardBody .card-stat b")].map((e) => e.textContent),
    foot: document.getElementById("cardFootNote").textContent,
  }));
  if (/carries no Dataset Card/i.test(resident.body)) {
    failures.push("the bundled causal dataset reports no card — embedded datasets are built with one");
  }
  if (!/causal/i.test(resident.title)) failures.push(`resident card title looks wrong: ${resident.title}`);
  if (!resident.stats.length) failures.push("resident card rendered no counts");
  if (/coalesced range/.test(resident.foot)) failures.push("resident read was reported as a ranged remote read");
  if (!/read from the loaded file/.test(resident.foot)) failures.push(`resident footnote does not say where it read from: "${resident.foot}"`);

  // ---- a file that genuinely has no card -------------------------------------
  // Still a real case — `rete build` is cardless unless a card flag is passed —
  // and the modal must say so plainly instead of showing an empty shell. The
  // gate's worldcup fixture is built without one.
  const bare = await readFile("/work/tests/gate/.cache/worldcup2026.rete");
  const bareServer = createServer((req, res) => {
    const common = { "Access-Control-Allow-Origin": "*", "Access-Control-Expose-Headers": "Content-Range,Content-Length,Accept-Ranges", "Accept-Ranges": "bytes" };
    const r = req.headers.range && /bytes=(\d+)-(\d*)/.exec(req.headers.range);
    if (r) {
      const s = Number(r[1]);
      const e = r[2] ? Math.min(Number(r[2]), bare.length - 1) : bare.length - 1;
      const body = bare.subarray(s, e + 1);
      res.writeHead(206, { ...common, "Content-Range": `bytes ${s}-${e}/${bare.length}`, "Content-Length": body.length });
      res.end(body); return;
    }
    res.writeHead(200, { ...common, "Content-Length": bare.length });
    res.end(req.method === "HEAD" ? undefined : bare);
  });
  const barePort = await listen(bareServer);
  const cardless = await open(`#url=${encodeURIComponent(`http://127.0.0.1:${barePort}/bare.rete`)}`);
  await openCard(cardless);
  const none = await cardless.evaluate(() => document.getElementById("cardBody").textContent);
  if (!/carries no Dataset Card/i.test(none)) {
    failures.push(`a cardless file did not say so: "${none.slice(0, 120)}"`);
  }
  bareServer.close();

  if (pageErrors.length) failures.push(`page errors: ${pageErrors.slice(0, 2).join(" | ")}`);

  await browser.close();
  server.close();

  const pass = failures.length === 0;
  console.log(JSON.stringify({
    verdict: pass ? "PASS" : "FAIL",
    note: "Dataset Card modal: rendered + coloured JSON, remote (card-tier read) and resident paths, curated fields present/absent, build record",
    cardRead,
    cardAndBuildRead: fullRead,
    fileBytes: fixture.length,
    sessionTraffic: traffic,
    failures,
  }, null, 2));
  process.exit(pass ? 0 : 1);
};

main().catch((e) => {
  console.log(JSON.stringify({ verdict: "FAIL", error: String(e && e.message).slice(0, 300) }, null, 2));
  process.exit(1);
});
