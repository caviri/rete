// The services layer and the thermal model of the FZK-Haus viewer (docs/building/systems*.js, thermal*.js):
// the layers register and toggle, every device has a part at its position, plates sit on their walls, a switch
// turns its lights on and off with real light, a tripped breaker takes everything downstream out and the page's
// set equals the scenario's list and the answer of the SPARQL query on the graph, a trace from a socket reaches
// the grid connection, the new WebMCP tools work, and the thermal day gives plausible numbers.
// Needs the network for the house (geometry, index, graph come from data.graphplaza.com) and the two systems files:
// with FZK_SYSTEMS_DIR set, the two R2 addresses are answered from that directory, otherwise the live R2 files are used.
// Usage: FZK_SYSTEMS_DIR=D:/.../data/cad/systems node check_building_systems.mjs
import { launchBrowser } from "./_browser.mjs";
import { readFile } from "node:fs/promises";

const PORT = process.env.PGPORT || "8090";
const DIR = process.env.FZK_SYSTEMS_DIR || "";
const NAME = "fzk-haus-systems-2026-11-28";
const fails = [];
const check = (ok, what) => { if (!ok) fails.push(what); };

const main = async () => {
  const browser = await launchBrowser({ args: ["--no-sandbox", "--use-gl=angle", "--use-angle=swiftshader", "--enable-unsafe-swiftshader", "--ignore-gpu-blocklist"] });
  const ctx = await browser.newContext({ viewport: { width: 1300, height: 850 } });
  const page = await ctx.newPage();
  const errs = [], bad = [], cons = [];
  page.on("pageerror", (e) => errs.push(String(e).slice(0, 200)));
  page.on("console", (m) => { if (m.type() === "error") cons.push(m.text().slice(0, 200)); });
  page.on("response", (r) => { if (r.status() >= 400) bad.push(r.status() + " " + r.url().slice(0, 120)); });
  if (DIR) {
    await page.route(`**/cad/${NAME}.json`, async (r) => r.fulfill({ status: 200, contentType: "application/json", headers: { "access-control-allow-origin": "*" }, body: await readFile(`${DIR}/${NAME}.json`) }));
    await page.route(`**/cad/${NAME}.rete`, async (r) => r.fulfill({ status: 200, contentType: "application/octet-stream", headers: { "access-control-allow-origin": "*" }, body: await readFile(`${DIR}/${NAME}.rete`) }));
  }
  await page.addInitScript(() => {
    window.__reg = [];
    Object.defineProperty(document, "modelContext", { value: { registerTool: (t, o) => { window.__reg.push({ name: t.name, exec: typeof t.execute === "function", signal: !!(o && o.signal) }); } } });
  });
  await page.goto(`http://localhost:${PORT}/building.html`, { waitUntil: "load" });
  await page.waitForFunction(() => window.reteBuilding && window.reteBuilding.ready, null, { timeout: 150000 });
  const ev = (f, a) => page.evaluate(f, a);
  const call = (name, args) => page.evaluate(
    ([n, a]) => window.reteBuildingTools.call(n, a).then((v) => ({ v, bytes: JSON.stringify(v).length }), (e) => ({ err: String(e.message || e) })),
    [name, args || {}]);
  const hasSvc = await ev(() => !!window.reteBuilding.svc);
  check(hasSvc, "the services layer loaded (the systems files were served)");
  if (!hasSvc) { console.log(JSON.stringify({ verdict: "FAIL", fails }, null, 2)); await browser.close(); process.exit(1); }
  const data = await ev(() => { const d = window.reteBuilding.systems; return { nodes: d.nodes.length, runs: d.runs.length, systems: d.systems.map((s) => s.id), scenarios: d.scenarios.map((s) => ({ id: s.id, kind: s.kind || "supply", off: s.off, affected: s.affected, switchable: s.off.every((id) => { const t = window.reteBuilding.svc && window.reteBuilding.svc.thingFor(id); return !!(t && t.hasState); }) })) }; });

  // ---- WebMCP: the registered list is the union of the page's tool arrays; every tool has execute, a schema and a signal
  const want = ["list_systems", "set_system_visibility", "list_devices", "get_device", "set_device_state", "trace_supply", "list_scenarios", "apply_scenario", "reset_states", "set_xray", "get_site", "set_site", "get_thermal_model", "run_thermal_day", "get_window_sun"];
  const reg = await ev(async () => {
    const app = window.reteBuilding, W = await import("/building/webmcp.js");
    const arrays = { house: W.buildingTools(), shell: W.shellTools(), services: app.svc ? W.systemsTools() : [], perf: W.perfTools() };
    try { arrays.viewport = (await import("/building/webmcp-viewport.js")).viewportTools(app); } catch (e) { arrays.viewport = []; }
    const names = Object.values(arrays).flat().map((t) => t.name);
    return { reg: window.__reg, ann: window.reteWebMCP, listed: window.reteBuildingTools.list().map((t) => t.name), expected: names, schemas: window.reteBuildingTools.list().every((t) => t.inputSchema && t.inputSchema.type === "object" && typeof t.description === "string" && t.description.length > 10) };
  });
  const regNames = reg.reg.map((t) => t.name);
  check(reg.ann.supported && reg.expected.length >= 30 && regNames.length === reg.expected.length && new Set(regNames).size === regNames.length && reg.expected.every((n) => regNames.includes(n) && reg.listed.includes(n)) && reg.listed.length === reg.expected.length, `the registered tools are exactly the union of the page's tool arrays (${reg.expected.length} expected, ${regNames.length} registered, ${reg.listed.length} listed)`);
  check(reg.reg.every((t) => t.exec && t.signal) && reg.schemas, "every registered tool has execute, an AbortSignal, an object schema and a description");
  check(want.every((n) => regNames.includes(n)), "the services tools are among them");

  // ---- layers
  const lay = await ev(() => { const a = window.reteBuilding, s = a.svc; return { rows: document.querySelectorAll("#layers .sys").length, state: s.layerState().map((l) => [l.id, l.visible, l.nodes + l.runs]), title: /Building services/.test(document.getElementById("layersSec").textContent), note: /not part of the original IFC/.test(document.getElementById("layers").textContent) }; });
  check(lay.rows >= 10, "a toggle row per system: " + lay.rows);
  check(lay.state.length === data.systems.length && lay.state.filter((l) => l[1]).map((l) => l[0]).join() === "lighting" && lay.title && lay.note, "layers: only lighting on, title and the 'not part of the IFC' note: " + JSON.stringify(lay.state.map((l) => l[0] + ":" + l[1])));
  check(lay.state.reduce((n, l) => n + l[2], 0) === data.nodes + data.runs, "layer counts add up to nodes + runs");
  let r = await call("set_system_visibility", { system: "electrical", visible: true });
  check(r.v && r.v.layers.find((l) => l.id === "electrical").visible === true && await ev(() => window.reteBuilding.svc.sysGroup.get("electrical").visible), "set_system_visibility shows a layer");
  await ev(() => document.getElementById("svcNone").click());
  check(await ev(() => window.reteBuilding.svc.layerState().every((l) => !l.visible)), "the None button hides every layer");
  await ev(() => document.getElementById("svcAll").click());
  check(await ev(() => window.reteBuilding.svc.layerState().every((l) => l.visible)), "the All button shows every layer");
  await ev(() => { const cb = document.querySelector("#layers label.sys input"); cb.click(); cb.click(); });
  r = await call("set_system_visibility", { system: "nope", visible: true });
  check(!!r.err, "set_system_visibility refuses an unknown system");
  await call("set_system_visibility", { system: "all", visible: false }); await call("set_system_visibility", { system: "lighting", visible: true });

  // ---- parts at their positions, plates on their walls
  const geo = await ev(async () => {
    const a = window.reteBuilding, s = a.svc, out = { missing: [], far: [], noPick: 0, meshes: 0, plates: 0, offWall: [], moved: s.report.shifted.length };
    const THREE = await import("three");
    const walls = []; a.nodeByGuid.forEach((n, g) => { if (n.c === "IfcWallStandardCase") { const o = a.meshByGuid.get(g); if (o) walls.push(o); } });
    const ray = new THREE.Raycaster();
    for (const t of s.nodes) {
      const o = t.obj; if (!o || !t.meshes.length) { out.missing.push(t.id); continue; }
      out.meshes += t.meshes.length;
      o.updateWorldMatrix(true, true);
      const box = new THREE.Box3().setFromObject(o).expandByScalar(0.3), p = new THREE.Vector3(...t.d.p);
      if (!box.containsPoint(p)) out.far.push(t.id);
      if (!a.pickables.has(o)) out.noPick++;
      if (["socket", "switch", "data-outlet", "thermostat"].includes(t.d.type)) {
        out.plates++;
        const n = new THREE.Vector3(1, 0, 0).applyQuaternion(o.getWorldQuaternion(new THREE.Quaternion()));
        ray.set(o.position.clone().addScaledVector(n, 0.05), n.clone().negate()); ray.far = 0.2;
        const h = ray.intersectObjects(walls, true)[0];
        if (!h || h.distance > 0.05 + 0.02) out.offWall.push(t.id + ":" + (h ? h.distance.toFixed(3) : "none"));
      }
    }
    for (const t of s.runs) if (!a.pickables.has(t.obj)) out.noPick++;
    return out;
  });
  check(!geo.missing.length && !geo.far.length && geo.noPick === 0, `every node has a part around its position and every node and run is pickable: ${JSON.stringify({ missing: geo.missing.slice(0, 4), far: geo.far.slice(0, 4), noPick: geo.noPick })}`);
  check(geo.plates >= 40 && geo.offWall.length === 0, `socket, switch, data outlet and thermostat plates sit on their wall (${geo.plates} plates): ${geo.offWall.slice(0, 5).join(" ")}`);
  check(geo.meshes > 300, "fixtures are built from primitives: " + geo.meshes + " meshes");

  // ---- a switch turns its light on and off, with real light
  const light = await ev(() => {
    const a = window.reteBuilding, s = a.svc, st = (id) => s.stateOf(s.thingFor(id));
    const rl = () => a.scene.children.find((o) => o.name === "svcLights").children.map((l) => l.intensity);
    const glow = (id) => Math.max(...a.lookOf(s.thingFor(id).dyn.lamp).emissive);
    const o = { before: [st("sw-kueche"), st("light-kueche-1"), glow("light-kueche-1"), Math.max(...rl())] };
    a.setState("sw-kueche", "on");
    o.on = [st("light-kueche-1"), st("spot-kueche-1"), st("spot-kueche-4"), glow("light-kueche-1"), glow("spot-kueche-2"), Math.max(...rl())];
    a.emit("interact", { id: "sw-kueche" });          // use in walk mode
    o.off = [st("sw-kueche"), st("light-kueche-1"), glow("light-kueche-1"), Math.max(...rl())];
    a.emit("interact", { id: "sw-kueche" });
    o.again = [st("sw-kueche"), st("light-kueche-1")];
    a.setState("sw-flur-a", "on");                    // a two-way switch: the other one follows
    o.twoWay = [st("sw-flur-b"), st("light-flur-1"), st("light-flur-2")];
    a.setState("sw-flur-b", "off");
    o.twoWay2 = [st("sw-flur-a"), st("light-flur-1")];
    a.setInteriorLights(false); o.allOff = [s.nodes.filter((t) => s.INDOOR_LIGHTS.has(t.d.type) && st(t.id) === "on").length, Math.max(...rl())];
    a.setInteriorLights(true); o.allOn = s.nodes.filter((t) => s.INDOOR_LIGHTS.has(t.d.type) && st(t.id) === "on").length;
    a.setInteriorLights(false);
    return o;
  });
  check(light.before[0] === "off" && light.before[2] === 0 && light.before[3] === 0, "lights start off: " + JSON.stringify(light.before));
  check(light.on[0] === "on" && light.on[1] === "on" && light.on[2] === "on" && light.on[3] > 1 && light.on[4] > 1 && light.on[5] > 1, "a switch turns on the lights it controls, they glow and the room gets a real light: " + JSON.stringify(light.on));
  check(light.off[0] === "off" && light.off[1] === "off" && light.off[2] === 0 && light.off[3] === 0 && light.again[0] === "on" && light.again[1] === "on", "use on a switch toggles the light: " + JSON.stringify([light.off, light.again]));
  check(light.twoWay.join() === "on,on,on" && light.twoWay2.join() === "off,off", "a two-way switch and its lights stay in step: " + JSON.stringify([light.twoWay, light.twoWay2]));
  check(light.allOff[0] === 0 && light.allOff[1] === 0 && light.allOn >= 20, "setInteriorLights drives the fixtures: " + JSON.stringify([light.allOff, light.allOn]));

  // ---- a breaker trips: the page, the scenario's list and the graph agree
  r = await call("list_scenarios");
  check(r.v && r.v.scenarios.length === data.scenarios.length, "list_scenarios: " + (r.v && r.v.scenarios.length));
  const kitchen = await ev(() => {
    const a = window.reteBuilding, s = a.svc, st = (id) => s.stateOf(s.thingFor(id)); a.setInteriorLights(true);
    const glowK = () => Math.max(...a.lookOf(s.thingFor("light-kueche-1").dyn.lamp).emissive);
    const before = [glowK(), s.isLive("light-kueche-1")];
    s.setState("cb-1", "off");           // C1 is the lighting circuit of the ground floor
    return { before, after: [glowK(), s.isLive("light-kueche-1"), s.why("light-kueche-1") && s.why("light-kueche-1").text, st("cb-1")], info: (() => { a.select("light-kueche-1"); return document.getElementById("info").textContent; })() };
  });
  check(kitchen.before[0] > 1 && kitchen.before[1] === true && kitchen.after[0] === 0 && kitchen.after[1] === false && /no power: breaker C1 is off/.test(kitchen.after[2] || "") && kitchen.after[3] === "off", "tripping C1 darkens the kitchen light and says why: " + JSON.stringify(kitchen.after));
  check(/not live/.test(kitchen.info) && /no power: breaker C1 is off/.test(kitchen.info), "the info panel says why a thing has no power");
  await ev(() => window.reteBuilding.svc.resetStates());
  // scenarios that switch off something with a state (a breaker, a valve); the structural ones remove walls and lintels, which the viewer does not model
  for (const s of data.scenarios.filter((x) => x.switchable)) {
    r = await call("apply_scenario", { id: s.id, run_query: true });
    const exp = new Set(s.affected), got = new Set(r.v ? Object.values(r.v.affectedByRoom).flat() : []);
    const full = r.v && !r.v.truncated;
    check(r.v && r.v.affectedCount === s.affected.length && r.v.sameAsStoredAnswer === true && (!full || (got.size === exp.size && [...exp].every((x) => got.has(x)))), `scenario ${s.id}: the page's set equals the stored list (${r.v && r.v.affectedCount} of ${s.affected.length}) ${r.err || ""}`);
    if (s.kind === "structure") check(r.v && r.v.graph && r.v.graph.approximate === true && r.v.graph.sameSet === true && r.v.graph.modelCount === s.affected.length && r.v.graph.count >= s.affected.length, `scenario ${s.id} (structure): the SPARQL query is an approximation that contains the exact set (${JSON.stringify(r.v && r.v.graph)})`);   // a path rule: everything that bears on the removed element, the exact set is a subset
    else check(r.v && r.v.graph && r.v.graph.sameSet === true && r.v.graph.count === s.affected.length, `scenario ${s.id}: the SPARQL query on the graph gives the same set (${JSON.stringify(r.v && r.v.graph)})`);
  }
  check(await ev(() => /graph/i.test(document.getElementById("footer-title").textContent) && !!window.reteBuilding.results.get() && window.reteBuilding.graphSource.includes("fzk-haus-systems")), "the query ran on the combined graph and its table shows (Results tab)");
  r = await call("apply_scenario", { id: "nope" });
  check(!!r.err, "apply_scenario refuses an unknown scenario");
  await ev(() => document.querySelector('.scen-btn[data-id="trip-rcd-2"]').click());
  check(await ev(() => /Affected: 75 things/.test(document.getElementById("scenOut").textContent) && document.querySelectorAll("#scenGroups details").length >= 4), "the scenario panel lists the affected things grouped by room");
  r = await call("reset_states");
  check(r.v && r.v.notLiveNow === 0 && await ev(() => window.reteBuilding.svc.nodes.every((t) => t.live)), "reset_states restores everything");

  // ---- trace
  r = await call("trace_supply", { id: "sock-buero-1" });
  check(r.v && r.v.source === "grid-connection" && r.v.path[0].id === "grid-connection" && r.v.path.some((p) => /^rcd-/.test(p.id)) && r.v.path.some((p) => /^cb-/.test(p.id)) && r.v.path.at(-1).id === "sock-buero-1" && r.v.runs.length >= 1, "trace from a socket reaches the grid connection: " + (r.v && r.v.text.slice(0, 160)));
  check(await ev(() => { const s = window.reteBuilding.svc; return s.tints.trace.size >= 5 && s.thingFor(s.lastTrace.runs[0]).tint != null; }), "the traced path and its cables are highlighted");
  r = await call("trace_supply", { id: "tap-outdoor" });
  check(r.v && r.v.source === "water-entry", "trace of a tap reaches the water entry: " + (r.v && r.v.source));
  r = await call("trace_supply", { id: "no-such" });
  check(!!r.err, "trace_supply refuses an unknown id");
  await ev(() => window.reteBuilding.svc.clearTrace());
  const info = await ev(() => { const a = window.reteBuilding; a.select("sock-buero-1"); const b = [...document.querySelectorAll("#info button")].find((x) => /Trace to source/.test(x.textContent)); b.click(); return { traced: a.svc.lastTrace && a.svc.lastTrace.source, text: document.getElementById("info").textContent }; });
  check(info.traced === "grid-connection" && /Fed by/.test(info.text) && /Belongs to/.test(info.text) && /Circuit/.test(info.text) && /Path:/.test(info.text), "the info panel shows what feeds it, its circuit and the trace button works");
  await ev(() => window.reteBuilding.svc.clearTrace());

  // ---- the new tools
  r = await call("list_systems");
  check(r.v && r.v.systems.length === data.systems.length && r.v.systems[0].sources, "list_systems");
  r = await call("list_devices", { system: "electrical", type: "socket", room: "Buero" });
  check(r.v && r.v.total === 5 && r.v.items.every((x) => x.type === "socket" && x.room === "Buero" && x.live === true), "list_devices filters by system, type and room: " + (r.v && r.v.total));
  r = await call("list_devices", { text: "breaker", limit: 5 });
  check(r.v && r.v.returned === 5 && r.v.total >= 19, "list_devices text and limit");
  r = await call("list_devices", { system: "nope" });
  check(!!r.err, "list_devices refuses an unknown system");
  r = await call("get_device", { id: "rcd-1" });
  const r2 = await call("get_device", { id: "cb-4" });
  check(r.v && r.v.state === "on" && r.v.allowed.join() === "on,off" && r.v.feeds.length >= 9 && r.v.fedBy.length >= 1 && r2.v && r2.v.groups.some((g) => g.kind === "circuit" && g.props.loadW > 0), "get_device: state, what it feeds, its circuit");
  r = await call("get_device", { id: "nope" });
  check(!!r.err, "get_device refuses an unknown id");
  r = await call("set_device_state", { id: "cb-4", state: "off" });
  check(r.v && r.v.state === "off" && r.v.notLiveNow > 5, "set_device_state trips a breaker: " + JSON.stringify(r.v && [r.v.state, r.v.notLiveNow]));
  r = await call("get_device", { id: "sock-wohnen-1" });
  check(r.v && r.v.live === false && /no power: breaker C4 is off/.test(r.v.why), "get_device says why: " + (r.v && r.v.why));
  r = await call("set_device_state", { id: "cb-4", state: "bogus" });
  check(!!r.err, "set_device_state refuses a bad state");
  r = await call("set_device_state", { id: "sock-wohnen-1", state: "on" });
  check(!!r.err, "a socket has no state to set");
  await call("reset_states");
  await ev(() => window.reteBuilding.clearResult());   // a shown query result dims the context, which is not x-ray
  r = await call("set_xray", { on: true });
  const xr = await ev(() => { const a = window.reteBuilding; const w = a.graph.nodes.find((n) => n.c === "IfcWallStandardCase"); let m; a.meshByGuid.get(w.n).traverse((o) => { if (o.isMesh && !m) m = o; }); return a.lookOf(m).alpha; });
  check(r.v && r.v.xray === true && xr < 0.2, "set_xray makes the walls translucent: " + xr);
  r = await call("set_xray", { isolate: "heating" });
  const iso = await ev(() => { const s = window.reteBuilding.svc; return [window.reteBuilding.lookOf(s.thingFor("sock-buero-1").meshes[0]).alpha, window.reteBuilding.lookOf(s.thingFor("loop-buero-1").meshes[0]).alpha, document.getElementById("isoSel").value]; });
  check(r.v && r.v.isolated === "heating" && iso[0] < 0.2 && iso[1] === 1 && iso[2] === "heating", "isolate fades the other systems: " + JSON.stringify(iso));
  r = await call("set_xray", { on: false, isolate: null });
  const xr2 = await ev(() => { const a = window.reteBuilding; const w = a.graph.nodes.find((n) => n.c === "IfcWallStandardCase"); let m; a.meshByGuid.get(w.n).traverse((o) => { if (o.isMesh && !m) m = o; }); return a.lookOf(m).alpha; });
  check(xr2 === 1 && r.v.isolated === null, "x-ray off restores the walls: " + xr2);
  r = await call("set_xray", { isolate: "nope" });
  check(!!r.err, "isolate refuses an unknown system");
  r = await call("get_site");
  check(r.v && Math.abs(r.v.northAzimuthDeg - 50) < 0.5 && Math.abs(r.v.latitude - 49.1) < 0.1, "get_site: " + JSON.stringify(r.v && [r.v.northAzimuthDeg, r.v.latitude]));
  r = await call("set_site", {});
  check(!!r.err, "set_site wants something to change");
  r = await call("get_thermal_model");
  check(r.v && r.v.zones.length === 7 && r.v.windows.length === 11 && r.v.constructions.length >= 5 && r.bytes < 18500 && r.v.surfacesPerRoom.length === 7, "get_thermal_model: " + (r.v && r.v.zones.length) + " zones, " + r.bytes + " bytes");

  // ---- speed: draw calls with every system on, stable memory, picking through the batches
  const speed = await ev(async () => {
    const A = window.reteBuilding, T = window.reteBuildingTools, snap = () => { A.renderNow(); const i = A.renderer.info; return [i.memory.geometries, i.memory.textures, i.programs.length]; };
    await T.call("set_system_visibility", { system: "all", visible: true }); A.renderNow();
    const all = { calls: A.renderer.info.render.calls, tris: A.renderer.info.render.triangles };
    await T.call("set_system_visibility", { system: "all", visible: false }); A.renderNow(); const none = A.renderer.info.render.calls;
    const base = snap(), rounds = [];
    for (let k = 0; k < 2; k++) { await T.call("set_system_visibility", { system: "all", visible: true }); const on = snap(); A.colourMode = null; A.setColourMode("class"); A.setColourMode("realistic"); await T.call("set_system_visibility", { system: "all", visible: false }); rounds.push([on, snap()]); }
    await T.call("set_system_visibility", { system: "lighting", visible: true });
    return { all, none, base, rounds };
  });
  check(speed.all.calls <= 90 && speed.all.calls < speed.none + 60, "draw calls with every system on: " + speed.all.calls + " (the house alone " + speed.none + ", before the batches 865)");
  check(speed.rounds.every((r) => r[1][0] === speed.base[0] && r[1][1] === speed.base[1]) && speed.rounds[0][0][2] === speed.rounds[1][0][2], "geometries and textures return to the same counts after all layers and the colour mode were toggled twice: " + JSON.stringify(speed));
  await ev(() => window.reteBuilding.svc.setXray(true));
  const ids = await ev(() => { const s = window.reteBuilding.svc; return s.nodes.filter((t) => t.d.type === 'socket').slice(0, 6).map((t) => t.id); });
  const hits = [];
  for (const id of ids) {
    const at = await ev(async (id) => { const A = window.reteBuilding; A.clearResult(); A.clearSelection(); await window.reteBuildingTools.call('set_system_visibility', { system: 'electrical', visible: true }); const t = A.svc.thingFor(id); const V = A.camera.position.constructor, Q = A.camera.quaternion.constructor; const n = new V(1, 0, 0).applyQuaternion(t.obj.getWorldQuaternion(new Q())), c = t.obj.getWorldPosition(new V()); A.camera.position.copy(c).addScaledVector(n, 0.45); A.controls.target.copy(c); A.camera.lookAt(c); A.controls.update(); A.renderNow(); const r = A.renderer.domElement.getBoundingClientRect(); return { x: r.left + r.width / 2, y: r.top + r.height / 2 }; }, id);
    await page.mouse.click(at.x, at.y); await page.waitForTimeout(300);
    hits.push([id, await ev(() => { const s = window.reteBuilding.selectedInfo(); return s && s.id; })]);
  }
  check(ids.length >= 4 && hits.filter((h) => h[0] === h[1]).length >= Math.ceil(ids.length / 2), 'a click on a socket drawn from the batches selects that socket (the click goes to the proxies; a neighbour in the same line of sight can win): ' + JSON.stringify(hits));
  await ev(() => { window.reteBuilding.clearSelection(); window.reteBuilding.svc.setXray(false); });
  await call("set_system_visibility", { system: "all", visible: false }); await call("set_system_visibility", { system: "lighting", visible: true });

  // ---- the new node types: pre-wall, pendants, unknown types
  const nw = await ev(async () => {
    const A = window.reteBuilding, s = A.svc, pw = s.nodes.find((t) => t.d.type === "pre-wall"), o = {};
    o.prewall = !!pw; if (pw) { o.hidden = !pw.obj.parent.visible; await window.reteBuildingTools.call("set_system_visibility", { system: "water-cold", visible: true }); A.renderNow(); o.shownByColdWater = pw.obj.parent.visible; await window.reteBuildingTools.call("set_system_visibility", { system: "water-cold", visible: false }); A.renderNow(); }
    const pend = s.nodes.filter((t) => t.d.props && t.d.props.dropM); o.pendants = pend.length;
    if (pend[0]) { const b = new (A.houseBox.constructor)().setFromObject(pend[0].obj); o.cord = +(b.max.z - b.min.z).toFixed(2); o.drop = pend[0].d.props.dropM; }
    return o;
  });
  check(!nw.prewall || (nw.hidden && nw.shownByColdWater), "the pre-wall shows with the cold-water layer: " + JSON.stringify(nw));
  check(nw.pendants === 0 || nw.cord > nw.drop * 0.9, "a pendant has a cord of its drop length: " + JSON.stringify(nw));

  // ---- the sliding terrace door
  const sd = await ev(async () => {
    const A = window.reteBuilding, T = A.camera.position.constructor, B = A.houseBox.constructor, od = A.systems.openings.find((x) => x.label === "Terrassentuer");
    const n = A.graph.nodes.find((x) => x.l === "Terrassentuer"), g = A.meshByGuid.get(n.n), info = A.slidingInfo(n.n), out = { op: od && od.operation, travel: od && od.travel, slide: od && od.slide, has: !!info };
    const mv = () => { let b = null; g.traverse((o) => { if (o.isMesh && o.userData.part === "leaf0-0") b = new B().setFromObject(o); }); return b; };
    const fixed = () => { let b = new B(); g.traverse((o) => { if (o.isMesh && /^(frame|fixed)/.test(o.userData.part || "")) b.union(new B().setFromObject(o)); }); return b; };
    const a0 = mv(), f0 = fixed(); A.setState(n.n, "open", { instant: true }); const a1 = mv(), f1 = fixed();
    const d = a1.getCenter(new T()).sub(a0.getCenter(new T())); out.moved = d.toArray().map((v) => +v.toFixed(3)); out.fixedStill = f0.min.equals(f1.min) && f0.max.equals(f1.max);
    let rot = 0; g.traverse((o) => { rot += Math.abs(o.rotation.x) + Math.abs(o.rotation.y) + Math.abs(o.rotation.z); }); out.rotation = +rot.toFixed(4);
    A.setState(n.n, "closed", { instant: true });
    const w = (await A.loadWalk()), run = (x) => { w.teleport(x, -1.5, { storey: 0, yaw: 0 }); w.step(0, 3); return +w.info().y.toFixed(2); };
    out.closedThrough = run(5.35); out.fixedThrough = run(6.6);
    A.setState(n.n, "open", { instant: true }); out.openThrough = run(5.35); out.openFixedThrough = run(6.6);
    A.setState(n.n, "closed", { instant: true }); w.leave();
    return out;
  });
  check(sd.has && sd.op === "sliding" && Math.abs(sd.moved[0] - sd.travel * Math.sign(sd.slide[0])) < 0.02 && Math.abs(sd.moved[1]) < 0.02 && Math.abs(sd.moved[2]) < 0.02 && sd.rotation === 0 && sd.fixedStill, "opening the terrace door slides one panel along the data's direction by its travel, with no rotation, the frame and the other panels stay: " + JSON.stringify(sd));
  check(sd.closedThrough < 0.2 && sd.openThrough > 0.8 && sd.openFixedThrough < 0.2 && sd.fixedThrough < 0.2, "the walker passes the cleared part of the open sliding door and not the fixed panels or the closed door: " + JSON.stringify([sd.closedThrough, sd.openThrough, sd.fixedThrough, sd.openFixedThrough]));

  // ---- z-fighting with the services on: faces that share a plane have a rule, except what cannot be seen
  const zf = await ev(async () => { const A = window.reteBuilding; await window.reteBuildingTools.call("set_system_visibility", { system: "all", visible: true }); const r = await A.findCoplanar(); await window.reteBuildingTools.call("set_system_visibility", { system: "all", visible: false }); await window.reteBuildingTools.call("set_system_visibility", { system: "lighting", visible: true }); const area = r.unhandled.reduce((a, g) => a + g.area, 0); return { tri: r.triangles, groups: r.groups, unhandled: r.unhandled.slice(0, 5).map((g) => g.pair + " " + g.area), area: +area.toFixed(3) }; });
  check(zf.tri > 50000 && zf.area < 0.5, "with every system on the unhandled coplanar overlap is under half a square metre (touching appliances): " + JSON.stringify(zf));

  // ---- the sun and the physics
  const sky = await ev(() => { const a = window.reteBuilding, th = a.thermal, o = {}; for (const d of ["2026-06-21", "2026-12-21"]) { const el = a.sun.position({ date: d, time: a.sun.sunTimes(d).solarNoon }).elevation, c = th.clearSky(d, el); o[d] = { el, dni: c.dni, dhi: c.dhi }; } return o; });
  check(sky["2026-06-21"].dni > 850 && sky["2026-06-21"].dni < 950 && sky["2026-12-21"].dni > 500 && sky["2026-12-21"].dni < 760, `clear-sky direct normal irradiance at solar noon: ${Math.round(sky["2026-06-21"].dni)} W/m2 in June, ${Math.round(sky["2026-12-21"].dni)} in December`);
  const day = async (a) => { const x = await call("run_thermal_day", a); return x.v || { err: x.err }; };
  const june = await day({ date: "2026-06-21", clearness: 1, heating: true });
  const dec = await day({ date: "2026-12-21", clearness: 1, heating: true });
  const finite = (d) => d.rooms.every((x) => [x.minC, x.maxC, x.solarGainKWh, x.heatingKWh, x.swingK].every(Number.isFinite));
  const juneOff = await day({ date: "2026-06-21", clearness: 1, heating: true, blinds: false, summerBypass: false, nightVentilation: false });
  const decOff = await day({ date: "2026-12-21", clearness: 1, heating: true, blinds: false, summerBypass: false, nightVentilation: false });
  const hi = (d) => Math.max(...d.rooms.map((x) => x.maxC));
  check(june.rooms && finite(june) && june.rooms.every((x) => x.minC > 18 && x.maxC < 30 && x.swingK < 6) && hi(june) >= 24 && hi(june) <= 28, "June with windows shut and the bypass, night ventilation and blinds on: the peak is 24 to 28 C: " + JSON.stringify(june.rooms && june.rooms.map((x) => [x.room, x.minC, x.maxC])));
  check(finite(juneOff) && hi(juneOff) > 30 && hi(juneOff) - hi(june) > 5, "with the three off the house is as hot as before: " + hi(juneOff) + " C against " + hi(june));
  check(Math.abs(hi(dec) - hi(decOff)) < 0.01 && Math.abs(dec.totals.heatingKWh - decOff.totals.heatingKWh) < 0.01, "December is the same with the summer measures on or off: " + dec.totals.heatingKWh + " kWh");
  await day({ date: "2026-06-21", clearness: 1 });   // the defaults again: the panel shows them
  check(june.options && june.options.summerBypass === true && june.options.nightVentilation === true && june.options.blinds === true && await ev(() => ["thBypass", "thNightVent", "thBlinds"].every((id) => document.getElementById(id) && document.getElementById(id).checked)), "the three options are in the Thermal panel, on by default: " + JSON.stringify([june.options, await ev(() => ["thBypass", "thNightVent", "thBlinds"].map((id) => document.getElementById(id) && document.getElementById(id).checked))]));
  check(dec.rooms && finite(dec) && dec.rooms.every((x) => x.minC > 15 && x.maxC < 28 && x.swingK < 4), "December temperatures are held near the set points: " + JSON.stringify(dec.rooms && dec.rooms.map((x) => [x.room, x.minC, x.maxC])));
  check(june.totals.solarKWh > dec.totals.solarKWh * 1.5 && june.totals.solarKWh > 15 && dec.totals.solarKWh > 3, `June solar gain through the windows exceeds December: ${june.totals.solarKWh} vs ${dec.totals.solarKWh} kWh`);
  check(dec.totals.heatingKWh > 15 && dec.totals.heatingKWh < 90 && june.totals.heatingKWh < 1, `winter heating energy per day is plausible: ${dec.totals.heatingKWh} kWh in December, ${june.totals.heatingKWh} in June`);
  check(june.runMs < 2000 && dec.runMs < 2000 && june.settled && dec.settled, `the thermal day runs in well under two seconds: ${june.runMs} ms, ${dec.runMs} ms`);
  const design = await day({ date: "2026-12-21", clearness: 0, outdoorMinC: -12, outdoorMaxC: -12, heating: true });
  const dkw = design.totals.heatingKWh / 24;
  check(dkw > 3.2 && dkw < 6.5, `at the design outdoor temperature of -12 C the heating needs ${dkw.toFixed(2)} kW on average (design load 4.82 kW, less the internal gains)`);
  check(dec.rooms.some((x) => x.sunOnRoom === "morning") && dec.rooms.some((x) => x.sunOnRoom === "afternoon"), "some rooms get the morning sun, others the afternoon sun: " + dec.rooms.map((x) => x.room + ":" + x.sunOnRoom).join(" "));
  const zs = await ev(async () => { const A = window.reteBuilding; await A.scenery.set({ on: true, treesShade: false }); const th = A.thermal; const w = th.windows[0]; th.invalidate(); const a = th.sunlit(w, A.sun.position({ date: "2026-06-21", time: 12 }).direction); const t = A.scenery.shadeTargets(); return { a, targets: t.length }; });
  check(zs.targets === 0, "the trees are not in the thermal ray casts unless 'trees shade the house' is on: " + JSON.stringify(zs));
  const ws = await call("get_window_sun", { time: "12:00", date: "2026-06-21" }), wn = await call("get_window_sun", { time: "01:00", date: "2026-06-21" });
  check(ws.v && ws.v.windows.length === 11 && ws.v.windows.some((w) => w.sunlitFraction > 0.5 && w.irradianceWm2 > 300) && wn.v.windows.every((w) => w.sunlitFraction === 0 && w.transmittedW === 0), "get_window_sun: lit at noon, dark at night");
  r = await call("get_window_sun", { window: "EG-Fenster-5", time: 11, date: "2026-06-21" });
  check(r.v && r.v.windows.length === 1 && r.v.windows[0].label === "EG-Fenster-5", "get_window_sun for one window");
  r = await call("get_window_sun", { window: "nope" });
  check(!!r.err, "get_window_sun refuses an unknown window");
  // what-ifs: true north, windows, ventilation, heat pump
  const peak = (d) => d.rooms.reduce((a, b) => (b.maxC > a.maxC ? b : a)).room;
  const baseJun = june, gainOf = (d) => d.rooms.map((x) => x.solarGainKWh);
  await call("set_site", { northAzimuthDeg: 230 });
  const turned = await day({ date: "2026-06-21", clearness: 1 }), decTurned = await day({ date: "2026-12-21", clearness: 1 });
  const moved = gainOf(dec).some((g, i) => Math.abs(g - gainOf(decTurned)[i]) > 0.25 * Math.max(g, 1)) || peak(dec) !== peak(decTurned);
  const sz = (d) => d.rooms.find((x) => x.room === "Schlafzimmer").solarGainKWh;
  check(moved && sz(decTurned) > 3 * sz(dec), `turning true north by 180 degrees moves the winter gains between rooms (the bedroom gets the sun): ${sz(dec)} -> ${sz(decTurned)} kWh, peak ${peak(dec)} -> ${peak(decTurned)}, ${JSON.stringify([gainOf(dec), gainOf(decTurned)])}`);
  check(turned.rooms.find((x) => x.room === "Wohnen").sunOnRoom !== baseJun.rooms.find((x) => x.room === "Wohnen").sunOnRoom, "...and the living room's sun moves from morning to afternoon: " + baseJun.rooms.find((x) => x.room === "Wohnen").sunOnRoom + " -> " + turned.rooms.find((x) => x.room === "Wohnen").sunOnRoom);
  await call("set_site", { northAzimuthDeg: 50 });
  const back = await day({ date: "2026-06-21", clearness: 1 });
  check(Math.abs(back.totals.solarKWh - june.totals.solarKWh) < 0.01, "north back at 50 degrees gives the first result again");
  await ev(() => { const a = window.reteBuilding; a.openables().forEach((o) => { if (o.kind === "window" && a.states(o.guid).includes("open")) a.setState(o.guid, "open", { instant: true }); }); });
  const opened = await day({ date: "2026-06-21", clearness: 1 });
  check(Math.max(...opened.rooms.map((x) => x.maxC)) < Math.max(...june.rooms.map((x) => x.maxC)) - 1.0, `open windows cool the rooms in June: ${Math.max(...june.rooms.map((x) => x.maxC))} -> ${Math.max(...opened.rooms.map((x) => x.maxC))}`);
  await call("reset_states");
  await call("set_device_state", { id: "hrv-unit", state: "off" });
  const noHrv = await day({ date: "2026-12-21", clearness: 1 });
  check(noHrv.ventilationHeatRecovery === false && noHrv.totals.heatingKWh > dec.totals.heatingKWh + 1, `closing the ventilation unit costs heat in winter: ${dec.totals.heatingKWh} -> ${noHrv.totals.heatingKWh} kWh`);
  await call("reset_states");
  await call("set_device_state", { id: "hp-outdoor", state: "off" });
  const noHp = await day({ date: "2026-12-21", clearness: 1 });
  check(noHp.totals.heatingKWh === 0 && Math.min(...noHp.rooms.map((x) => x.minC)) < Math.min(...dec.rooms.map((x) => x.minC)) - 1, `heat pump off in winter: no heating, rooms cool: ${Math.min(...dec.rooms.map((x) => x.minC))} -> ${Math.min(...noHp.rooms.map((x) => x.minC))}`);
  await call("reset_states");
  r = await call("run_thermal_day", { date: "2026-13-45" });
  check(!!r.err, "run_thermal_day refuses a bad date");

  // ---- the panel, the views, the export
  await day({ date: "2026-06-21", clearness: 1 });
  const ui = await ev(async () => {
    const a = window.reteBuilding, th = a.thermal;
    const o = { panel: !document.getElementById("thermalPanel").hidden, rows: document.querySelectorAll("#thTable tr[data-g]").length, canvas: document.getElementById("thChart").width > 100 };
    a.thermalUi.setSunView(true); a.sun.set({ time: 9 }); const m = a.thermalUi.sunFractions(); const morning = Object.values(m).filter((v) => v > 0).length;
    a.sun.set({ time: 17 }); const e = a.thermalUi.sunFractions(); const evening = Object.values(e).filter((v) => v > 0).length;
    o.sunViews = [morning, evening, JSON.stringify(Object.keys(m).filter((k) => m[k] > 0).sort()) !== JSON.stringify(Object.keys(e).filter((k) => e[k] > 0).sort())];
    a.thermalUi.setSunView(false);
    a.thermalUi.setTempView(true); a.sun.set({ time: 15 }); const c1 = a.thermalUi.view; o.temp = c1.temps;
    const col = (guid) => { let c = null; a.scene.traverse((x) => { if (x.userData && x.userData.room === guid && x.isMesh) c = x.material.color.getHexString(); }); return c; };
    o.tint = [...new Set(th.zones.map((z) => col(z.guid)))].length;
    a.thermalUi.setTempView(false);
    o.csv = th.csv().split("\n").length; o.csvHead = th.csv().split("\n")[0].slice(0, 60);
    return o;
  });
  check(ui.panel && ui.rows === 7 && ui.canvas, "the thermal panel shows a chart and a table row per room");
  check(ui.sunViews[0] > 0 && ui.sunViews[1] > 0 && ui.sunViews[2], "sun on windows: different windows are lit in the morning and the afternoon: " + JSON.stringify(ui.sunViews));
  check(ui.temp && ui.tint >= 3, "room temperatures tint the floors in 3D: " + ui.tint + " colours");
  check(ui.csv === 146 && /^time,hour,outdoor_C/.test(ui.csvHead), "CSV export has a row per step: " + ui.csv);
  r = await call("run_sparql", { query: "PREFIX sys: <https://w3id.org/rete/cad/sys#> PREFIX brick: <https://brickschema.org/schema/Brick#> SELECT (COUNT(?z) AS ?n) WHERE { ?z a sys:Zone }" });
  check(r.v && /^"7"/.test(r.v.rows[0].n), "run_sparql sees the combined graph (7 zones): " + JSON.stringify(r.v && r.v.rows));
  r = await call("list_examples");
  check(r.v && r.v.examples.length === 14 && /brick/.test(r.v.prefixes) && r.v.graph && r.v.graph.includes(NAME), "list_examples lists the second group and names the combined graph: " + (r.v && r.v.examples.length));
  const ex = await ev(async () => {
    const out = {};
    for (const k of [6, 7, 8, 9, 10, 11, 12, 13]) {
      document.querySelectorAll(".qbtn")[k].click();
      const label = window.reteBuilding.examples[k].label;
      await new Promise((res) => { let n = 0; const iv = setInterval(() => { n++; if (document.getElementById("footer-title").textContent === label && window.reteBuilding.results.get() && window.reteBuilding.results.get().label === label || n > 300) { clearInterval(iv); res(); } }, 100); });
      out[label] = [document.querySelectorAll("#results tbody tr").length, (window.reteBuilding.resultPicks || []).length, window.reteBuilding.svc.nodes.filter((t) => t.tint != null).length, window.reteBuilding.lastQueryInfo().matched];
    }
    return out;
  });
  const exv = Object.values(ex);
  check(exv.length === 8 && exv.every((v) => v[0] > 0) && ex["Sockets per room"][0] === 7 && ex["What the kitchen breaker feeds"][1] === 8 && ex["What the kitchen breaker feeds"][2] >= 8 && ex["Load per circuit against its breaker"][1] === 18 && ex["Fixtures that drain to the sewer"][1] === 6 && ex["South-facing windows: area and g-value"][3] === 3, "the services queries run on the graph and highlight in 3D: " + JSON.stringify(ex));

  check(errs.length === 0, "no page errors: " + errs.join(" | "));
  check(bad.length === 0, "no failed requests: " + bad.join(" | "));
  const conErr = cons.filter((c) => !/Failed to load resource/.test(c));
  check(conErr.length === 0, "no console errors: " + conErr.join(" | "));

  // ---- phone width
  const phone = await browser.newContext({ viewport: { width: 390, height: 800 }, hasTouch: true });
  const pp = await phone.newPage(); const perr = [];
  pp.on("pageerror", (e) => perr.push(String(e).slice(0, 200)));
  if (DIR) {
    await pp.route(`**/cad/${NAME}.json`, async (rt) => rt.fulfill({ status: 200, contentType: "application/json", headers: { "access-control-allow-origin": "*" }, body: await readFile(`${DIR}/${NAME}.json`) }));
    await pp.route(`**/cad/${NAME}.rete`, async (rt) => rt.fulfill({ status: 200, contentType: "application/octet-stream", headers: { "access-control-allow-origin": "*" }, body: await readFile(`${DIR}/${NAME}.rete`) }));
  }
  await pp.goto(`http://localhost:${PORT}/building.html`, { waitUntil: "load" });
  await pp.waitForFunction(() => window.reteBuilding && window.reteBuilding.ready, null, { timeout: 150000 });
  const ph = await pp.evaluate(async () => {
    const a = window.reteBuilding; await a.loadThermal(); await a.thermalUi.runAndShow({});
    const p = document.getElementById("thermalPanel").getBoundingClientRect();
    return { sw: document.documentElement.scrollWidth, iw: innerWidth, panelW: Math.round(p.width), panelRight: Math.round(p.right), layers: document.querySelectorAll("#layers .sys").length, quality: a.getQuality() };
  });
  check(ph.sw <= ph.iw && ph.panelRight <= ph.iw + 1 && ph.layers >= 10 && perr.length === 0, "phone width: " + JSON.stringify(ph) + perr.join("|"));

  const pass = fails.length === 0;
  console.log(JSON.stringify({ verdict: pass ? "PASS" : "FAIL", nodes: data.nodes, runs: data.runs, meshes: geo.meshes, shiftedFixtures: geo.moved, thermal: { juneSolarKWh: june.totals.solarKWh, decSolarKWh: dec.totals.solarKWh, decHeatingKWh: dec.totals.heatingKWh, designKW: +dkw.toFixed(2), dniJune: Math.round(sky["2026-06-21"].dni), runMs: [june.runMs, dec.runMs] }, fails }, null, 2));
  await browser.close();
  process.exit(pass ? 0 : 1);
};
main();
