// The clustered graph view of the building viewer (docs/building/graphview.js), run in its harness page
// docs/building/graphview-dev.html. Checks: mounts without errors; the top level is a bounded number of clusters
// whose member counts add up to the typed resources; expanding keeps the counts; aggregated edge counts equal a
// SPARQL count; focus() of a device reveals and selects it; every grouping works; the predicate filter changes the
// edges; search; the layout settles and stops rendering when idle; list view matches the canvas; ego view, keyboard,
// highlight and state tints; the strip height and 390 px width.
// With FZK_SYSTEMS_DIR set (a folder holding fzk-haus-systems-2026-11-28.rete) the R2 file is served from there,
// otherwise the live URL is used. Usage: node check_building_graph.mjs   (PGPORT = static server port)
import { launchBrowser } from "./_browser.mjs";
import { readFileSync } from "node:fs";

const PORT = process.env.PGPORT || "8090";
const DIR = process.env.FZK_SYSTEMS_DIR;
const R2 = "https://data.graphplaza.com/cad/fzk-haus-systems-2026-11-28.rete";
const SYS = "https://w3id.org/rete/cad/sys#";
const fails = [];
const check = (ok, what) => { if (!ok) fails.push(what); };
const num = (t) => Number(/"(-?[\d.]+)"/.exec(t)[1]);

const main = async () => {
  const browser = await launchBrowser({ args: ["--no-sandbox"] });
  const ctx = await browser.newContext({ viewport: { width: 1300, height: 800 } });
  const page = await ctx.newPage();
  const errs = [];
  page.on("pageerror", (e) => errs.push(String(e).slice(0, 200)));
  page.on("console", (m) => { if (m.type() === "error" && !/Failed to load resource/.test(m.text())) errs.push(m.text().slice(0, 200)); });
  if (DIR) await page.route(R2, (r) => r.fulfill({ body: readFileSync(DIR + "/fzk-haus-systems-2026-11-28.rete"), contentType: "application/octet-stream" }));
  await page.goto(`http://localhost:${PORT}/building/graphview-dev.html`, { waitUntil: "load" });
  await page.waitForFunction(() => window.gvReady, null, { timeout: 60000 });
  await page.evaluate(() => window.gvReady);
  const gv = (fn, arg) => page.evaluate(`(async () => { const g = window.graphView; const a = ${JSON.stringify(arg === undefined ? null : arg)}; return (${fn.toString()})(g, a); })()`);
  const idle = () => page.evaluate(() => window.graphView.whenIdle());
  const state = () => page.evaluate(() => window.graphView.getState());
  const sparql = (q) => page.evaluate((q) => window.gvQuery(q), q);
  const sum = (a) => a.reduce((x, y) => x + y, 0);

  // ---- mount, top level
  check(await page.evaluate(() => !!document.querySelector(".gv canvas") && !!document.querySelector(".gv-bar") && !!document.querySelector(".gv[role=region]")), "view mounted with canvas, toolbar, labelled region");
  await idle();
  let s = await state();
  const typed = num((await sparql("SELECT (COUNT(DISTINCT ?s) AS ?n) WHERE { ?s a ?t }")).rows[0].n);
  check(s.grouping === "class", "default grouping is class: " + s.grouping);
  check(s.counts.clusters >= 2 && s.counts.clusters <= 40 && s.counts.nodes === 0, `bounded top level: ${s.counts.clusters} clusters, ${s.counts.nodes} nodes`);
  check(sum(s.clusters.map((c) => c.count)) === typed && s.universe === typed, `cluster counts add up to the typed resources (${sum(s.clusters.map((c) => c.count))} vs ${typed})`);
  const pixels = () => page.evaluate(() => { const c = document.querySelector(".gv canvas"); const d = c.getContext("2d").getImageData(0, 0, c.width, c.height).data; let n = 0; for (let i = 0; i < d.length; i += 4) if (d[i + 3] > 0) n++; return n; });
  check(await pixels() > 2000, "canvas is painted");

  // ---- idle: the layout settled, nothing renders
  const f0 = (await state()).frames; await page.waitForTimeout(700); s = await state();
  check(s.frames === f0 && s.rendering === false && s.layout.active === false, `idle: frames ${f0} -> ${s.frames}, rendering ${s.rendering}, layout active ${s.layout.active}`);

  // ---- hover: tooltip and neighbourhood
  { const h0 = s.clusters[0], hp = { x: h0.x * s.view.k + s.view.x, y: h0.y * s.view.k + s.view.y };
    const box = await page.locator(".gv canvas").boundingBox(); await page.mouse.move(box.x + hp.x + 40, box.y + hp.y + 40); await page.mouse.move(box.x + hp.x, box.y + hp.y); await page.waitForTimeout(250);
    const tip = await page.evaluate(() => { const t = document.querySelector(".gv-tip"); return t && !t.hidden ? t.textContent : ""; });
    check(tip.includes(h0.label) && tip.includes(String(h0.count)), "hover shows a tooltip with the cluster name and count: " + JSON.stringify(tip));
    await page.mouse.move(box.x + 2, box.y + 2); }
  // ---- expand a cluster by clicking it
  const click = async (x, y) => { const box = await page.locator(".gv canvas").boundingBox(); await page.mouse.click(box.x + x, box.y + y); };
  const toScreen = (c, st) => ({ x: c.x * st.view.k + st.view.x, y: c.y * st.view.k + st.view.y });
  const small = s.clusters.filter((c) => c.count >= 3 && c.count <= 60).sort((a, b) => b.count - a.count)[0];
  check(!!small, "a small cluster exists to expand");
  const before = s;
  const p = toScreen(small, s); await click(p.x, p.y); await idle(); s = await state();
  check(s.expanded.includes(small.id) && s.counts.nodes === small.count && s.counts.clusters === before.counts.clusters - 1, `click expands a small cluster into its ${small.count} nodes (nodes ${s.counts.nodes}, clusters ${s.counts.clusters})`);
  check(s.counts.members === typed, "counts stay consistent after expanding: " + s.counts.members);
  const mem = await gv((g, id) => g.clusterMembers(id), small.id);
  check(mem.length === small.count && s.nodes.every((n) => mem.includes(n.iri)), "the nodes shown are the cluster's members");
  check(s.path.length === 1 && s.path[0].id === small.id, "breadcrumb path: " + JSON.stringify(s.path));
  check(await page.evaluate(() => /›/.test(document.querySelector(".gv-crumbs").textContent)), "breadcrumb is shown");
  // a big cluster expands into sub-clusters
  const big = before.clusters.slice().sort((a, b) => b.count - a.count)[0];
  await gv((g, id) => g.collapseAll(), null); await idle();
  await gv((g, id) => g.expand(id), big.id); await idle(); s = await state();
  check(s.expanded.includes(big.id) && s.counts.members === typed && s.counts.clusters + s.counts.nodes > before.counts.clusters, `big cluster expands (${s.counts.clusters} clusters, ${s.counts.nodes} nodes)`);
  // crumb collapses back
  await page.click(".gv-crumbs button >> text=All"); await idle(); s = await state();
  check(s.expanded.length === 0 && s.counts.clusters === before.counts.clusters, "crumb 'All' collapses back to the top level");

  // ---- aggregated edges equal a SPARQL count
  s = await state();
  const enabled = s.enabledPredicates;
  const q = `SELECT (COUNT(*) AS ?n) WHERE { ?s ?p ?o . FILTER(?p IN (${enabled.map((p) => `<${p}>`).join(",")})) FILTER EXISTS { ?s a ?x } FILTER EXISTS { ?o a ?y } }`;
  const total = num((await sparql(q)).rows[0].n);
  check(s.counts.links + s.counts.innerLinks === total, `edge counts (${s.counts.links} between + ${s.counts.innerLinks} inside) equal the SPARQL count ${total}`);
  const top = s.edges.filter((e) => e.kind === "cluster").sort((a, b) => b.count - a.count)[0];
  if (top) {
    const A = (await gv((g, id) => g.clusterMembers(id), top.from.slice(2))), B = (await gv((g, id) => g.clusterMembers(id), top.to.slice(2)));
    const qq = `SELECT (COUNT(*) AS ?n) WHERE { VALUES ?s { ${A.map((x) => `<${x}>`).join(" ")} } VALUES ?o { ${B.map((x) => `<${x}>`).join(" ")} } ?s ?p ?o . FILTER(?p IN (${top.predicates.map((x) => `<${x.iri}>`).join(",")})) }`;
    const n = num((await sparql(qq)).rows[0].n);
    check(n === top.count, `the heaviest cluster edge (${top.from.slice(-18)} -> ${top.to.slice(-18)}) counts ${top.count}, SPARQL says ${n}`);
  } else fails.push("no cluster-level edge to compare");

  // ---- focus a device found by query
  const dq = await sparql(`SELECT ?d WHERE { ?d a <${SYS}Device> . ?d <https://brickschema.org/schema/Brick#feeds> ?x } ORDER BY ?d LIMIT 7`);
  const dev = (dq.rows[Math.min(3, dq.rows.length - 1)] || {}).d ? dq.rows[Math.min(3, dq.rows.length - 1)].d.slice(1, -1) : null;
  check(!!dev, "found a device by query");
  if (dev) {
    await gv((g, d) => g.focus(d), dev); await idle(); s = await state();
    const node = s.nodes.find((n) => n.iri === dev);
    check(s.selected === dev && !!node && s.expanded.length >= 1, `focus reveals and selects the device (${s.expanded.length} clusters opened)`);
    check(await page.evaluate(() => !!document.querySelector(".gv-card")), "side card is shown");
    const card = await page.evaluate(() => ({ t: document.querySelector(".gv-card h3").textContent, links: document.querySelectorAll(".gv-card details").length, btn: document.querySelectorAll(".gv-card li button").length }));
    check(card.links > 0 && card.btn > 0, "card lists links grouped by predicate: " + JSON.stringify(card));
    const sp = toScreen(node, s); check(sp.x > 0 && sp.x < s.size.w && sp.y > 0 && sp.y < s.size.h, "the selected node is inside the viewport");
    check(s.edges.some((e) => e.sel), "the selected node's links are drawn");
    // a link in the card goes there
    await page.evaluate(() => document.querySelectorAll(".gv-card details").forEach((d) => { d.open = true; }));
    const nb = await page.evaluate(() => document.querySelector(".gv-card li button").dataset.iri);
    await page.click(".gv-card li button >> nth=0"); await idle(); s = await state();
    check(s.selected === nb, "clicking a link in the card selects that neighbour");
    await gv((g, d) => g.focus(d), dev); await idle();
    // keyboard
    await page.focus(".gv canvas"); const exp0 = (await state()).expanded.length;
    await page.keyboard.press("Backspace"); await idle(); s = await state();
    check(s.expanded.length === exp0 - 1, `Backspace goes up one level (${exp0} -> ${s.expanded.length})`);
    await gv((g, d) => g.focus(d), dev); await idle(); await page.focus(".gv canvas");
    await page.keyboard.press("ArrowRight"); await page.keyboard.press("ArrowDown"); await idle(); s = await state();
    check(s.selected !== null, "arrow keys keep a selection");
    await page.keyboard.press("/"); check(await page.evaluate(() => document.activeElement === document.querySelector(".gv-search input")), "'/' focuses the search box");
    await page.keyboard.press("Escape"); await page.focus(".gv canvas"); await page.keyboard.press("Escape"); s = await state();
    check(s.selected === null, "Esc clears the selection");
    // expand neighbours
    await gv((g, d) => g.focus(d), dev); await idle(); const n0 = (await state()).counts.nodes;
    await gv((g) => g.expandNeighbours(), null); await idle(); s = await state();
    check(s.counts.nodes > n0 && s.counts.members === typed && s.counts.pulled > 0, `expand neighbours pulls nodes out (${n0} -> ${s.counts.nodes}), members still ${s.counts.members}`);
    // ego
    await gv((g, d) => g.ego(d, 1), dev); await idle(); s = await state();
    check(s.ego && s.ego.iri === dev && s.counts.nodes >= 2 && s.counts.clusters === 0, `ego view shows ${s.counts.nodes} nodes`);
    check(await page.evaluate(() => /Back/.test(document.querySelector(".gv-crumbs").textContent)), "ego view has a way back");
    await page.click(".gv-crumbs button >> text=Back"); await idle(); s = await state();
    check(s.ego === null && s.counts.members === typed, "back leaves ego view");
    // highlight and state tints
    await gv((g, d) => g.highlight([d]), dev); s = await state(); check(s.highlight.length === 1 && s.highlight[0] === dev, "highlight is kept");
    await idle(); const fr = (await state()).frames;
    await page.evaluate((d) => { window.gvStates.set(d, { label: "off", color: "#ff3355" }); window.graphView.updateStates(); }, dev);
    await page.waitForTimeout(150); check((await state()).frames > fr, "updateStates repaints");
    await gv((g) => g.highlight([]), null);
  }

  // ---- search finds a thing by label
  const lq = await sparql(`SELECT ?d ?l WHERE { ?d a <${SYS}Breaker> ; <http://www.w3.org/2000/01/rdf-schema#label> ?l } ORDER BY ?d LIMIT 1`);
  if (lq.rows.length) {
    const iri = lq.rows[0].d.slice(1, -1), label = /^"([\s\S]*?)"/.exec(lq.rows[0].l)[1];
    const word = label.split(/[\s:,]+/).filter((w) => w.length > 3).slice(0, 2).join(" ");
    const hits = await gv((g, t) => g.search(t), word);
    check(hits.some((h) => h.iri === iri), `search '${word}' finds the breaker`);
    await gv((g) => g.collapseAll(), null); await idle();
    await page.fill(".gv-search input", label); await page.waitForSelector(".gv-hits button");
    await page.keyboard.press("Enter"); await idle(); await page.waitForTimeout(200); s = await state();
    check(s.selected === iri && s.nodes.some((n) => n.iri === iri), "search box: Enter focuses the hit");
    await gv((g) => g.collapseAll(), null);
  } else fails.push("no breaker with a label");

  // ---- groupings
  s = await state();
  const avail = s.groupings.filter((g) => g.available);
  check(avail.length >= 2, "at least two groupings available: " + avail.map((g) => g.id).join(","));
  const perGrouping = {};
  for (const g of avail) {
    await gv((gg, id) => gg.setGrouping(id), g.id); await idle(); const t = await state();
    perGrouping[g.id] = t.counts.clusters;
    check(t.grouping === g.id && t.counts.clusters >= 1 && t.counts.clusters <= 40 && sum(t.clusters.map((c) => c.count)) === typed, `grouping ${g.id}: ${t.counts.clusters} clusters add up to ${typed}`);
    const big2 = t.clusters.slice().sort((a, b) => b.count - a.count)[0];
    await gv((gg, id) => gg.expand(id), big2.id); await idle(); const u = await state();
    check(u.expanded.length === 1 && u.counts.members === typed, `grouping ${g.id}: expanding the largest cluster keeps the counts`);
  }
  check(["system", "room"].every((id) => avail.some((g) => g.id === id)), "system and room groupings are available on this graph");
  await gv((g) => g.setGrouping("class"), null); await idle();
  // the select element drives it too
  await page.selectOption(".gv-bar select >> nth=0", "namespace"); await idle(); check((await state()).grouping === "namespace", "the grouping selector works");
  await gv((g) => g.setGrouping("class"), null); await idle();

  // ---- predicate filter
  await gv((g) => g.select(null), null); await idle(); s = await state(); const e0 = s.counts.edges;
  await gv((g) => g.setFilter({ predicates: [] }), null); await idle(); s = await state();
  check(s.counts.edges === 0 && s.enabledPredicates.length === 0, "no predicates: no edges");
  const pick = (await state()).predicates.slice().sort((a, b) => b.count - a.count)[0];
  await gv((g, p) => g.setFilter({ predicates: [p] }), pick.iri); await idle(); s = await state();
  check(s.counts.edges > 0 && s.edges.every((e) => e.predicates.every((x) => x.iri === pick.iri)), "one predicate: only its edges");
  await gv((g) => g.setFilter({ predicates: null }), null); await idle(); s = await state();
  check(s.counts.edges === e0, "defaults restore the edges");
  await page.click(".gv-bar button >> text=Edges"); await page.waitForSelector(".gv-pop input[type=checkbox]");
  const boxes = await page.locator(".gv-pop input[type=checkbox]:checked").count();
  await page.locator(".gv-pop input[type=checkbox]:checked").first().click(); await idle(); s = await state();
  check(s.enabledPredicates.length === boxes - 1, "the edges popover unchecks a predicate");
  await page.keyboard.press("Escape"); await page.click(".gv-pop button >> text=Defaults"); await page.click(".gv-pop button >> text=Close"); await idle();
  // text / class filters change the universe
  await gv((g) => g.setFilter({ text: "kitchen" }), null); await idle(); s = await state();
  check(s.universe > 0 && s.universe < typed, `text filter narrows the nodes (${s.universe})`);
  await gv((g) => g.setFilter({ text: "" }), null); await idle(); s = await state(); check(s.universe === typed, "text filter cleared");

  // ---- flow
  const feeds = (await state()).predicates.find((p) => /feeds$/.test(p.iri));
  if (feeds) {
    await gv((g) => g.setGrouping("system"), null); await idle();
    const el = (await state()).clusters.slice().sort((a, b) => b.count - a.count).find((c) => /Electrical/i.test(c.label)) || (await state()).clusters[0];
    await gv((g, id) => g.expandAll(id), el.id); await idle();
    await gv((g, p) => g.setFlow(p), feeds.iri); await idle(); s = await state();
    const fe = s.edges.filter((e) => e.kind === "node" && e.predicate === feeds.iri);
    const pos = new Map(s.nodes.map((n) => [n.iri, n.x]));
    const fwd = fe.filter((e) => pos.get(e.to) > pos.get(e.from)).length;
    check(s.flow === feeds.iri && fe.length >= 5 && fwd >= 0.8 * fe.length, `flow layout: ${fwd} of ${fe.length} feeds edges run left to right`);
    await gv((g) => g.setFlow(null), null); await idle(); check((await state()).flow === null, "flow off");
    await gv((g) => g.setGrouping("class"), null); await idle();
  } else fails.push("no feeds predicate");

  // ---- list view matches the canvas
  await gv((g) => g.collapseAll(), null); await idle();
  await page.click(".gv-bar button >> text=List"); await idle(); s = await state();
  const rows = await page.evaluate(() => [...document.querySelectorAll(".gv-list tbody tr")].map((r) => [r.dataset.kind, r.children[0].textContent, r.children[3].textContent]));
  check(s.list && rows.length === s.counts.clusters + s.counts.nodes && rows.length > 0, `list rows (${rows.length}) match the canvas (${s.counts.clusters} clusters + ${s.counts.nodes} nodes)`);
  check(s.clusters.every((c) => rows.some((r) => r[0] === "cluster" && r[1] === c.label && r[2] === String(c.count))), "list shows each cluster's name and count");
  await page.click(".gv-list tbody tr >> nth=0"); await idle(); s = await state();
  check(s.expanded.length === 1, "a list row expands its cluster");
  await page.click(".gv-bar button >> text=List"); await idle(); check(!(await state()).list && !(await page.$(".gv-list")), "list toggles off");
  await gv((g) => g.collapseAll(), null); await idle();

  // ---- refresh keeps the view
  const exp = (await state()).clusters[0].id; await gv((g, id) => g.expand(id), exp); await idle();
  await gv((g) => g.refresh(), null); await idle(); s = await state();
  check(s.expanded.includes(exp) && s.counts.members === typed, "refresh re-reads the graph and keeps the expanded cluster");
  await gv((g) => g.collapseAll(), null); await idle();

  // ---- strip height and 390 px
  await page.evaluate(() => { document.body.classList.add("strip"); window.graphView.resize(); window.graphView.fit(); }); await idle();
  s = await state(); check(s.size.h > 150 && s.size.h < 260 && s.size.w > 1000, "300 px strip: canvas " + s.size.w + "x" + s.size.h);
  check(await pixels() > 1000, "strip canvas is painted");
  await page.setViewportSize({ width: 390, height: 700 }); await page.evaluate(() => window.graphView.resize()); await idle();
  s = await state();
  check(s.size.w <= 390 && await page.evaluate(() => document.documentElement.scrollWidth <= 391), "390 px: no horizontal page overflow, canvas " + s.size.w);
  check(await page.evaluate(() => { const b = document.querySelector(".gv-bar"); return b.scrollWidth >= b.clientWidth && getComputedStyle(b).overflowX === "auto"; }), "390 px: the toolbar scrolls sideways");
  await page.waitForTimeout(150); check(await pixels() > 500, "390 px canvas is painted");
  if (dev) {
    await gv((g, d) => g.focus(d), dev); await idle(); s = await state();
    check(s.selected === dev && await page.evaluate(() => !!document.querySelector(".gv-card")), "390 px: focus works and shows the card");
    const card = await page.evaluate(() => { const r = document.querySelector(".gv-card").getBoundingClientRect(); const m = document.querySelector(".gv-main").getBoundingClientRect(); return r.left >= m.left - 1 && r.right <= m.right + 1; });
    check(card, "390 px: the card fits the width");
  }
  await page.setViewportSize({ width: 1300, height: 800 });

  // ---- host options: a second view with its own grouping, labels, colours; destroy removes it
  const host = await page.evaluate(async () => {
    const { mountGraphView } = await import("/building/graphview.js");
    const el = document.createElement("div"); el.style.cssText = "position:fixed;left:0;top:0;width:500px;height:300px;z-index:50"; document.body.append(el);
    let hovered = 0;
    const v = mountGraphView(el, { query: window.gvQuery, theme: "light", labelOf: (i) => (i.endsWith("/site") ? "MY SITE" : undefined), colorOf: () => "#336699", onHover: () => { hovered++; },
      groupings: [{ id: "first", label: "First letter", levels: [{ label: "Letter", sparql: "SELECT ?n ?g WHERE { ?n <http://www.w3.org/2000/01/rdf-schema#label> ?l . BIND(UCASE(SUBSTR(STR(?l), 1, 1)) AS ?g) }" }] }] });
    await v.ready; await v.setGrouping("first"); await v.whenIdle();
    const st = v.getState(); const out = { n: st.counts.clusters, members: st.counts.members, has: st.groupings.some((g) => g.id === "first"), grp: st.grouping, light: el.querySelector(".gv").dataset.theme };
    v.destroy(); out.gone = !el.querySelector(".gv"); el.remove(); return out;
  });
  check(host.has && host.grp === "first" && host.n >= 3 && host.members === typed && host.light === "light" && host.gone, "host grouping, theme and destroy: " + JSON.stringify(host));

  check(errs.length === 0, "no page or console errors: " + errs.join(" | "));
  const pass = fails.length === 0;
  console.log(JSON.stringify({ verdict: pass ? "PASS" : "FAIL", typed, groupings: perGrouping, fails }, null, 2));
  await browser.close();
  process.exit(pass ? 0 : 1);
};
main().catch((e) => { console.log(JSON.stringify({ verdict: "FAIL", fails: [...fails, "exception: " + (e && e.stack || e)] }, null, 2)); process.exit(1); });
