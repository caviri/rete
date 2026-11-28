// The viewport pass of the FZK-Haus viewer (docs/building/viewport.js, section.js, fly.js, ground.js, structure.js, statevars.js, reset.js,
// webmcp-viewport.js): the four-view layout, section cuts, camera modes, the structure layer, the state-variable store, reset, and the
// the WebMCP tools of the viewport pass. It drives only the app object (window.reteBuilding) and the tool array, never the page's markup.
// Needs the network for the house (geometry, index, graph come from data.graphplaza.com) and the two systems files:
// with FZK_SYSTEMS_DIR set, the two R2 addresses are answered from that directory, otherwise the live R2 files are used.
// Usage: FZK_SYSTEMS_DIR=D:/.../data/cad/systems node check_building_viewport.mjs
import { launchBrowser } from "./_browser.mjs";
import { readFile } from "node:fs/promises";

const PORT = process.env.PGPORT || "8090";
const DIR = process.env.FZK_SYSTEMS_DIR || "";
const NAME = "fzk-haus-systems-2026-11-28";
const fails = [];
const check = (ok, what) => { if (!ok) fails.push(what); };
const ARGS = ["--no-sandbox", "--use-gl=angle", "--use-angle=swiftshader", "--enable-unsafe-swiftshader", "--ignore-gpu-blocklist"];

async function open(browser, { width, height, systems = true }) {
  const ctx = await browser.newContext({ viewport: { width, height } });
  const page = await ctx.newPage();
  const errs = [];
  page.on("pageerror", (e) => errs.push(String(e).slice(0, 200)));
  page.on("console", (m) => { if (m.type() === "error") errs.push(m.text().slice(0, 200)); });
  if (!systems) {
    await page.route(`**/cad/${NAME}.json`, (r) => r.fulfill({ status: 404, body: "not found" }));
    await page.route(`**/cad/${NAME}.rete`, (r) => r.fulfill({ status: 404, body: "not found" }));
  } else if (DIR) {
    await page.route(`**/cad/${NAME}.json`, async (r) => r.fulfill({ status: 200, contentType: "application/json", headers: { "access-control-allow-origin": "*" }, body: await readFile(`${DIR}/${NAME}.json`) }));
    await page.route(`**/cad/${NAME}.rete`, async (r) => r.fulfill({ status: 200, contentType: "application/octet-stream", headers: { "access-control-allow-origin": "*" }, body: await readFile(`${DIR}/${NAME}.rete`) }));
  }
  await page.goto(`http://localhost:${PORT}/building.html`, { waitUntil: "load" });
  await page.waitForFunction(() => window.reteBuilding && window.reteBuilding.ready, null, { timeout: 150000 });
  // no animation: the idle test and the pixel tests need a still picture
  await page.evaluate(async () => {
    const a = window.reteBuilding; if (a.scenery) a.scenery.set({ animate: false }); a.sun.playDay(false);
    const T = (await import("/building/webmcp-viewport.js")).viewportTools(a);
    window.__tools = Object.fromEntries(T.map((t) => [t.name, t]));
  });
  return { ctx, page, errs };
}
// the mean colour of a rectangle of the canvas (CSS px of the viewport), or the whole picture hash
const PIX = `(r) => { const a = window.reteBuilding, c = a.renderer.domElement; a.renderNow(); const k = document.createElement("canvas"); k.width = c.width; k.height = c.height; const g = k.getContext("2d"); g.drawImage(c, 0, 0); const s = c.width / c.clientWidth; const d = g.getImageData(Math.round(r[0] * s), Math.round(r[1] * s), Math.max(1, Math.round(r[2] * s)), Math.max(1, Math.round(r[3] * s))).data; let R = 0, G = 0, B = 0, n = d.length / 4; const set = new Set(); for (let i = 0; i < d.length; i += 4) { R += d[i]; G += d[i + 1]; B += d[i + 2]; set.add((d[i] >> 4) * 256 + (d[i + 1] >> 4) * 16 + (d[i + 2] >> 4)); } return { r: R / n, g: G / n, b: B / n, colours: set.size }; }`;

const main = async () => {
  const browser = await launchBrowser({ args: ARGS });
  const { page, errs } = await open(browser, { width: 1300, height: 850 });
  const ev = (f, a) => page.evaluate(f, a);
  const pix = (r) => ev(eval(PIX), r);
  const call = (name, args) => page.evaluate(([n, a]) => window.__tools[n].execute(a || {}).then((v) => ({ v, bytes: JSON.stringify(v).length }), (e) => ({ err: String(e.message || e) })), [name, args || {}]);
  const A = "window.reteBuilding";
  check(await ev(() => !!(window.reteBuilding.svc && window.reteBuilding.statevars && window.reteBuilding.structure && window.reteBuilding.viewport && window.reteBuilding.section)), "the services layer, state variables, structure, viewport and section loaded");

  // ---- the tools
  const names = ["set_layout", "get_viewport", "set_section", "set_camera_mode", "set_ground_cutaway", "list_structure", "trace_load_path", "remove_structural_element", "restore_structure", "list_state_variables", "get_value", "set_value", "colour_by", "reset"];
  const shape = await ev(() => Object.values(window.__tools).map((t) => [t.name, typeof t.description === "string" && t.description.length > 30, t.inputSchema && t.inputSchema.type === "object", typeof t.execute === "function"]));
  check(shape.length === names.length && shape.every((s) => names.includes(s[0])) && names.every((n) => shape.some((s) => s[0] === n && s[1] && s[2] && s[3])), "the viewport tools (the viewport contract lists " + names.length + ") each with name, description, inputSchema and execute: " + shape.map((s) => s[0]).join());

  // ---- fresh-load snapshot (for the reset check at the end)
  const snap = () => ev(() => {
    const a = window.reteBuilding; a.shadowDirty(); a.renderNow();
    return { states: a.allStates(), vis: a.getVisibility(), layout: a.viewport.getLayout(), max: a.viewport.getMaximised(), section: a.section.get(), cutaway: a.ground.getCutaway(), mode: a.mode, xray: a.svc.isXray(), iso: a.svc.isolated, sun: [a.sun.get().date, +a.sun.get().time.toFixed(4)],
      quality: a.getQuality(), cam: [...a.camera.position.toArray().map((v) => +v.toFixed(3)), ...a.controls.target.toArray().map((v) => +v.toFixed(3))], scol: a.structure.getColourMode(), colouring: a.statevars.colouring(), removed: a.structure.state().removed, pinned: a.statevars.pinned(),
      colourMode: a.getColourMode(), calls: a.renderer.info.render.calls, tris: a.renderer.info.render.triangles, lights: a.getInteriorLights(), live: a.svc.nodes.filter((t) => !t.live).length, selected: a.selectedInfo() };
  });
  const fresh = await snap();
  check(fresh.layout === "single" && fresh.live === 0 && fresh.mode === "orbit", "fresh state: single layout, nothing without supply, orbit camera");
  await page.waitForTimeout(800); const idleA = await ev(() => window.reteBuilding.frameCount); await page.waitForTimeout(1500);
  check(await ev(() => window.reteBuilding.frameCount) === idleA, "zero renders while idle (single layout)");
  const singleCalls = await ev(() => { const a = window.reteBuilding; a.viewport.invalidate(); a.renderNow(); return a.viewport.stats().lastFrame; });
  check(singleCalls.calls > 0 && singleCalls.calls <= 90 && singleCalls.panes === 1, "single layout draws in few calls: " + JSON.stringify(singleCalls));

  // ---- quad layout
  const q = await ev(() => { const a = window.reteBuilding; a.viewport.setLayout("quad"); a.renderNow(); return { panes: a.viewport.panes(), layout: a.viewport.getLayout(), stats: a.viewport.stats(), sec: a.section.get() }; });
  check(q.layout === "quad" && q.panes.map((p) => p.id).sort().join() === "3d,front,plan,side", "quad has four panes: " + q.panes.map((p) => p.id));
  const byId = Object.fromEntries(q.panes.map((p) => [p.id, p]));
  check(["plan", "front", "side"].every((id) => byId[id].projection === "orthographic") && byId["3d"].projection === "perspective", "the three drawings are orthographic, the 3D pane perspective");
  check(byId.front.facade && byId.front.facade.name === "south-east" && byId.side.facade && byId.side.facade.name === "north-east", "the elevations say which façade they show: " + JSON.stringify([byId.front.facade, byId.side.facade]));
  check(q.sec.z.enabled === true && Math.abs(q.sec.z.position - 1.2) < 1e-6 && q.sec.applyTo3D === false, "quad: plan cut 1.2 m above the ground floor, the 3D pane not cut: " + JSON.stringify(q.sec.z));
  const rects = q.panes.map((p) => p.rect);
  check(rects.every((r) => r.w > 100 && r.h > 100) && rects.every((r, i) => rects.every((s, j) => i === j || r.x + r.w <= s.x || s.x + s.w <= r.x || r.y + r.h <= s.y || s.y + s.h <= r.y)), "the four panes do not overlap");
  const qCalls = await ev(() => { const a = window.reteBuilding; a.viewport.invalidate(); a.renderNow(); return a.viewport.stats().lastFrame; });
  check(qCalls.panes === 4 && qCalls.calls <= 5 * 90, "quad draws four panes in bounded calls: " + JSON.stringify(qCalls));
  const px = await Promise.all(q.panes.map((p) => pix([p.rect.x + 4, p.rect.y + 4, p.rect.w - 8, p.rect.h - 8])));
  const key = (c) => [c.r, c.g, c.b].map((v) => Math.round(v / 6)).join("/");
  check(new Set(px.map(key)).size === 4 && px.every((c) => c.colours > 12), "the four panes show different content: " + px.map(key).join(" "));
  // scale: the front elevation at 40 px per metre; the wall row spans the wall length * 40 px
  const scale = await ev(async () => {
    const a = window.reteBuilding, V = a.viewport; a.viewport.maximise("front"); V.setView("front", { centre: [6, 1.5], pixelsPerMetre: 40 }); a.renderNow();
    const walls = [...a.nodeByGuid].filter(([, n]) => n.c === "IfcWallStandardCase").map(([g]) => a.meshByGuid.get(g));
    const THREE = await import("three"); const b = new THREE.Box3(); walls.forEach((w) => b.expandByObject(w));
    const p0 = V.project("front", [b.min.x, 0, 1.5]), p1 = V.project("front", [b.max.x, 0, 1.5]), pane = V.pane("front");
    const c = a.renderer.domElement, k = document.createElement("canvas"); k.width = c.width; k.height = c.height; const g = k.getContext("2d"); g.drawImage(c, 0, 0);
    const y = Math.round(p0.y), row = g.getImageData(0, y, c.width, 1).data; let lo = -1, hi = -1; const bg = [0xe8, 0xeb, 0xef];
    for (let x = 0; x < c.width; x++) { const o = x * 4, isBg = Math.abs(row[o] - bg[0]) < 6 && Math.abs(row[o + 1] - bg[1]) < 6 && Math.abs(row[o + 2] - bg[2]) < 6; if (!isBg) { if (lo < 0) lo = x; hi = x; } }
    const sec = a.section.get(); a.viewport.maximise(null);
    return { lenM: b.max.x - b.min.x, projPx: p1.x - p0.x, pixelsPerMetre: pane.view.pixelsPerMetre, drawnPx: hi - lo + 1, projLeft: p0.x, lo };
  });
  check(Math.abs(scale.projPx - scale.lenM * 40) < 0.5 && scale.pixelsPerMetre === 40, "the front pane projects a wall of " + scale.lenM.toFixed(2) + " m to " + scale.projPx.toFixed(1) + " px at 40 px/m");
  check(Math.abs(scale.drawnPx - scale.lenM * 40) < 4, "and draws it that wide (" + scale.drawnPx + " px for " + (scale.lenM * 40).toFixed(1) + ")");

  // ---- picking in every pane
  const picks = await ev(async () => {
    const a = window.reteBuilding, V = a.viewport, THREE = await import("three"), out = {};
    V.fit(); a.renderNow(); const sr = a.renderer.domElement.parentElement.getBoundingClientRect();
    const wall = [...a.nodeByGuid].find(([, n]) => n.c === "IfcWallStandardCase" && /^Wand-Ext/.test(n.l))[0], box = new THREE.Box3().setFromObject(a.meshByGuid.get(wall)), c = box.getCenter(new THREE.Vector3()), s = box.getSize(new THREE.Vector3());
    // a point on the wall seen from each pane: sample along the wall, the first hit that is the wall counts
    const tryPane = (id, pts) => { for (const w of pts) { const p = V.project(id, w), h = V.pick(sr.left + p.x, sr.top + p.y); if (h && h.pane === id && h.id === wall) return true; } return false; };
    const along = (n) => Array.from({ length: n }, (_, i) => (i + 0.5) / n);
    const horiz = s.x > s.y, pts = (z) => along(12).map((u) => horiz ? [box.min.x + u * s.x, c.y, z] : [c.x, box.min.y + u * s.y, z]);
    out.plan = tryPane("plan", pts(0.5));   // the cut wall: its cut face is what is seen
    out.cut = V.pane("plan").cut;
    out.wall = a.nodeByGuid.get(wall).l; out.horiz = horiz;
    const ids = []; for (const id of ["plan", "front", "side", "3d"]) { const q = V.pane(id).rect; const h = V.pick(sr.left + q.x + q.w / 2, sr.top + q.y + q.h / 2); ids.push([id, h && h.pane]); }
    out.centre = ids;
    // selecting in one pane is seen in the other: select with a pick in the plan
    const pp = V.project("plan", [c.x, c.y, 0.5]); const h = V.pick(sr.left + pp.x, sr.top + pp.y); if (h) a.select(h.id, { focus: false }); out.selected = a.selectedInfo() && a.selectedInfo().id; out.hit = h && h.id; a.clearSelection();
    return out;
  });
  check(picks.plan, "a click on the cut wall in the plan picks the wall: " + JSON.stringify([picks.wall, picks.plan]));
  check(picks.centre.every((x) => x[1] === x[0]), "pick reports the pane under the pointer: " + JSON.stringify(picks.centre));
  const pickFront = await ev(async () => {
    const a = window.reteBuilding, V = a.viewport, THREE = await import("three"); V.fit(); a.renderNow(); const sr = a.renderer.domElement.parentElement.getBoundingClientRect();
    const res = {}; const find = (cls, f) => [...a.nodeByGuid].filter(([, n]) => n.c === cls && (!f || f(n)));
    // the front elevation looks at the south face: the south window "EG-Fenster" and the pane's own point of it
    for (const id of ["front", "side", "3d"]) {
      let found = 0, tried = 0;
      for (const [g, n] of find("IfcWindow")) { const bx = new THREE.Box3().setFromObject(a.meshByGuid.get(g)), c = bx.getCenter(new THREE.Vector3()); const p = V.project(id, id === "3d" ? [c.x, c.y, c.z] : [c.x, c.y, c.z]); tried++; const h = V.pick(sr.left + p.x, sr.top + p.y); if (h && h.pane === id && h.id === g) found++; }
      res[id] = [found, tried];
    }
    return res;
  });
  check(pickFront.front[0] >= 1 && pickFront.side[0] >= 1 && pickFront["3d"][0] >= 1, "windows are picked in the front, side and 3D panes (found/tried): " + JSON.stringify(pickFront));
  // pan and zoom change only that pane
  const pz = await ev(() => { const a = window.reteBuilding, V = a.viewport; V.fit(); a.renderNow(); const b = V.pane("front").view, f0 = V.stats().drawn; V.setView("plan", { centre: [3, 3], pixelsPerMetre: 55 }); a.renderNow(); const d1 = V.stats().drawn; const pl = V.pane("plan").view, fr = V.pane("front").view; return { plan: pl, frontSame: JSON.stringify(fr) === JSON.stringify(b), drawn: ["plan", "front", "side", "3d"].map((k) => d1[k] - f0[k]) }; });
  check(pz.plan.pixelsPerMetre === 55 && pz.plan.centre[0] === 3 && pz.frontSame, "pan and zoom of the plan leave the front pane's view alone");
  await page.waitForTimeout(600);
  const s0 = await ev(() => window.reteBuilding.viewport.stats().drawn); await ev(() => window.reteBuilding.viewport.setView("plan", { pixelsPerMetre: 60 })); await page.waitForTimeout(700);
  const s1 = await ev(() => window.reteBuilding.viewport.stats().drawn); const dr = ["plan", "front", "side", "3d"].map((k) => s1[k] - s0[k]);
  check(dr[0] === 1 && dr[1] === 0 && dr[2] === 0 && dr[3] === 0, "a zoom of the plan redraws only the plan pane (plan, front, side, 3d): " + dr.join());
  await page.waitForTimeout(800); const idleQ = await ev(() => window.reteBuilding.frameCount); await page.waitForTimeout(1500);
  check(await ev(() => window.reteBuilding.frameCount) === idleQ, "zero renders while idle (quad layout)");
  // maximise
  const mx = await ev(() => { const a = window.reteBuilding, V = a.viewport; V.maximise("plan"); a.renderNow(); const m = V.panes(), st = a.renderer.domElement.parentElement, g = V.getMaximised(); V.maximise(null); return [m.length, m[0].rect.w === st.clientWidth && m[0].rect.h === st.clientHeight, g, V.getMaximised()]; });
  check(mx[0] === 1 && mx[1] && mx[2] === "plan" && mx[3] === null, "maximise gives one pane the whole viewport and null gives the grid back");
  // true north up on the plan
  const nu = await ev(() => { const a = window.reteBuilding, V = a.viewport; V.setNorthUp(true); a.renderNow(); const p = V.pane("plan"); const r = [V.project("plan", [6, 5, 0]), V.project("plan", [6 + 0.766, 5 + 0.6428, 0])]; V.setNorthUp(false); return { up: p.northUp, dx: +(r[1].x - r[0].x).toFixed(2), dy: +(r[1].y - r[0].y).toFixed(2) }; });
  check(nu.up === true && Math.abs(nu.dx) < 0.01 && nu.dy < 0, "with north up, the true north direction points up the plan: " + JSON.stringify(nu));

  // ---- section cuts
  const cut = await ev(async () => {
    const a = window.reteBuilding, V = a.viewport, S = a.section, THREE = await import("three"); V.setLayout("quad"); a.renderNow();
    const roofs = [...a.nodeByGuid].filter(([, n]) => n.c === "IfcSlab" && /^Dach/.test(n.l)).map(([g]) => g), out = {};
    const sr = a.renderer.domElement.parentElement.getBoundingClientRect(), bb = new THREE.Box3(); roofs.forEach((g) => bb.expandByObject(a.meshByGuid.get(g)));
    const c = bb.getCenter(new THREE.Vector3()), p = V.project("plan", [c.x, c.y, bb.max.z]);
    const h = V.pick(sr.left + p.x, sr.top + p.y); out.planPickNotRoof = !!h && !roofs.includes(h.id); out.planPick = h && a.nodeByGuid.get(h.id).l;
    // the same point seen in the front elevation sees the roof (y cut off)
    const pf = V.project("front", [c.x, c.y, bb.max.z - 0.4]), hf = V.pick(sr.left + pf.x, sr.top + pf.y); out.frontPickRoof = !!hf && roofs.includes(hf.id);
    // 3D: cut on, picking from above ignores the cut roof
    V.setLayout("single"); S.set("z", { enabled: false }); a.camera.position.set(c.x + 0.01, c.y - 12, 16); a.controls.target.set(c.x, c.y, 1); a.camera.lookAt(c.x, c.y, 1); a.camera.updateMatrixWorld(true); a.renderNow();
    const pr = V.project("3d", [c.x, c.y, bb.max.z - 0.4]), hr = V.pick(sr.left + pr.x, sr.top + pr.y); out.uncutPick = hr && a.nodeByGuid.get(hr.id) ? a.nodeByGuid.get(hr.id).l : null; out.uncutRoof = !!hr && roofs.includes(hr.id);
    S.set("z", { enabled: true, position: 1.4 }); a.renderNow(); const hc = V.pick(sr.left + pr.x, sr.top + pr.y); out.cutPick = hc && a.nodeByGuid.get(hc.id) ? a.nodeByGuid.get(hc.id).l : null; out.cutRoof = !!hc && roofs.includes(hc.id);
    out.applyTo3D = S.get().applyTo3D; out.clipped = S.clipped([c.x, c.y, 3]) && !S.clipped([c.x, c.y, 1]);
    return out;
  });
  check(cut.planPickNotRoof, "the plan pane (z cut) does not pick the roof above the cut: " + cut.planPick);
  check(cut.frontPickRoof, "the front elevation (no cut) picks the roof");
  check(cut.uncutRoof && !cut.cutRoof && cut.applyTo3D === true && cut.clipped, "in the 3D pane a z cut hides the roof from picking (" + cut.uncutPick + " -> " + cut.cutPick + ")");
  // pixels: the cut changes the 3D picture (the roof is red-brown, the cut house is plaster and floors)
  await ev(() => { const a = window.reteBuilding; a.section.set("z", { enabled: false }); a.camera.position.set(16, -14, 12); a.controls.target.set(6, 5, 1); a.camera.lookAt(6, 5, 1); a.camera.updateMatrixWorld(true); });
  const rA = await pix([300, 150, 650, 450]);
  await ev(() => window.reteBuilding.section.set("z", { enabled: true, position: 1.4 }));
  const rB = await pix([300, 150, 650, 450]);
  check(rA.r > rB.r + 3 && Math.abs(rA.g - rB.g) + Math.abs(rA.r - rB.r) > 6, "a 3D cut changes the picture (the red roof is gone): " + [rA.r, rB.r].map((v) => v.toFixed(0)));
  // an embedded lintel shows where the cut passes through it, not elsewhere; cut faces are drawn (the walls are not hollow)
  const emb = await ev(() => {
    const a = window.reteBuilding, S = a.section, svc = a.svc, t = svc.thingFor("lintel-haustuer"), out = {}; S.reset(); a.viewport.setLayout("single"); a.renderNow();
    svc.layers.get("structure").setVisible(true); out.hiddenAtFirst = t.obj.visible === false; S.set("z", { enabled: true, position: 1.2 }); out.hiddenAt12 = t.obj.visible === false;
    S.set("z", { position: 2.1 }); out.shownAt21 = t.obj.visible === true; a.renderNow(); out.vis = t.meshes.map((m) => m.userData.el.vis);
    S.set("z", { enabled: false }); svc.setXray(true); out.xray = t.obj.visible === true; svc.setXray(false); out.after = t.obj.visible === false;
    a.renderNow(); const cap = []; for (const set of a.batchSetList) for (const b of set.batches.values()) cap.push(b.mats.opaque.userData.cap.value); out.capOff = cap.every((v) => v === 0);
    return out;
  });
  check(emb.hiddenAtFirst && emb.hiddenAt12 && emb.shownAt21 && emb.vis.every(Boolean), "the embedded lintel over the front door is hidden until a cut passes through it: " + JSON.stringify(emb));
  check(emb.xray && emb.after, "x-ray shows embedded parts, switching it off hides them again");
  // sections: x and y, flip, storey API, validation
  const sx = await ev(() => { const S = window.reteBuilding.section; const r = {}; r.set = S.set("y", { enabled: true, position: 3.1, flip: true }); r.storeys = S.storeys().length; r.z = S.setStorey(1); try { S.set("w", {}); } catch (e) { r.err = e.message; } try { S.setStorey(9); } catch (e) { r.err2 = e.message; } S.reset(); r.after = S.get(); return r; });
  check(sx.set.flip === true && sx.set.position === 3.1 && sx.storeys === 2 && Math.abs(sx.z - 3.9) < 1e-6 && /axis/.test(sx.err) && /storey/.test(sx.err2) && !sx.after.y.enabled && !sx.after.z.enabled, "section set, storey, errors and reset: " + JSON.stringify([sx.z, sx.err, sx.err2]));

  // ---- camera modes
  const modes = await ev(async () => {
    const a = window.reteBuilding, C = a.camera, out = {}, seen = []; a.on("mode", (e) => seen.push(e.mode));
    out.start = C.getMode(); a.camera.position.set(14, -10, 8); a.controls.target.set(6, 5, 2); a.camera.lookAt(6, 5, 2); const p0 = C.position.toArray();
    const f = await C.setMode("fly"); out.fly = f.mode; out.keepsPose = f.position.every((v, i) => Math.abs(v - p0[i]) < 0.01); out.appMode = a.mode;
    // flight: from inside the living room through the south wall, no collision; walk: blocked
    await C.setMode("fly", { position: [6, 2, 1.65], look: [0, -1, 0] }); const z0 = C.position.y; a.fly.move(0, 9, 0); out.flyY = +C.position.y.toFixed(2); out.flyMoved = +(z0 - C.position.y).toFixed(2);
    a.fly.enter({ position: [6, 3, -2], yaw: 0, pitch: 0 }); a.fly.move(0, 2, 0); out.below = +C.position.z.toFixed(2);
    const w = await C.setMode("walk", { position: [6, 2, 1.65] }); out.walk = [w.mode, a.walk.info().room];
    a.walk.teleport(6, 2, { storey: 0, yaw: Math.PI }); const m = a.walk.step(0, -9); out.walkMoved = +m.toFixed(2); out.walkY = +a.walk.info().y;
    const f2 = await C.setMode("fly", { position: [6, 2, 1.65], look: [0, 1, 0] }); out.f2 = f2.mode;
    const back = await C.setMode("walk"); out.flyToWalk = [back.mode, +a.walk.info().z.toFixed(2), a.walk.info().room];
    const o = await C.setMode("orbit"); out.orbit = o.mode; out.seen = seen; out.cm = C.getMode();
    try { await C.setMode("hover"); } catch (e) { out.err = e.message; }
    return out;
  });
  check(modes.start === "orbit" && modes.fly === "fly" && modes.appMode === "fly" && modes.keepsPose, "orbit to fly keeps the camera where it is: " + JSON.stringify([modes.fly, modes.keepsPose]));
  check(modes.flyMoved > 8.5 && modes.flyY < -5, "fly passes through the south wall: y " + modes.flyY);
  check(modes.below < -1, "fly goes below the ground: z " + modes.below);
  check(modes.walk[0] === "walk" && modes.walkMoved < 3.5 && modes.walkY > 0.2, "walk stops at the wall (moved " + modes.walkMoved + " m, y " + modes.walkY + ")");
  check(modes.flyToWalk[0] === "walk" && Math.abs(modes.flyToWalk[1] - 1.65) < 0.02 && !!modes.flyToWalk[2], "fly to walk puts the walker on the floor below: " + JSON.stringify(modes.flyToWalk));
  check(modes.orbit === "orbit" && modes.cm === "orbit" && modes.seen.includes("fly") && modes.seen.includes("walk") && modes.seen.includes("orbit") && /orbit, walk or fly/.test(modes.err || ""), "mode events fire and a bad mode is refused: " + modes.seen.join());

  // ---- structure drawn at true size
  const st = await ev(async () => {
    const a = window.reteBuilding, svc = a.svc, THREE = await import("three"), out = { bad: [], n: 0 };
    for (const n of svc.data.nodes.filter((x) => x.system === "structure")) {
      const t = svc.thingFor(n.id); const b = new THREE.Box3(); t.meshes.forEach((m) => b.expandByObject(m)); const s = b.getSize(new THREE.Vector3());
      out.n++;
      if (n.type === "bracing") { const ok = b.expandByScalar(0.03).containsPoint(new THREE.Vector3(...n.a)) && b.containsPoint(new THREE.Vector3(...n.b)); if (!ok) out.bad.push(n.id); continue; }
      const sz = n.size; if (!sz || Math.abs(s.x - sz[0]) > 0.02 || Math.abs(s.y - sz[1]) > 0.02 || Math.abs(s.z - sz[2]) > 0.02) out.bad.push(n.id + " " + s.toArray().map((v) => v.toFixed(2)) + " vs " + sz);
    }
    const braces = svc.data.nodes.filter((x) => x.type === "bracing").map((n) => { const t = svc.thingFor(n.id), g = t.meshes[0].geometry; g.computeBoundingBox(); const P = g.attributes.position; let lo = 1e9, hi = -1e9; const k = new THREE.Vector3(...n.b).sub(new THREE.Vector3(...n.a)); const L = k.length(); k.normalize(); const a0 = new THREE.Vector3(...n.a); for (let i = 0; i < P.count; i++) { const d = new THREE.Vector3().fromBufferAttribute(P, i).sub(a0).dot(k); lo = Math.min(lo, d); hi = Math.max(hi, d); } return Math.abs((hi - lo) - L) < 0.01 && Math.abs(lo) < 0.01; });
    out.braces = braces;
    return out;
  });
  check(st.n === 71 && st.bad.length === 0, "the 71 structure nodes are drawn at their positions and sizes: " + st.bad.slice(0, 3).join(" | "));
  check(st.braces.length === 4 && st.braces.every(Boolean), "the four wind braces run from a to b at their length");

  // ---- trench: a ray from outside reaches a strip foundation
  const tr = await ev(async () => {
    const a = window.reteBuilding, THREE = await import("three"), out = {}, V = a.viewport, G = a.ground; V.setLayout("single"); a.svc.layers.forEach((l) => l.setVisible(false));
    const view = () => { a.camera.position.set(6, -6.5, 2.6); a.controls.target.set(6, 0.1, -0.7); a.camera.lookAt(6, 0.1, -0.7); a.camera.updateMatrixWorld(true); a.camera.updateProjectionMatrix(); };
    view(); const sr = a.renderer.domElement.parentElement.getBoundingClientRect(); const target = [6, -0.05, -0.7];
    const at = () => { const c = a.camera, v = new THREE.Vector3(...target).project(c); const x = Math.round((v.x + 1) / 2 * sr.width), y = Math.round((1 - v.y) / 2 * sr.height); const k = document.createElement("canvas"), cv = a.renderer.domElement; k.width = cv.width; k.height = cv.height; const g = k.getContext("2d"); a.renderNow(); g.drawImage(cv, 0, 0); const d = g.getImageData(x - 2, y - 2, 5, 5).data; let R = 0, Gc = 0, B = 0; for (let i = 0; i < d.length; i += 4) { R += d[i]; Gc += d[i + 1]; B += d[i + 2]; } const n = d.length / 4; return [R / n, Gc / n, B / n]; };
    out.none = at(); G.setCutaway("trench"); out.mode = G.getCutaway(); out.trench = G.trench(); view(); out.cut = at(); out.structureOn = a.svc.layerState().find((l) => l.id === "structure").visible;
    G.setCutaway("hidden"); out.hidden = at(); G.setCutaway("none"); out.back = at();
    try { G.setCutaway("deep"); } catch (e) { out.err = e.message; }
    return out;
  });
  const green = (c) => c[1] > c[0] + 12 && c[1] > c[2] + 12;
  check(tr.mode === "trench" && tr.trench && tr.trench.baseZ < -1 && tr.structureOn, "trench: the cutaway is set, the pit goes down to the foundation base, the Structure layer is on");
  check(green(tr.none) && !green(tr.cut) && green(tr.back) && /Cutaway/.test(tr.err || ""), "a view from outside sees lawn without the trench and the foundation face with it: " + JSON.stringify([tr.none, tr.cut, tr.hidden].map((c) => c.map((v) => Math.round(v)))));
  check(!green(tr.hidden), "hidden ground shows the foundation too: " + tr.hidden.map(Math.round));

  // ---- load paths and removal
  const lp = await ev(() => {
    const a = window.reteBuilding, S = a.structure, out = {}; const r = S.list({ role: "rafter" })[3]; const down = S.tracePath(r.id, "down");
    out.id = r.id; out.path = down.path; out.reaches = down.path[0] === r.id && down.path[down.path.length - 1] === "ground" && down.path.length >= 4; out.inSet = down.ids.includes("ground");
    const up = S.tracePath("strip-ext-west", "up"); out.upCount = up.ids.length; S.clearPath();
    out.roles = S.list({ loadBearing: false }).map((e) => e.role).filter((v, i, a2) => a2.indexOf(v) === i); out.ifc = S.list({ fromIfc: true }).length; out.designed = S.list({ fromIfc: false }).length; out.emb = S.list({ embedded: true }).length;
    S.setColourMode("role"); out.mode = S.getColourMode(); out.legend = S.legend().items.length; S.setColourMode("load-bearing"); S.setColourMode("from-ifc"); S.setColourMode("material");
    try { S.setColourMode("rainbow"); } catch (e) { out.cerr = e.message; }
    return out;
  });
  check(lp.reaches && lp.inSet, "the load path of a rafter reaches the ground: " + lp.path.join(" > "));
  check(lp.upCount > 20 && lp.ifc === 64 && lp.designed === 71 && lp.emb > 30 && /material|role/.test(lp.cerr || "") && lp.legend > 10, "up-path, list filters, colour modes: " + JSON.stringify([lp.upCount, lp.ifc, lp.designed, lp.emb]));
  const sc = await ev(async () => {
    const a = window.reteBuilding, svc = a.svc, out = [];
    for (const s of svc.data.scenarios.filter((x) => x.kind === "structure")) { const r = svc.applyScenario(s.id); const got = new Set(r.affected), exp = new Set(s.affected); out.push([s.id, got.size, exp.size, got.size === exp.size && [...exp].every((x) => got.has(x)), r.kind, r.same]); }
    // by hand: remove an interior wall, then restore
    const w = a.structure.list({ role: "load-bearing-wall" })[1]; const st0 = a.structure.remove(w.id); const n1 = st0.unsupported.length; const info = a.structure.info(st0.unsupported[0] ? st0.unsupported[0].id : w.id); a.structure.restore(); const n2 = a.structure.state();
    svc.resetStates(); const q = await svc.runScenarioQuery("remove-wall-int-2").catch((e) => ({ err: e.message }));
    return { out, byHand: [w.label, n1, n2.removed.length, n2.unsupported.length], reason: st0.unsupported[0] && st0.unsupported[0].reason, q: q && { approximate: q.approximate, same: q.same, g: q.graphCount, m: q.modelCount, err: q.err } };
  });
  check(sc.out.length === 4 && sc.out.every((o) => o[3] && o[4] === "structure" && o[5] === true), "the four structural scenarios give exactly the stored lists: " + JSON.stringify(sc.out.map((o) => o.slice(0, 3))));
  check(sc.byHand[1] > 0 && sc.byHand[2] === 0 && sc.byHand[3] === 0 && /load path/.test(sc.reason || ""), "removing a bearing wall marks what loses support, with a reason; restore clears it: " + JSON.stringify(sc.byHand));
  check(!sc.q.err ? sc.q.approximate === true && sc.q.same === true && sc.q.g >= sc.q.m : true, "the graph query of a structural scenario is a superset of the exact set: " + JSON.stringify(sc.q));

  // ---- supply scenarios are unchanged by reading the blocking states from the rule
  const sp = await ev(() => {
    const a = window.reteBuilding, svc = a.svc, out = []; const block = a.statevars.supplyBlocking();
    for (const s of svc.data.scenarios.filter((x) => x.kind === "supply")) { const r = svc.applyScenario(s.id); out.push([s.id, r.affected.length, s.affected.length, r.same]); }
    svc.resetStates(); return { out, deadAtRest: svc.nodes.filter((t) => !t.live).length, types: Object.keys(block).sort().join() };
  });
  check(sp.out.length === 11 && sp.out.every((o) => o[3] && o[1] === o[2]), "all 11 supply scenarios give the stored lists: " + JSON.stringify(sp.out.filter((o) => !o[3])));
  check(sp.deadAtRest === 0 && sp.types === "access-point,breaker,floor-loop,grid-connection,heat-pump-indoor,heat-pump-outdoor,hrv-unit,net-switch,rcd,router,valve,water-heater", "the blocking states come from rule-supply, for the 12 types that gate the supply: " + sp.types);

  // ---- state variables
  const sv = await ev(() => {
    const a = window.reteBuilding, V = a.statevars, out = {}, d = a.systems;
    const fixed = d.openings.filter((o) => o.operation === "fixed").map((o) => o.guid), swing = d.openings.filter((o) => o.kind === "window" && o.operation !== "fixed").map((o) => o.guid);
    out.fixedStates = fixed.map((g) => a.states(g).join()); out.swingStates = [...new Set(swing.map((g) => a.states(g).join()))];
    try { a.setState(fixed[0], "open"); out.fixedErr = "no error"; } catch (e) { out.fixedErr = e.message; }
    out.toggle = a.toggleState(fixed[0]); out.state = a.getState(fixed[0]);
    const bad = d.rooms.find((r) => r.label === "Bad").guid, wohn = d.rooms.find((r) => r.label === "Wohnen").guid;
    out.errs = []; const t = (f) => { try { f(); out.errs.push("none"); } catch (e) { out.errs.push(e.message); } };
    t(() => V.setValue("room-setpoint", wohn, 99)); t(() => V.setValue("room-setpoint", wohn, "warm")); t(() => V.setValue("state-window", swing[0], "ajar")); t(() => V.setValue("room-setpoint", "no-such-room", 20)); t(() => V.setValue("no-such-variable", wohn, 1)); t(() => V.setValue("state-window", wohn, "open")); t(() => V.setValue("room-occupancy", wohn, -1));
    out.levelBad = V.levelOf("room-air-temperature", bad, 22.5).label; out.levelWohnen = V.levelOf("room-air-temperature", wohn, 22.5).label; out.spBad = V.getValue("room-setpoint", bad); out.spWohnen = V.getValue("room-setpoint", wohn);
    out.edge = [V.levelOf("room-air-temperature", bad, 22.5).label, V.levelOf("room-air-temperature", bad, 25.5).label, V.levelOf("room-air-temperature", bad, 10).label, V.levelOf("room-air-temperature", wohn, 24).label];
    out.vars = V.variables().length; out.dataVars = d.stateVariables.length; out.room = V.variables({ kind: "room" }).length; out.surf = V.variables({ thing: d.thermal.surfaces[0].id }).map((x) => x.id).length;
    out.rules = V.rules().length; out.setOk = V.setValue("room-setpoint", wohn, 21.5); V.resetValues();
    out.opening = V.setValue("state-window", swing[0], "tilted"); out.winState = a.getState(swing[0]); a.setState(swing[0], "closed");
    return out;
  });
  check(sv.fixedStates.length === 2 && sv.fixedStates.every((s) => s === "closed") && sv.swingStates.length === 1 && sv.swingStates[0] === "closed,tilted,open", "fixed windows have one state, the others three: " + JSON.stringify([sv.fixedStates, sv.swingStates]));
  check(/not a state/.test(sv.fixedErr) && sv.toggle === "closed" && sv.state === "closed", "a fixed window cannot be opened: " + sv.fixedErr);
  check(/outside the range/.test(sv.errs[0]) && /must be a number/.test(sv.errs[1]) && /not a state/.test(sv.errs[2]) && /No thing/.test(sv.errs[3]) && /Unknown state variable/.test(sv.errs[4]) && /does not apply/.test(sv.errs[5]) && /outside the range/.test(sv.errs[6]), "setValue rejects a value out of range, a word for a number, an unknown state, thing or variable: " + JSON.stringify(sv.errs));
  check(sv.levelBad === "comfortable" && sv.levelWohnen === "warm" && sv.spBad === 24 && sv.spWohnen === 20, "levelOf at 22.5 C: the bath (set point 24) is " + sv.levelBad + ", the living room (20) is " + sv.levelWohnen);
  check(sv.edge.join() === "comfortable,warm,cold,hot", "level bounds: lower included, upper excluded, unbounded ends: " + sv.edge.join());
  check(sv.vars === sv.dataVars && sv.room === 7 && sv.surf >= 2 && sv.rules === 8 && sv.setOk.value === 21.5 && sv.opening.value === "tilted" && sv.winState === "tilted", "38 variables, filters, rules, setValue on numbers and states: " + JSON.stringify([sv.vars, sv.room, sv.surf, sv.rules]));

  // ---- thermal day: a pinned room, colour by level, the legend of the data
  const th = await ev(async () => {
    const a = window.reteBuilding, V = a.statevars, out = {}, wohn = a.systems.rooms.find((r) => r.label === "Wohnen").guid, bad = a.systems.rooms.find((r) => r.label === "Bad").guid;
    a.sun.set({ date: "2026-01-20", time: 15 }); const T = await a.loadThermal(); T.run({}); const base = T.last().rows.find((r) => r.room === "Wohnen");
    V.setValue("room-air-temperature", wohn, 21); T.run({}); const pin = T.last().rows.find((r) => r.room === "Wohnen"), oth = T.last().rows.find((r) => r.room === "Bad");
    out.base = [base.minC, base.maxC]; out.pin = [pin.minC, pin.maxC, pin.pinned]; out.other = [oth.minC, oth.maxC]; out.pinned = V.pinned().length; out.store = V.getValue("room-air-temperature", wohn);
    V.unpin("room-air-temperature", wohn); T.run({}); out.unpinned = T.last().rows.find((r) => r.room === "Wohnen").maxC - T.last().rows.find((r) => r.room === "Wohnen").minC > 0.05; out.pinsAfter = V.pinned().length;
    const lg = V.colourBy("room-air-temperature"), vv = a.systems.stateVariables.find((x) => x.id === "room-air-temperature");
    out.legend = lg.items.map((i) => [i.label, i.color]); out.expect = vv.levels.map((l) => [l.label, l.color]); out.things = lg.things.length; out.levels = Object.fromEntries(lg.things.map((t) => [t.label, t.level]));
    a.renderNow(); let n = 0; a.house.traverse((o) => { if (o.parent && o.parent.name === "statevarColour" && o.visible) n++; }); out.meshes = n;
    out.sunSet = a.statevars.getValue("room-air-temperature", bad);
    V.colourBy(null); let m = 0; a.house.traverse((o) => { if (o.parent && o.parent.name === "statevarColour" && o.visible) m++; }); out.meshesOff = m;
    try { V.colourBy("room-co2"); out.co2 = "ok"; } catch (e) { out.co2 = e.message; }
    try { V.colourBy("state-window"); } catch (e) { out.enumErr = e.message; }
    return out;
  });
  check(Math.abs(th.pin[0] - 21) < 0.001 && Math.abs(th.pin[1] - 21) < 0.001 && th.pin[2] === true && th.other[1] - th.other[0] > 0.05 && th.base[1] - th.base[0] > 0.01 && th.unpinned && th.pinsAfter === 0, "a pinned room temperature holds all day (21.00 to 21.00), the others move: " + JSON.stringify([th.base, th.pin, th.other]));
  check(JSON.stringify(th.legend) === JSON.stringify(th.expect) && th.things === 7 && th.meshes === 14 && th.meshesOff === 0, "colour by room temperature: the legend is the five levels of the data, 14 tinted meshes (floor and volume of 7 rooms), none after: " + th.meshes);
  check(Object.values(th.levels).length === 7 && Object.values(th.levels).every((l) => typeof l === "string"), "every room has a level after a thermal day: " + JSON.stringify(th.levels));
  check(/cannot colour/.test(th.enumErr || "") && th.co2 !== undefined, "colourBy refuses an enumerated variable: " + (th.enumErr || "").slice(0, 60));
  const surf = await ev(async () => {
    const a = window.reteBuilding, V = a.statevars, out = {}; a.sun.set({ date: "2026-06-21", time: 8 }); const T = await a.loadThermal(); T.publish({ surfaces: true });
    const am = Object.fromEntries(V.things("surface").map((t) => [t.id, null])); const lvl = (x) => { try { return V.getValue("surface-sunlit-fraction", x); } catch { return null; } };
    const ids = a.systems.thermal.surfaces.filter((s) => s.boundary === "outside").map((s) => s.id); out.am = ids.map(lvl); a.sun.set({ time: 17 }); T.publish({ surfaces: true }); out.pm = ids.map(lvl);
    out.east = a.systems.thermal.surfaces.filter((s) => s.boundary === "outside" && s.kind === "window").map((s) => [s.azimuthDeg, lvl(s.id)]);
    out.ids = ids.length; void am; a.statevars.colourBy("surface-sunlit-fraction"); a.renderNow(); let n = 0; a.house.traverse((o) => { if (o.parent && o.parent.name === "statevarColour" && o.visible) n++; }); out.surfMeshes = n; V.colourBy(null);
    return out;
  });
  const sumAm = surf.am.reduce((s, v) => s + v, 0), sumPm = surf.pm.reduce((s, v) => s + v, 0);
  const nDiff = surf.am.filter((v, i) => Math.abs(v - surf.pm[i]) > 0.3).length;
  check(surf.ids > 20 && surf.am.every((v) => v >= 0 && v <= 1) && nDiff >= 4 && surf.surfMeshes === surf.ids, "sunlit fraction per exterior surface differs between morning and afternoon (" + nDiff + " of " + surf.ids + " surfaces change by more than 0.3; sums " + sumAm.toFixed(1) + " and " + sumPm.toFixed(1) + ") and is drawn on the surfaces: " + surf.surfMeshes);
  const east = surf.east.filter((e) => e[0] > 100 && e[0] < 170).length;
  void east;

  // ---- the tools
  let r = await call("set_layout", { layout: "quad", maximise: "front" });
  check(r.v && r.v.layout === "quad" && r.v.maximised === "front" && r.v.panes.length === 1, "set_layout quad + maximise: " + JSON.stringify(r.v && r.v.maximised) + (r.err || ""));
  r = await call("set_layout", { layout: "quad", maximise: "none" }); r = await call("get_viewport");
  check(r.v && r.v.panes.length === 4 && r.v.panes.find((p) => p.id === "front").facade.name === "south-east" && r.bytes < 18000, "get_viewport: layout, panes, façade names");
  r = await call("set_layout", { layout: "triple" }); check(!!r.err, "set_layout refuses an unknown layout");
  r = await call("set_section", { axis: "x", position: 5, enabled: true }); check(r.v && r.v.x.enabled && r.v.x.position === 5, "set_section");
  r = await call("set_section", { axis: "z", storey: 1 }); check(r.v && Math.abs(r.v.z.position - 3.9) < 1e-6 && r.v.z.enabled, "set_section with a storey: " + (r.v && r.v.z.position));
  r = await call("set_section", { axis: "q" }); check(!!r.err, "set_section refuses an axis");
  await call("reset", { scope: "view" });
  r = await call("set_camera_mode", { mode: "fly", position: [6, -3, 1], look: [0, 1, 0] }); check(r.v && r.v.mode === "fly", "set_camera_mode fly");
  r = await call("set_camera_mode", { mode: "orbit" }); check(r.v && r.v.mode === "orbit", "set_camera_mode orbit");
  r = await call("set_ground_cutaway", { mode: "trench" }); check(r.v && r.v.cutaway === "trench" && r.v.trench, "set_ground_cutaway"); await call("set_ground_cutaway", { mode: "none" });
  r = await call("list_structure", { role: "lintel" }); check(r.v && r.v.total === 16 && r.v.items.every((e) => e.role === "lintel" && e.embedded), "list_structure by role: " + (r.v && r.v.total));
  r = await call("list_structure", { from_ifc: true, load_bearing: true }); check(r.v && r.v.total > 20 && r.v.items.every((e) => e.fromIfc && e.loadBearing), "list_structure by from_ifc and load_bearing");
  r = await call("trace_load_path", { id: "lintel-haustuer", direction: "down" }); check(r.v && r.v.path[r.v.path.length - 1] === "ground", "trace_load_path: " + (r.v && r.v.path.join(">")));
  r = await call("trace_load_path", { id: "nothing" }); check(!!r.err && /list_structure/.test(r.err), "trace_load_path refuses an unknown element");
  await ev(() => window.reteBuilding.structure.clearPath());
  r = await call("remove_structural_element", { id: "lintel-eg-fenster-2" }); check(r.v && r.v.removedElement === "lintel-eg-fenster-2" && r.v.removed.length === 1 && r.v.unsupported.length === 1, "remove_structural_element: " + (r.v && r.v.unsupported.map((u) => u.id)));
  r = await call("restore_structure", {}); check(r.v && r.v.removed.length === 0 && r.v.unsupported.length === 0, "restore_structure");
  r = await call("list_state_variables", { applies_to: "room" }); check(r.v && r.v.variables.length === 7 && r.bytes < 18000, "list_state_variables for rooms");
  r = await call("list_state_variables", { applies_to: "nothing-at-all" }); check(!!r.err, "list_state_variables refuses an unknown thing");
  const wohn = await ev(() => window.reteBuilding.systems.rooms.find((x) => x.label === "Wohnen").guid);
  r = await call("get_value", { variable: "room-air-temperature", thing: wohn }); check(r.v && typeof r.v.value === "number" && Math.abs(r.v.value - 20) < 1 && r.v.level === "comfortable", "get_value: " + JSON.stringify(r.v));
  r = await call("set_value", { variable: "room-setpoint", thing: wohn, value: 99 }); check(!!r.err && /range/.test(r.err), "set_value refuses an out of range number");
  r = await call("set_value", { variable: "room-setpoint", thing: wohn, value: 21 }); check(r.v && r.v.value === 21, "set_value");
  await ev(() => window.reteBuilding.statevars.resetValues());
  r = await call("colour_by", { variable: "room-air-temperature" }); check(r.v && r.v.items.length === 5 && r.v.things.length === 7, "colour_by returns the legend");
  r = await call("colour_by", {}); check(r.v && r.v.colouring === null, "colour_by without a variable removes the colouring");

  // ---- windows: a two-leaf window is a Stulpfenster, nothing fixed stays in the opening, only the handle leaf tilts
  const win = await ev(async () => {
    const a = window.reteBuilding, THREE = await import("three"), out = [], P = a.PROXY, was = P.side; P.side = THREE.DoubleSide;
    const rc = new THREE.Raycaster(), hc = a.HOUSE_CENTER;
    for (const o of a.systems.openings.filter((x) => x.kind === "window" && x.operation !== "fixed")) {
      const g = a.meshByGuid.get(o.guid), b = new THREE.Box3(new THREE.Vector3(...o.box[0]), new THREE.Vector3(...o.box[1])), c = b.getCenter(new THREE.Vector3()), s = b.getSize(new THREE.Vector3());
      const wAx = s.x > s.y ? "x" : "y", tAx = wAx === "x" ? "y" : "x", sg = Math.sign(c[tAx] - hc[tAx]) || 1, rec = { label: o.label, leaves: o.leaves.length };
      // (b) nothing of the fixed meshes sits in the opening except the outer frame ring: no triangle centroid inside the opening inset by the frame width
      let inner = 0; const fixedMeshes = g.children.filter((m) => m.isMesh); rec.fixedMeshes = fixedMeshes.length;
      for (const m of fixedMeshes) { const Pp = m.geometry.attributes.position, I = m.geometry.index, n = I ? I.count / 3 : Pp.count / 3; for (let t = 0; t < n; t++) { const v = new THREE.Vector3(); for (let q = 0; q < 3; q++) { const i = I ? I.getX(t * 3 + q) : t * 3 + q; v.x += Pp.getX(i) / 3; v.y += Pp.getY(i) / 3; v.z += Pp.getZ(i) / 3; } if (v[wAx] > b.min[wAx] + 0.125 && v[wAx] < b.max[wAx] - 0.125 && v.z > b.min.z + 0.125 && v.z < b.max.z - 0.125) inner++; } }
      rec.innerTris = inner;
      a.setState(o.guid, "open", { instant: true }); a.settleStates(); g.updateMatrixWorld(true);
      // rays from inside to outside over the opening, frame to frame
      let fixedHits = 0, rays = 0;
      for (let u = b.min[wAx] + 0.14; u <= b.max[wAx] - 0.14; u += 0.05) for (let z = b.min.z + 0.14; z <= b.max.z - 0.14; z += 0.05) {
        const org = new THREE.Vector3(); org[wAx] = u; org[tAx] = c[tAx] - sg * 0.8; org.z = z; const d = new THREE.Vector3(); d[tAx] = sg; rc.set(org, d); rc.far = 1.6; rays++;
        for (const h of rc.intersectObject(g, true)) if (!h.object.userData.moving) { fixedHits++; break; }
      }
      rec.fixedHits = fixedHits; rec.rays = rays;
      a.setState(o.guid, "tilted", { instant: true }); a.settleStates(); rec.tilt = a.openables().find((x) => x.guid === o.guid).leaves.map((l) => l.maxT > 0);
      a.setState(o.guid, "closed", { instant: true }); a.settleStates(); out.push(rec);
    }
    P.side = was; return out;
  });
  check(win.length === 9 && win.every((w) => w.leaves === 2 && w.innerTris === 0 && w.fixedMeshes === 1), "a two-leaf window keeps only its outer frame fixed: no triangle of the fixed mesh inside the opening: " + JSON.stringify(win.map((w) => [w.innerTris, w.fixedMeshes])));
  check(win.every((w) => w.fixedHits === 0 && w.rays > 100), "with both leaves open, rays through the opening hit nothing fixed (" + win.map((w) => w.rays).join() + " rays each): " + win.map((w) => w.fixedHits).join());
  check(win.every((w) => w.tilt.length === 2 && w.tilt.filter(Boolean).length === 1), "tilted: only the handle leaf tilts: " + JSON.stringify(win[0].tilt));

  // ---- rooms are drawn by their border: no translucent volume around the camera
  const rm = await ev(async () => {
    const a = window.reteBuilding, out = {}, wohn = a.systems.rooms.find((r) => r.label === "Wohnen").guid;
    a.setVisibility({ classes: [...a.getVisibility().classes, "IfcSpace"] });
    await a.camera.setMode("walk", { position: [6, 2, 1.65] }); out.room = a.walk.info().room; out.style = a.rooms.getStyle();
    a.select(wohn, { focus: false }); a.statevars.colourBy("room-air-temperature"); a.renderNow();
    const spaces = [...a.nodeByGuid].filter(([, n]) => n.k === "Space").map(([g]) => g);
    out.alphas = spaces.map((g) => { const l = a.lookOf(a.meshByGuid.get(g)); return l ? +l.alpha.toFixed(2) : null; });
    out.outline = !!(a.rooms.outline(wohn) && a.rooms.outline(wohn).visible); out.outlines = spaces.filter((g) => a.rooms.outline(g) && a.rooms.outline(g).visible).length;
    const ov = a.scene.getObjectByName("statevarColour"); out.vols = ov ? ov.children.filter((m) => m.userData.kind === "vol" && m.visible).length : -1; out.floors = ov ? ov.children.filter((m) => m.userData.kind === "floor" && m.visible).length : -1;
    a.rooms.setStyle("fill"); a.renderNow(); out.fillAlphas = spaces.map((g) => ({ g, a: a.lookOf(a.meshByGuid.get(g)).alpha })); out.inside = a.rooms.insideRoom(); out.insideAlpha = out.fillAlphas.find((x) => x.g === a.rooms.insideRoom());
    out.fillVols = ov.children.filter((m) => m.userData.kind === "vol" && m.visible).length; out.insideVol = ov.children.some((m) => m.userData.kind === "vol" && m.userData.room === out.inside && m.visible);
    a.rooms.setStyle("floor"); a.renderNow(); out.floorTint = !!(a.rooms.floor(wohn) && a.rooms.floor(wohn).visible);
    try { a.rooms.setStyle("cloud"); } catch (e) { out.err = e.message; }
    a.rooms.setStyle("outline"); a.statevars.colourBy(null); await a.camera.setMode("orbit"); a.setVisibility({ classes: a.getVisibility().classes.filter((c) => c !== "IfcSpace") }); a.clearSelection();
    return out;
  });
  check(rm.room === "Wohnen" && rm.style === "outline" && rm.alphas.every((v) => v === 0), "default style: no room volume is drawn filled (alphas of the 7 volumes: " + rm.alphas.join() + ")");
  check(rm.outline && rm.outlines >= 3 && rm.vols === 0 && rm.floors === 7, "the selected room has an outline, the colour overlay tints floors only: " + JSON.stringify([rm.outline, rm.outlines, rm.vols, rm.floors]));
  check(rm.insideAlpha && rm.insideAlpha.a === 0 && rm.fillAlphas.some((x) => x.a > 0) && rm.insideVol === false && rm.floorTint && /outline/.test(rm.err || ""), "fill style: never for the room the camera is in; floor style tints the floor; a bad style is refused: " + JSON.stringify([rm.insideAlpha, rm.err]));

  // ---- reset: everything back as on a fresh load
  const mess = await ev(async () => {
    const a = window.reteBuilding, V = a.viewport;
    V.setLayout("quad"); V.maximise("side"); a.section.set("x", { enabled: true, position: 4 }); a.section.set("z", { enabled: true, position: 2 }); a.ground.setCutaway("trench");
    a.setSun = a.sun.set({ date: "2026-12-21", time: 9.5 }); a.setState(a.systems.openings.find((o) => o.kind === "door").guid, "open"); a.setState("cb-10", "off"); a.structure.remove("20bTaetQDApP5w8egFxj13"); a.structure.setColourMode("role");
    a.statevars.setValue("room-air-temperature", a.systems.rooms[2].guid, 22); a.statevars.colourBy("room-air-temperature"); a.svc.setXray(true); a.svc.isolate("water-cold"); a.setQuality("low"); a.setColourMode("class"); a.setVisibility({ roof: false });
    a.select(a.graph.nodes.find((n) => n.c === "IfcWallStandardCase").n); a.camera.position.set(3, 3, 3); a.controls.update(); await a.camera.setMode("fly"); a.sun.playDay(false);
    return { layout: V.getLayout(), removed: a.structure.state().removed.length, pinned: a.statevars.pinned().length, mode: a.mode };
  });
  check(mess.layout === "quad" && mess.removed === 1 && mess.pinned === 1 && mess.mode === "fly", "the page is changed in many ways before the reset: " + JSON.stringify(mess));
  const rs = await call("reset", { scope: "all" });
  check(rs.v && rs.v.scope === "all" && rs.v.changed.length >= 10, "reset('all') says what it changed: " + (rs.v && rs.v.changed.join(", ")));
  await page.waitForTimeout(900);
  const after = await snap();
  const diff = Object.keys(fresh).filter((k) => JSON.stringify(fresh[k]) !== JSON.stringify(after[k]));
  check(diff.length === 0, "after reset('all') the page is as on a fresh load (differs in: " + diff.map((k) => k + " " + JSON.stringify(fresh[k]).slice(0, 80) + " -> " + JSON.stringify(after[k]).slice(0, 80)).join("; ") + ")");
  const hashA = await pix([0, 0, 600, 400]);
  void hashA;
  for (const scope of ["view", "states", "time", "simulation"]) { const x = await call("reset", { scope }); check(x.v && x.v.scope === scope, "reset " + scope); }
  r = await call("reset", { scope: "everything" }); check(!!r.err, "reset refuses an unknown scope");

  check(errs.length === 0, "no console or page errors: " + errs.slice(0, 3).join(" | "));

  // ---- the same page without the systems files: layout, sections, fly and reset work on the house alone
  const bare = await open(browser, { width: 1300, height: 850, systems: false });
  const bv = await bare.page.evaluate(async () => {
    const a = window.reteBuilding, out = {}; out.none = [!!a.svc, !!a.structure, !!a.statevars];
    a.viewport.setLayout("quad"); a.renderNow(); out.panes = a.viewport.panes().length; a.section.set("y", { enabled: true, position: 5 }); a.renderNow(); a.section.set("y", { enabled: false });
    const f = await a.camera.setMode("fly", { position: [6, -6, 2], look: [0, 1, 0] }); a.fly.move(0, 3, 0); out.fly = f.mode; await a.camera.setMode("orbit");
    a.ground.setCutaway("trench"); a.renderNow(); a.ground.setCutaway("none"); out.reset = (await a.reset("all")).scope; out.layout = a.viewport.getLayout();
    out.tools = {}; for (const n of ["list_structure", "list_state_variables", "colour_by"]) out.tools[n] = await window.__tools[n].execute({}).then(() => "ok", (e) => e.message);
    out.stateWin = a.states(a.stateList().find((s) => s.kind === "window").id).join();
    return out;
  });
  check(bv.none.join() === "false,false,false" && bv.panes === 4 && bv.fly === "fly" && bv.reset === "all" && bv.layout === "single", "without the systems files: quad, section, fly, cutaway and reset work on the house alone: " + JSON.stringify(bv));
  check(/not loaded/.test(bv.tools.list_structure) && /not loaded/.test(bv.tools.list_state_variables) && /not loaded/.test(bv.tools.colour_by), "the structure and state-variable tools say they are not loaded: " + JSON.stringify(bv.tools));
  const bareErrs = bare.errs.filter((e) => !/status of 404/.test(e));
  check(bareErrs.length === 0, "no errors without the systems files (the 404 of the two files is expected): " + bareErrs.slice(0, 3).join(" | "));
  await bare.ctx.close();

  // ---- phone width: the quad is a 2x2 grid that still works
  const ph = await open(browser, { width: 390, height: 780 });
  const pv = await ph.page.evaluate(() => {
    const a = window.reteBuilding, V = a.viewport, out = {}; V.setLayout("quad"); a.renderNow(); const sr = a.renderer.domElement.parentElement.getBoundingClientRect(); const P = V.panes(); out.panes = P.map((p) => [p.id, Math.round(p.rect.w), Math.round(p.rect.h)]);
    out.picks = P.map((p) => { const h = V.pick(sr.left + p.rect.x + p.rect.w / 2, sr.top + p.rect.y + p.rect.h / 2); return h ? h.pane : null; });
    V.maximise("plan"); a.renderNow(); out.max = V.panes()[0].rect.w; V.maximise(null); V.zoomBy("plan", 1.5); out.zoom = V.pane("plan").view.pixelsPerMetre > 0;
    return out;
  });
  check(pv.panes.length === 4 && pv.panes.every((p) => p[1] >= 150 && p[2] >= 150) && pv.picks.every((p, i) => p === pv.panes[i][0]) && pv.max > 350, "phone width: 2x2 grid, every pane picks, maximise fills the view: " + JSON.stringify(pv.panes));
  check(ph.errs.length === 0, "no errors at phone width: " + ph.errs.slice(0, 3).join(" | "));
  await ph.ctx.close();

  await browser.close();
  console.log(JSON.stringify({ verdict: fails.length ? "FAIL" : "PASS", fails }, null, 2));
  process.exit(fails.length ? 1 : 0);
};
main().catch((e) => { console.log(JSON.stringify({ verdict: "FAIL", fails: [...fails, "crashed: " + String(e && e.stack || e).slice(0, 600)] }, null, 2)); process.exit(1); });
