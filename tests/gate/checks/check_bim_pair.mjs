// The BIM pair viewer: its corpus query panel answers (the corpus file is read
// from R2, so a renamed file shows up here), and its WebMCP tools switch
// project and view and query the corpus. Needs the network: models and corpus
// come from data.graphplaza.com. Usage: node check_bim_pair.mjs
import { launchBrowser } from "./_browser.mjs";

const PORT = process.env.PGPORT || "8090";
const fails = [];
const check = (ok, what) => { if (!ok) fails.push(what); };

const main = async () => {
  const browser = await launchBrowser({ args: ["--use-gl=angle", "--use-angle=swiftshader", "--enable-unsafe-swiftshader", "--ignore-gpu-blocklist"] });
  const ctx = await browser.newContext({ viewport: { width: 1300, height: 850 } });
  const page = await ctx.newPage();
  const errs = [], bad = [];
  page.on("pageerror", (e) => errs.push(String(e).slice(0, 200)));
  page.on("response", (r) => { if (r.status() >= 400) bad.push(r.status() + " " + r.url().slice(0, 120)); });
  // A stand-in for the browser's WebMCP, to see what the page registers.
  await page.addInitScript(() => {
    window.__reg = [];
    Object.defineProperty(document, "modelContext", {
      value: { registerTool: (t, o) => { window.__reg.push({ name: t.name, exec: typeof t.execute === "function", signal: !!(o && o.signal) }); } },
    });
  });
  await page.goto(`http://localhost:${PORT}/bim-pair.html`, { waitUntil: "load" });
  await page.waitForFunction(() => window.reteWebMCP && document.getElementById("loading").style.display === "none", null, { timeout: 120000 });
  const call = (name, args) => page.evaluate(
    ([n, a]) => window.reteBimTools.call(n, a).then((v) => ({ v, bytes: JSON.stringify(v).length }), (e) => ({ err: String(e.message || e) })),
    [name, args || {}]);

  const reg = await page.evaluate(() => ({ reg: window.__reg, ann: window.reteWebMCP, listed: window.reteBimTools.list().length }));
  check(reg.ann.supported === true && reg.reg.length === 8 && reg.listed === 8 && reg.reg.every((t) => t.exec && t.signal), `8 tools registered (got ${reg.reg.length})`);

  // The page's own panel, as a person uses it.
  await page.click("#sqlBtn");
  await page.click("#examples button >> nth=0");
  await page.waitForFunction(() => document.querySelectorAll("#results tbody tr").length > 0 || /error/i.test(document.getElementById("sqlMsg").textContent), null, { timeout: 60000 });
  const panel = await page.evaluate(() => ({ rows: document.querySelectorAll("#results tbody tr").length, msg: document.getElementById("sqlMsg").textContent }));
  check(panel.rows > 0 && !panel.msg, "the first ready query answers in the panel: " + JSON.stringify(panel));
  await page.click("#sql-x");

  let r = await call("get_state");
  check(r.v && r.v.project === "0" && r.v.view === "architecture" && r.v.queryPanelOpen === false && /\.ifc$/.test(r.v.ifc.structural), "get_state: " + JSON.stringify(r).slice(0, 160));
  r = await call("list_projects");
  check(r.v && r.v.projects.length === 3 && r.v.projects[0].shown === true, "list_projects");
  r = await call("set_view", { view: "both" });
  check(r.v && r.v.view === "both" && await page.evaluate(() => document.querySelector("#modes button.on").dataset.mode === "both"), "set_view both");
  r = await call("set_view", { view: "inside" });
  check(!!r.err, "set_view refuses an unknown view");
  r = await call("show_project", { project: "3" });
  check(r.v && r.v.project === "3" && r.v.loading === false && r.v.view === "both" && await page.evaluate(() => document.getElementById("proj").value === "3"), "show_project 3: " + JSON.stringify(r).slice(0, 160));
  r = await call("show_project", { project: "99" });
  check(!!r.err, "show_project refuses a project without a preview");
  r = await call("reset_camera");
  check(r.v && r.v.ok, "reset_camera");
  r = await call("list_examples");
  check(r.v && r.v.examples.length === 5 && /SELECT/.test(r.v.examples[0].query), "list_examples");

  r = await call("run_sparql", { query: "PREFIX g: <https://w3id.org/rete/gnibim#> SELECT (COUNT(?m) AS ?n) WHERE { ?m a g:Model }" });
  check(r.v && r.v.kind === "select" && /^"224"/.test(r.v.rows[0].n), "run_sparql counts the 224 models: " + JSON.stringify(r).slice(0, 160));
  check(await page.evaluate(() => !document.getElementById("sql").hidden && document.querySelectorAll("#results tbody tr").length === 1 && /Agent query/.test(document.getElementById("sql-count").textContent)), "the page shows the agent's query and its row");
  r = await call("set_query_panel", { open: false });
  check(r.v && r.v.queryPanelOpen === false, "set_query_panel closes it");
  r = await call("run_sparql", { query: r.v ? (await call("list_examples")).v.examples[3].query : "", limit: 500, show: false });
  check(r.v && r.v.total > 20 && r.v.returned > 20 && r.bytes <= 18500 && await page.evaluate(() => document.getElementById("sql").hidden), "run_sparql with show false stays out of the page and under the size cap: " + JSON.stringify({ t: r.v && r.v.total, n: r.v && r.v.returned, b: r.bytes }));
  r = await call("run_sparql", { query: "ASK { ?s ?p ?o }" });
  check(r.v && r.v.kind === "ask" && r.v.boolean === true, "run_sparql ASK");
  r = await call("run_sparql", { query: "SELEC nonsense" });
  check(!!r.err, "run_sparql throws the engine's parse error");
  check(await page.evaluate(() => /run_sparql/.test((document.getElementById("agentPill") || {}).textContent || "")), "the page names the agent's last call");
  check(errs.length === 0, "no page errors: " + errs.join(" | "));
  check(bad.length === 0, "no failed requests: " + bad.join(" | "));

  const pass = fails.length === 0;
  console.log(JSON.stringify({ verdict: pass ? "PASS" : "FAIL", tools: reg.reg.map((t) => t.name), fails }, null, 2));
  await browser.close();
  process.exit(pass ? 0 : 1);
};
main();
