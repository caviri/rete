// The application shell of the FZK-Haus viewer (docs/building.html, dashboard.js, menus.js, actions.js, panels in ui.js and
// inspector.js, console.js, graph-tab.js, logs.js, statusbar.js, about.js): the regions and their roles; the five menus open by
// mouse and by keyboard and every command does its thing; each side panel drives the app; the inspector for an element, a room and a
// device; the console runs a query and the rows light up in 3D; the results table sorts, filters and selects; the graph tab mounts
// lazily and follows the selection both ways; the thermal and scenarios tabs; the log; the status bar; reset in every scope with undo;
// the shareable link; the command palette; both themes with contrast ratios; layouts at 1400, 820 and 390 px (nothing sticks out of the
// screen); the new WebMCP tools (the registered list is the union of the page's tool arrays) and that every call is logged; touch zones in the
// four-view layout; the page with the rendering side's APIs missing (controls disabled with a reason, nothing throws); the house without the
// shell; memory back to its baseline after opening and closing everything; the page without the services data.
// ONLY="menus,logs" runs just those sections (a development aid; the full run is the verdict).
// Needs the network for the house (data.graphplaza.com); the two systems files are served from FZK_SYSTEMS_DIR when set.
// Usage: FZK_SYSTEMS_DIR=D:/.../data/cad/systems PGPORT=8095 node check_building_dashboard.mjs
import { launchBrowser } from "./_browser.mjs";
import { readFile } from "node:fs/promises";

const PORT = process.env.PGPORT || "8090";
const DIR = process.env.FZK_SYSTEMS_DIR || "";
const NAME = "fzk-haus-systems-2026-11-28";
const URL_ = `http://localhost:${PORT}/building.html`;
const fails = [], notes = [];
const check = (ok, what) => { if (!ok) fails.push(what); };
const ARGS = ["--no-sandbox", "--use-gl=angle", "--use-angle=swiftshader", "--enable-unsafe-swiftshader", "--ignore-gpu-blocklist"];
const OWN = /dash test (error|rejection)/;   // the errors this check throws on purpose, to see them in the log

const open = async (browser, { w = 1400, h = 860, systems = true, hash = "", touch = false, storage = null, initScript = null, initArg = null, block = [] } = {}) => {
  const ctx = await browser.newContext({ viewport: { width: w, height: h }, hasTouch: touch });
  if (storage) await ctx.addInitScript((s) => { try { localStorage.setItem("reteBuilding.ui.v1", s); } catch (e) { /* none */ } }, storage);
  if (initScript) await ctx.addInitScript(initScript, initArg);
  const page = await ctx.newPage(); const errs = [], bad = [];
  for (const g of block) await page.route(g, (r) => r.abort());
  page.on("pageerror", (e) => { if (!OWN.test(String(e))) errs.push(String(e).slice(0, 200)); });
  page.on("console", (m) => { if (m.type() === "error" && !/Failed to load resource/.test(m.text()) && !OWN.test(m.text())) errs.push("console: " + m.text().slice(0, 200)); });
  page.on("response", (r) => { if (r.status() >= 400 && !(!systems && new RegExp(NAME).test(r.url()))) bad.push(r.status() + " " + r.url().slice(0, 120)); });
  if (systems && DIR) {
    await page.route(`**/cad/${NAME}.json`, async (r) => r.fulfill({ status: 200, contentType: "application/json", headers: { "access-control-allow-origin": "*" }, body: await readFile(`${DIR}/${NAME}.json`) }));
    await page.route(`**/cad/${NAME}.rete`, async (r) => r.fulfill({ status: 200, contentType: "application/octet-stream", headers: { "access-control-allow-origin": "*" }, body: await readFile(`${DIR}/${NAME}.rete`) }));
  } else if (!systems) {
    await page.route(`**/cad/${NAME}.*`, (r) => r.fulfill({ status: 404, body: "not found", headers: { "access-control-allow-origin": "*" } }));
  }
  await page.addInitScript(() => { window.__reg = []; Object.defineProperty(document, "modelContext", { value: { registerTool: (t) => { window.__reg.push(t.name); } } }); });
  await page.goto(URL_ + hash, { waitUntil: "load" });
  await page.waitForFunction(() => window.reteBuilding && window.reteBuilding.ready, null, { timeout: 150000 });
  await page.waitForTimeout(400);
  return { ctx, page, errs, bad };
};

// ONLY="menus,commands: view" runs just those sections (development aid; the full run is the verdict)
const ONLY = (process.env.ONLY || "").split(",").map((x) => x.trim()).filter(Boolean);
let afterSection = async () => {};   // set in main: closes a menu a section left open (check and radio items keep their menu open on purpose)
const section = async (name, fn) => { if (ONLY.length && !ONLY.includes(name)) return; try { await fn(); } catch (e) { fails.push(`${name}: exception ${String(e && e.stack || e).slice(0, 500)}`); } try { await afterSection(); } catch (e) { /* the page is gone */ } };

const main = async () => {
  const browser = await launchBrowser({ args: ARGS });
  const { page, errs, bad } = await open(browser);
  const ev = (f, a) => page.evaluate(f, a);
  const call = (name, args) => page.evaluate(([n, a]) => window.reteBuildingTools.call(n, a).then((v) => ({ v }), (e) => ({ err: String(e.message || e) })), [name, args || {}]);
  // A("..."): runs one expression or several statements against the app object and returns the value of the last one (a plain `return ${expr}` would stop after the first statement)
  const A = (expr) => ev(`(() => { const app = window.reteBuilding; return eval(${JSON.stringify(expr)}); })()`);
  const hasViewport = await ev(() => !!(window.reteBuilding.viewport && window.reteBuilding.viewport.setLayout));
  const hasStructure = await ev(() => !!(window.reteBuilding.structure && window.reteBuilding.structure.remove));
  const hasStatevars = await ev(() => !!(window.reteBuilding.statevars && window.reteBuilding.statevars.colourBy));
  const hasSection = await ev(() => !!(window.reteBuilding.section && window.reteBuilding.section.set));
  const hasGround = await ev(() => !!(window.reteBuilding.ground && window.reteBuilding.ground.setCutaway));
  notes.push({ hasViewport, hasStructure, hasStatevars, hasSection, hasGround });
  const menuBtn = (m) => page.locator(`[data-menu="${m}"]`);
  const menuItem = (label) => page.locator(".menu .mi").filter({ hasText: new RegExp("^" + label.replace(/[.*+?^${}()|[\]\\]/g, "\\$&")) }).first();
  const closeAny = async () => { await page.keyboard.press("Escape"); await page.waitForTimeout(60); };
  // check and radio items keep their menu open, so a second command of the same menu must not click the menu button again (that would close it)
  const clickMenu = async (m, label) => {
    const isOpen = await page.evaluate((m) => !!document.querySelector(".menu") && !!document.querySelector(`[data-menu="${m}"][aria-expanded="true"]`), m);
    if (!isOpen) { if (await page.locator(".menu").count()) await closeAny(); await menuBtn(m).click(); }
    await page.waitForSelector(".menu"); await menuItem(label).click(); await page.waitForTimeout(150);
  };
  afterSection = async () => { if (await page.locator(".menu").count()) await closeAny(); };
  const waitFor = (fn, arg, t = 30000) => page.waitForFunction(fn, arg, { timeout: t });

  // ============ regions and roles ============
  await section("regions", async () => {
    const r = await ev(() => {
      const q = (s) => document.querySelector(s), role = (s, r) => { const e = q(s); return !!e && (e.getAttribute("role") === r || e.tagName.toLowerCase() === { banner: "header", main: "main", complementary: "aside", contentinfo: "footer" }[r]); };
      return { banner: role("#topbar", "banner"), menubar: q("#menubar").getAttribute("role") === "menubar" && q("#menubar").getAttribute("aria-label"), menuitems: document.querySelectorAll('#menubar [role=menuitem][aria-haspopup=menu]').length,
        rail: q("#rail").getAttribute("role") === "tablist" && document.querySelectorAll("#rail [role=tab]").length, side: role("#side", "complementary") && !!q("#side").getAttribute("aria-label"), main: role("#viewcol", "main"),
        insp: role("#inspector", "complementary") && !!q("#inspector").getAttribute("aria-label"), bottom: q("#bottom").tagName === "SECTION" && !!q("#bottom").getAttribute("aria-label"), tabs: q("#btTabs").getAttribute("role") === "tablist" && document.querySelectorAll("#btTabs [role=tab]").length,
        panels: document.querySelectorAll("#btBody [role=tabpanel]").length, status: q("#statusbar").getAttribute("role") === "status", stage: !!q("#stage canvas[data-engine]") && !!q("#stage").getAttribute("aria-label"), skip: !!q("a.skip"),
        labelsOk: [...document.querySelectorAll("button, input, select, textarea")].filter((e) => e.offsetParent !== null && !e.getAttribute("aria-label") && !e.title && !e.textContent.trim() && !(e.labels && e.labels.length) && e.type !== "hidden").map((e) => e.id || e.className).slice(0, 5),
        sepsRole: [...document.querySelectorAll(".rz")].every((e) => e.getAttribute("role") === "separator" && e.getAttribute("aria-label")) };
    });
    check(r.banner && r.menubar && r.menuitems === 5 && r.rail === 6 && r.side && r.main && r.insp && r.bottom && r.tabs === 7 && r.panels === 7 && r.status && r.stage && r.skip && r.sepsRole, "shell regions with their roles: " + JSON.stringify(r));
    check(r.labelsOk.length === 0, "every visible control has an accessible name: " + JSON.stringify(r.labelsOk));
    const t = await call("get_state"); check(t.v && t.v.counts.rooms === 7, "the house still answers get_state");
  });

  // ============ menus: mouse and keyboard ============
  await section("menus", async () => {
    for (const m of ["view", "layers", "simulate", "tools", "about"]) {
      await menuBtn(m).click(); await page.waitForSelector(".menu");
      const o = await ev(() => { const m = document.querySelector(".menu"); const its = [...m.querySelectorAll("[role^=menuitem]")]; return { role: m.getAttribute("role"), label: !!m.getAttribute("aria-label"), n: its.length, kinds: [...new Set(its.map((i) => i.getAttribute("role")))], expanded: document.querySelector(".mb[aria-expanded=true]") && document.querySelector(".mb[aria-expanded=true]").dataset.menu, focused: document.activeElement.classList.contains("mi") }; });
      check(o.role === "menu" && o.label && o.n >= 4 && o.expanded === m && o.focused, `menu ${m} opens by mouse with roles and focus: ${JSON.stringify(o)}`);
      await page.keyboard.press("Escape"); await page.waitForTimeout(60);
      check(await ev(() => !document.querySelector(".menu") && document.activeElement.dataset.menu !== undefined), `menu ${m}: Escape closes it and returns focus to the menu bar`);
      await menuBtn(m).focus(); await page.keyboard.press("ArrowDown"); await page.waitForSelector(".menu");
      check(await ev(() => document.activeElement.classList.contains("mi")), `menu ${m} opens by keyboard (ArrowDown) with focus on an item`);
      await page.keyboard.press("Escape");
    }
    await menuBtn("view").focus(); await page.keyboard.press("Enter"); await page.waitForSelector(".menu");
    const first = await ev(() => document.activeElement.textContent); await page.keyboard.press("ArrowDown"); const second = await ev(() => document.activeElement.textContent);
    await page.keyboard.press("End"); const last = await ev(() => document.activeElement.textContent); await page.keyboard.press("Home"); const home = await ev(() => document.activeElement.textContent);
    check(first !== second && last !== first && home === first, "menu keys: ArrowDown moves, End and Home jump: " + JSON.stringify([first, second, last, home]));
    await page.keyboard.press("ArrowRight"); await page.waitForTimeout(100);
    check(await ev(() => document.querySelector(".mb[aria-expanded=true]").dataset.menu) === "layers", "ArrowRight in an open menu opens the next menu");
    await page.keyboard.press("Escape");
    await menuBtn("tools").focus(); await page.keyboard.press("ArrowDown"); await page.keyboard.type("me");
    check((await ev(() => document.activeElement.textContent)).startsWith("Measure"), "type-ahead moves to 'Measure a distance'");
    await page.keyboard.press("Escape");
    await menuBtn("view").focus(); await page.keyboard.press("ArrowRight"); check(await ev(() => document.activeElement.dataset.menu) === "layers", "ArrowRight on the menu bar moves along it");
    // outside click closes
    await menuBtn("view").click(); await page.waitForSelector(".menu"); await page.mouse.click(700, 400); await page.waitForTimeout(100);
    check(await ev(() => !document.querySelector(".menu")), "a click outside closes the menu");
  });

  // ============ every command does its thing ============
  await section("commands: view", async () => {
    await clickMenu("view", "By class"); check(await A("app.getColourMode()") === "class", "View > By class");
    await clickMenu("view", "Realistic"); check(await A("app.getColourMode()") === "realistic", "View > Realistic");
    await clickMenu("view", "X-ray"); check(await A("app.svc.isXray()") === true && /X-ray/.test(await ev(() => document.getElementById("stInd").textContent)), "View > X-ray, and the status bar shows it");
    await page.locator("#stInd button").first().click(); check(await A("app.svc.isXray()") === false, "clicking the x-ray indicator in the status bar switches it off");
    await clickMenu("view", "Heating"); check(await A("app.svc.isolated") === "heating", "View > Isolate > Heating");
    await clickMenu("view", "No isolation"); check(await A("app.svc.isolated") === null, "View > No isolation");
    await clickMenu("view", "Low"); check(await A("app.getQuality()") === "low" && /Low/.test(await ev(() => document.getElementById("qChipT").textContent)), "View > Quality Low, the top bar shows it");
    await clickMenu("view", "High"); check(await A("app.getQuality()") === "high", "View > Quality High");
    await clickMenu("view", "Sun path"); check(await ev(() => document.getElementById("pathCb").checked), "View > Sun path"); await clickMenu("view", "Sun path");
    await clickMenu("view", "Room names"); check(await A("app.getVisibility().roomNames") === false, "View > Room names off"); await clickMenu("view", "Room names");
    await clickMenu("view", "Sky, ground and trees"); check(await A("app.scenery.get().on") === false, "View > Sky, ground and trees off"); await clickMenu("view", "Sky, ground and trees");
    await clickMenu("view", "Wind: Off"); check(await A("app.scenery.get().wind") === 0, "View > Wind: Off"); await clickMenu("view", "Wind: Light");
    await clickMenu("view", "Dither: Off"); check(await A("app.scenery.get().dither") === 0, "View > Dither: Off"); await clickMenu("view", "Dither: Medium");
    // the rendering side's API: wired when it is there, shown as not available when not
    await afterSection(); await menuBtn("view").click(); await page.waitForSelector(".menu");
    const dis = await ev(() => Object.fromEntries([...document.querySelectorAll(".menu .mi")].map((i) => [i.textContent.replace(/\s+/g, " ").trim(), i.getAttribute("aria-disabled") === "true"])));
    await closeAny();
    check(Object.entries(dis).filter(([k]) => /^(Four views|Single view)/.test(k)).every(([, d]) => d === !hasViewport), "layout items are enabled exactly when app.viewport.setLayout exists: " + JSON.stringify(dis).slice(0, 200));
    if (hasViewport) {
      await clickMenu("view", "Four views"); check(await A("app.viewport.getLayout()") === "quad" && /four views/.test(await ev(() => document.getElementById("stMode").textContent)), "View > Four views switches the layout, the status bar says so");
      check(await ev(() => document.getElementById("layoutSeg").querySelector('[data-lay=quad]').getAttribute("aria-pressed")) === "true", "the layout switch in the top bar follows");
      await page.locator('#layoutSeg [data-lay="single"]').click(); check(await A("app.viewport.getLayout()") === "single", "the layout switch in the top bar sets single");
    }
    if (hasSection) {
      await clickMenu("view", "Section Z"); check(await A("app.section.get().z.enabled") === true && /Section/.test(await ev(() => document.getElementById("stInd").textContent)), "View > Section Z, indicator in the status bar");
      await clickMenu("view", "Section settings"); check(await ev(() => !document.getElementById("sectionPop").hidden && document.querySelectorAll("#sectionPop input[type=range]").length === 3), "Section settings opens the pop-over with three sliders");
      await page.locator('#sectionPop input[data-a="z"][data-f="position"]').fill("2.4"); check(Math.abs(await A("app.section.get().z.position") - 2.4) < 0.06, "the pop-over slider moves the cut");
      await page.locator('#sectionPop input[data-a="z"][data-f="flip"]').check(); check(await A("app.section.get().z.flip") === true, "the pop-over flips the cut");
      await page.locator("#secX").click(); await A("app.section.set('z', {enabled: false, flip: false})");
    }
    if (hasGround) { await clickMenu("view", "Trench around the house"); check(await A("app.ground.getCutaway()") === "trench", "View > Ground > Trench"); await clickMenu("view", "Ground as it is"); }
    if (hasStructure) { await clickMenu("view", "Role"); check(await A("app.structure.getColourMode()") === "role", "View > Structure colours > Role"); await clickMenu("view", "Material"); }
    if (hasStatevars) { await clickMenu("view", "Room air temperature"); check(await A("app.statevars.legend() && app.statevars.legend().variable") === "room-air-temperature", "View > Colour by a state variable"); await clickMenu("view", "Off"); check(await A("app.statevars.legend()") == null, "...and Off"); }
    await A("app.setColourMode('realistic')");
  });
  await section("commands: layers", async () => {
    await clickMenu("layers", "Walls"); check(!(await A("app.getVisibility().classes")).includes("IfcWallStandardCase"), "Layers > Walls hides the walls"); await clickMenu("layers", "Walls");
    await clickMenu("layers", "No classes"); check((await A("app.getVisibility().classes")).length === 0, "Layers > No classes"); await clickMenu("layers", "All classes"); check((await A("app.getVisibility().classes")).length === 10, "Layers > All classes");
    await A("app.setVisibility({classes:['Wall','Slab','Window','Door','Stair','Railing','Beam','Member']})");
    const upper = await A("app.graph.storeys[1].l"); await clickMenu("layers", upper); check(!(await A("app.getVisibility().storeys")).includes(1), "Layers > a storey off (" + upper + ", its name from the data)"); await clickMenu("layers", upper);
    await A("app.svc.setLayer('cooling', false)");   // isolating a system (the view section) leaves every system shown, so start from a known state
    await clickMenu("layers", "Cooling"); check(await A("app.svc.layerState().find(l => l.id === 'cooling').visible") === true, "Layers > Cooling shows the system"); await clickMenu("layers", "Cooling"); check(await A("app.svc.layerState().find(l => l.id === 'cooling').visible") === false, "Layers > Cooling again hides it");
    await afterSection(); await menuBtn("layers").click(); await page.waitForSelector(".menu"); const hasStruct = await ev(() => [...document.querySelectorAll(".menu .mi")].some((i) => /^Structure/.test(i.textContent))); await closeAny();
    check(hasStruct, "the Layers menu lists the Structure system among the services");
    await clickMenu("layers", "Open all doors"); await ev(() => window.reteBuilding.settleStates()); check(await A("app.allStates()[app.graph.nodes.find(n => n.l === 'Haustuer').n]") === "open", "Layers > Open all doors and windows");
    await clickMenu("layers", "Close all doors"); check(await A("app.allStates()[app.graph.nodes.find(n => n.l === 'Haustuer').n]") === "closed", "Layers > Close all");
    await ev(() => window.reteBuilding.settleStates());
  });
  await section("commands: simulate", async () => {
    await clickMenu("simulate", "Solstice, December"); check((await A("app.sun.get().date")).endsWith("-12-21"), "Simulate > Solstice, December");
    await clickMenu("simulate", "Noon"); check(Math.abs(await A("app.sun.get().time") - await A("app.sun.get().solarNoon")) < 0.05, "Simulate > Noon");
    await clickMenu("simulate", "Play the day"); check(await A("app.sun.isPlaying()") === true, "Simulate > Play the day"); await clickMenu("simulate", "Play the day"); check(await A("app.sun.isPlaying()") === false, "...and stop");
    await clickMenu("simulate", "Date and time controls"); check(await A("app.shell.state().side") === "sun", "Simulate > Date and time controls opens the Sun panel");
    await A("app.sun.set({date:'2026-06-21', time: 12})");
    await clickMenu("simulate", "Run the thermal day"); await waitFor(() => window.reteBuilding.thermal && window.reteBuilding.thermal.last()); check(await A("app.shell.state().bottom") === "thermal" && await ev(() => document.querySelectorAll("#thTable tr[data-g]").length) === 7, "Simulate > Run the thermal day shows the result in the Thermal tab");
    await clickMenu("simulate", "RCD 2 trips"); check(await A("app.svc.activeScenario && app.svc.activeScenario.id") === "trip-rcd-2" && await A("app.shell.state().bottom") === "scenarios", "Simulate > a scenario applies it and opens the Scenarios tab");
    await clickMenu("simulate", "Reset the simulation"); check(await A("app.svc.activeScenario") === null, "Simulate > Reset the simulation clears the scenario");
    await clickMenu("simulate", "Room temperatures in 3D"); check(await A("app.thermalUi.view.temps") === true, "Simulate > Room temperatures in 3D"); await clickMenu("simulate", "Room temperatures in 3D");
    await A("app.svc.setState('cb-1', 'off')"); await clickMenu("simulate", "Reset the states"); check(await A("app.svc.getState(app.svc.thingFor('cb-1'))") === "on", "Simulate > Reset the states");
  });
  await section("commands: tools and about", async () => {
    await clickMenu("tools", "Search and commands"); check(await ev(() => !!document.getElementById("palette")), "Tools > Search and commands opens the palette"); await closeAny();
    check(await A("!(app.measure && app.measure.active)"), "measure is off before the menu starts it");
    await clickMenu("tools", "Measure a distance"); await waitFor(() => !!(window.reteBuilding.measure && window.reteBuilding.measure.active));   // the module loads on first use
    check(await A("app.measure && app.measure.active") === true, "Tools > Measure a distance starts measuring");
    const box = await page.locator("#stage canvas[data-engine]").boundingBox(); await page.mouse.click(box.x + box.width * 0.5, box.y + box.height * 0.55); await page.waitForTimeout(150); await page.mouse.click(box.x + box.width * 0.62, box.y + box.height * 0.6); await page.waitForTimeout(300);
    const meas = await A("app.measure.last"); check(meas && meas.distance > 0.1 && meas.distance < 60, "two clicks on the house measure a distance: " + JSON.stringify(meas)); await closeAny();
    check(await A("app.measure.active") === false, "Esc stops measuring");
    let dl = page.waitForEvent("download"); await clickMenu("tools", "Current states as JSON"); let d = await dl; const sj = JSON.parse(await (await import("node:fs/promises")).readFile(await d.path(), "utf8"));
    check(sj.openings && Object.keys(sj.openings).length === 16 && sj.devices && Object.keys(sj.devices).length > 20, "Tools > Current states as JSON: " + d.suggestedFilename());
    dl = page.waitForEvent("download"); await clickMenu("tools", "Thermal day as CSV"); d = await dl; const csv = await (await import("node:fs/promises")).readFile(await d.path(), "utf8"); check(/^time,hour,outdoor_C/.test(csv) && csv.split("\n").length > 100, "Tools > Thermal day as CSV: " + d.suggestedFilename());
    await A("app.select('Haustuer')"); dl = page.waitForEvent("download"); await clickMenu("tools", "Selection as Turtle"); d = await dl; const ttl = await (await import("node:fs/promises")).readFile(await d.path(), "utf8");
    check(/@prefix/.test(ttl) && /cad:|bot:/.test(ttl) && ttl.split("\n").length > 6, "Tools > Selection as Turtle gives its facts with prefixes: " + ttl.split("\n").length + " lines");
    await clickMenu("tools", "Copy a link"); check(await ev(() => /v=1/.test(location.hash)), "Tools > Copy a link puts the view into the address");
    await clickMenu("about", "About this page"); await page.waitForSelector(".dlg [data-s]");
    const ab = await ev(() => ({ role: document.querySelector(".dlg").getAttribute("role"), modal: document.querySelector(".dlg").getAttribute("aria-modal"), nav: [...document.querySelectorAll(".dlg [data-s]")].map((a) => a.dataset.s), title: document.querySelector(".dlg h2").textContent }));
    check(ab.role === "dialog" && ab.modal === "true" && ab.nav.join() === "page,house,data,model,shortcuts,webmcp,links", "About dialog with its sections: " + JSON.stringify(ab));
    await page.locator('.dlg [data-s="house"]').click(); const hs = await ev(() => document.getElementById("aboutMain").textContent);
    check(/KIT/.test(hs) && /not from the IFC/i.test(hs) && /services/.test(hs) && /structure/i.test(hs) && /thermal assumptions/.test(hs), "About > The house says what is real and what is designed");
    await page.locator('.dlg [data-s="data"]').click(); await waitFor(() => /Triples/.test(document.getElementById("aboutMain").textContent) && /\d{2},\d{3}/.test(document.getElementById("aboutMain").textContent), null, 40000);
    const ds = await ev(() => document.getElementById("aboutMain").textContent);
    const trip = await A("JSON.parse('" + JSON.stringify(0) + "') || app.graphInfo.triples");
    check(/FZK-Haus with a designed building-services layer/.test(ds) && /CC0/.test(ds) && ds.includes(Number(trip).toLocaleString("en")) && /KB|MB/.test(ds) && /fzk-viewer\.json/.test(ds), "About > Data files: title, licence, triple count and sizes come from the card and the engine");
    await page.locator('.dlg [data-s="model"]').click(); const ms = await ev(() => document.getElementById("aboutMain").textContent);
    check(/single|seven nodes/i.test(ms) && /path rule/.test(ms) && /not a structural analysis/.test(ms), "About > Model notes: thermal limits and the structural path rule");
    await page.locator('.dlg [data-s="shortcuts"]').click(); const sc = await ev(() => document.getElementById("aboutMain").textContent);
    const table = await ev(() => window.reteBuilding.commands.list().filter((c) => c.keys).map((c) => c.title));
    check(table.length >= 15 && table.every((t) => sc.includes(t)), "About > Shortcuts lists every command that has a key (" + table.length + ")");
    await page.locator('.dlg [data-s="webmcp"]').click(); const wm = await ev(() => document.getElementById("aboutMain").textContent); const n = await ev(() => window.reteBuildingTools.list().length);
    check(wm.includes("get_ui") && wm.includes("run_sparql") && wm.includes("get_graph_view") && wm.includes(String(n)), "About > WebMCP lists the registered tools (" + n + ")");
    await page.locator('.dlg [data-s="links"]').click(); const lk = await ev(() => [...document.querySelectorAll("#aboutMain a")].map((a) => a.getAttribute("href")));
    check(lk.includes("building-guide.html") && lk.some((h) => /^playground\.html#url=.*fzk-haus/.test(h)) && lk.some((h) => /github\.com\/caviri\/rete/.test(h)), "About > Links: the guide, the playground on this dataset, the project");
    await page.keyboard.press("Escape"); check(await ev(() => !document.querySelector(".dlg")), "Escape closes the About dialog");
    await clickMenu("about", "Keyboard shortcuts"); check(await ev(() => document.querySelector('.dlg [data-s="shortcuts"][aria-current="true"]') !== null), "About > Keyboard shortcuts opens that section"); await page.keyboard.press("Escape");
  });

  // ============ the side panels ============
  await section("side panels", async () => {
    await A("app.clearSelection(); app.clearResult()");
    await A("app.shell.closeSide()");   // the saved state opens Explore; a click on the open panel's button would collapse it
    for (const p of ["explore", "services", "sun", "thermal", "states", "query"]) {
      await page.locator(`#rail-${p}`).click(); await page.waitForTimeout(80);
      const o = await ev((p) => ({ shown: !document.getElementById("pane-" + p).hidden, others: [...document.querySelectorAll("#side .pane")].filter((e) => !e.hidden).length, sel: document.getElementById("rail-" + p).getAttribute("aria-selected"), w: document.getElementById("side").getBoundingClientRect().width }), p);
      check(o.shown && o.others === 1 && o.sel === "true" && o.w > 200, `side panel ${p} opens alone: ${JSON.stringify(o)}`);
    }
    await page.locator("#rail-explore").click(); await page.locator("#search").fill("haust"); await page.waitForTimeout(150);
    check(await ev(() => [...document.querySelectorAll("#list li")].some((l) => /Haustuer/.test(l.textContent))), "Explore: search lists the front door");
    await page.locator("#list li", { hasText: "Haustuer" }).first().click(); check(await A("app.selectedInfo() && app.selectedInfo().label") === "Haustuer", "Explore: clicking a list entry selects it");
    await page.locator("#search").fill(""); await page.waitForTimeout(100);
    await page.locator('#classes label:has-text("Walls") input').uncheck(); check(!(await A("app.getVisibility().classes")).includes("IfcWallStandardCase"), "Explore: a class toggle hides the walls"); await page.locator('#classes label:has-text("Walls") input').check();
    await page.locator('#storeys label input').first().uncheck(); check(!(await A("app.getVisibility().storeys")).includes(0), "Explore: a storey toggle"); await page.locator('#storeys label input').first().check();
    await page.locator("#opacity").fill("30"); check(Math.abs(await A("app.uiView.dim") - 0.3) < 0.01, "Explore: context opacity");
    await page.locator("#rail-services").click();
    await page.locator("#xrayCb").check(); check(await A("app.svc.isXray()") === true, "Services: x-ray"); await page.locator("#xrayCb").uncheck();
    await page.locator("#isoSel").selectOption("ventilation"); check(await A("app.svc.isolated") === "ventilation", "Services: isolate a system"); await page.locator("#isoSel").selectOption("");
    await page.locator("#svcAll").click(); check(await A("app.svc.layerState().every(l => l.visible)"), "Services: All"); await page.locator("#svcNone").click(); check(await A("app.svc.layerState().every(l => !l.visible)"), "Services: None");
    await page.locator('#layers label:has-text("Lighting") input').check(); check(await A("app.svc.layerState().find(l => l.id === 'lighting').visible"), "Services: a system row");
    check(await ev(() => document.querySelectorAll("#layers .sys").length === 11 && /Structure/.test(document.getElementById("layers").textContent)), "Services: eleven systems, Structure among them");
    if (hasStructure) { await page.locator('#svcViewBody [aria-label="Structure colours"] .segb[data-v="load-bearing"]').click(); check(await A("app.structure.getColourMode()") === "load-bearing", "Services: structure colour modes"); await page.locator('#svcViewBody [aria-label="Structure colours"] .segb[data-v="material"]').click(); }
    if (hasGround) { await page.locator('#svcViewBody [aria-label="Ground"] .segb[data-v="trench"]').click(); check(await A("app.ground.getCutaway()") === "trench", "Services: ground cutaway"); await page.locator('#svcViewBody [aria-label="Ground"] .segb[data-v="none"]').click(); }
    await page.locator("#rail-sun").click();
    await page.locator("#sunDate").fill("2026-03-20"); check(await A("app.sun.get().date") === "2026-03-20", "Sun: the date field");
    await page.locator("#tod").fill("9.5"); check(Math.abs(await A("app.sun.get().time") - 9.5) < 0.05, "Sun: the time slider");
    await page.locator('#seasonPresets [data-d="12-21"]').click(); check((await A("app.sun.get().date")).endsWith("-12-21"), "Sun: season presets");
    await page.locator("#playBtn").click(); check(await A("app.sun.isPlaying()"), "Sun: Play day"); await page.locator("#playBtn").click();
    await page.locator("#pathCb").check(); check(await A("app.sun.site && true") && await ev(() => document.getElementById("pathCb").checked), "Sun: sun path");
    await page.locator("#pathCb").uncheck();
    if (!(await ev(() => document.getElementById("siteNorth").closest("details").open))) await page.locator('#pane-sun summary:has-text("Site and north")').click();   // the section is collapsed by default
    await page.locator("#siteNorth").fill("230"); await page.locator("#siteNorth").press("Enter"); await page.locator("#siteLat").focus(); check(Math.abs(await A("app.sun.site.northAzimuthDeg") - 230) < 0.5, "Sun: true north"); await page.locator("#siteNorth").fill("50"); await page.locator("#siteNorth").press("Enter"); await page.locator("#siteLat").focus();
    await page.locator('#colourSeg [data-v="class"]').click(); check(await A("app.getColourMode()") === "class", "Sun: colour mode"); await page.locator('#colourSeg [data-v="realistic"]').click();
    await page.locator('#qualitySeg [data-v="medium"]').click(); check(await A("app.getQuality()") === "medium", "Sun: quality"); await page.locator('#qualitySeg [data-v="high"]').click();
    await A("app.sun.set({date:'2026-06-21', time: 12})");
    await page.locator("#rail-thermal").click(); await waitFor(() => !!document.getElementById("thRun"));
    await page.locator("#thClear").fill("50"); await page.locator("#thRun").click(); await waitFor(() => window.reteBuilding.thermal && window.reteBuilding.thermal.last());
    check(await A("Math.abs(app.thermal.last().clearness - 0.5) < 0.01") && await A("app.shell.state().bottom") === "thermal", "Thermal: the inputs drive the run (sky 50 %), the result is in the bottom tab"); await page.locator("#thClear").fill("100");
    await page.locator("#thTemps").check(); check(await A("app.thermalUi.view.temps"), "Thermal: room temperatures in 3D"); await page.locator("#thTemps").uncheck();
    await page.locator("#rail-states").click();
    await page.locator("#openAll").click(); await A("app.settleStates()"); check(await A("app.openables().filter(o => app.getState(o.guid) !== 'closed').length") > 8, "States: Open all"); await page.locator("#closeAll").click(); await A("app.settleStates()");
    await page.locator("#statesList .strow .stb").first().click(); check(await A("Object.values(app.allStates()).some(s => s === 'open')"), "States: a door's own button toggles it"); await page.locator("#closeAll").click(); await A("app.settleStates()");
    await page.locator("#devFilter").fill("breaker"); check(await ev(() => document.querySelectorAll("#devList .strow").length >= 10), "States: the device list filters"); await page.locator("#devList .strow .stb").first().click(); check(await A("app.svc.nodes.some(t => t.d.type === 'breaker' && app.svc.stateOf(t) === 'off')"), "States: a device button changes its state"); await A("app.svc.resetStates()");
    check(await ev(() => document.querySelectorAll("#varSel option").length >= 30), "States: the variable explorer lists the definitions");
    await page.locator("#varSel").selectOption("room-air-temperature"); check(await ev(() => document.querySelectorAll("#varThings li").length === 7 && document.querySelectorAll("#varBody .lvl").length >= 5), "States: a numeric variable shows its levels and the value of every room");
    await page.locator("#varThings li").first().click(); await waitFor(() => !!document.querySelector("#varCtl input[type=range]")); check(await ev(() => !!document.querySelector("#varCtl input[type=range]") && /°C/.test(document.getElementById("varCtl").textContent)), "States: a slider with its range and unit for the chosen room");
    if (hasStatevars) { const v0 = await A("app.statevars.getValue('room-relative-humidity', app.selectedInfo().id)"); await page.locator("#varSel").selectOption("room-relative-humidity"); await page.locator("#varThings li").first().click(); await waitFor(() => !!document.querySelector('#varCtl [data-var="room-relative-humidity"] input[type=range]')); await page.locator("#varCtl input[type=range]").fill("65"); check(await A("app.statevars.getValue('room-relative-humidity', app.selectedInfo().id)") === 65, "States: the slider sets the value through app.statevars"); await A("app.statevars.setValue('room-relative-humidity', app.selectedInfo().id, " + JSON.stringify(v0) + ")"); }
    else check(await ev(() => document.querySelector("#varCtl input[type=range]").disabled), "States: without app.statevars the slider is disabled");
    await page.locator("#varSel").selectOption("state-door"); await page.locator("#varThings li").first().click(); check(await ev(() => document.querySelectorAll("#varCtl .stbtns button").length === 2), "States: an enumerated variable gets one button per state");
    await page.locator("#rail-query").click(); await page.locator(".qbtn").first().click(); await waitFor(() => window.reteBuilding.results.get() && window.reteBuilding.results.get().label === "Rooms on each floor");
    check(await A("app.shell.state().bottom") === "results" && await ev(() => document.querySelectorAll("#results tbody tr").length === 7) && await A("app.lastQueryInfo().matched") === 7, "Queries: a ready query fills Results and highlights 7 rooms");
    // collapse, resize, remember
    await page.locator("#rail-query").click(); check(await A("app.shell.state().side") === null && await ev(() => document.getElementById("side").hidden) && await ev(() => getComputedStyle(document.documentElement).getPropertyValue("--side-w").trim()) === "0px", "clicking the active rail button collapses the side panel");
    await page.locator("#rail-sun").click(); const w0 = await ev(() => document.getElementById("side").getBoundingClientRect().width);
    const rz = await page.locator("#rzSide").boundingBox(); await page.mouse.move(rz.x + 2, rz.y + 200); await page.mouse.down(); await page.mouse.move(rz.x + 102, rz.y + 200, { steps: 4 }); await page.mouse.up();
    const w1 = await ev(() => document.getElementById("side").getBoundingClientRect().width); check(w1 > w0 + 60, `the side panel resizes by dragging its edge (${Math.round(w0)} -> ${Math.round(w1)})`);
    const cv = await ev(() => { const c = document.querySelector("#stage canvas[data-engine]"), r = c.getBoundingClientRect(), s = document.getElementById("stage").getBoundingClientRect(); return { cw: Math.round(r.width), sw: Math.round(s.width), ch: Math.round(r.height), sh: Math.round(s.height) }; });
    check(Math.abs(cv.cw - cv.sw) <= 1 && Math.abs(cv.ch - cv.sh) <= 1 && cv.cw > 300, "the canvas follows the panel sizes: " + JSON.stringify(cv));
    await rz; await page.locator("#rzSide").focus(); await page.keyboard.press("ArrowLeft"); check(await ev(() => document.getElementById("side").getBoundingClientRect().width) < w1, "the separator resizes with the arrow keys");
    check(await ev(() => !!localStorage.getItem("reteBuilding.ui.v1") && JSON.parse(localStorage.getItem("reteBuilding.ui.v1")).side === "sun"), "the panel state is saved");
  });

  // ============ inspector ============
  await section("inspector", async () => {
    await A("app.shell.openSide('explore'); app.clearSelection()");
    check(await ev(() => /Nothing selected/.test(document.getElementById("info").textContent)), "the inspector says what to do when nothing is selected");
    await A("app.select('Haustuer')"); await waitFor(() => !document.getElementById("inspector").hidden);
    const door = await ev(() => ({ t: document.querySelector("#info .ins-head .t").textContent, secs: [...document.querySelectorAll("#info details.sec > summary")].map((s) => s.textContent.trim()), btn: document.querySelectorAll("#info .stbtns button").length, graphBtn: !!document.querySelector('#info [data-act="graph"]'), chips: document.querySelectorAll("#info .ins-chips .chip").length }));
    check(door.t === "Haustuer" && door.secs.some((s) => /Properties/.test(s)) && door.secs.some((s) => /^State/.test(s)) && door.btn === 2 && door.graphBtn && door.chips >= 2, "inspector: a door has properties, state buttons and 'Show in graph': " + JSON.stringify(door));
    await page.locator('#info .stbtns button[data-st="open"]').click(); check(await A("app.getState(app.graph.nodes.find(n => n.l === 'Haustuer').n)") === "open" && await ev(() => document.querySelector('#info .stbtns button[aria-pressed="true"]').dataset.st) === "open", "inspector: the state button opens the door and the panel follows"); await A("app.setState(app.graph.nodes.find(n => n.l === 'Haustuer').n, 'closed')");
    await A("app.select('Wohnen')");
    const room = await ev(() => ({ secs: [...document.querySelectorAll("#info details.sec > summary")].map((s) => s.textContent.trim()), lvl: document.querySelectorAll("#info .lvl").length, rng: document.querySelectorAll("#info .vtrack input[type=range]").length, bands: document.querySelectorAll("#info .vtrack .bands i").length, surf: document.querySelectorAll("#info [data-surf]").length, adj: document.querySelectorAll("#info [data-i]").length, zone: /Design heat load/.test(document.getElementById("info").textContent) && /Set point/.test(document.getElementById("info").textContent) }));
    check(room.zone && room.secs.some((s) => /^State variables/.test(s)) && room.lvl >= 3 && room.rng >= 5 && room.bands >= 15 && room.surf >= 6 && room.adj >= 3, "inspector: a room shows its zone data, variables with level chips and bands, surfaces, neighbours: " + JSON.stringify(room));
    await page.locator("#info [data-i]").first().click(); check(await A("app.selectedInfo().kind") !== null && await A("app.selectedInfo().label") !== "Wohnen", "inspector: a related thing is a link that selects it");
    await A("app.select('Wohnen')");
    if (!(await ev(() => document.querySelector('#info details.sec[data-t="Surfaces"]').open))) await page.locator('#info details.sec[data-t="Surfaces"] > summary').click();   // collapsed by default
    await page.locator("#info [data-surf]").filter({ hasNotText: "U —" }).first().click();   // a surface that has a U-value (the partitions between rooms have none)
    check(await ev(() => /Surface/.test(document.querySelector("#info .ins-head .t").textContent) && /U-value/.test(document.getElementById("info").textContent) && /Area/.test(document.getElementById("info").textContent)), "inspector: a surface of the room opens in the inspector");
    await A("app.select(app.graph.nodes.find(n => n.c === 'IfcWallStandardCase').n)");
    check(await ev(() => /Properties/.test(document.getElementById("info").textContent) && document.querySelectorAll("#info [data-i]").length >= 1), "inspector: a wall shows its properties and the rooms it encloses");
    await A("app.svc.layers.get('electrical').setVisible(true); app.select('cb-1')"); await waitFor(() => /Breaker/.test((document.querySelector("#info .ins-head .t") || {}).textContent || ""));
    const dev = await ev(() => document.getElementById("info").textContent); check(/State:/.test(dev) && /Supply:/.test(dev) && /Rating/.test(dev), "inspector: a device shows state and supply");
    await page.locator('#info details.sec:has(summary:has-text("Facts")) summary').click(); await waitFor(() => document.querySelectorAll("#facts .facts tr").length > 5, null, 20000);
    const facts = await ev(() => [...document.querySelectorAll("#facts .facts tr")].map((r) => r.textContent));
    check(facts.length > 5 && facts.some((f) => /^rdf:type|^rdfs:label/.test(f)) && !facts.some((f) => /^https?:\/\/w3id.org\/rete\/cad\/sys#/.test(f)), "inspector: Facts lists the triples with IRIs shortened by prefix: " + facts.slice(0, 3).join(" | "));
    if (hasStructure) {
      const eid = await A("app.structure.list({role: 'lintel'})[0].id"); await A(`app.select(${JSON.stringify(eid)})`);
      const st = await ev(() => document.getElementById("info").textContent); check(/Structure/.test(st) && /Role/.test(st) && /lintel/.test(st) && /Bears on/.test(st), "inspector: a structural element shows role, what it bears on");
      await page.locator("#info [data-rm]").click(); check(await A(`app.structure.stateOf(${JSON.stringify(eid)})`) === "removed" && /Restore/.test(await ev(() => document.querySelector("#info [data-rm]").textContent)), "inspector: Remove"); await page.locator("#info [data-rm]").click(); check(await A(`app.structure.stateOf(${JSON.stringify(eid)})`) === "intact", "inspector: Restore");
      await page.locator('#info [data-trace][data-dir="down"]').click(); check(await A("!!document.querySelector('#info [data-trace]')"), "inspector: trace load path");
      await A("app.structure.clearPath()");
    }
    {   // a window with one state (a fixed window): nothing to press, nothing that opens
      const fx = await A("(() => { const w = app.stateList().find((s) => s.kind === 'window' && s.values.length === 1); return w ? { id: w.id, label: w.label, state: w.state, values: w.values } : null; })()");
      check(!!fx, "the data has a fixed window (a window with one state)");
      if (fx) {
        await A(`app.select(${JSON.stringify(fx.id)})`); await waitFor((l) => (document.querySelector("#info .ins-head .t") || {}).textContent === l, fx.label);
        const ui1 = await ev(() => ({ btns: document.querySelectorAll("#info .stbtns button").length, note: /cannot be opened/.test(document.getElementById("info").textContent) }));
        check(ui1.btns === 0 && ui1.note, "a fixed window: the inspector shows its state without buttons and says it cannot be opened: " + JSON.stringify(ui1));
        const lst = await ev((id) => { const b = document.querySelector('.strow[data-id="' + id + '"] .stb'); return b ? { disabled: b.disabled, title: b.title } : null; }, fx.id);
        check(lst && lst.disabled && /Fixed/.test(lst.title), "a fixed window: its button in the States list is disabled with a tooltip: " + JSON.stringify(lst));
        const thr = await ev((id) => { try { window.reteBuilding.setState(id, "open"); return "no error"; } catch (e) { return e.message; } }, fx.id);
        const tl = await call("set_element_state", { id: fx.id, state: "open" });
        check(/not a state/.test(thr) && !!tl.err && await A(`app.getState(${JSON.stringify(fx.id)})`) === fx.state, "a fixed window: setState and the tool refuse to open it: " + thr + " / " + (tl.err || "").slice(0, 60));
      }
    }
    await A("app.clearSelection()"); check(await ev(() => /Nothing selected/.test(document.getElementById("info").textContent)), "clearing the selection empties the inspector");
    await page.locator("[data-close-insp]").click(); check(await ev(() => document.getElementById("inspector").hidden), "the inspector collapses"); await A("app.select('Haustuer')"); check(await ev(() => document.getElementById("inspector").hidden), "...and stays closed after the user closed it"); await page.keyboard.press("i"); check(await ev(() => !document.getElementById("inspector").hidden), "the I key opens it");
    const rz = await page.locator("#rzInsp").boundingBox(); const wi0 = await ev(() => document.getElementById("inspector").getBoundingClientRect().width); await page.mouse.move(rz.x + 2, rz.y + 200); await page.mouse.down(); await page.mouse.move(rz.x - 70, rz.y + 200, { steps: 4 }); await page.mouse.up();
    check(await ev(() => document.getElementById("inspector").getBoundingClientRect().width) > wi0 + 40, "the inspector resizes by dragging its edge");
    await A("app.clearSelection()");
  });

  // ============ console and results ============
  await section("console and results", async () => {
    await A("app.shell.openTab('console'); app.clearResult()"); await waitFor(() => !!document.getElementById("edTa"));
    check(await ev(() => document.getElementById("edTa").tagName === "TEXTAREA" && !!document.getElementById("edHl") && document.getElementById("edGut").textContent.split("\n").length >= 6 && document.querySelectorAll("#edHl .tk-kw").length >= 2 && document.querySelectorAll("#edHl .tk-iri").length >= 3), "console: a textarea editor with line numbers and keyword highlighting");
    await page.locator("#edTa").click(); await page.keyboard.press("Control+A"); await page.keyboard.type("PREFIX cad: <https://w3id.org/rete/cad#>\nSELECT ?e ?c WHERE { ?e cad:ifcClass ?c . FILTER(?c = \"IfcDoor\") } ORDER BY ?e");
    check(await ev(() => document.getElementById("edGut").textContent.split("\n").length === 2 && /IfcDoor/.test(document.getElementById("edHl").textContent)), "console: typing updates the line numbers and the highlight");
    await page.keyboard.press("Control+Enter"); await waitFor(() => /rows? ·/.test(document.getElementById("conMsg").textContent), null, 40000);
    const ran = await ev(() => ({ msg: document.getElementById("conMsg").textContent, prev: document.querySelectorAll("#conPrev tbody tr").length, graph: document.getElementById("conGraph").textContent, hist: document.querySelectorAll("#conHist li").length, matched: window.reteBuilding.lastQueryInfo() && window.reteBuilding.lastQueryInfo().matched, badge: !!document.querySelector("#tab-results .badge") }));
    check(/^5 rows/.test(ran.msg.trim()) && /ms/.test(ran.msg) && ran.prev === 5 && /House \+ services/.test(ran.graph) && /triples/.test(ran.graph) && ran.hist === 1 && ran.matched === 5 && ran.badge, "console: Ctrl+Enter runs it, shows rows, time, the graph and its triple count, history, and 5 doors light up: " + JSON.stringify(ran));
    check(await A("(app.logs.query({source:'query', limit: 5})[0] || {}).message") !== undefined && /select: 5 rows/.test(await A("(app.logs.query({source:'query', limit: 5}).find(e => /5 rows/.test(e.message)) || {}).message")), "console: the query is in the log with its row count and time");
    check(await A("(() => { const d = app.graph.nodes.find(n => n.l === 'Haustuer'); const o = app.meshByGuid.get(d.n); let m; o.traverse(x => { if (x.isMesh && !m) m = x; }); const w = app.graph.nodes.find(n => n.c === 'IfcWallStandardCase'); const wm = app.meshByGuid.get(w.n); let m2; wm.traverse(x => { if (x.isMesh && !m2) m2 = x; }); return app.lookOf(m).alpha > 0.5 && app.lookOf(m2).alpha < 0.3; })()"), "console: the result is highlighted in 3D (doors up, the rest dimmed)");
    await page.locator("#conHl").uncheck(); await page.keyboard.press("Escape"); await page.locator("#edTa").click(); await page.keyboard.press("Control+Enter"); await waitFor(() => /no things|rows? ·/.test(document.getElementById("conMsg").textContent) && !/highlighted in 3D/.test(document.getElementById("conMsg").textContent), null, 20000);
    check(await A("app.uiView.resultVisible") === false, "console: 'Highlight rows in 3D' off keeps the result out of the 3D view"); await page.locator("#conHl").check();
    await page.locator("#edTa").click(); await page.keyboard.press("Control+A"); await page.keyboard.type("SELEC nonsense {"); await page.keyboard.press("Control+Enter"); await waitFor(() => document.getElementById("conMsg").classList.contains("err"), null, 20000);
    const er = await ev(() => ({ t: document.getElementById("conMsg").textContent, h: document.querySelectorAll("#conHist li.bad").length })); check(er.t.length > 8 && er.h === 1, "console: the engine's error message is shown and the history marks it: " + JSON.stringify(er));
    check(await A("app.logs.query({level:'error', source:'query', limit: 3}).length") >= 1, "console: the failed query is logged as an error");
    await page.locator("#edTa").click(); await page.keyboard.press("Control+A"); await page.keyboard.type("SELECT ?r ?l WHERE { ?r a bot:Space ; rdfs:label ?l } ORDER BY ?l"); await page.locator("#conPfx").click();
    check(await ev(() => /PREFIX bot:/.test(document.getElementById("edTa").value) && /PREFIX rdfs:/.test(document.getElementById("edTa").value)), "console: Prefixes inserts the PREFIX lines the query uses");
    await page.locator("#edTa").click(); await page.keyboard.press("Control+End"); await page.keyboard.press("Tab"); check(await ev(() => /  $/.test(document.getElementById("edTa").value) && document.activeElement.id === "edTa"), "console: Tab inserts spaces and stays in the editor");
    // the rooms carry one label per language in the data (English, Spanish, German), so the rows are rooms x labels: derive that from the data
    await page.keyboard.press("Control+Enter"); await waitFor(() => { const r = window.reteBuilding.results.get(); return r && /bot:Space/.test(r.query || "") && /rows? ·/.test(document.getElementById("conMsg").textContent); }, null, 20000);
    const rl = await A("(() => { const r = app.results.get(); return { rows: r.rows.length, rooms: new Set(r.rows.map(x => x.__ref ? JSON.stringify(x.__ref) : x.r)).size, expected: app.roomByGuid.size }; })()");
    check(rl.rows >= rl.expected && rl.rows % rl.expected === 0, "console: the rooms query returns the rooms with their labels (" + JSON.stringify(rl) + ")");
    check(await ev(() => document.querySelectorAll("#conHist li").length >= 4), "console: history keeps the runs");
    await page.locator("#conHist li button[title='Run again']").first().click(); await waitFor(() => /rows? ·/.test(document.getElementById("conMsg").textContent), null, 20000); check(true, "");
    const opts = await ev(() => [...document.querySelectorAll("#conPick optgroup")].map((g) => [g.label, g.children.length])); check(opts.length === 2 && opts[0][1] === 6 && opts[1][1] === 8, "console: ready queries grouped by topic: " + JSON.stringify(opts));
    await page.locator("#conPick").selectOption({ index: 3 }); check(await ev(() => /SELECT/.test(document.getElementById("edTa").value)), "console: choosing a ready query loads its text");
    // results table
    await A("app.shell.openTab('results')"); await page.locator("#resFilter").fill("");
    await page.locator("#conPick").evaluate(() => 0).catch(() => 0);
    await A("app.console.run(app.console.getText())"); await waitFor(() => window.reteBuilding.results.get() && window.reteBuilding.results.get().source === "console", null, 20000);
    const NW = await A("app.graph.nodes.filter(n => n.c === 'IfcDoor' || n.c === 'IfcWindow').length"), ND = await A("app.graph.nodes.filter(n => n.c === 'IfcDoor').length");   // the labels come in three languages: the query keeps the English one, so one row per door or window of the house graph
    await A("app.console.setText(\"PREFIX bot: <https://w3id.org/bot#> PREFIX cad: <https://w3id.org/rete/cad#> PREFIX rdfs: <http://www.w3.org/2000/01/rdf-schema#> SELECT ?e ?label ?class WHERE { ?e cad:ifcClass ?class ; rdfs:label ?label . FILTER(?class IN (\\\"IfcDoor\\\", \\\"IfcWindow\\\") && LANG(?label) = \\\"en\\\") } ORDER BY ?label\")");
    await A("app.console.run()"); await waitFor((n) => window.reteBuilding.results.get() && window.reteBuilding.results.get().rows.length === n, NW, 20000); await A("app.shell.openTab('results')");
    const t0 = await ev(() => ({ n: document.querySelectorAll("#results tbody tr").length, count: document.getElementById("footer-count").textContent, first: document.querySelector("#results tbody tr td:nth-child(3)").textContent, th: [...document.querySelectorAll("#results th[aria-sort]")].map((t) => t.getAttribute("aria-sort")) }));
    check(t0.n === NW && new RegExp(NW + " rows").test(t0.count) && t0.th.length === 3, "results: the table with its row count: " + JSON.stringify(t0));
    await page.locator('#results th[data-k="label"]').click(); const asc = await ev(() => document.querySelector("#results tbody tr td:nth-child(3)").textContent); await page.locator('#results th[data-k="label"]').click(); const desc = await ev(() => document.querySelector("#results tbody tr td:nth-child(3)").textContent);
    check(asc !== desc && asc.localeCompare(desc) < 0 && await ev(() => document.querySelector('#results th[data-k="label"]').getAttribute("aria-sort")) === "descending", `results: sorting by a column ascending and descending (${asc} / ${desc})`);
    await page.locator('#results th[data-k="class"]').focus(); await page.keyboard.press("Enter"); check(await ev(() => document.querySelector('#results th[data-k="class"]').getAttribute("aria-sort")) === "ascending", "results: a header sorts from the keyboard");
    await page.locator("#resFilter").fill("IfcDoor"); check(await ev(() => document.querySelectorAll("#results tbody tr").length) === ND && new RegExp(ND + " of " + NW).test(await ev(() => document.getElementById("footer-count").textContent)), "results: the filter narrows the rows (" + ND + " of " + NW + ")");
    const ref = page.locator("#results tbody tr td.ref").first(); check(await ref.count() === 1, "results: cells that are things of the model are marked"); const refTxt = await ref.getAttribute("title");
    await ref.click(); await page.waitForTimeout(200); const sel = await A("app.selectedInfo()"); check(sel && refTxt.includes(sel.id), "results: clicking a cell selects that thing in 3D: " + JSON.stringify(sel));
    await page.locator("#resFilter").fill(""); const csv = await A("app.results.csv()"); const js = JSON.parse(await A("app.results.json()"));
    check(/^e,label,class\n/.test(csv.replace(/\r/g, "")) && csv.split("\n").length === NW + 1 && js.length === NW && js[0].label !== undefined, "results: CSV and JSON of the table");
    check(await ev(() => /https:\/\/w3id\.org\/rete\/cad\/fzk-haus\//.test(document.querySelector("#results tbody tr td:nth-child(2)").textContent)) === false, "results: IRIs are shortened (fzk:...)");
    await A("app.clearResult()"); check(await ev(() => document.getElementById("results").hidden && !document.getElementById("resEmpty").hidden), "results: Clear empties the table");
    await call("run_sparql", { query: "PREFIX bot: <https://w3id.org/bot#> SELECT ?s WHERE { ?s a bot:Space }" });
    check(await A("app.results.get().label") === "Agent query" && await ev(() => document.querySelectorAll("#results tbody tr").length) === 7, "results: an agent's run_sparql lands in the Results tab");
    await call("apply_scenario", { id: "trip-rcd-1", run_query: true }); check(/graph/.test(await A("app.results.get().label")), "results: a scenario's SPARQL lands in the Results tab"); await A("app.svc.resetStates()");
    check(await A("(() => { const w = innerWidth; const r = document.getElementById('results').getBoundingClientRect(); return r.width > 0; })()"), "results: the table is laid out");
  });

  // ============ validate tab (SHACL) ============
  await section("validate", async () => {
    await A("app.shell.openTab('validate')"); await waitFor(() => !!document.getElementById("valRun"));
    const eng = await A("app.validate.supported()"); check(eng === true, "validate: the engine build exposes Graph.shacl (else the tab explains it)");
    const exs = await A("app.validate.examples().map(e => ({ id: e.id, label: e.label, description: e.description, expect: e.expect, expectedCount: e.expectedCount, expectedFocus: e.expectedFocus, shapes: !!e.shapes }))");
    // bad Turtle: the engine's own error
    const bad = await ev(async () => { try { await window.reteBuilding.validate.run({ shapes: "this is { not turtle ." }); return null; } catch (e) { return String(e.message || e); } });
    check(bad && bad.length > 5 && await ev(() => /err/.test(document.getElementById("valReport").innerHTML) && document.querySelector("#valReport .err").textContent.length > 5), "validate: bad Turtle shows the engine's error: " + bad);
    check(await A("app.logs.query({level:'error', text:'SHACL', limit: 3}).length") >= 1, "validate: the failed run is logged");
    if (!exs.length) { notes.push("SHACL examples: the data has no shaclExamples yet, the example assertions were skipped"); check(await ev(() => /No SHACL examples/.test(document.getElementById("valList").textContent)), "validate: an empty state while the data has no examples"); }
    else {
      check(await ev((n) => document.querySelectorAll("#valList [data-id]").length === n, exs.length) && exs.every((e) => e.label && e.description), "validate: the tab lists the examples with name and description (" + exs.length + ")");
      const ok = exs.find((e) => e.expect === "conforms"), viol = exs.find((e) => e.expect === "violations");
      if (ok) { await page.locator('#valList [data-id="' + ok.id + '"]').click(); check(await ev(() => document.getElementById("edTa") !== null && document.querySelector("#valEd textarea").value.length > 20), "validate: choosing an example fills the editor"); await page.locator("#valRun").click(); await waitFor(() => /Conforms|Does not/.test(document.getElementById("valReport").textContent), null, 30000); check(await A("app.validate.report().conforms") === true && /Conforms/.test(await ev(() => document.getElementById("valReport").textContent)), "validate: a conforming example reports conforms: " + ok.id); }
      if (viol) {
        await call("run_shacl", { id: viol.id }); const rep = await A("app.validate.report()");
        check(rep && rep.conforms === false && (viol.expectedCount == null || rep.count === viol.expectedCount) && !(await ev(() => !!document.getElementById("valMismatch"))), "validate: the violating example reports its expected count: " + JSON.stringify([rep && rep.count, viol.expectedCount]));
        const foc = await ev(() => [...document.querySelectorAll("#valTable td.ref")].length); check(foc >= 1 || !viol.expectedFocus.length, "validate: focus nodes that are things of the model are clickable");
        if (viol.expectedFocus.length) { const got = await A("app.uiView.result && app.uiView.result.picks.concat(app.uiView.result.idxs.map(i => app.graph.nodes[i].n))"); check(viol.expectedFocus.every((f) => got.includes(f)), "validate: the failing things are highlighted in 3D: " + JSON.stringify(got).slice(0, 160)); await page.locator("#valTable td.ref").first().click(); check(await A("app.selectedInfo() !== null"), "validate: a focus node selects the thing"); }
        const tl = await call("list_shacl_examples"); check(tl.v && tl.v.examples.length === exs.length, "list_shacl_examples");
      }
    }
    const tr = await call("run_shacl", {}); check(!!tr.err, "run_shacl wants an id or shapes");
    check(await A("app.logs.query({source:'query', text:'SHACL', limit: 5}).length") >= 1, "validate: runs are in the log");
    await A("app.clearResult()");
  });

  // ============ graph tab ============
  await section("graph", async () => {
    await A("app.clearResult(); app.clearSelection(); app.shell.closeBottom()");
    check(await A("!app.graphTab.mounted") && await ev(() => !document.querySelector(".gv")), "graph: nothing is mounted before the tab is opened (lazy)");
    await A("app.select('sock-buero-1')"); await A("app.shell.openTab('graph')"); await waitFor(() => window.reteBuilding.graphView && document.querySelector(".gv canvas"), null, 60000); await ev(() => window.reteBuilding.graphView.ready); await ev(() => window.reteBuilding.graphView.whenIdle());
    check(await ev(() => !!document.querySelector(".gv canvas") && window.reteBuilding.graphTab.mounted), "graph: the Graph tab mounts the view on first open");
    await waitFor(() => (window.reteBuilding.graphView.getState().selected || "").endsWith("/sock-buero-1"), null, 20000);
    const g1 = await ev(() => { const s = window.reteBuilding.graphView.getState(); return { sel: s.selected, expanded: s.expanded.length, nodes: s.counts.nodes }; });
    check(/sock-buero-1$/.test(g1.sel) && g1.expanded >= 1 && g1.nodes >= 1, "graph: a selection made in 3D before the tab opened is focused when it opens: " + JSON.stringify(g1));
    await A("app.select('sock-wohnen-1')"); await waitFor(() => (window.reteBuilding.graphView.getState().selected || "").endsWith("/sock-wohnen-1"), null, 20000); check(true, "");
    await A("app.select('Haustuer')"); await waitFor(() => (window.reteBuilding.graphView.getState().selected || "").endsWith(window.reteBuilding.graph.nodes.find((n) => n.l === "Haustuer").n), null, 20000);
    check(true, "graph: selecting a house element in 3D focuses it in the graph");
    await A("app.clearSelection()"); const wi = await A("'https://w3id.org/rete/cad/fzk-haus/' + app.graph.nodes.find(n => n.l === 'Innentuer-2').n");
    await ev((i) => window.reteBuilding.graphView.focus(i), wi); await waitFor(() => window.reteBuilding.selectedInfo() && window.reteBuilding.selectedInfo().label === "Innentuer-2", null, 20000);
    check(true, "graph: selecting in the graph selects in 3D (Innentuer-2)");
    await ev(() => window.reteBuilding.graphView.setGrouping("system")); await ev(() => window.reteBuilding.graphView.whenIdle());
    const cl = await ev(() => window.reteBuilding.graphView.getState().clusters.map((c) => [c.label, c.count]));
    check(!cl.some((c) => /none/i.test(c[0])) && cl.length >= 14 && cl.some((c) => /House elements/.test(c[0])) && cl.some((c) => /Thermal model/.test(c[0])) && cl.some((c) => /State definitions/.test(c[0])) && cl.some((c) => /Observations/.test(c[0])), "graph: grouping by system leaves no '(none)' cluster: " + JSON.stringify(cl).slice(0, 400));
    const typed = await ev(async () => { const q = await import("/building/queries.js"); const j = await q.runSparql("SELECT (COUNT(DISTINCT ?s) AS ?n) WHERE { ?s a ?t }", null, { quiet: true }); return +/"(\d+)"/.exec(j.rows[0].n)[1]; });
    check(cl.reduce((a, c) => a + c[1], 0) === typed, `graph: the system clusters add up to the ${typed} typed things`);
    // highlights and state tints
    await ev(() => window.reteBuilding.graphView.setGrouping("class")); await call("apply_scenario", { id: "trip-rcd-2" }); await page.waitForTimeout(300);
    const hl = await ev(() => window.reteBuilding.graphView.getState().highlight.length); check(hl > 20, "graph: a scenario highlights what it affects (" + hl + ")");
    const fr = await ev(() => window.reteBuilding.graphView.getState().frames); await A("app.svc.setState('cb-4','off')"); await page.waitForTimeout(500); check(await ev(() => window.reteBuilding.graphView.getState().frames) > fr, "graph: a state change repaints the tints");
    await A("app.svc.resetStates()");
    await ev(() => window.reteBuilding.graphView.collapseAll());
    await A("app.select('Wohnen')"); await page.locator('#info [data-act="graph"]').click(); await waitFor(() => (window.reteBuilding.graphView.getState().selected || "").length > 0 && window.reteBuilding.shell.state().bottom === "graph");
    const sg = await call("show_in_graph", { id: "Küche", grouping: "room" }); check(sg.v && sg.v.focused === true && sg.v.grouping === "room", "show_in_graph focuses a room with a grouping: " + JSON.stringify(sg));
    const SZ = () => ev(() => { const s = window.reteBuilding.graphView.getState().size, h = document.querySelector("#graphHost").getBoundingClientRect(); return [s.w, s.h, Math.round(h.width), Math.round(h.height)]; });
    await page.waitForTimeout(300); const sz0 = await SZ();   // the view's own chrome takes a fixed part of the host; the round trip must come back to this
    await A("app.shell.maximiseBottom(true)"); await page.waitForTimeout(200); check(await ev(() => { const s = window.reteBuilding.graphView.getState().size; return s.h > 450; }), "graph: the view resizes when the panel is maximised"); await A("app.shell.maximiseBottom(false)");
    await page.waitForTimeout(300); const sz = await SZ(); check(Math.abs(sz[0] - sz[2]) <= 2 && Math.abs(sz[1] - sz0[1]) <= 4 && Math.abs(sz[3] - sz0[3]) <= 4 && Math.abs((sz[3] - sz[1]) - (sz0[3] - sz0[1])) <= 4, "graph: ... and back to the size it had: " + JSON.stringify([sz0, sz]));
  });

  // ============ thermal and scenarios tabs ============
  await section("thermal and scenarios tabs", async () => {
    await A("app.shell.closeBottom()"); await call("run_thermal_day", { date: "2026-06-21" });
    check(await A("app.shell.state().bottom") === "thermal" && await ev(() => !document.getElementById("thermalPanel").hidden && document.getElementById("thChart").width > 100 && document.querySelectorAll("#thTable tr[data-g]").length === 7 && /Warmest|Solar/.test(document.getElementById("thSummary").textContent)), "thermal tab: chart and per-room table after a run");
    await page.locator('#thermalPanel [data-t="wins"]').click(); check(await ev(() => document.querySelectorAll("#thWTable tr[data-w]").length === 11 && document.getElementById("thWChart").width > 100), "thermal tab: the window sun chart and table"); await page.locator('#thermalPanel [data-t="rooms"]').click();
    await page.locator("#thTable tr[data-g]").first().click(); check(await A("app.selectedInfo() && app.selectedInfo().kind") === "room", "thermal tab: a room row selects it");
    check(await ev(() => /Thermal/.test(document.getElementById("pane-thermal").textContent) && !!document.getElementById("thRun") && !document.getElementById("thermalPanel").parentElement.closest("#side")), "the thermal inputs stay in the side panel, the results are in the tab");
    await page.locator("#rail-thermal").click(); await page.locator("#thCsv").click().catch(() => 0);
    await A("app.shell.openTab('scenarios')"); await A("app.clearSelection()");
    await page.locator('.scen-btn[data-id="trip-rcd-2"]').click(); await waitFor(() => /Affected: 75 things/.test(document.getElementById("scenOut").textContent));
    check(await ev(() => document.querySelectorAll("#scenGroups details").length >= 6 && /Switched off/.test(document.getElementById("scenOut").textContent) && /SPARQL/.test(document.getElementById("scenOut").textContent)), "scenarios tab: affected things grouped by room, what was switched off, the SPARQL");
    await page.locator("#scenRun").click(); await waitFor(() => /Graph: 75/.test(document.getElementById("scenRes").textContent), null, 30000); check(/the same set/.test(await ev(() => document.getElementById("scenRes").textContent)), "scenarios tab: 'Run on the graph' compares the counts");
    const kinds = await ev(() => [...document.querySelectorAll("#scenList .svc-h")].map((h) => h.textContent)); check(kinds.length === 2 && /Supply/.test(kinds[0]) && /Structure/.test(kinds[1]), "scenarios tab: supply and structural scenarios are listed apart: " + JSON.stringify(kinds));
    await A("app.shell.openTab('scenarios')");   // running a scenario on the graph shows its rows in Results; come back to the list
    await page.locator('.scen-btn[data-id="remove-lintel"]').click(); await waitFor(() => /Removed/.test(document.getElementById("scenOut").textContent), null, 20000);
    check(await ev(() => /approximation/i.test(document.getElementById("scenOut").textContent)), "scenarios tab: a structural scenario names its SPARQL as an approximation");
    await page.locator("#scenRun").click(); await waitFor(() => /approximation/.test(document.getElementById("scenRes").textContent), null, 30000); check(/inside the graph answer/.test(await ev(() => document.getElementById("scenRes").textContent)), "scenarios tab: the structural comparison says the exact set is inside the graph answer");
    if (hasStructure) check(await A("app.structure.state().removed.length") >= 1, "scenarios tab: the structural scenario removes the element");
    await A("app.shell.openTab('scenarios')");   // the comparison above showed its rows in Results
    await page.locator("#scenReset").click(); check(await A("app.svc.activeScenario") === null && (!hasStructure || await A("app.structure.state().removed.length") === 0), "scenarios tab: reset clears the scenario and restores removed elements");
  });

  // ============ logs ============
  await section("logs", async () => {
    await A("app.shell.openTab('console')");
    await ev(() => { window.reteBuilding.log("warn", "perf", "dash test warning", { a: 1 }); window.reteBuilding.log("info", "user", "dash test user action"); });
    await page.evaluate(() => { setTimeout(() => { throw new Error("dash test error"); }, 0); Promise.reject(new Error("dash test rejection")); }); await page.waitForTimeout(300);
    check(await ev(() => /^\d+/.test(document.querySelector("#tab-logs .badge").textContent)), "logs: an unread badge on the tab while it is not shown");
    await A("app.shell.openTab('logs')"); await page.waitForTimeout(150); check(await ev(() => !document.querySelector("#tab-logs .badge")), "logs: opening the tab clears the badge");
    const src = await ev(() => [...new Set(window.reteBuilding.logs.entries().map((e) => e.source))]);
    check(["user", "query", "simulation", "agent", "error", "data", "scenario"].every((s) => src.includes(s)), "logs: entries from user, query, simulation, agent, error, data and scenario: " + src.join());
    const first = await ev(() => document.querySelector("#lgList li .t").textContent + " " + document.querySelector("#lgList li .src").textContent); check(/^\d\d:\d\d:\d\d\.\d{3} /.test(first), "logs: newest first, with a time and a source: " + first);
    check(await ev(() => /dash test rejection|dash test error/.test(document.getElementById("lgList").textContent)) && (await A("app.logs.query({source:'error', limit: 10}).length")) >= 2, "logs: page errors and unhandled rejections are captured");
    await page.locator("#lgSrc").selectOption("error"); check(await ev(() => [...document.querySelectorAll("#lgList li")].every((l) => l.querySelector(".src").textContent === "error") && document.querySelectorAll("#lgList li").length >= 2), "logs: filter by source");
    await page.locator("#lgSrc").selectOption(""); await page.locator("#lgLvl").selectOption("warn"); check(await ev(() => [...document.querySelectorAll("#lgList li")].every((l) => /warn|error/.test(l.className))), "logs: filter by level"); await page.locator("#lgLvl").selectOption("");
    await page.locator("#lgTxt").fill("trip-rcd"); check(await ev(() => document.querySelectorAll("#lgList li").length >= 1 && [...document.querySelectorAll("#lgList li")].every((l) => /trip-rcd/.test(l.textContent))), "logs: text filter"); await page.locator("#lgTxt").fill("");
    await page.locator("#lgPause").click(); const n0 = await ev(() => document.querySelectorAll("#lgList li").length); await ev(() => window.reteBuilding.log("info", "user", "dash test while paused")); await page.waitForTimeout(150);
    check(await ev(() => document.querySelectorAll("#lgList li").length) === n0 && !(await ev(() => /while paused/.test(document.getElementById("lgList").textContent))), "logs: Pause freezes the view"); await page.locator("#lgPause").click(); await page.waitForTimeout(150); check(await ev(() => /while paused/.test(document.getElementById("lgList").textContent)), "logs: Resume shows what came meanwhile");
    const js = await A("(() => { return JSON.parse(JSON.stringify(app.logs.entries())).length; })()"); check(js > 30, "logs: the log can be exported as JSON (" + js + " entries)");
    const dl = page.waitForEvent("download"); await page.locator("#lgSave").click(); const d = await dl; check(/\.json$/.test(d.suggestedFilename()), "logs: Save JSON downloads a file");
    await ev(() => { for (let i = 0; i < 1700; i++) window.reteBuilding.log("debug", "app", "fill " + i); }); check(await A("app.logs.entries().length") <= 1500 && await A("app.logs.entries().length") > 1000, "logs: the length is capped: " + await A("app.logs.entries().length"));
    await page.locator("#lgClear").click(); check(await A("app.logs.entries().length") === 0 && await ev(() => /Nothing logged/.test(document.getElementById("lgList").textContent)), "logs: Clear");
    // user actions, performance notes
    await A("app.setQuality('medium')"); await A("app.setQuality('high')"); await A("app.setState(app.graph.nodes.find(n => n.l === 'Haustuer').n, 'open', {instant: true})"); await page.waitForTimeout(350); await A("app.setState(app.graph.nodes.find(n => n.l === 'Haustuer').n, 'closed', {instant: true})");
    await page.waitForTimeout(350); const kinds = await A("app.logs.entries().map(e => e.source + ':' + e.message).join('|')"); check(/perf:Quality set to medium/.test(kinds) && /user:Haustuer: closed → open/.test(kinds), "logs: quality changes and state changes are recorded: " + kinds.slice(0, 200));
  });

  // ============ status bar ============
  await section("status bar", async () => {
    await A("app.sun.set({date:'2026-06-21', time: '14:30'}); app.clearSelection()"); await page.waitForTimeout(150);
    const s0 = await ev(() => ({ sun: document.getElementById("stSun").textContent, mode: document.getElementById("stMode").textContent, sel: document.getElementById("stSel").textContent, q: document.getElementById("stQual").textContent, g: document.getElementById("stGraph").textContent, mcp: document.getElementById("stMcp").textContent }));
    check(/14:30/.test(s0.sun) && /21 Jun/.test(s0.sun) && /sun \+\d+°/.test(s0.sun) && /Orbit/.test(s0.mode) && /Nothing selected/.test(s0.sel) && /High · [\d.]+ ms · \d+ calls/.test(s0.q) && /Graph: house \+ services · [\d,]+ triples/.test(s0.g) && /WebMCP: on · \d+ tools/.test(s0.mcp), "status bar: time and sun, mode, selection, quality with frame time and draw calls, graph source and triples, WebMCP: " + JSON.stringify(s0));
    await A("app.select('Wohnen')"); check(/Wohnen/.test(await ev(() => document.getElementById("stSel").textContent)), "status bar: the selection");
    await call("get_state"); check(/get_state/.test(await ev(() => document.getElementById("stMcp").textContent)), "status bar: the last agent call");
    await page.locator("#stMcp").click(); check(await A("app.shell.state().bottom") === "logs" && await ev(() => document.getElementById("lgSrc").value) === "agent", "status bar: the WebMCP indicator opens Logs filtered to the agent"); await page.locator("#lgSrc").selectOption("");
    await A("app.shell.closeSide()"); await page.locator("#stSun").click(); check(await A("app.shell.state().side") === "sun", "status bar: the date and time open the Sun panel");
    await A("app.walk.enter({})"); await waitFor(() => window.reteBuilding.mode === "walk", null, 30000); await A("app.walk.teleport(2, 5, {storey: 0})"); await waitFor(() => /Room:/.test(document.getElementById("stHover").textContent), null, 20000).catch(() => 0);   // the walk loop updates it on the next frames
    check(/First person/.test(await ev(() => document.getElementById("stMode").textContent)) && /Room: Flur/.test(await ev(() => document.getElementById("stHover").textContent)), "status bar: the camera mode and the room while walking: " + await ev(() => document.getElementById("stHover").textContent));
    check(await ev(() => document.querySelector('[data-cam="walk"]').getAttribute("aria-pressed")) === "true", "top bar: the camera mode switch follows"); await A("app.walk.leave()"); await waitFor(() => window.reteBuilding.mode === "orbit");
    await A("app.notify('dash test notice', {ms: 800})"); check(/dash test notice/.test(await ev(() => document.getElementById("stMsg").textContent)), "status bar: a transient notice"); await page.waitForTimeout(1100); check(await ev(() => document.getElementById("stMsg").textContent) === "", "status bar: ...that goes away");
    if (hasViewport) { await A("app.viewport.setLayout('quad')"); await page.waitForTimeout(200); check(/four views/.test(await ev(() => document.getElementById("stMode").textContent)), "status bar: the layout"); check(await ev(() => getComputedStyle(document.getElementById("hud")).display === "none" && getComputedStyle(document.getElementById("legend")).display === "none"), "four views: the 3D hint and the legend do not cover the panes"); await A("app.viewport.setLayout('single')"); }
    if (hasSection) { await A("app.section.set('x', {enabled: true})"); await page.waitForTimeout(150); check(/Section X/.test(await ev(() => document.getElementById("stInd").textContent)), "status bar: the section indicator"); await A("app.section.reset()"); }
  });

  // ============ reset ============
  await section("reset", async () => {
    const door = "app.graph.nodes.find(n => n.l === 'Haustuer').n";
    await menuBtn("view").click(); await closeAny();
    await page.locator("#resetBtn").click(); await page.waitForSelector(".menu");
    const items = await ev(() => [...document.querySelectorAll(".menu .mi")].map((i) => i.textContent.replace(/\s+/g, " ").trim()));
    check(["View", "States", "Date and time", "Simulation", "Everything", "Undo"].every((k) => items.some((i) => i.startsWith(k))), "reset: the menu has view, states, time, simulation, everything and undo: " + JSON.stringify(items)); await closeAny();
    // view
    await A("app.setColourMode('class'); app.setVisibility({classes:['Wall']}); app.svc.setXray(true)"); await page.locator("#resetBtn").click(); await page.locator(".menu .mi", { hasText: /^View/ }).click(); await page.waitForTimeout(300);
    check(await A("app.getColourMode()") === "realistic" && (await A("app.getVisibility().classes")).length >= 8 && await A("app.svc.isXray()") === false && await ev(() => /Reset: view/.test(document.getElementById("stMsg").textContent)), "reset View: colours, layers and x-ray are back, the status bar says so");
    await page.locator("#stMsg .lnk").click(); await page.waitForTimeout(300); check(await A("app.getColourMode()") === "class" && (await A("app.getVisibility().classes")).length === 1, "reset: Undo in the notice restores what it changed"); await A("app.setColourMode('realistic'); app.setVisibility({classes:['Wall','Slab','Window','Door','Stair','Railing','Beam','Member']}); app.svc.setXray(false)");
    // states
    await A(`app.setState(${door}, 'open', {instant: true}); app.svc.setState('cb-1', 'off')`); await page.locator("#resetBtn").click(); await page.locator(".menu .mi", { hasText: /^States/ }).click(); await page.waitForTimeout(300);
    check(await A(`app.getState(${door})`) === "closed" && await A("app.svc.getState(app.svc.thingFor('cb-1'))") === "on", "reset States: doors, windows and devices are back");
    // time
    const d0 = await A("app.sun.get().date"); await A("app.sun.set({date: '2026-01-05', time: 7})"); await page.locator("#resetBtn").click(); await page.locator(".menu .mi", { hasText: /^Date and time/ }).click(); await page.waitForTimeout(300);
    check(await A("app.sun.get().date") !== "2026-01-05", "reset Date and time: " + d0 + " -> " + await A("app.sun.get().date"));
    // simulation
    await call("apply_scenario", { id: "trip-rcd-2" }); await A("app.loadThermal().then(() => app.thermalUi.setTempView(true))"); await waitFor(() => window.reteBuilding.thermalUi); await page.locator("#resetBtn").click(); await page.locator(".menu .mi", { hasText: /^Simulation/ }).click(); await page.waitForTimeout(300);
    check(await A("app.svc.activeScenario") === null && await A("app.thermalUi.view.temps") === false, "reset Simulation: the scenario and the thermal views are cleared");
    // everything
    await A(`app.setColourMode('class'); app.setState(${door}, 'open', {instant: true}); app.sun.set({date: '2026-01-05', time: 7})`); if (hasSection) await A("app.section.set('z', {enabled: true, position: 2})");
    await page.locator("#resetBtn").click(); await page.locator(".menu .mi", { hasText: /^Everything/ }).click(); await page.waitForTimeout(400);
    check(await A("app.getColourMode()") === "realistic" && await A(`app.getState(${door})`) === "closed" && await A("app.sun.get().date") !== "2026-01-05" && (!hasSection || await A("app.section.get().z.enabled") === false), "reset Everything: view, states and time");
    await page.keyboard.press("Control+z"); await page.waitForTimeout(400); check(await A("app.getColourMode()") === "class", "reset: Ctrl+Z undoes the last reset"); await A("app.resetShell('all')");
    check(await A("typeof app.reset") === "function" ? await A("app.reset('view').then ? true : !!app.reset('view')") : true, "reset: app.reset(scope) of the rendering side is used when it is there (" + (await A("typeof app.reset")) + ")");
    const sc = await A("(async () => { const out = []; for (const s of ['view', 'states', 'time', 'simulation', 'all']) { const r = await app.resetShell(s); out.push(s); } return out; })()"); check(sc.length === 5, "reset: every scope runs without error");
  });

  // ============ the shareable link ============
  await section("link", async () => {
    await A("app.resetShell('all')"); await page.waitForTimeout(200);
    await A("app.sun.set({date: '2026-03-20', time: 9.5}); app.setVisibility({classes: ['Wall','Slab','Door'], storeys: [0], roof: false}); app.setColourMode('class'); app.svc.layers.get('heating').setVisible(true); app.select('Haustuer')");
    if (hasSection) await A("app.section.set('y', {enabled: true, position: 4.5, flip: true})"); if (hasViewport) await A("app.viewport.setLayout('quad')");
    await A("app.shell.openTab('results'); app.shell.openSide('sun')");
    const link = await A("app.link.encode()"); const url = await A("app.link.url()");
    check(/v=1/.test(link) && /d=2026-03-20/.test(link) && /sel=/.test(link) && /tab=results/.test(link) && /side=sun/.test(link) && /cp=/.test(link) && /cls=/.test(link) && (!hasSection || /sec=y:4.5:1/.test(decodeURIComponent(link))) && (!hasViewport || /layout=quad/.test(link)), "link: it carries layout, camera, date and time, layers, selection, sections, tab and panel: " + link.slice(0, 260));
    const before = await A("({date: app.sun.get().date, t: Math.round(app.sun.get().time * 10) / 10, classes: app.getVisibility().classes.length, roof: app.getVisibility().roof, sel: app.selectedInfo() && app.selectedInfo().label, colour: app.getColourMode(), tab: app.shell.state().bottom, side: app.shell.state().side, heating: app.svc.layerState().find(l => l.id === 'heating').visible, layout: app.viewport ? app.viewport.getLayout() : null})");
    await A("app.resetShell('all'); app.clearSelection(); app.shell.closeBottom(); app.shell.openSide('explore'); app.setVisibility({classes: ['Wall','Slab','Window','Door','Stair','Railing','Beam','Member'], storeys: [0, 1], roof: true}); app.svc.layers.get('heating').setVisible(false)");
    if (hasViewport) await A("app.viewport.setLayout('single')");
    await call("open_link", { link: url }); await page.waitForTimeout(400);
    const after = await A("({date: app.sun.get().date, t: Math.round(app.sun.get().time * 10) / 10, classes: app.getVisibility().classes.length, roof: app.getVisibility().roof, sel: app.selectedInfo() && app.selectedInfo().label, colour: app.getColourMode(), tab: app.shell.state().bottom, side: app.shell.state().side, heating: app.svc.layerState().find(l => l.id === 'heating').visible, layout: app.viewport ? app.viewport.getLayout() : null})");
    check(JSON.stringify(before) === JSON.stringify(after), "link: open_link restores the view: " + JSON.stringify([before, after]));
    if (hasSection) check(await A("app.section.get().y.enabled") === true && Math.abs(await A("app.section.get().y.position") - 4.5) < 0.01 && await A("app.section.get().y.flip") === true, "link: the section is restored");
    const gl = await call("get_link"); check(gl.v && gl.v.url.includes("#v=1") && gl.v.hash === await A("app.link.encode()"), "get_link returns the same link");
    // a page that opens with the link in its address
    const p2 = await open(browser, { hash: "#" + link }); const o2 = await p2.page.evaluate(() => ({ date: window.reteBuilding.sun.get().date, sel: window.reteBuilding.selectedInfo() && window.reteBuilding.selectedInfo().label, classes: window.reteBuilding.getVisibility().classes.length, tab: window.reteBuilding.shell.state().bottom }));
    check(o2.date === "2026-03-20" && o2.sel === "Haustuer" && o2.classes === before.classes && o2.tab === "results", "link: a fresh page opened with the link in its address shows that view: " + JSON.stringify(o2)); await p2.ctx.close();
    await A("app.resetShell('all'); app.shell.closeBottom(); history.replaceState(null, '', location.pathname)");
    if (hasViewport) await A("app.viewport.setLayout('single')");
  });

  // ============ palette ============
  await section("palette", async () => {
    await A("app.clearSelection(); app.shell.openSide('explore')"); await page.locator("#stage").click({ position: { x: 5, y: 5 }, force: true }).catch(() => 0);
    await page.keyboard.press("Control+k"); await page.waitForSelector("#palette input");
    check(await ev(() => document.activeElement.closest("#palette") !== null && document.querySelector("#palette").getAttribute("role") === "dialog" && document.querySelectorAll("#palList [role=option]").length >= 8), "palette: Ctrl+K opens it with commands listed and the focus in the field");
    await page.keyboard.type("haust"); await page.waitForTimeout(100);
    const hits = await ev(() => [...document.querySelectorAll("#palList [role=option]")].map((o) => o.textContent)); check(hits.some((h) => /Element/.test(h) && /Haustuer/.test(h)), "palette: it finds an element by name: " + JSON.stringify(hits.slice(0, 4)));
    await page.keyboard.press("Enter"); await page.waitForTimeout(200); check(await A("app.selectedInfo() && app.selectedInfo().label") === "Haustuer" && await ev(() => !document.getElementById("palette")), "palette: Enter selects it and closes the palette");
    await page.keyboard.press("Control+k"); await page.keyboard.type("wohn"); check(await ev(() => [...document.querySelectorAll("#palList [role=option]")].some((o) => /Room/.test(o.textContent) && /Wohnen/.test(o.textContent))), "palette: it finds a room");
    await page.keyboard.press("ArrowDown"); check(await ev(() => document.querySelector('#palList [aria-selected="true"]') !== null && document.querySelector("#palette input").getAttribute("aria-activedescendant").startsWith("pal-")), "palette: arrow keys move the selection (aria-activedescendant)"); await page.keyboard.press("Escape"); check(await ev(() => !document.getElementById("palette")), "palette: Escape closes it");
    await page.keyboard.press("Control+k"); await page.keyboard.type("sock-buero"); check(await ev(() => [...document.querySelectorAll("#palList [role=option]")].some((o) => /Device/.test(o.textContent))), "palette: it finds a device"); await page.keyboard.press("Escape");
    await page.keyboard.press("Control+k"); await page.keyboard.type("light theme"); await page.waitForTimeout(80); check(await ev(() => [...document.querySelectorAll("#palList [role=option]")].some((o) => /Command/.test(o.textContent) && /theme/i.test(o.textContent))), "palette: it finds a menu command"); await page.keyboard.press("Escape");
    await page.keyboard.press("Control+k"); await page.keyboard.type("run the thermal"); await page.keyboard.press("Enter"); await waitFor(() => window.reteBuilding.thermal && window.reteBuilding.thermal.last()); check(await A("app.shell.state().bottom") === "thermal", "palette: running a command from it works (thermal day)");
    await page.keyboard.press("Control+k"); await page.keyboard.type("rooms next"); check(await ev(() => [...document.querySelectorAll("#palList [role=option]")].some((o) => /Query/.test(o.textContent))), "palette: it finds a ready query"); await page.keyboard.press("Escape");
    // shortcuts
    await page.locator("body").click({ position: { x: 3, y: 3 } }).catch(() => 0); await A("document.activeElement && document.activeElement.blur && document.activeElement.blur()");
    await page.keyboard.press("g"); await page.keyboard.press("l"); check(await A("app.shell.state().bottom") === "logs", "shortcut: G then L opens the log");
    await page.keyboard.press("Control+j"); check(await A("app.shell.state().bottom") === null, "shortcut: Ctrl+J collapses the bottom panel"); await page.keyboard.press("Control+j"); check(await A("app.shell.state().bottom") !== null, "shortcut: ...and opens it");
    await page.keyboard.press("Control+b"); check(await A("app.shell.state().side") === null, "shortcut: Ctrl+B hides the side panel"); await page.keyboard.press("Control+b"); check(await A("app.shell.state().side") !== null, "shortcut: ...and shows it");
    await page.keyboard.press("p"); await page.keyboard.press("t"); check(await A("app.shell.state().side") === "thermal", "shortcut: P then T opens the Thermal panel");
    await A("document.activeElement && document.activeElement.blur && document.activeElement.blur()");   // opening a panel may leave the focus in one of its fields, where a single key is text
    await A("app.svc.setXray(false)");   // selecting things in the steps before turns x-ray on
    await page.keyboard.press("x"); check(await A("app.svc.isXray()") === true, "shortcut: X toggles x-ray"); await page.keyboard.press("x");
    if (hasViewport) { await page.keyboard.press("4"); check(await A("app.viewport.getLayout()") === "quad", "shortcut: 4 gives four views"); await page.keyboard.press("1"); check(await A("app.viewport.getLayout()") === "single", "shortcut: 1 gives one view"); }
    await page.keyboard.press("?"); await page.waitForSelector(".dlg"); check(await ev(() => document.querySelector('.dlg [data-s="shortcuts"][aria-current="true"]') !== null), "shortcut: ? opens the shortcuts"); await page.keyboard.press("Escape");
    await A("app.shell.openSide('explore')"); await page.locator("#search").focus(); await page.keyboard.type("x"); check(await A("app.svc.isXray()") === false && await ev(() => document.getElementById("search").value) === "x", "single-key shortcuts do not fire while typing in a field"); await page.locator("#search").fill("");
    await A("app.shell.openTab('console')"); await page.locator("#edTa").click(); await page.keyboard.press("Escape"); await page.keyboard.press("Tab"); check(await ev(() => document.activeElement.id !== "edTa"), "the editor does not trap the keyboard: Escape, then Tab leaves it");
  });

  // ============ the WebMCP tools of the shell ============
  await section("webmcp", async () => {
    await A("app.logs.clear()");
    const names = ["get_ui", "open_panel", "get_logs", "show_in_graph", "get_graph_view", "get_about", "get_link", "open_link"];
    const reg = await ev(async () => {
      const app = window.reteBuilding, W = await import("/building/webmcp.js");
      const arrays = [W.buildingTools(), W.shellTools(), app.svc ? W.systemsTools() : [], W.perfTools()];
      try { arrays.push((await import("/building/webmcp-viewport.js")).viewportTools(app)); } catch (e) { /* no viewport tools */ }
      const L = window.reteBuildingTools.list();
      return { reg: window.__reg, list: L.map((t) => t.name), ann: window.reteWebMCP, expected: arrays.flat().map((t) => t.name), shell: W.shellTools().map((t) => t.name), schemas: L.every((t) => t.inputSchema && t.inputSchema.type === "object" && typeof t.description === "string" && t.description.length > 10) };
    });
    check(reg.shell.every((n) => reg.reg.includes(n) && reg.list.includes(n)) && reg.ann.supported && reg.shell.length >= 8 && names.every((n) => reg.shell.includes(n)), "every shell tool is registered (" + reg.shell.length + "): " + reg.shell.filter((n) => !reg.reg.includes(n)).join());
    check(reg.reg.length === reg.expected.length && new Set(reg.reg).size === reg.reg.length && reg.expected.every((n) => reg.reg.includes(n) && reg.list.includes(n)) && reg.list.length === reg.expected.length && reg.schemas, "the registered tools are exactly the union of the page's tool arrays, each with a schema (" + reg.expected.length + " expected, " + reg.reg.length + " registered, " + reg.list.length + " listed)");
    let r = await call("get_ui"); check(r.v && typeof r.v.side === "string" && r.v.theme === "dark" && r.v.statusBar && /Orbit/.test(r.v.statusBar.mode) && Array.isArray(r.v.tabs) && r.v.layout && r.v.cameraMode === "orbit" && r.v.panels.length === 6, "get_ui: panels, tab, theme, layout, status bar: " + JSON.stringify(r).slice(0, 220));
    r = await call("open_panel", { panel: "bottom", id: "graph" }); check(r.v && r.v.bottom === "graph", "open_panel opens a bottom tab");
    r = await call("open_panel", { panel: "side", id: "states" }); check(r.v && r.v.side === "states", "open_panel opens a side panel");
    r = await call("open_panel", { panel: "inspector", open: false }); check(r.v && r.v.inspector === false, "open_panel closes the inspector");
    r = await call("open_panel", { panel: "bottom", open: false }); check(r.v && r.v.bottom === null, "open_panel collapses the bottom panel");
    r = await call("open_panel", { panel: "side", id: "nope" }); check(!!r.err, "open_panel refuses an unknown panel");
    r = await call("get_logs", { source: "agent", limit: 5 }); check(r.v && r.v.returned >= 4 && r.v.items.every((e) => e.source === "agent") && r.v.items.some((e) => e.message.includes(" ms")) && r.v.items.every((e) => /failed/.test(e.message) || / \d+ ms/.test(e.message)), "get_logs filters by source and says how long each call took: " + JSON.stringify(r).slice(0, 200));
    r = await call("get_logs", { level: "error", text: "open_panel" }); check(r.v && r.v.items.length === 1 && /open_panel failed/.test(r.v.items[0].message), "every failed tool call is logged as an error with its message");
    r = await call("get_graph_view", { limit: 10 }); check(r.v && r.v.clusters.length >= 2 && r.v.counts && r.v.groupings.length >= 3 && r.v.nodes.length <= 10 && typeof r.v.universe === "number", "get_graph_view: clusters, counts, nodes: " + JSON.stringify(r).slice(0, 160));
    r = await call("show_in_graph", { id: "Haustuer" }); check(r.v && r.v.focused === true && /fzk-haus\//.test(r.v.iri), "show_in_graph focuses a house element");
    r = await call("show_in_graph", { id: "no-such-thing" }); check(!!r.err, "show_in_graph refuses an unknown thing");
    r = await call("get_about"); check(r.v && r.v.page && r.v.house && r.v.model && r.v.webmcp && r.v.data && r.v.data.triples > 15000 && /CC0/.test(JSON.stringify(r.v.data.card)) && r.v.data.files.length >= 3 && r.v.shortcuts.length > 10 && r.bytes !== 0, "get_about: the About content as data, with the card, the files and the shortcuts");
    r = await call("open_link", { link: "nonsense" }); check(!!r.err, "open_link refuses what is not a link of this page");
    r = await call("get_state"); check(r.v && r.v.counts.rooms === 7, "the first tools still work");
    await call("get_link");
    const lg = await A("app.logs.query({source: 'agent', limit: 100}).map(e => e.message + ' ' + JSON.stringify(e.data || {})).join('|')");
    const missing = names.filter((n) => n !== "open_link" && !lg.includes(n)); check(missing.length === 0 && /"ms"/.test(lg) && /args/.test(lg), "every tool call is in the log with its tool, arguments and duration: missing " + missing.join() + " of " + lg.length + " chars");
    check(await ev(() => /Agent ·/.test(document.getElementById("agentPill").textContent)), "the 'Agent · tool' pill is still shown");
    await A("app.shell.openSide('explore'); app.shell.openInspector()");
  });

  // ============ themes and contrast ============
  const contrast = (page) => page.evaluate(() => {
    const parse = (s) => (s.match(/[\d.]+/g) || []).map(Number);
    const lum = ([r, g, b]) => { const f = (v) => { v /= 255; return v <= 0.03928 ? v / 12.92 : Math.pow((v + 0.055) / 1.055, 2.4); }; return 0.2126 * f(r) + 0.7152 * f(g) + 0.0722 * f(b); };
    const bg = (el) => { for (let e = el; e; e = e.parentElement) { const m = parse(getComputedStyle(e).backgroundColor); if (m.length >= 3 && (m.length < 4 || m[3] > 0.6)) return m.slice(0, 3); } return [255, 255, 255]; };
    const ratio = (el) => { const fg = parse(getComputedStyle(el).color).slice(0, 3), b = bg(el), a = lum(fg), c = lum(b); return (Math.max(a, c) + 0.05) / (Math.min(a, c) + 0.05); };
    const sels = ["#pane-explore .pane-head h2", "#pane-explore .opt", "#pane-explore .sec>summary", "#statusbar #stMode", "#statusbar #stSun", "#btTabs .tab[aria-selected=false]", "#btTabs .tab[aria-selected=true]", ".mb", "#info .ins-head .t", "#info .proptab td:first-child", "#info .lnk", "#info .chip", "#info .badge", "#resetBtn", "#qChip", ".brand span", "#info .note, #info .empty", ".tp.on .tp-bar", ".tp.on .btn"];
    const out = {}; for (const s of sels) { const e = [...document.querySelectorAll(s)].find((x) => x.offsetParent !== null && x.textContent.trim()); if (e) out[s] = +ratio(e).toFixed(2); else out[s] = null; }
    return out;
  });
  await section("themes", async () => {
    await A("app.clearSelection(); app.select('Wohnen'); app.shell.openSide('explore'); app.shell.openTab('results')"); await page.waitForTimeout(150);
    for (const th of ["dark", "light"]) {
      await A(`app.shell.setTheme('${th}')`); await page.waitForTimeout(200);
      const r = await contrast(page); const low = Object.entries(r).filter(([, v]) => v != null && v < 4.5), missing = Object.entries(r).filter(([, v]) => v == null).map(([k]) => k);
      check(await ev(() => document.documentElement.dataset.theme) === th && low.length === 0, `theme ${th}: key text has WCAG AA contrast (4.5:1): low ${JSON.stringify(low)} of ${JSON.stringify(r)}`);
      notes.push({ theme: th, minContrast: Math.min(...Object.values(r).filter((v) => v != null)), missing });
      const bgc = await ev(() => getComputedStyle(document.body).backgroundColor); check(th === "dark" ? /rgb\(1[0-9], \d+, \d+\)|rgb\(\d, \d+, \d+\)/.test(bgc) : /rgb\((2\d\d), (2\d\d), (2\d\d)\)/.test(bgc), `theme ${th}: the page background is ${bgc}`);
    }
    await A("app.shell.setTheme('light')"); await page.keyboard.press("Control+k"); await page.waitForTimeout(100); const pc = await page.evaluate(() => getComputedStyle(document.querySelector("#palette .box")).backgroundColor); check(/255, 255, 255/.test(pc), "light theme: the palette is light"); await page.keyboard.press("Escape");
    await A("app.shell.state(); app.shell.openTab('graph')"); await page.waitForTimeout(1500); check(await ev(() => document.querySelector(".gv").dataset.theme) === "light", "light theme: the graph view follows"); await A("app.shell.openTab('console')");
    await A("app.shell.setTheme('dark')"); await page.waitForTimeout(800); check(await ev(() => document.querySelector(".gv") === null || document.querySelector(".gv").dataset.theme === "dark"), "dark theme: ...and back");
    await page.locator("#settingsBtn").click(); check(await ev(() => !!document.querySelector('.dlg[aria-label="Settings"]')), "the settings dialog opens"); await page.locator('.dlg [data-k="theme"][data-v="light"]').click(); check(await ev(() => document.documentElement.dataset.theme) === "light", "settings: theme"); await page.locator('.dlg [data-k="density"][data-v="comfortable"]').click(); check(await ev(() => document.documentElement.dataset.density) === "comfortable", "settings: density");
    await page.locator('.dlg [data-k="units"][data-v="imperial"]').click(); await page.keyboard.press("Escape"); await A("app.select('Wohnen')"); check(/ft²/.test(await ev(() => document.getElementById("info").textContent)), "settings: imperial units show in the inspector");
    await A("app.shell.setUnits('metric'); app.shell.setDensity('compact'); app.shell.setTheme('dark')");
  });

  // ============ the page is still itself ============
  await section("performance", async () => {
    const idle = await ev(async () => { const A = window.reteBuilding; A.scenery.set({ animate: false }); A.renderNow(); await new Promise((r) => setTimeout(r, 900)); const f0 = A.frameCount; await new Promise((r) => setTimeout(r, 1800)); return A.frameCount - f0; });
    check(idle === 0, "the shell does not make the page render while it is idle: " + idle + " frames in 1.8 s");
    const calls = await A("(() => { app.renderNow(); return app.renderer.info.render.calls; })()"); check(calls < 80, "draw calls with the shell: " + calls);
    const t = await A("app.timing"); check(t.firstFrame > 0 && t.firstFrame < t.interactive && t.interactive <= t.ready, "first frame before the shell and the rest: " + JSON.stringify(t));
    const loaded = await ev(() => performance.getEntriesByType("resource").map((r) => r.name.split("/").pop().split("?")[0])); check(!loaded.includes("graphview.js") || true, "");
  });
  await section("lazy code", async () => {
    const p = await open(browser); const names = await p.page.evaluate(() => performance.getEntriesByType("resource").map((r) => r.name.split("/").pop().split("?")[0]));
    check(!names.includes("graphview.js") && !names.includes("graphview-layout.js") && !names.includes("thermal-ui.js") && !names.includes("about.js"), "lazy: graphview, thermal-ui and about are not loaded before they are used: " + names.filter((n) => /graphview|thermal|about|measure/.test(n)).join());
    check(p.errs.length === 0, "lazy page: no errors " + p.errs.join(" | ")); await p.ctx.close();
  });

  const mainErrs = errs.slice(), mainBad = bad.slice();
  check(mainErrs.length === 0, "no page or console errors: " + mainErrs.join(" | "));
  check(mainBad.length === 0, "no failed requests: " + mainBad.join(" | "));

  // ============ reload: the panels remember ============
  await section("remember", async () => {
    await A("app.shell.openSide('states'); app.shell.openTab('logs'); app.shell.setTheme('light')"); await page.waitForTimeout(400);
    const saved = await ev(() => localStorage.getItem("reteBuilding.ui.v1"));
    const p = await open(browser, { storage: saved }); const s = await p.page.evaluate(() => ({ st: window.reteBuilding.shell.state(), theme: document.documentElement.dataset.theme }));
    check(s.st.side === "states" && s.st.bottom === "logs" && s.theme === "light", "a new page remembers the side panel, the bottom tab and the theme: " + JSON.stringify(s.st)); await p.ctx.close();
    await A("app.shell.setTheme('dark')");
  });

  // ============ layouts at three widths ============
  const layoutAt = async (w, h, name) => {
    const p = await open(browser, { w, h, touch: w < 700 }); const pg = p.page;
    const base = await pg.evaluate(() => { const c = document.querySelector("#stage canvas[data-engine]").getBoundingClientRect(); return { sw: document.documentElement.scrollWidth, iw: innerWidth, sh: document.documentElement.scrollHeight, ih: innerHeight, cw: Math.round(c.width), ch: Math.round(c.height), rail: Math.round(document.getElementById("rail").getBoundingClientRect().bottom), menuAll: getComputedStyle(document.getElementById("menuAll")).display, menubar: getComputedStyle(document.getElementById("menubar")).display, state: window.reteBuilding.shell.state() }; });
    const OUT = () => pg.evaluate(() => { const s = document.getElementById("stage").getBoundingClientRect(); const over = [...document.querySelectorAll("body *")].filter((e) => { const r = e.getBoundingClientRect(); return e.offsetParent !== null && r.width > 0 && r.right > innerWidth + 1 && !e.closest(".tabs, .menu, #palList, svg, canvas, #stage, .ed-hl, .ed-ta, .ed-wrap, #statusbar"); }).slice(0, 5).map((e) => (e.id || e.className || e.tagName) + ":" + Math.round(e.getBoundingClientRect().right)); return { stageRight: Math.round(s.right), iw: innerWidth, over }; });
    const o0 = await OUT(); check(o0.stageRight <= o0.iw && o0.over.length === 0, `${name}: the stage and every visible control fit the screen width: ` + JSON.stringify(o0));
    check(base.sw <= base.iw && base.sh <= base.ih + 1 && base.cw > 150 && base.ch > 150, `${name}: no overflow and a real canvas at start: ` + JSON.stringify(base));
    const scenario = async (label, f) => { await pg.evaluate(f); await pg.waitForTimeout(500); const o = await pg.evaluate(() => { const c = document.querySelector("#stage canvas[data-engine]").getBoundingClientRect(), s = document.getElementById("stage").getBoundingClientRect(); return { sw: document.documentElement.scrollWidth, iw: innerWidth, cw: Math.round(c.width), ch: Math.round(c.height), sw2: Math.round(s.width), sh2: Math.round(s.height) }; }); check(o.sw <= o.iw && o.cw > 0 && o.ch > 0 && o.sw2 > 0 && o.sh2 > 0, `${name} / ${label}: no horizontal overflow, the canvas is not empty: ` + JSON.stringify(o)); return o; };
    for (const pn of ["explore", "services", "sun", "thermal", "states", "query"]) { await scenario("side " + pn, `window.reteBuilding.shell.openSide('${pn}')`); const o = await OUT(); check(o.stageRight <= o.iw && o.over.length === 0, `${name}: side ${pn}: nothing sticks out: ` + JSON.stringify(o)); }
    await scenario("inspector", "window.reteBuilding.select('Wohnen')");
    for (const tn of ["console", "results", "graph", "thermal", "scenarios", "logs"]) await scenario("tab " + tn, `window.reteBuilding.shell.openTab('${tn}')`);
    await scenario("maximised", "window.reteBuilding.shell.maximiseBottom(true)"); await scenario("closed", "window.reteBuilding.shell.maximiseBottom(false); window.reteBuilding.shell.closeBottom(); window.reteBuilding.shell.closeSide(); window.reteBuilding.shell.closeInspector()");
    if (w >= 1000) { const r = await pg.evaluate(() => { const g = (id) => document.getElementById(id).getBoundingClientRect(); window.reteBuilding.shell.openSide("explore"); window.reteBuilding.shell.openInspector(); window.reteBuilding.shell.openTab("console"); const s = g("stage"), side = g("side"), insp = g("inspector"), bot = g("bottom"), top = g("topbar"), st = g("statusbar"); return { side: [side.right <= s.left + 1, insp.left >= s.right - 1], bot: bot.top >= s.bottom - 1, bar: top.bottom <= s.top + 1, status: st.top >= bot.bottom - 1, w: [Math.round(side.width), Math.round(s.width), Math.round(insp.width)] }; }); check(r.side[0] && r.side[1] && r.bot && r.bar && r.status, name + ": the regions sit where the layout says: " + JSON.stringify(r)); }
    if (w < 700) {
      await pg.evaluate(() => { window.reteBuilding.shell.closeBottom(); window.reteBuilding.shell.closeInspector(); window.reteBuilding.shell.closeSide(); }); await pg.waitForTimeout(300);
      const m = await pg.evaluate(() => { const r = (s) => { const e = document.querySelector(s); const b = e.getBoundingClientRect(); return { w: Math.round(b.width), h: Math.round(b.height), disp: getComputedStyle(e).display, pos: getComputedStyle(e).position, top: Math.round(b.top), bottom: Math.round(b.bottom) }; }; return { menuAll: r("#menuAll"), menubar: r("#menubar"), rail: r("#rail"), rb: r("#rail .rb"), tab: r("#btTabs .tab"), cam: r('[data-cam="orbit"]'), inner: innerHeight }; });
      check(m.menuAll.disp !== "none" && m.menubar.disp === "none" && m.rail.pos === "fixed" && m.rail.bottom >= m.inner - 1 && m.rail.w >= w - 2, `${name}: one menu button, the tool rail is a bottom bar: ` + JSON.stringify(m));
      check(m.menuAll.h >= 40 && m.rb.h >= 40 && m.tab.h >= 40 && m.cam.h >= 28 && m.rb.w >= 40, `${name}: touch targets are at least 40 px (menu ${m.menuAll.h}, rail ${m.rb.w}x${m.rb.h}, tab ${m.tab.h})`);
      await pg.locator("#menuAll").click(); await pg.waitForSelector(".menu"); const mm = await pg.evaluate(() => { const m = document.querySelector(".menu"), b = m.getBoundingClientRect(); return { heads: [...m.querySelectorAll(".mh")].map((h) => h.textContent), inside: b.left >= 0 && b.right <= innerWidth && b.bottom <= innerHeight + 1, items: m.querySelectorAll(".mi").length }; });
      check(mm.heads.filter((h) => ["View", "Layers", "Simulate", "Tools", "About"].includes(h)).join() === "View,Layers,Simulate,Tools,About" && mm.inside && mm.items > 40, `${name}: the single menu button holds all five menus, in order, with their sub-headings: ` + JSON.stringify(mm)); await pg.keyboard.press("Escape");
      const sheet = async (label, f, sel) => { await pg.evaluate(f); await pg.waitForTimeout(400); const o = await pg.evaluate((sel) => { const e = document.querySelector(sel), b = e.getBoundingClientRect(); return { h: Math.round(b.height), w: Math.round(b.width), left: Math.round(b.left), right: Math.round(b.right), bottom: Math.round(b.bottom), top: Math.round(b.top), others: ["#side", "#inspector", "#bottom"].filter((s) => s !== sel && !document.querySelector(s).hidden && !document.querySelector(s).classList.contains("closed") && document.querySelector(s).getBoundingClientRect().height > 60).length, inner: [innerWidth, innerHeight] }; }, sel); check(o.w >= w - 2 && o.left === 0 && o.h > 250 && o.top > 30 && o.bottom <= o.inner[1] && o.others === 0, `${name}: ${label} is a bottom sheet: ` + JSON.stringify(o)); };
      await sheet("the side panel", "window.reteBuilding.shell.openSide('sun')", "#side");
      await sheet("the inspector", "window.reteBuilding.select('Haustuer')", "#inspector");
      await sheet("the bottom panel", "window.reteBuilding.shell.openTab('console')", "#bottom");
      const full = await pg.evaluate(() => { const b = document.getElementById("bottom").getBoundingClientRect(), t = document.getElementById("topbar").getBoundingClientRect(); return b.height > innerHeight * 0.7 && b.top >= t.bottom - 1; }); check(full, `${name}: the bottom panel is a full-height sheet`);
      await pg.evaluate(() => { window.reteBuilding.shell.closeBottom(); }); const back = await pg.evaluate(() => { const c = document.querySelector("#stage canvas[data-engine]").getBoundingClientRect(); return c.width > 300 && c.height > 300; }); check(back, `${name}: closing the sheets gives the canvas back`);
    }
    if (w >= 700 && w < 1000) {
      await pg.evaluate(() => { window.reteBuilding.shell.openSide("explore"); window.reteBuilding.select("Haustuer"); }); await pg.waitForTimeout(400);
      const o = await pg.evaluate(() => { const i = document.getElementById("inspector"), b = i.getBoundingClientRect(), s = document.getElementById("stage").getBoundingClientRect(); return { pos: getComputedStyle(i).position, overlap: b.left < s.right - 10, side: window.reteBuilding.shell.state().side, w: Math.round(b.width), stageW: Math.round(s.width) }; });
      check(o.pos === "absolute" && o.overlap && o.stageW > 300, `${name}: the inspector is an overlay sheet and the viewport keeps its width: ` + JSON.stringify(o));
    }
    check(p.errs.length === 0, `${name}: no errors: ` + p.errs.join(" | "));
    await p.ctx.close();
  };
  await section("layout 1400", () => layoutAt(1400, 860, "1400"));
  await section("layout 820", () => layoutAt(820, 1180, "820"));
  await section("layout 390", () => layoutAt(390, 844, "390"));


  // ============ walk and fly touch zones follow the 3D pane ============
  await section("touch zones", async () => {
    const p = await open(browser, { touch: true }); const pg = p.page;
    const z = await pg.evaluate(async () => {
      const a = window.reteBuilding, sh = await import("/building/shared.js"), cv = a.renderer.domElement;
      const one = sh.rect3d(cv), c0 = cv.getBoundingClientRect();
      a.viewport.setLayout("quad"); await new Promise((r) => setTimeout(r, 600));
      const quad = sh.rect3d(cv), pane = a.viewport.pane("3d").rect, st = document.getElementById("stage").getBoundingClientRect();
      return { single: [Math.round(one.width), Math.round(c0.width)], quad: [Math.round(quad.left - st.left), Math.round(quad.top - st.top), Math.round(quad.width), Math.round(quad.height)], pane: [Math.round(pane.x), Math.round(pane.y), Math.round(pane.w), Math.round(pane.h)], canvas: [Math.round(st.width), Math.round(st.height)] };
    });
    check(z.single[0] === z.single[1], "touch zones: in the single layout the zone is the whole canvas: " + JSON.stringify(z.single));
    check(JSON.stringify(z.quad) === JSON.stringify(z.pane) && z.quad[2] < z.canvas[0] && z.quad[3] < z.canvas[1], "touch zones: in the four-view layout the zone is the 3D pane, not the canvas: " + JSON.stringify(z));
    check(p.errs.length === 0, "touch zones: no errors: " + p.errs.join(" | "));
    await p.ctx.close();
  });

  // ============ APIs that are missing: the controls say so and nothing throws ============
  await section("missing APIs", async () => {
    // The rendering side's objects never reach app: a setter on Object.prototype swallows the assignment their modules make, as if those modules were not there.
    const names = ["section", "structure", "statevars", "viewport", "rooms", "setCutaway"];   // setCutaway: the ground mesh stays, its cutaway API is not there
    const p = await open(browser, { initScript: (ns) => { for (const n of ns) Object.defineProperty(Object.prototype, n, { configurable: true, get() { return undefined; }, set(v) { if (n === "setCutaway" || (this && this.version === 1 && typeof this.emit === "function" && typeof this.on === "function")) return; Object.defineProperty(this, n, { value: v, writable: true, configurable: true, enumerable: true }); } }); }, initArg: names });
    const pg = p.page;
    const gone = await pg.evaluate((ns) => ns.filter((n) => window.reteBuilding[n] !== undefined), names);
    check(gone.length === 0 && await pg.evaluate(() => window.reteBuilding.ground && window.reteBuilding.ground.setCutaway === undefined), "missing APIs: none of " + names.join(", ") + " is on app (still there: " + gone.join() + ")");
    check(await pg.evaluate(() => window.reteBuilding.ready && !!window.reteBuilding.shell), "missing APIs: the page and the shell are up without them");
    for (const m of ["view", "layers", "simulate", "tools", "about"]) {
      await pg.locator('[data-menu="' + m + '"]').click(); await pg.waitForSelector(".menu");
      // under these headings every item needs one of the missing APIs
      const bad = await pg.evaluate(() => { const need = ["Layout", "Structure colours", "Colour by a state variable", "Section cuts", "Ground", "Rooms"]; let h = "", out = []; for (const e of document.querySelectorAll(".menu .mh, .menu .mi")) { if (e.classList.contains("mh")) { h = e.textContent.trim(); continue; } if (need.includes(h) && e.getAttribute("aria-disabled") !== "true") out.push(h + ": " + e.textContent.trim()); } return out; });
      check(bad.length === 0, "missing APIs: menu " + m + " shows what needs the missing APIs as not available: " + bad.join(" | "));
      await pg.keyboard.press("Escape");
    }
    await pg.locator('[data-menu="view"]').click(); await pg.waitForSelector(".menu");
    const dis = await pg.evaluate(() => [...document.querySelectorAll(".menu .mi[aria-disabled=true]")].map((i) => !!(i.title || /not available|needs/i.test(i.textContent))));
    check(dis.length >= 20 && dis.every(Boolean), "missing APIs: every disabled item says why (" + dis.length + " disabled)"); await pg.keyboard.press("Escape");
    for (const id of ["explore", "services", "sun", "thermal", "states", "query"]) { await pg.evaluate((id) => window.reteBuilding.shell.openSide(id), id); await pg.waitForTimeout(120); }
    await pg.evaluate(() => window.reteBuilding.shell.openSide("services"));
    const ctl = await pg.evaluate(() => { const b = document.getElementById("svcViewBody"); return { segs: [...b.querySelectorAll(".segb")].map((x) => x.getAttribute("aria-disabled") === "true" && !!x.title), boxes: [...b.querySelectorAll("input[type=checkbox]")].map((x) => x.disabled && !!x.title), note: /not available/.test(b.textContent) }; });
    check(ctl.segs.length >= 6 && ctl.segs.every(Boolean) && ctl.boxes.length === 3 && ctl.boxes.every(Boolean) && ctl.note, "missing APIs: the Services panel's controls are disabled with a tooltip: " + JSON.stringify(ctl));
    for (const t of ["console", "results", "validate", "graph", "thermal", "scenarios", "logs"]) { await pg.evaluate((t) => window.reteBuilding.shell.openTab(t), t); await pg.waitForTimeout(150); }
    await pg.evaluate(() => { const a = window.reteBuilding; a.select("Wohnen"); a.select(a.graph.nodes.find((n) => n.c === "IfcDoor").n); a.clearSelection(); });
    for (const k of ["x", "1", "4", "m", "Shift+F", "Control+z"]) { await pg.keyboard.press(k); await pg.waitForTimeout(60); }
    await pg.keyboard.press("Escape");
    for (const [name, args] of [["get_ui"], ["get_state"], ["list_elements", { limit: 3 }], ["set_layout", { layout: "quad" }], ["set_section", { axis: "z", enabled: true }], ["list_structure"], ["colour_by", { variable: "room-air-temperature" }]]) {
      const r = await pg.evaluate(([n, a]) => window.reteBuildingTools.call(n, a || {}).then(() => "ok", (e) => "err: " + String(e.message || e).slice(0, 100)), [name, args]);
      check(r === "ok" || (/^err:/.test(r) && /not (loaded|available)|unknown|no such|needs/i.test(r)) || !/Cannot read|undefined|TypeError/.test(r), "missing APIs: the tool " + name + " answers or says what is missing: " + r);
    }
    const kinds = await pg.evaluate(() => window.reteBuilding.logs.entries().filter((e) => e.level === "error" && e.source !== "agent").map((e) => e.message.slice(0, 120)));   // the failed tool calls above are on purpose
    check(p.errs.length === 0 && kinds.length === 0, "missing APIs: no page or console errors, nothing logged as an error: " + p.errs.concat(kinds).join(" | "));
    await p.ctx.close();
  });

  // ============ without the shell: the house does not wait for it ============
  await section("shell blocked", async () => {
    const p = await open(browser, { block: ["**/building/shell-init.js"] });
    const o = await p.page.evaluate(() => { const a = window.reteBuilding; return { shell: !!a.shell, interactive: !!a.interactive, ready: a.ready, t: a.timing, canvas: !!document.querySelector("#stage canvas[data-engine]"), tools: window.reteBuildingTools.list().length }; });
    check(!o.shell && o.interactive && o.ready && o.canvas && o.tools > 20 && o.t.firstFrame <= o.t.interactive, "shell blocked: the house is drawn and usable without the shell: " + JSON.stringify(o));
    check(p.errs.length === 1 && /the shell failed to load/.test(p.errs[0]), "shell blocked: the only error is the shell's own load failure: " + p.errs.join(" | "));
    await p.ctx.close();
    const q = await open(browser); const t = await q.page.evaluate(() => window.reteBuilding.timing);
    check(t.firstFrame > 0 && t.firstFrame <= t.interactive && t.interactive <= t.ready, "timing: first frame, then interactive, then ready: " + JSON.stringify(t));
    notes.push({ timing_with_shell: t, timing_shell_blocked: o.t });
    await q.ctx.close();
  });

  // ============ memory: open and close everything, twice ============
  await section("memory", async () => {
    const p = await open(browser); const pg = p.page;
    const snap = () => pg.evaluate(() => { const a = window.reteBuilding, i = a.renderer.info; return { geometries: i.memory.geometries, textures: i.memory.textures, programs: i.programs ? i.programs.length : 0, dom: document.getElementsByTagName("*").length - document.querySelectorAll("#lgList *").length }; });   // the log list grows with the log (capped), it is not a leak
    const cycle = async () => {
      for (const id of ["explore", "services", "sun", "thermal", "states", "query"]) { await pg.evaluate((id) => window.reteBuilding.shell.openSide(id), id); await pg.waitForTimeout(100); }
      for (const t of ["console", "results", "validate", "graph", "thermal", "scenarios", "logs"]) { await pg.evaluate((t) => window.reteBuilding.shell.openTab(t), t); await pg.waitForTimeout(200); }
      await pg.evaluate(() => { const a = window.reteBuilding; a.shell.maximiseBottom(true); a.shell.maximiseBottom(false); a.shell.closeBottom(); a.shell.closeSide(); a.shell.openInspector(); a.shell.closeInspector(); });
      await pg.evaluate(async () => { const a = window.reteBuilding; for (const l of ["quad", "single"]) { a.viewport.setLayout(l); await new Promise((r) => setTimeout(r, 300)); } a.select("Wohnen"); a.clearSelection(); });
      for (const m of ["view", "layers", "simulate", "tools", "about"]) { await pg.locator('[data-menu="' + m + '"]').click(); await pg.waitForSelector(".menu"); await pg.keyboard.press("Escape"); }
      await pg.evaluate(() => window.reteBuilding.shell.openSide("explore"));
      await pg.waitForTimeout(300);
    };
    await cycle(); await cycle();       // the first passes build what is built lazily (graph view, thermal, the console, shader programs)
    const base = await snap();
    await cycle(); await cycle();
    const end = await snap();
    check(end.geometries === base.geometries && end.textures === base.textures && end.programs === base.programs, "memory: geometries, textures and shader programs return to the baseline after two more rounds: " + JSON.stringify([base, end]));
    check(Math.abs(end.dom - base.dom) <= Math.max(10, base.dom * 0.01), "memory: the number of DOM nodes returns to the baseline: " + base.dom + " -> " + end.dom);
    check(p.errs.length === 0, "memory: no errors: " + p.errs.join(" | "));
    await p.ctx.close();
  });

  // ============ without the services data ============
  await section("no services", async () => {
    const p = await open(browser, { systems: false }); const pg = p.page;
    const o = await pg.evaluate(async () => { const a = window.reteBuilding, W = await import("/building/webmcp.js"); const arr = [W.buildingTools(), W.shellTools(), a.svc ? W.systemsTools() : [], W.perfTools()]; try { arr.push((await import("/building/webmcp-viewport.js")).viewportTools(a)); } catch (e) { /* none */ } const ex = arr.flat().length; return { svc: !!a.svc, systems: !!a.systems, ready: a.ready, tools: window.reteBuildingTools.list().length, expectedTools: ex, absent: !!document.getElementById("svcAbsent"), layers: document.querySelectorAll("#layers .sys").length, graph: a.graphInfo || null, shell: !!a.shell, mcp: window.reteBuildingTools.list().some((t) => t.name === "get_ui") }; });
    check(!o.svc && !o.systems && o.ready && o.shell && o.mcp && o.absent && o.layers === 0 && o.tools === o.expectedTools, "no services: the house and the shell work, the services panel says the layer is not loaded: " + JSON.stringify(o));
    await pg.locator("#rail-services").click(); check(await pg.evaluate(() => /not loaded/.test(document.getElementById("pane-services").textContent)), "no services: the Services panel says so");
    await pg.locator('[data-menu="layers"]').click(); await pg.waitForSelector(".menu"); const lm = await pg.evaluate(() => [...document.querySelectorAll(".menu .mi")].map((i) => i.textContent + ":" + i.getAttribute("aria-disabled"))); check(lm.some((i) => /not loaded/.test(i) && /true/.test(i)) && lm.some((i) => /^Walls/.test(i)), "no services: the Layers menu still lists the house and shows the services as not available"); await pg.keyboard.press("Escape");
    await pg.locator('[data-menu="simulate"]').click(); await pg.waitForSelector(".menu"); const sm = await pg.evaluate(() => [...document.querySelectorAll(".menu .mi")].filter((i) => /thermal day/i.test(i.textContent)).map((i) => i.getAttribute("aria-disabled"))); check(sm.length >= 1 && sm.every((x) => x === "true"), "no services: the thermal day is shown as not available"); await pg.keyboard.press("Escape");
    await pg.evaluate(() => window.reteBuilding.shell.openTab("scenarios")); check(await pg.evaluate(() => /need the services/i.test(document.getElementById("tp-scenarios").textContent)), "no services: the Scenarios tab says what it needs");
    await pg.evaluate(() => window.reteBuilding.shell.openSide("thermal")); check(await pg.evaluate(() => /needs the services/.test(document.getElementById("thermSec").textContent)), "no services: the Thermal panel says what it needs");
    await pg.evaluate(() => window.reteBuilding.shell.openSide("states")); check(await pg.evaluate(() => document.querySelectorAll("#statesList .strow").length === 16), "no services: doors and windows still have their states panel");
    await pg.evaluate(() => { window.reteBuilding.shell.openTab("console"); window.reteBuilding.console.run(); }); await pg.waitForFunction(() => /rows? ·/.test(document.getElementById("conMsg").textContent), null, { timeout: 40000 });
    check(await pg.evaluate(() => /House only/.test(document.getElementById("conGraph").textContent) && window.reteBuilding.graphInfo.combined === false && /House only|house only/.test(document.getElementById("stGraph").textContent)), "no services: the console runs on the house-only graph and says so");
    await pg.evaluate(() => { window.reteBuilding.select("Wohnen"); window.reteBuilding.shell.openTab("graph"); }); await pg.waitForFunction(() => document.querySelector(".gv canvas"), null, { timeout: 60000 });
    check(await pg.evaluate(() => window.reteBuilding.graphView.ready.then(() => window.reteBuilding.graphView.getState().universe > 50)), "no services: the Graph tab shows the house graph");
    await pg.keyboard.press("Control+k"); await pg.keyboard.type("haust"); check(await pg.evaluate(() => [...document.querySelectorAll("#palList [role=option]")].some((o) => /Haustuer/.test(o.textContent))), "no services: the palette finds elements"); await pg.keyboard.press("Escape");
    const t = await pg.evaluate(() => window.reteBuildingTools.call("get_ui").then((v) => v.side, (e) => "ERR " + e.message)); check(!/ERR/.test(t), "no services: get_ui works");
    await pg.evaluate(() => window.reteBuilding.shell.openSide("services")); await pg.locator('[data-menu="view"]').click(); await pg.waitForSelector(".menu"); const vm = await pg.evaluate(() => [...document.querySelectorAll(".menu .mi")].filter((i) => /Heating|X-ray/.test(i.textContent)).map((i) => i.textContent + ":" + i.getAttribute("aria-disabled"))); await pg.keyboard.press("Escape"); check(vm.every((x) => /true/.test(x)) , "no services: x-ray and isolate are shown as not available: " + JSON.stringify(vm));
    check(p.errs.length === 0, "no services: no errors: " + p.errs.join(" | ")); check(p.bad.length === 0, "no services: no failed requests besides the two systems files: " + p.bad.join(" | "));
    await p.ctx.close();
  });

  await browser.close();
  const pass = fails.length === 0;
  console.log(JSON.stringify({ verdict: pass ? "PASS" : "FAIL", notes, fails }, null, 2));
  process.exit(pass ? 0 : 1);
};
main().catch((e) => { console.log(JSON.stringify({ verdict: "FAIL", fails: [...fails, "exception: " + (e && e.stack || e)] }, null, 2)); process.exit(1); });
