// The playground's WebMCP tools: what it registers with a browser that has
// WebMCP, and that every tool does what it says on a bundled graph and on a
// graph built in the page. No network: remote files are covered by the lazy
// checks. Usage: node check_webmcp.mjs
import { launchBrowser } from "./_browser.mjs";

const PORT = process.env.PGPORT || "8090";
const fails = [];
const check = (ok, what) => { if (!ok) fails.push(what); };

const main = async () => {
  const browser = await launchBrowser();
  const ctx = await browser.newContext({ acceptDownloads: true });
  const page = await ctx.newPage();
  const errs = [];
  page.on("pageerror", (e) => errs.push(String(e).slice(0, 200)));
  // A stand-in for the browser's WebMCP, to see what the page registers.
  await page.addInitScript(() => {
    window.__reg = [];
    Object.defineProperty(document, "modelContext", {
      value: { registerTool: (t, o) => { window.__reg.push({ name: t.name, exec: typeof t.execute === "function", type: t.inputSchema && t.inputSchema.type, signal: !!(o && o.signal) }); } },
    });
  });
  await page.goto(`http://localhost:${PORT}/playground.html#dataset=scholar&load=bundled&mode=sparql&ex=0`, { waitUntil: "domcontentloaded" });
  await page.waitForFunction(() => window.reteWebMCP, null, { timeout: 60000 });
  const call = (name, args) => page.evaluate(
    ([n, a]) => window.retePlaygroundTools.call(n, a).then((v) => ({ v, bytes: JSON.stringify(v).length }), (e) => ({ err: String(e.message || e) })),
    [name, args || {}]);

  const reg = await page.evaluate(() => ({ reg: window.__reg, ann: window.reteWebMCP, listed: window.retePlaygroundTools.list().length }));
  check(reg.ann.supported === true && reg.reg.length === 22 && reg.listed === 22, `22 tools registered (got ${reg.reg.length})`);
  check(reg.reg.every((t) => t.exec && t.signal && t.type === "object"), "every tool has execute, an object schema and an abort signal");

  // Reading tools leave the page alone only with "Follow the agent" off; it is
  // on by default. The first part of this check runs with it off.
  let r = await call("get_settings");
  check(r.v && r.v.followAgent === true && !!r.v.strategy && typeof r.v.concurrentReads === "boolean", "get_settings: " + JSON.stringify(r).slice(0, 200));
  r = await call("set_settings", { follow_agent: false });
  check(r.v && r.v.followAgent === false, "set_settings turns following off");
  r = await call("get_state");
  check(r.v && r.v.dataset === "scholar" && r.v.source === "memory" && r.v.mode === "sparql", "get_state");
  r = await call("list_datasets", { query: "jazz" });
  check(r.v && r.v.datasets.some((d) => d.key === "linked-jazz"), "list_datasets finds linked-jazz");
  r = await call("list_datasets", { limit: 200 });
  check(r.v && r.v.returned > 10 && r.bytes <= 18500, "list_datasets stays under the size cap");
  {
    let seen = r.v.returned, next = r.v.next, pages = 1;
    while (next != null && pages < 30) { const p = await call("list_datasets", { limit: 200, offset: next }); seen += p.v.returned; next = p.v.next; pages++; }
    check(seen === r.v.matched, `list_datasets pages through the catalog (${seen} of ${r.v.matched})`);
  }

  const before = await page.evaluate(() => document.getElementById("q").value);
  r = await call("run_sparql", { query: "SELECT (COUNT(*) AS ?n) WHERE { ?s ?p ?o }" });
  check(r.v && r.v.kind === "select" && /^"\d+"/.test(r.v.rows[0].n), "run_sparql returns the count as an RDF term");
  r = await call("run_sparql", { query: "SELECT ?s ?p ?o WHERE { ?s ?p ?o }", limit: 500 });
  check(r.v && r.v.truncated === true && r.v.returned < r.v.total && r.bytes <= 18500, "run_sparql cuts a large result and says so");
  check(await page.evaluate(() => document.getElementById("q").value) === before, "run_sparql leaves the editor alone");
  r = await call("run_sparql", { query: "ASK { ?s ?p ?o }" });
  check(r.v && r.v.kind === "ask" && r.v.boolean === true, "run_sparql ASK");
  r = await call("run_sparql", { query: "CONSTRUCT { ?s ?p ?o } WHERE { ?s ?p ?o } LIMIT 3" });
  check(r.v && Array.isArray(r.v.triples) && r.v.triples.length === 3, "run_sparql CONSTRUCT");
  r = await call("run_sparql", { query: "SELEC nonsense" });
  check(!!r.err, "run_sparql throws the engine's parse error");
  r = await call("run_sparql", { query: "INSERT DATA { <urn:a> <urn:b> <urn:c> }" });
  check(r.err && /read-only/.test(r.err), "run_sparql refuses an update");

  r = await call("get_schema");
  check(r.v && r.v.classes.length > 0 && r.bytes <= 18500, "get_schema");
  r = await call("get_dataset_card");
  check(r.v && r.v.card && typeof r.v.card === "object" && r.v.card.title && r.bytes <= 18500, "get_dataset_card keeps its shape under the cap");
  r = await call("list_examples");
  check(r.v && r.v.examples.length > 3 && !!r.v.examples[0].query, "list_examples");
  r = await call("find_entities", { prefix: "gr" });
  check(r.v && r.v.matches.length > 0 && !!r.v.matches[0].subject, "find_entities");

  r = await call("select_example", { index: 1 });
  check(r.v && r.v.ran && r.v.finished && /row/.test(r.v.summary), "select_example runs it in the page");
  r = await call("select_example", { index: 9999 });
  check(!!r.err, "select_example refuses an unknown index");
  r = await call("show_query", { query: "SELECT ?p (COUNT(*) AS ?n) WHERE { ?s ?p ?o } GROUP BY ?p ORDER BY DESC(?n) LIMIT 5" });
  check(r.v && r.v.finished && /^5 row/.test(r.v.summary), "show_query runs on the whole index: " + (r.v && r.v.summary));
  check(r.v && !/PDF viewer/.test(r.v.shown) && /\d/.test(r.v.shown), "the text of the page has rows and no menu options");
  check(await page.evaluate(() => document.querySelectorAll("#out table tr").length >= 6 && [...document.querySelectorAll("#out select")].every((s) => !s.hidden)), "the page shows the table with its menus");
  check(await page.evaluate(() => document.getElementById("q").value.includes("GROUP BY ?p")), "show_query puts the query in the editor");
  r = await call("read_page");
  check(r.v && r.v.shown.length > 20, "read_page");

  for (const tab of ["schema", "shacl", "reach", "coherence", "provenance", "explore", "build", "sparql"]) {
    r = await call("open_tab", { tab });
    check(r.v && r.v.mode === tab, "open_tab " + tab);
  }
  await call("open_tab", { tab: "shacl" });
  r = await call("run_tab");
  check(r.v && r.v.finished, "run_tab in SHACL");
  await call("open_tab", { tab: "explore" });
  r = await call("run_tab");
  check(!!r.err, "run_tab refuses a tab without Run");
  r = await call("open_tab", { tab: "nope" });
  check(!!r.err, "open_tab refuses an unknown tab");

  // With "Follow the agent" on, the page shows what the reading tools read.
  await call("open_dataset", { key: "scholar" });
  r = await call("set_settings", { follow_agent: true, strategy: "progressive" });
  check(r.v && r.v.followAgent === true && r.v.strategy === "progressive", "set_settings: follow on, strategy");
  await call("open_tab", { tab: "schema" });
  const fq = "SELECT ?s WHERE { ?s ?p ?o } LIMIT 7";
  r = await call("run_sparql", { query: fq });
  check(r.v && r.v.rows.length === 7 && r.v.shownInPage === true, "run_sparql while following");
  check(await page.evaluate((q) => document.getElementById("q").value === q && document.querySelectorAll("#out table tr").length === 8, fq), "the page shows the agent's query and its 7 rows");
  r = await call("get_state");
  check(r.v && r.v.mode === "sparql" && r.v.strategy === "whole", "following brings the SPARQL tab forward, on the whole index");
  r = await call("get_schema");
  check(r.v && r.v.classes.length > 0 && await page.evaluate(() => (document.querySelector(".panel.active") || { dataset: {} }).dataset.panel === "schema"), "get_schema brings the Schema tab forward");
  r = await call("get_dataset_card");
  check(r.v && r.v.card && await page.evaluate(() => !document.getElementById("cardModal").classList.contains("hidden")), "get_dataset_card opens the card");
  r = await call("describe_dataset");
  check(r.v && r.v.key === "scholar" && r.v.open && r.v.description && r.v.examples.length > 3 && r.v.topClasses.length > 0, "describe_dataset: " + JSON.stringify(r).slice(0, 160));
  check(await page.evaluate(() => document.getElementById("cardModal").classList.contains("hidden")), "the next call closes the card it opened");
  r = await call("describe_dataset", { key: "linked-jazz" });
  check(r.v && r.v.open === false && r.v.title && r.v.note, "describe_dataset of a dataset that is not open");
  r = await call("describe_dataset", { key: "nope" });
  check(!!r.err, "describe_dataset refuses an unknown key");
  r = await call("get_history");
  check(r.v && r.v.runs.length > 0 && r.v.runs[0].query === fq, "get_history has the run the page showed");
  r = await call("get_cache");
  check(r.v && Array.isArray(r.v.ranges) && Array.isArray(r.v.wholeFiles) && typeof r.v.rangeBytes === "number", "get_cache");
  r = await call("clear_cache", { what: "ranges" });
  check(r.v && r.v.cleared === "ranges" && r.v.rangeBytes === 0, "clear_cache ranges");
  r = await call("clear_cache", { what: "file", file: "nope" });
  check(!!r.err, "clear_cache refuses an unknown file");
  r = await call("get_activity", { limit: 100 });
  check(r.v && r.v.calls.length > 20 && r.v.calls[0].tool === "clear_cache" && r.v.calls[0].state === "error" && r.v.calls.some((c) => c.tool === "run_sparql" && c.state === "ok" && /rows/.test(c.note)), "get_activity lists the calls, newest first");
  const ui = await page.evaluate(() => {
    document.getElementById("settingsBtn").click();
    const sec = document.getElementById("agentSection");
    const out = { rows: sec ? sec.querySelectorAll("#agentLog .stg-logrow").length : -1, follow: sec && document.getElementById("agentFollow").checked,
      info: sec ? document.getElementById("agentInfo").textContent : "", pill: (document.getElementById("mcpPill") || {}).textContent || "" };
    document.getElementById("settingsModalClose").click();
    return out;
  });
  check(ui.rows > 20 && ui.follow === true && /22 tools/.test(ui.info) && /get_activity/.test(ui.pill), "Settings lists the agent's calls: " + JSON.stringify(ui).slice(0, 200));
  await call("set_settings", { follow_agent: false, strategy: "whole" });

  // Build a graph, query it, read its card, save it.
  const B = String.fromCharCode(92);
  const ttl = "@prefix ex: <https://example.org/> .\n@prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> .\n" +
    'ex:a a ex:Thing ; rdfs:label "Line one' + B + 'nline two" ; ex:knows ex:b .\nex:b a ex:Thing ; rdfs:label "B" .\n';
  r = await call("build_rete", { data: ttl, format: "ttl", title: "Agent test graph", description: "Two things.", license: "CC0" });
  check(r.v && r.v.key === "agent-test-graph" && r.v.triples === 5 && r.v.opened, "build_rete: " + JSON.stringify(r).slice(0, 200));
  r = await call("get_state");
  check(r.v && r.v.dataset === "agent-test-graph" && r.v.mode === "sparql" && r.v.source === "memory", "the built graph is open");
  r = await call("run_sparql", { query: "SELECT ?l WHERE { <https://example.org/a> <http://www.w3.org/2000/01/rdf-schema#label> ?l }" });
  check(r.v && r.v.rows.length === 1 && r.v.rows[0].l === '"Line one' + B + 'nline two"', "the built graph answers, line break kept");
  r = await call("get_dataset_card");
  check(r.v && r.v.card && r.v.card.title === "Agent test graph" && r.v.card.license === "CC0", "the built file carries its card");
  const [dl] = await Promise.all([page.waitForEvent("download", { timeout: 15000 }), call("download_rete")]);
  check(dl.suggestedFilename() === "agent-test-graph.rete", "download_rete saves the file");
  r = await call("build_rete", { data: "this is not turtle", format: "ttl", title: "Broken" });
  check(r.err && /^Build failed: /.test(r.err), "build_rete reports the parser's message: " + r.err);
  r = await call("build_rete", { data: ttl, title: "scholar" });
  check(r.err && /bundled/.test(r.err), "build_rete refuses a bundled key");

  r = await call("open_dataset", { key: "nope" });
  check(!!r.err, "open_dataset refuses an unknown key");
  r = await call("open_dataset", { url: "javascript:alert(1)" });
  check(!!r.err, "open_dataset refuses an address that is not http(s)");
  r = await call("open_dataset", { key: "linked-jazz" });
  check(r.v && r.v.dataset === "linked-jazz" && r.v.source === "memory", "open_dataset opens a bundled graph");
  r = await call("find_entities", { prefix: "Miles" });
  check(r.v && r.v.matches.some((m) => /Miles Davis/.test(m.label)), "find_entities on the new graph");
  check(errs.length === 0, "no page errors: " + errs.join(" | "));

  const pass = fails.length === 0;
  console.log(JSON.stringify({ verdict: pass ? "PASS" : "FAIL", tools: reg.reg.map((t) => t.name), fails }, null, 2));
  await browser.close();
  process.exit(pass ? 0 : 1);
};
main();
