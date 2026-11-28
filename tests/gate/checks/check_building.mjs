// The FZK-Haus 3D viewer (docs/building.html + docs/building/*.js): it loads without
// errors, its WebMCP tools register and each works, the solar maths matches known
// values for Karlsruhe, a door's state changes and its mesh really moves, walk mode
// is blocked by a closed door and passes an open one, colour mode and the sun change
// the scene. Needs the network: geometry, index and graph come from data.graphplaza.com.
// The services layer may or may not be there (a 404 on the systems JSON is ignored); nothing here depends on it.
// Usage: node check_building.mjs
import { launchBrowser } from "./_browser.mjs";

const PORT = process.env.PGPORT || "8090";
const fails = [];
const check = (ok, what) => { if (!ok) fails.push(what); };
const SYSTEMS = /fzk-haus-systems-/;

const main = async () => {
  const browser = await launchBrowser({ args: ["--no-sandbox", "--use-gl=angle", "--use-angle=swiftshader", "--enable-unsafe-swiftshader", "--ignore-gpu-blocklist"] });
  const ctx = await browser.newContext({ viewport: { width: 1300, height: 850 } });
  const page = await ctx.newPage();
  const errs = [], bad = [], cons = [];
  page.on("pageerror", (e) => errs.push(String(e).slice(0, 200)));
  page.on("console", (m) => { if (m.type() === "error") cons.push(m.text().slice(0, 200)); });
  page.on("response", (r) => { if (r.status() >= 400 && !SYSTEMS.test(r.url())) bad.push(r.status() + " " + r.url().slice(0, 120)); });
  // A stand-in for the browser's WebMCP, to see what the page registers.
  await page.addInitScript(() => {
    window.__reg = [];
    Object.defineProperty(document, "modelContext", {
      value: { registerTool: (t, o) => { window.__reg.push({ name: t.name, exec: typeof t.execute === "function", signal: !!(o && o.signal) }); } },
    });
  });
  await page.goto(`http://localhost:${PORT}/building.html`, { waitUntil: "load" });
  await page.waitForFunction(() => window.reteBuilding && window.reteBuilding.ready, null, { timeout: 150000 });
  const call = (name, args) => page.evaluate(
    ([n, a]) => window.reteBuildingTools.call(n, a).then((v) => ({ v, bytes: JSON.stringify(v).length }), (e) => ({ err: String(e.message || e) })),
    [name, args || {}]);
  const ev = (f, a) => page.evaluate(f, a);
  const settle = () => ev(() => window.reteBuilding.settleStates());   // jump to the end pose (walk mode keeps the loop busy)

  // ---- WebMCP registration: the registered list is the union of the page's tool arrays (no services data here: no services tools)
  const want = ["get_state", "list_elements", "select_element", "set_visibility", "list_rooms", "go_to_room", "set_element_state", "list_states", "get_sun", "set_sun", "set_colour_mode", "walk", "list_examples", "run_sparql", "get_performance", "set_quality", "set_scenery"];
  const reg = await ev(async () => {
    const app = window.reteBuilding, W = await import("/building/webmcp.js");
    const arrays = [W.buildingTools(), W.shellTools(), app.svc ? W.systemsTools() : [], W.perfTools()];
    try { arrays.push((await import("/building/webmcp-viewport.js")).viewportTools(app)); } catch (e) { /* no viewport tools */ }
    const L = window.reteBuildingTools.list();
    return { reg: window.__reg, ann: window.reteWebMCP, listed: L.map((t) => t.name), expected: arrays.flat().map((t) => t.name), schemas: L.every((t) => t.inputSchema && t.inputSchema.type === "object" && typeof t.description === "string" && t.description.length > 10) };
  });
  const regNames = reg.reg.map((t) => t.name);
  check(reg.ann.supported === true && regNames.length === reg.expected.length && new Set(regNames).size === regNames.length && reg.expected.every((n) => regNames.includes(n) && reg.listed.includes(n)) && reg.listed.length === reg.expected.length,
    `the registered tools are exactly the union of the page's tool arrays (${reg.expected.length} expected, ${regNames.length} registered, ${reg.listed.length} listed)`);
  check(reg.reg.every((t) => t.exec && t.signal) && reg.schemas && want.every((n) => regNames.includes(n)), "every registered tool has execute, an AbortSignal, a schema and a description; the first 17 are among them");

  // ---- the app object the next pass relies on
  const api = await ev(() => {
    const a = window.reteBuilding;
    const fns = ["on", "off", "emit", "requestRender", "select", "focus", "registerLayer", "registerInfo", "registerPickable", "getState", "setState", "states", "allStates", "setInteriorLights", "setSite"];
    return { missing: fns.filter((f) => typeof a[f] !== "function"), objs: ["scene", "camera", "renderer", "house", "meshByGuid", "roomByGuid", "sun", "walk"].filter((k) => !a[k]),
      sunFns: ["get", "set", "position"].filter((f) => typeof a.sun[f] !== "function"), meshes: a.meshByGuid.size, rooms: a.roomByGuid.size };
  });
  check(!api.missing.length && !api.objs.length && !api.sunFns.length && api.meshes >= 100 && api.rooms === 7, "app object: " + JSON.stringify(api));
  const evts = await ev(() => { const got = []; const a = window.reteBuilding; ["select", "state", "mode", "time"].forEach((e) => a.on(e, () => got.push(e))); window.__evts = got; return true; });

  // ---- the solar maths, Karlsruhe (49.10 N, 8.43 E): solar noon elevation = 90 - lat + declination
  const sun = await ev(() => {
    const A = window.reteBuilding.sun, out = {};
    for (const [k, d] of [["jun", "2026-06-21"], ["dec", "2026-12-21"], ["mar", "2026-03-20"]]) {
      const t = A.sunTimes(d), p = A.position({ date: d, time: t.solarNoon });
      out[k] = { el: p.elevation, az: p.azimuth, rise: t.sunrise, set: t.sunset };
    }
    const dst = A.position({ date: "2026-06-21", time: "13:30" }), iso = A.position("2026-06-21T11:30:00Z");
    out.dst = Math.abs(dst.elevation - iso.elevation) + Math.abs(dst.azimuth - iso.azimuth);
    const cet = A.position({ date: "2026-12-21", time: "12:00" }), iso2 = A.position("2026-12-21T11:00:00Z");
    out.cet = Math.abs(cet.elevation - iso2.elevation);
    const night = A.position({ date: "2026-06-21", time: "02:00" });
    out.night = night.isDay;
    const before = A.get(); out.pure = JSON.stringify(A.get()) === JSON.stringify(before);
    return out;
  });
  check(Math.abs(sun.jun.el - 64.3) < 1, "solar noon 21 Jun elevation ~64.3: " + sun.jun.el.toFixed(2));
  check(Math.abs(sun.dec.el - 17.5) < 1, "solar noon 21 Dec elevation ~17.5: " + sun.dec.el.toFixed(2));
  check(Math.abs(sun.mar.el - 40.9) < 1, "solar noon equinox elevation ~40.9: " + sun.mar.el.toFixed(2));
  check(Math.abs(sun.jun.az - 180) < 1.5 && Math.abs(sun.dec.az - 180) < 1.5, "solar noon is south: " + sun.jun.az.toFixed(1) + " " + sun.dec.az.toFixed(1));
  check(sun.jun.rise > 5 && sun.jun.rise < 5.8 && sun.jun.set > 21 && sun.jun.set < 22, "sunrise/sunset 21 Jun: " + sun.jun.rise.toFixed(2) + " " + sun.jun.set.toFixed(2));
  check(sun.dst < 0.05 && sun.cet < 0.05, "local time handles CEST and CET: " + sun.dst.toFixed(3) + " " + sun.cet.toFixed(3));
  check(sun.night === false, "2 am is night");

  // ---- the light direction follows the sun (and true north)
  let r = await call("set_sun", { date: "2026-06-21", time: "09:00" });
  const d1 = await ev(() => window.reteBuilding.getSunDirection().toArray());
  r = await call("set_sun", { time: "16:00" });
  const d2 = await ev(() => window.reteBuilding.getSunDirection().toArray());
  check(r.v && r.v.date === "2026-06-21" && r.v.time === "16:00" && d1[0] > d2[0] + 0.3 && d1[2] > 0.3 && d2[2] > 0.3, "light direction changes with time (east in the morning, west in the afternoon): " + JSON.stringify([d1, d2]));
  await ev(() => window.reteBuilding.setSite({ northAzimuthDeg: 90 }));
  r = await call("set_sun", { time: "12:30" });
  const d3 = await ev(() => window.reteBuilding.getSunDirection().toArray());
  check(r.v && r.v.azimuthDeg > 150 && r.v.azimuthDeg < 210 && d3[0] < -0.3 && Math.abs(d3[1]) < 0.4, "true north turned 90 deg moves the sun in the model frame: " + JSON.stringify(d3));
  await ev(() => window.reteBuilding.setSite({ northAzimuthDeg: 0 }));
  r = await call("set_sun", { time: "30:00" });
  check(!!r.err, "set_sun refuses a bad time");
  r = await call("get_sun");
  check(r.v && /^\d\d:\d\d$/.test(r.v.sunrise) && /^\d\d:\d\d$/.test(r.v.sunset) && r.v.direction.length === 3 && typeof r.v.elevationDeg === "number", "get_sun: " + JSON.stringify(r).slice(0, 200));

  // ---- night: lit from inside; day: not
  await call("set_sun", { date: "2026-06-21", time: "23:30" });
  check(await ev(() => window.reteBuilding.getInteriorLights() === true && window.reteBuilding.getSunDirection().z < 0), "night: interior lights on, sun below the horizon");
  await call("set_sun", { time: "13:30" });
  check(await ev(() => window.reteBuilding.getInteriorLights() === false), "day: interior lights off");

  // ---- colour mode
  const wallMat = () => ev(() => { const A = window.reteBuilding; const n = A.graph.nodes.find((x) => x.c === "IfcWallStandardCase" && /Wand-Int/.test(x.l)); let part = null; A.meshByGuid.get(n.n).traverse((o) => { if (o.isMesh && !part && o.userData.els) { const el = o.userData.els.find((e) => e && e.set.root.visible); if (el) part = el.parts[0]; } }); return { map: !!part.spec.map, hex: part.base.getHexString(), flat: !!part.spec.flat }; });   // the part of the wall that is drawn now
  const real = await wallMat();
  r = await call("set_colour_mode", { mode: "class" });
  const cls = await wallMat();
  r = await call("set_colour_mode", { mode: "plaid" });
  check(!!r.err, "set_colour_mode refuses an unknown mode");
  await call("set_colour_mode", { mode: "realistic" });
  const back = await wallMat();
  check(real.map && !cls.map && cls.hex === "cbb89d" && cls.flat && back.map, "colour mode switches materials: " + JSON.stringify([real, cls, back]));

  // ---- states: a door moves
  r = await call("list_states");
  const doors = r.v.items.filter((s) => s.kind === "door"), wins = r.v.items.filter((s) => s.kind === "window");
  check(doors.length === 5 && wins.length === 11 && doors.every((d) => d.state === "closed" && d.allowed.join() === "closed,open") && wins.every((w) => ["closed,tilted,open", "closed"].includes(w.allowed.join())) && wins.filter((w) => w.allowed.length === 3).length >= 9, "list_states: 5 doors, 11 windows (fixed glazing has the one state): " + JSON.stringify([doors.map((d) => d.allowed.join()), wins.map((w) => w.allowed.join())]));
  const door = doors.find((d) => d.label === "Innentuer-1"), win = wins[0];
  const pos = (id) => ev((i) => { const o = window.reteBuilding.meshByGuid.get(i); let sx = 0, sy = 0, n = 0; o.updateWorldMatrix(true, true);
    o.traverse((m) => { if (m.isMesh && m.userData.role !== "frame") { m.geometry.computeBoundingBox(); const c = m.geometry.boundingBox.getCenter(m.position.clone()); m.localToWorld(c); sx += c.x; sy += c.y; n++; } }); return [sx / n, sy / n]; }, id);
  const p0 = await pos(door.id);
  r = await call("set_element_state", { id: door.id, state: "open" });
  check(r.v && r.v.state === "open", "set_element_state door open");
  await page.waitForFunction((i) => window.reteBuilding.openness(i) > 0.99, door.id, { timeout: 60000 });   // the real animation
  const p1 = await pos(door.id);
  check(Math.hypot(p1[0] - p0[0], p1[1] - p0[1]) > 0.2, "the open door's leaf moved: " + JSON.stringify([p0, p1]));
  r = await call("set_element_state", { id: door.id, state: "tilted" });
  check(!!r.err, "a door cannot be tilted");
  await call("set_element_state", { id: door.id, state: "closed" }); await settle();
  const p2 = await pos(door.id);
  check(Math.hypot(p2[0] - p0[0], p2[1] - p0[1]) < 0.02, "closing puts the leaf back");
  const w0 = await pos(win.id);
  await call("set_element_state", { id: win.id, state: "tilted" }); await settle();
  const wz = await ev((i) => { const o = window.reteBuilding.meshByGuid.get(i); let zmax = -1; o.updateWorldMatrix(true, true); const bb = new (o.position.constructor)(); o.traverse((m) => { if (m.isMesh && m.userData.role === "glass") { m.geometry.computeBoundingBox(); const c = m.geometry.boundingBox.max.clone(); m.localToWorld(c); zmax = Math.max(zmax, c.z); } }); return zmax; }, win.id);
  check((await call("list_states")).v.items.find((s) => s.id === win.id).state === "tilted" && wz > 0, "a window tilts");
  await call("set_element_state", { id: win.id, state: "closed" }); await settle();
  r = await call("set_element_state", { id: "no-such-thing", state: "open" });
  check(!!r.err, "set_element_state refuses an unknown id");

  // ---- walk: blocked by a closed door, through an open one
  const hid = doors.find((d) => d.label === "Haustuer").id;
  r = await call("walk", { enter: true });
  check(r.v && r.v.walk.active === true && r.v.walk.x < 0 && Math.abs(r.v.walk.z - 1.65) < 0.01 && (await ev(() => window.reteBuilding.mode)) === "walk", "walk enters outside the front door: " + JSON.stringify(r.v && r.v.walk));
  const closedMove = await ev(() => { const w = window.reteBuilding.walk; w.teleport(-2, 5, { storey: 0, yaw: -Math.PI / 2 }); w.step(4, 0); return w.info(); });
  check(closedMove.x < 0.05 && closedMove.room == null, "the closed front door blocks the walker: x=" + closedMove.x);
  await call("set_element_state", { id: hid, state: "open" }); await settle();
  const openMove = await ev(() => { const w = window.reteBuilding.walk; w.teleport(-2, 5, { storey: 0 }); w.step(4, 0); return w.info(); });
  check(openMove.x > 1.5 && openMove.room === "Flur", "the open front door lets the walker in: " + JSON.stringify(openMove));
  const wall = await ev(() => { const w = window.reteBuilding.walk; w.teleport(2, 5, { storey: 0 }); w.step(0, -3); const a = w.info().y; w.teleport(2, 5, { storey: 0 }); w.step(0, 3); return [a, w.info().y]; });
  check(wall[0] > 3.4 && wall[1] < 6.0, "walls stop the walker: " + JSON.stringify(wall));
  r = await call("go_to_room", { room: "Küche" });
  check(r.v && r.v.walk.room === "Küche", "go_to_room moves the walker: " + JSON.stringify(r.v));
  r = await call("go_to_room", { room: "Galerie" });
  check(r.v && r.v.walk.room === "Galerie" && r.v.walk.storey === 1 && r.v.walk.z > 4, "go_to_room puts the walker on the upper floor: " + JSON.stringify(r.v));
  r = await call("get_state");
  check(r.v && r.v.walk.active === true && r.v.walk.room === "Galerie" && r.v.mode === "walk", "get_state reports walking");
  r = await call("walk", { enter: false });
  check(r.v && r.v.walk.active === false && (await ev(() => window.reteBuilding.mode)) === "orbit", "walk leave returns to orbit");

  // ---- the other tools
  r = await call("get_state");
  check(r.v && r.v.counts.doors === 5 && r.v.counts.windows === 11 && r.v.colourMode === "realistic" && r.v.visible.classes.includes("IfcDoor") && r.v.sun.date === "2026-06-21", "get_state: " + JSON.stringify(r).slice(0, 160));
  r = await call("list_rooms");
  check(r.v && r.v.rooms.length === 7 && r.v.rooms.every((x) => x.floorAreaM2 > 0), "list_rooms");
  r = await call("list_elements", { class: "Window", storey: 0 });
  check(r.v && r.v.total === 9 && r.v.items.every((x) => x.class === "IfcWindow" && x.state), "list_elements class + storey: total " + (r.v && r.v.total));
  r = await call("list_elements", { room: "Bad", text: "tuer" });
  check(r.v && r.v.total >= 1 && r.v.items.every((x) => /tuer/i.test(x.label)), "list_elements room + text");
  r = await call("select_element", { id: "Haustuer" });
  check(r.v && r.v.selected.label === "Haustuer" && await ev(() => /State:/.test(document.getElementById("info").textContent) && !!document.querySelector("#info .stbtns button")), "select_element shows the door and its state buttons");
  check((await ev(() => window.__evts)).includes("select") && (await ev(() => window.__evts)).includes("state") && (await ev(() => window.__evts)).includes("mode"), "events select, state, mode are emitted");
  r = await call("select_element", { id: "Wand" });
  check(!!r.err, "select_element refuses an ambiguous or unknown id");
  r = await call("set_visibility", { classes: ["Door", "Slab"], storeys: [0] });
  check(r.v && r.v.visible.classes.join() === "IfcSlab,IfcDoor" && r.v.visible.storeys.join() === "0"
    && await ev(() => { const A = window.reteBuilding; const w = A.graph.nodes.find((n) => n.c === "IfcWallStandardCase"); return A.meshByGuid.get(w.n).visible === false; }), "set_visibility hides walls");
  r = await call("set_visibility", { classes: ["Spaceship"] });
  check(!!r.err, "set_visibility refuses an unknown class");
  await call("set_visibility", { classes: ["Wall", "Slab", "Window", "Door", "Stair", "Railing", "Beam", "Member"], storeys: [0, 1], roof: true });
  r = await call("list_examples");
  check(r.v && r.v.examples.length >= 6 && /SELECT/.test(r.v.examples[0].query), "list_examples");
  r = await call("run_sparql", { query: "PREFIX bot: <https://w3id.org/bot#> SELECT (COUNT(?s) AS ?n) WHERE { ?s a bot:Space }" });
  check(r.v && r.v.kind === "select" && /^"7"/.test(r.v.rows[0].n), "run_sparql counts the 7 rooms: " + JSON.stringify(r).slice(0, 160));
  check(await ev(() => { const a = window.reteBuilding; return !!a.results.get() && a.results.get().label === "Agent query" && /Agent query/.test(document.getElementById("footer-title").textContent) && document.querySelectorAll("#results tbody tr").length === 1; }), "the page shows the agent's query result (Results tab)");
  r = await call("run_sparql", { query: "ASK { ?s ?p ?o }" });
  check(r.v && r.v.kind === "ask" && r.v.boolean === true, "run_sparql ASK");
  r = await call("run_sparql", { query: "PREFIX cad: <https://w3id.org/rete/cad#> SELECT ?s ?p ?o WHERE { ?s ?p ?o }", limit: 500, show: false });
  check(r.v && r.v.returned > 20 && r.bytes <= 18500 && r.v.truncated === true, "run_sparql stays under the size cap: " + JSON.stringify({ n: r.v && r.v.returned, b: r.bytes }));
  r = await call("run_sparql", { query: "SELEC nonsense" });
  check(!!r.err, "run_sparql throws the engine's parse error");
  check(await ev(() => /run_sparql/.test((document.getElementById("agentPill") || {}).textContent || "")), "the page names the agent's last call");

  // ---- speed: draw calls, idle, programs, the three tools
  const perf = await call("get_performance");
  const pv = perf.v || {};
  check(pv.render && pv.render.drawCalls > 5 && pv.render.drawCalls <= 60 && pv.memory.programs > 3 && ["low", "medium", "high"].includes(pv.quality) && Array.isArray(pv.modulesLoaded) && pv.modulesLoaded.includes("batch") && pv.batches.length >= 1 && pv.timingsMs.ready > 0,
    "get_performance: house and scenery in at most 60 draw calls: " + JSON.stringify({ q: pv.quality, r: pv.render, m: pv.memory, mods: pv.modulesLoaded }));
  const idle = await ev(async () => {
    const A = window.reteBuilding; A.scenery.set({ animate: false }); A.renderNow(); await new Promise((r) => setTimeout(r, 900));
    const f0 = A.frameCount; await new Promise((r) => setTimeout(r, 2000)); const still = A.frameCount - f0;
    A.scenery.set({ animate: true, wind: 0.4 }); await new Promise((r) => setTimeout(r, 300));
    const g0 = A.frameCount; await new Promise((r) => setTimeout(r, 1000)); const moving = A.frameCount - g0;
    A.scenery.set({ animate: false }); await new Promise((r) => setTimeout(r, 300));
    return { still, moving, animating: A.scenery.get().animating };
  });
  check(idle.still === 0 && idle.moving > 2, "no renders while idle with the scenery animation off, and it renders while it runs: " + JSON.stringify(idle));
  const progs = await ev(async () => {
    const A = window.reteBuilding, n = () => { A.renderNow(); return A.renderer.info.programs.length; };
    A.sun.set({ date: "2026-06-21", time: 13 }); const day = n(); A.sun.set({ time: 23.5 }); const night = n(); A.sun.set({ time: 13 }); const day2 = n();
    A.sun.set({ date: "2026-12-21", time: 2 }); const winter = n(); A.setInteriorLights(true); const lit = n(); A.setInteriorLights(false); A.sun.set({ date: "2026-06-21", time: 13 });
    A.scenery.set({ on: false }); const off = n(); A.scenery.set({ on: true }); const on = n();
    return [day, night, day2, winter, lit, off, on];
  });
  check(new Set(progs).size === 1, "the number of shader programs stays the same through day, night, lights and the scenery switch: " + progs.join());
  r = await call("set_quality", { level: "medium" });
  const q1 = await ev(() => ({ q: window.reteBuilding.getQuality(), shadows: window.reteBuilding.renderer.shadowMap.enabled, pr: window.reteBuilding.renderer.getPixelRatio() }));
  await call("set_quality", { level: "low" });
  const q2 = await ev(() => ({ q: window.reteBuilding.getQuality(), shadows: window.reteBuilding.renderer.shadowMap.enabled, anim: window.reteBuilding.scenery.get().animating }));
  const r2 = await call("set_quality", { level: "ultra" });
  await call("set_quality", { level: "high" });
  check(r.v && q1.q === "medium" && q1.shadows === true && q2.q === "low" && q2.shadows === false && q2.anim === false && !!r2.err, "set_quality: three levels switch shadows and the scenery animation, an unknown level is refused: " + JSON.stringify([q1, q2]));
  r = await call("set_scenery", { dither: 0.8, wind: 0.2, on: true });
  const sc = r.v || {};
  check(sc.on === true && sc.dither === 0.8 && sc.wind === 0.2 && sc.treesShade === false, "set_scenery sets dither and wind, trees shade the house is off by default: " + JSON.stringify(sc));
  r = await call("set_scenery", { on: false });
  const skyState = await ev(() => { const A = window.reteBuilding; A.renderNow(); return { sky: A.sky.visible, calls: A.renderer.info.render.calls }; });
  check(r.v && r.v.on === false && skyState.sky === true, "set_scenery off brings the plain sky back: " + JSON.stringify(skyState));
  await call("set_scenery", { on: true, dither: 0.55, wind: 0.4, animate: false });
  const pick = await ev(() => { const A = window.reteBuilding; A.renderNow(); const cv = A.renderer.domElement.getBoundingClientRect(); return { n: A.scenery.layout() && A.scenery.layout().nearTrees.length }; });
  check(pick.n > 5, "the scenery has trees near the house");
  // picking and highlight through the batches: click the house's front door in the canvas
  const clicked = await ev(async () => {
    const A = window.reteBuilding; A.clearResult(); A.clearSelection(); A.frameAll(true); A.controls.update(); A.renderNow();
    const THREE = A.camera.position.constructor, cy = (n) => new (A.houseBox.constructor)().setFromObject(A.meshByGuid.get(n.n)).getCenter(new THREE()).y;
    const o = A.meshByGuid.get(A.graph.nodes.filter((n) => n.c === "IfcWindow" && n.st === 0).sort((a, b) => cy(a) - cy(b))[0].n);   // a window on the south face, seen from the default camera
    const b = new (A.houseBox.constructor)().setFromObject(o), c = b.getCenter(new THREE()); c.project(A.camera);
    const r = A.renderer.domElement.getBoundingClientRect(), x = r.left + (c.x + 1) / 2 * r.width, y = r.top + (1 - c.y) / 2 * r.height;
    return { x, y };
  });
  await page.mouse.click(clicked.x, clicked.y); await page.waitForTimeout(400);
  const sel = await ev(() => { const A = window.reteBuilding, s = A.selectedInfo(); let look = null; if (s) { const o = A.meshByGuid.get(s.id); o.traverse((m) => { if (m.isMesh && !look && m.userData.els) { const e = m.userData.els[0]; const p = e.parts[0], b = p.batch; look = { a: b.col.array[p.v0 * 3] }; } }); } return { s: s && s.label, look }; });
  check(sel.s !== null && sel.look, "a click in the canvas picks an element (through the proxies of the batches): " + JSON.stringify([sel, clicked, await ev(() => [window.reteBuilding.mode, innerWidth, innerHeight])]));
  await ev(() => { window.reteBuilding.clearSelection(); window.reteBuilding.setVisibility({ roof: true }); });

  // ---- z-fighting: no two faces of different elements share a plane without a rule (polygon offset) that lets one win
  const zf = await ev(async () => { const A = window.reteBuilding; A.setVisibility({ classes: ["Wall", "Slab", "Window", "Door", "Stair", "Railing", "Beam", "Member"], storeys: [0, 1], roof: true }); const r = await A.findCoplanar(); return { tri: r.triangles, pairs: r.pairs, handled: r.handled.length, unhandled: r.unhandled.slice(0, 6).map((g) => g.pair + " " + g.area + " m2 at " + g.at.join(",")) }; });
  check(zf.tri > 15000 && zf.pairs > 100 && zf.handled > 3 && zf.unhandled.length === 0, "the z-fighting diagnostic finds coplanar overlapping faces and every pair is handled by a polygon offset: " + JSON.stringify(zf));
  const clip = await ev(async () => { const A = window.reteBuilding; A.renderNow(); const o = { near: A.camera.near, far: A.camera.far };
    await window.reteBuildingTools.call("walk", { room: "Wohnen" }); A.renderNow(); o.walkNear = A.camera.near; o.walkFar = A.camera.far; await window.reteBuildingTools.call("walk", { enter: false }); A.renderNow(); return o; });
  check(clip.near >= 0.1 && clip.near <= 2 && clip.far <= 1100 && clip.walkNear <= 0.1 && clip.walkFar <= 500, "tight camera planes: orbit " + clip.near.toFixed(2) + " to " + clip.far + ", walk " + clip.walkNear + " to " + clip.walkFar);

  // ---- windows: the whole casement opens and the hole is a real one, from inside
  const winT = await ev(async () => {
    const A = window.reteBuilding, T = A.camera.position.constructor, B = A.houseBox.constructor, out = {};
    const vis = (o) => { for (; o; o = o.parent) if (!o.visible) return false; return true; };
    const n = A.graph.nodes.find((x) => x.l === "EG-Fenster-4"), g = A.meshByGuid.get(n.n);
    const parts = () => { const r = {}; g.traverse((o) => { if (o.isMesh && o.userData.part) r[o.userData.part] = new B().setFromObject(o); }); return r; };
    const centre = (b) => b.getCenter(new T());
    const box = new B().setFromObject(g), c = centre(box), s = box.getSize(new T());
    const thin = s.x < s.y ? new T(1, 0, 0) : new T(0, 1, 0), outward = thin.clone().multiplyScalar(Math.sign(c.clone().sub(A.houseBox.getCenter(new T())).dot(thin)));
    const closed = parts(); out.names = Object.keys(closed);
    const pane = closed["leaf0-0"], pc = centre(pane);
    const R = (await import("three")).Raycaster, hits = () => { const ray = new R(pc.clone().addScaledVector(outward, -1.2), outward.clone(), 0, 8); ray.camera = A.camera; return ray.intersectObjects(A.house.children, true).filter((h) => vis(h.object) && !h.object.isSprite && !h.object.userData.batch).map((h) => h.object.userData.role || "other"); };
    out.closedHits = hits();
    A.setState(n.n, "open", { instant: true }); const open = parts(); out.openHits = hits();
    A.setState(n.n, "tilted", { instant: true }); out.tiltedHits = hits();
    A.setState(n.n, "closed", { instant: true });
    const moved = (k) => !closed[k].min.equals(open[k].min) || !closed[k].max.equals(open[k].max);
    out.sashMoved = moved("sash0") && moved("leaf0-0") && moved("handle0"); out.frameFixed = !moved("frame");
    return out;
  });
  check(winT.closedHits.includes("glass") && winT.openHits.length === 0 && winT.tiltedHits.includes("glass") === true, "a ray from inside through the pane: the closed window hits the glass, the open one hits nothing of the house: " + JSON.stringify([winT.closedHits, winT.openHits, winT.tiltedHits]));
  check(winT.sashMoved && winT.frameFixed && winT.names.includes("sash0") && winT.names.includes("handle0"), "opening a window moves the sash ring and the handle with the pane, the outer frame stays: " + JSON.stringify(winT));

  // ---- the clock and the sun in the sky
  await call("set_sun", { date: "2026-06-21", time: "14:20" });
  const clk = await ev(async () => {
    const A = window.reteBuilding, el = document.getElementById("clock"), r = el.getBoundingClientRect(), cv = A.renderer.domElement.getBoundingClientRect();
    const o = { time: document.getElementById("clkT").textContent, date: document.getElementById("clkD").textContent, inside: r.left >= cv.left && r.right <= cv.right + 1 && r.top >= cv.top && r.bottom <= cv.bottom + 1, text: el.textContent };
    el.dispatchEvent(new WheelEvent("wheel", { deltaY: -1, cancelable: true })); o.afterWheel = document.getElementById("clkT").textContent;
    A.sun.set({ time: "14:20" });
    const u = A.scenery.get(); const dome = A.scene.getObjectByName("skyDome"); const d = dome.material.uniforms.uSunDir.value, g = A.getSunDirection();
    o.dome = d.distanceTo(g) < 1e-6; o.up = g.z > 0.3;
    // look straight at the sun and read the pixel in the middle of the canvas, then one 30 degrees away
    const cam = A.camera, T = cam.position.constructor, gl = A.renderer.getContext(), w = gl.drawingBufferWidth, h = gl.drawingBufferHeight, px = new Uint8Array(4);
    const read = (dir) => { cam.position.set(6, -30, 3); cam.lookAt(cam.position.clone().add(dir)); cam.updateMatrixWorld(true); A.scenery.set({ animate: false }); A.renderNow(); gl.readPixels(w >> 1, h >> 1, 1, 1, gl.RGBA, gl.UNSIGNED_BYTE, px); return px[0] + px[1] + px[2]; };
    A.mode = 'x'; o.sunPx = read(g.clone()); const side = g.clone().applyAxisAngle(new T(0, 0, 1), 0.52); o.offPx = read(side); A.mode = 'orbit'; A.frameAll(true);
    return o;
  });
  check(clk.time === "14:20" && /21 Jun 2026/.test(clk.date) && clk.inside && clk.afterWheel === "14:30" && /sun/.test(clk.text), "the clock overlay shows the time and date set through the sun API and scrubs by 10 minutes on the wheel: " + JSON.stringify(clk));
  check(clk.dome && clk.up && clk.sunPx > 700 && clk.offPx < 600, "the sun disc is drawn where the light comes from: the dome uses the light direction, looking along it the pixel is white (" + clk.sunPx + "), 30 degrees away it is not (" + clk.offPx + ")");

  // ---- layers and info providers (the hooks for the systems pass)
  const hook = await ev(() => {
    const A = window.reteBuilding, g = new A.house.constructor(); let toggled = null;
    const h = A.registerLayer({ id: "t", label: "Test layer", color: "#f5b301", group: g, count: 3 });
    A.on("layer", (e) => { toggled = e; }); const before = document.querySelectorAll("#layers .sys").length - 1; h.el.querySelector("input").click();
    A.registerInfo((t) => (t.kind === "element" && t.label === "Haustuer" ? "<div class=\"kv\" id=\"hookRow\">hook row</div>" : null));
    A.select("Haustuer");
    return { rows: document.querySelectorAll("#layers .sys").length - before, vis: g.visible, toggled, info: !!document.getElementById("hookRow") };
  });
  check(hook.rows === 1 && hook.vis === false && hook.toggled && hook.toggled.visible === false && hook.info, "registerLayer and registerInfo work: " + JSON.stringify(hook));

  check(errs.length === 0, "no page errors: " + errs.join(" | "));
  check(bad.length === 0, "no failed requests: " + bad.join(" | "));
  const conErr = cons.filter((c) => !/Failed to load resource/.test(c));
  check(conErr.length === 0, "no console errors: " + conErr.join(" | "));

  // ---- phone width
  const phone = await browser.newContext({ viewport: { width: 390, height: 800 }, hasTouch: true });
  const pp = await phone.newPage(); const perr = [];
  pp.on("pageerror", (e) => perr.push(String(e).slice(0, 200)));
  await pp.goto(`http://localhost:${PORT}/building.html`, { waitUntil: "load" });
  await pp.waitForFunction(() => window.reteBuilding && window.reteBuilding.ready, null, { timeout: 150000 });
  const ph = await pp.evaluate(() => ({ sw: document.documentElement.scrollWidth, iw: innerWidth, q: window.reteBuilding.getQuality(), walkBtn: !!document.getElementById("walkBtn").getBoundingClientRect().width }));
  check(ph.sw <= ph.iw && ph.q === "low" && ph.walkBtn && perr.length === 0, "phone width: " + JSON.stringify(ph) + perr.join("|"));

  const pass = fails.length === 0;
  console.log(JSON.stringify({ verdict: pass ? "PASS" : "FAIL", tools: reg.reg.map((t) => t.name), sun: { jun: +sun.jun.el.toFixed(2), dec: +sun.dec.el.toFixed(2), mar: +sun.mar.el.toFixed(2) }, fails }, null, 2));
  await browser.close();
  process.exit(pass ? 0 : 1);
};
main();
