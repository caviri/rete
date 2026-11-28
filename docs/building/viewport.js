// The viewport: one WebGL canvas, one renderer, one to four panes drawn with viewport + scissor.
//
// API (app.viewport; events 'layout' {layout, maximised, panes}, 'pane' {id, change}, 'hover' {pane, id, label}):
//   setLayout('single' | 'quad')   single: the 3D pane alone (what the page always did). quad: plan, 3D, front and side elevation.
//   getLayout()                    -> 'single' | 'quad'
//   maximise(paneId | null)        one pane of the quad fills the viewport (the others stop drawing); null: back to the grid
//   getMaximised()                 -> paneId | null
//   panes()                        -> descriptors of the panes now on screen; pane(id) -> one descriptor
//        {id: 'plan'|'front'|'side'|'3d', label, projection: 'orthographic'|'perspective', rect: {x, y, w, h} (CSS px in the viewport),
//         view: {centre: [a, b], pixelsPerMetre, scale: 'm per 100 px'} (ortho), axes: ['x','y'] (what the screen's right and up are),
//         facade: {name: 'south-east', azimuthDeg, normal: [nx, ny]} (elevations: the façade that faces the camera),
//         cut: {axis, enabled, position} | null, maximised}
//   setView(id, {centre: [a, b], pixelsPerMetre})   pan and zoom an orthographic pane;  fit(id?)  fit the house;  zoomBy(id, factor)
//   setNorthUp(bool)               the plan pane turned so that true north is up (default: model axes, the compass shows north)
//   project(id, [x, y, z])         world point -> {x, y} in CSS px of the viewport, for that pane (tests, labels)
//   pick(clientX, clientY)         -> {pane, id, pick, dist, point} | null: what is under the pointer, ignoring what a cut removed
//   paneAt(clientX, clientY)       -> pane id | null
//   invalidate(id?)                draw a pane (or all) again;  stats() -> {frames, drawn: {paneId: n}, drawCalls}
//   setBackground('#hex')          background of the orthographic panes
// Orthographic panes: wheel or pinch zooms about the pointer, drag pans, click selects, double click opens or closes a door or window.
// Sections (section.js): plan obeys the z cut, front the y cut, side the x cut. Orthographic panes have no sky, scenery, shadows or
// time-of-day light: a plain background and an even light, so a drawing reads the same at noon and at night.
// A pane redraws only when its own content changed (camera of the 3D pane, pan or zoom of an orthographic pane); a change in the
// scene (a state, a layer, a tint, a cut) redraws every pane. preserveDrawingBuffer keeps the others as they are.
// Phone width: the quad stays a 2x2 grid; maximise() (a button on every pane) gives one pane the whole viewport.
import * as THREE from 'three';
import { app } from './app.js';
import * as SEC from './section.js';
import { reset, captureDefaults, captureLayers } from './reset.js';
import { initGround } from './ground.js';
import { initRooms } from './rooms.js';

const GAP = 3, MIN_PPM = 2, MAX_PPM = 600;
const KINDS = { plan: { label: 'Plan', cut: 'z', ax: ['x', 'y'] }, front: { label: 'Front elevation', cut: 'y', ax: ['x', 'z'] }, side: { label: 'Side elevation', cut: 'x', ax: ['y', 'z'] }, '3d': { label: '3D', cut: null, ax: null } };
const ORDER = ['plan', '3d', 'front', 'side'];
const COMPASS8 = ['north', 'north-east', 'east', 'south-east', 'south', 'south-west', 'west', 'north-west'];
const panes = new Map();
let stage, canvas, host, layout = 'single', maximised = null, autoMax = false, bg = '#e8ebef', northUp = false, needClear = true, stageSize = { w: 800, h: 600 };
const stats = { frames: 0, drawn: { plan: 0, front: 0, side: 0, '3d': 0 }, drawCalls: 0, frameCalls: 0, frameTris: 0, framePanes: 0 };
const ray = new THREE.Raycaster();
const $el = (tag, css, cls) => { const e = document.createElement(tag); if (css) e.style.cssText = css; if (cls) e.className = cls; return e; };

// ---------- true north and façade names ----------
const site = () => (app.sun && app.sun.site) || { northAzimuthDeg: 0, trueNorth: [0, 1] };
export function facadeOf(nx, ny) {
  const az = (((Math.atan2(nx, ny) * 180 / Math.PI - site().northAzimuthDeg) % 360) + 360) % 360;   // outward normal, clockwise from true north
  return { name: COMPASS8[Math.round(az / 45) % 8], azimuthDeg: Math.round(az), normal: [nx, ny] };
}
const facadeNormal = id => id === 'front' ? [0, -1] : id === 'side' ? [1, 0] : null;   // the front elevation looks at the face whose outward normal is -y, the side one at +x

// ---------- panes ----------
function makePane(id) {
  const k = KINDS[id], ortho = id !== '3d';
  const p = { id, kind: id, ortho, rect: { x: 0, y: 0, w: 100, h: 100 }, dirty: true, view: { c: [6, 5], ppm: 30 }, u: [1, 0], v: [0, 1], camera: null, fitted: false, ui: {} };
  if (ortho) p.camera = new THREE.OrthographicCamera(-1, 1, 1, -1, 0.05, 600);
  const root = $el('div', 'position:absolute;pointer-events:none;overflow:hidden;box-sizing:border-box;border:1px solid rgba(120,140,170,.35)', 'vp-pane'); root.dataset.pane = id;
  if (ortho) { const o = $el('canvas', 'position:absolute;inset:0;width:100%;height:100%;pointer-events:none'); root.appendChild(o); p.ui.overlay = o; }
  const lab = $el('div', 'position:absolute;left:8px;top:6px;font:600 11px/1.3 system-ui,sans-serif;color:' + (ortho ? '#1d2735' : '#e6edf7') + ';background:' + (ortho ? 'rgba(255,255,255,.7)' : 'rgba(10,16,26,.6)') + ';padding:2px 7px;border-radius:5px;white-space:nowrap;max-width:calc(100% - 50px);overflow:hidden;text-overflow:ellipsis', 'vp-label');
  const mx = $el('button', 'position:absolute;right:6px;top:5px;width:22px;height:22px;pointer-events:auto;border:1px solid rgba(120,140,170,.5);border-radius:5px;background:rgba(255,255,255,.75);color:#1d2735;font:600 12px system-ui;line-height:1;cursor:pointer;padding:0', 'vp-max');
  mx.textContent = '⤢'; mx.title = 'Maximise this view'; mx.dataset.pane = id;
  mx.addEventListener('click', e => { e.stopPropagation(); maximise(maximised === id ? null : id); });
  mx.addEventListener('pointerdown', e => e.stopPropagation());
  root.append(lab, mx); p.ui.root = root; p.ui.label = lab; p.ui.max = mx;
  if (k.cut) {   // a small cut-position scrubber on the pane
    const box = $el('div', 'position:absolute;right:6px;bottom:6px;display:flex;gap:5px;align-items:center;pointer-events:auto;background:rgba(255,255,255,.78);border:1px solid rgba(120,140,170,.4);border-radius:6px;padding:2px 6px;font:11px system-ui,sans-serif;color:#1d2735', 'vp-cut');
    const cb = $el('input'); cb.type = 'checkbox'; cb.title = 'Cut ' + k.cut; cb.dataset.cut = k.cut;
    const rg = $el('input', 'width:84px;margin:0'); rg.type = 'range'; rg.dataset.cut = k.cut; rg.step = '0.05';
    const tx = $el('span', 'min-width:44px;text-align:right'), tl = $el('span'); tl.textContent = 'cut ' + k.cut; box.append(cb, tl, rg, tx);
    for (const e of [box, cb, rg]) { e.addEventListener('pointerdown', ev => ev.stopPropagation()); e.addEventListener('wheel', ev => ev.stopPropagation()); }
    cb.addEventListener('change', () => app.section.set(k.cut, { enabled: cb.checked }));
    rg.addEventListener('input', () => app.section.set(k.cut, { position: +rg.value, enabled: true }));
    root.appendChild(box); p.ui.cutCb = cb; p.ui.cutRange = rg; p.ui.cutText = tx;
  }
  host.appendChild(root);
  return p;
}
function activeIds() { return layout === 'single' ? ['3d'] : maximised ? [maximised] : ORDER; }
function computeRects() {
  const W = stageSize.w, H = stageSize.h, ids = activeIds();
  if (ids.length === 1) { Object.assign(panes.get(ids[0]).rect, { x: 0, y: 0, w: W, h: H }); return; }
  const w2 = Math.floor((W - GAP) / 2), h2 = Math.floor((H - GAP) / 2);
  const cell = [[0, 0], [w2 + GAP, 0], [0, h2 + GAP], [w2 + GAP, h2 + GAP]];
  ORDER.forEach((id, i) => Object.assign(panes.get(id).rect, { x: cell[i][0], y: cell[i][1], w: i % 2 ? W - w2 - GAP : w2, h: i > 1 ? H - h2 - GAP : h2 }));
}
function applyRects() {
  const ids = activeIds(), multi = ids.length > 1 || layout === 'quad';
  for (const [id, p] of panes) {
    const on = ids.includes(id);
    p.ui.root.style.display = on && (layout === 'quad') ? '' : 'none';
    if (!on) continue;
    const r = p.rect, s = p.ui.root.style;
    s.left = r.x + 'px'; s.top = r.y + 'px'; s.width = r.w + 'px'; s.height = r.h + 'px';
    if (p.ortho) { const o = p.ui.overlay, dpr = Math.min(devicePixelRatio || 1, 2); o.width = Math.round(r.w * dpr); o.height = Math.round(r.h * dpr); }
    p.ui.max.textContent = maximised === id ? '⤡' : '⤢'; p.ui.max.title = maximised === id ? 'Back to four views' : 'Maximise this view';
    p.dirty = true;
  }
  const p3 = panes.get('3d');
  if (p3 && ids.includes('3d')) { const r = p3.rect; app.camera.aspect = r.w / r.h; app.camera.updateProjectionMatrix(); }
  needClear = true; void multi;
}
function readStage() { stageSize = { w: Math.max(2, stage.clientWidth || 800), h: Math.max(2, stage.clientHeight || 600) }; }
export function resize() {
  if (!stage) return false;
  readStage(); computeRects(); applyRects(); app.requestRender();
  return true;   // the camera aspect was set from the pane
}

// ---------- ortho cameras ----------
function bounds() {
  const b = app.houseBox || new THREE.Box3(new THREE.Vector3(0, 0, 0), new THREE.Vector3(12, 10, 6));
  const lo = b.min.clone(), hi = b.max.clone(); lo.z = Math.min(lo.z, -1.2);
  return { lo, hi, c: lo.clone().add(hi).multiplyScalar(0.5) };
}
function setBasis(p) {
  if (p.id === 'plan' && northUp) { const n = site().trueNorth || [0, 1], l = Math.hypot(n[0], n[1]) || 1; p.v = [n[0] / l, n[1] / l]; p.u = [p.v[1], -p.v[0]]; }
  else { p.u = [1, 0]; p.v = [0, 1]; }
}
function viewBox(p) {   // the house in the pane's own axes
  const { lo, hi } = bounds(); const pts = [];
  for (let i = 0; i < 8; i++) { const w = [i & 1 ? hi.x : lo.x, i & 2 ? hi.y : lo.y, i & 4 ? hi.z : lo.z]; pts.push(p.id === 'plan' ? [w[0], w[1]] : p.id === 'front' ? [w[0], w[2]] : [w[1], w[2]]); }
  let a0 = 1e9, a1 = -1e9, b0 = 1e9, b1 = -1e9;
  for (const q of pts) { const x = q[0] * p.u[0] + q[1] * p.u[1], y = q[0] * p.v[0] + q[1] * p.v[1]; a0 = Math.min(a0, x); a1 = Math.max(a1, x); b0 = Math.min(b0, y); b1 = Math.max(b1, y); }
  return { a0, a1, b0, b1 };
}
export function fit(id) {
  for (const p of (id ? [panes.get(id)] : [...panes.values()])) {
    if (!p || !p.ortho) continue;
    setBasis(p); const q = viewBox(p), w = Math.max(40, p.rect.w), h = Math.max(40, p.rect.h);
    const ppm = Math.min(w * 0.86 / Math.max(q.a1 - q.a0, 0.1), h * 0.86 / Math.max(q.b1 - q.b0, 0.1));
    const ca = (q.a0 + q.a1) / 2, cb = (q.b0 + q.b1) / 2;
    p.view = { c: [ca * p.u[0] + cb * p.v[0], ca * p.u[1] + cb * p.v[1]], ppm: Math.min(MAX_PPM, Math.max(MIN_PPM, ppm)) };
    p.fitted = true; p.dirty = true;
  }
  app.requestRender(); return id ? describe(panes.get(id)) : panes_();
}
function updateCamera(p) {
  if (!p.ortho) return;
  const cam = p.camera, { w, h } = p.rect, hw = w / 2 / p.view.ppm, hh = h / 2 / p.view.ppm, [c0, c1] = p.view.c, B = bounds();
  cam.left = -hw; cam.right = hw; cam.top = hh; cam.bottom = -hh;
  if (p.id === 'plan') { cam.position.set(c0, c1, B.hi.z + 80); cam.up.set(p.v[0], p.v[1], 0); cam.lookAt(c0, c1, 0); }
  else if (p.id === 'front') { cam.position.set(c0, B.lo.y - 80, c1); cam.up.set(0, 0, 1); cam.lookAt(c0, B.lo.y - 79, c1); }
  else { cam.position.set(B.hi.x + 80, c0, c1); cam.up.set(0, 0, 1); cam.lookAt(B.hi.x + 79, c0, c1); }
  cam.near = 0.05; cam.far = 400; cam.updateProjectionMatrix(); cam.updateMatrixWorld(true);
}
// pixel in the pane -> the two world coordinates of the pane's axes
function unproject(p, px, py) {
  const sx = (px - p.rect.w / 2) / p.view.ppm, sy = (p.rect.h / 2 - py) / p.view.ppm;
  return [p.view.c[0] + sx * p.u[0] + sy * p.v[0], p.view.c[1] + sx * p.u[1] + sy * p.v[1]];
}
function panPixels(p, dx, dy) { const k = 1 / p.view.ppm; p.view.c[0] += -dx * k * p.u[0] + dy * k * p.v[0]; p.view.c[1] += -dx * k * p.u[1] + dy * k * p.v[1]; }
function zoomAt(p, factor, px, py) {
  const before = unproject(p, px, py), nz = Math.min(MAX_PPM, Math.max(MIN_PPM, p.view.ppm * factor)); if (nz === p.view.ppm) return;
  p.view.ppm = nz; const after = unproject(p, px, py);
  p.view.c[0] += before[0] - after[0]; p.view.c[1] += before[1] - after[1];
}
function paneChanged(p, change) { p.dirty = true; app.requestRender('panes'); app.emit('pane', { id: p.id, change }); }

// ---------- overlay: grid, scale bar, north, ground line ----------
const nice = m => { const e = Math.pow(10, Math.floor(Math.log10(m))), f = m / e; return (f < 1.5 ? 1 : f < 3.5 ? 2 : f < 7.5 ? 5 : 10) * e; };
function drawOverlay(p) {
  const o = p.ui.overlay, g = o.getContext('2d'), dpr = o.width / p.rect.w;
  g.setTransform(dpr, 0, 0, dpr, 0, 0); g.clearRect(0, 0, p.rect.w, p.rect.h);
  const { w, h } = p.rect, ppm = p.view.ppm;
  const toPx = (a, b) => { const dx = a - p.view.c[0], dy = b - p.view.c[1]; return [w / 2 + (dx * p.u[0] + dy * p.u[1]) * ppm, h / 2 - (dx * p.v[0] + dy * p.v[1]) * ppm]; };
  // grid every 1, 5 or 10 m, only where the lines are at least 14 px apart; drawn as light lines (the house is drawn over them? no: this is on top, kept faint)
  const step = [1, 2, 5, 10, 20, 50].find(s => s * ppm >= 18) || 50;
  g.strokeStyle = 'rgba(40,60,90,.13)'; g.lineWidth = 1; g.beginPath();
  if (p.id === 'plan' && northUp) { /* a rotated grid is noise: none */ } else {
    const tl = unproject(p, 0, 0), br = unproject(p, w, h);
    const a0 = Math.floor(Math.min(tl[0], br[0]) / step) * step, a1 = Math.max(tl[0], br[0]), b0 = Math.floor(Math.min(tl[1], br[1]) / step) * step, b1 = Math.max(tl[1], br[1]);
    for (let a = a0; a <= a1; a += step) { const q = toPx(a, p.view.c[1]); g.moveTo(q[0] + 0.5, 0); g.lineTo(q[0] + 0.5, h); }
    for (let b = b0; b <= b1; b += step) { const q = toPx(p.view.c[0], b); g.moveTo(0, q[1] + 0.5); g.lineTo(w, q[1] + 0.5); }
  }
  g.stroke();
  if (p.id !== 'plan') {   // the ground line at the terrain level of the site
    const tz = ((app.systems && app.systems.structure && app.systems.structure.groundZ) ?? -0.3), q = toPx(p.view.c[0], tz);
    g.strokeStyle = 'rgba(94,120,60,.8)'; g.setLineDash([6, 4]); g.beginPath(); g.moveTo(0, q[1]); g.lineTo(w, q[1]); g.stroke(); g.setLineDash([]);
    g.fillStyle = 'rgba(70,95,40,.9)'; g.font = '10px system-ui'; g.fillText('ground ' + tz.toFixed(2) + ' m', 8, q[1] - 3);
  }
  // scale bar
  const len = nice(110 / ppm), px = len * ppm, x0 = 10, y0 = h - 14;
  g.fillStyle = '#1d2735'; g.strokeStyle = '#1d2735'; g.lineWidth = 2; g.beginPath(); g.moveTo(x0, y0 - 4); g.lineTo(x0, y0); g.lineTo(x0 + px, y0); g.lineTo(x0 + px, y0 - 4); g.stroke();
  g.font = '600 11px system-ui,sans-serif'; g.fillText(len >= 1 ? len + ' m' : len * 100 + ' cm', x0 + px / 2 - 12, y0 - 7);
  // the compass arrow on the plan: where true north is
  if (p.id === 'plan') {
    const n = site().trueNorth || [0, 1], l = Math.hypot(n[0], n[1]) || 1, nx = n[0] / l, ny = n[1] / l;
    const sx = nx * p.u[0] + ny * p.u[1], sy = nx * p.v[0] + ny * p.v[1], cx = w - 30, cy = 52;
    g.save(); g.translate(cx, cy); g.fillStyle = 'rgba(255,255,255,.75)'; g.beginPath(); g.arc(0, 0, 22, 0, 7); g.fill();
    g.strokeStyle = '#b3261e'; g.fillStyle = '#b3261e'; g.lineWidth = 2; g.beginPath(); g.moveTo(-sx * 12, sy * 12); g.lineTo(sx * 14, -sy * 14); g.stroke();
    g.beginPath(); g.arc(sx * 15, -sy * 15, 3, 0, 7); g.fill(); g.fillStyle = '#1d2735'; g.font = '700 10px system-ui'; g.fillText('N', sx * 15 - 3, -sy * 15 - 5 - (sy < 0 ? -14 : 0)); g.restore();
  }
}
function describeLabel(p) {
  const k = KINDS[p.id]; let t = k.label;
  if (p.id !== '3d') {
    const f = facadeNormal(p.id), fa = f ? facadeOf(...f) : null;
    if (fa) t += ' · ' + fa.name + ' façade';
    const m100 = 100 / p.view.ppm, sc = 3779.5 / p.view.ppm;   // a 96 dpi screen: 1 m = 3780 px at 1:1
    t += ' · 1:' + Math.max(1, Math.round(nice(sc) )) + ' (' + (m100 >= 10 ? Math.round(m100) : +m100.toFixed(1)) + ' m per 100 px)';
    const c = app.section && app.section.get()[k.cut]; if (c && c.enabled) t += ' · cut ' + k.cut + ' = ' + c.position.toFixed(2) + ' m';
  }
  return t;
}
function syncPaneUi(p) {
  p.ui.label.textContent = describeLabel(p);
  const k = KINDS[p.id]; if (k.cut && app.section) {
    const c = app.section.get()[k.cut], b = bounds(), a = k.cut;
    p.ui.cutCb.checked = c.enabled; p.ui.cutRange.min = (b.lo[a] - 1).toFixed(2); p.ui.cutRange.max = (b.hi[a] + 1).toFixed(2); p.ui.cutRange.value = c.position;
    p.ui.cutText.textContent = c.position.toFixed(2) + ' m';
  }
}

// ---------- drawing ----------
let saved = null;
const lights = () => { const L = app.lighting; return { sun: L.sun(), hemi: L.hemi(), amb: L.amb(), ground: L.ground(), contact: L.contact() }; };
function enterOrtho(p) {
  const L = lights(); saved = { hid: [], fog: app.scene.fog, sun: [L.sun.intensity, L.sun.position.clone(), L.sun.color.clone(), L.sun.castShadow], hemi: L.hemi.intensity, amb: L.amb.intensity, exp: app.renderer.toneMappingExposure, shadows: app.renderer.shadowMap.enabled, bg: app.renderer.getClearColor(new THREE.Color()).clone(), ba: app.renderer.getClearAlpha() };
  for (const c of app.scene.children) {
    const keep = c === app.house || c.isLight || c.name === 'svcLights' || c.name === 'structureExtras';
    if (!keep && c.visible) { saved.hid.push(c); c.visible = false; }
  }
  app.scene.fog = null;
  if (app.roomLabels && p.id !== 'plan') { saved.hid.push(app.roomLabels); app.roomLabels.visible = false; }   // the room names are for the plan only
  L.hemi.intensity = 1.25; L.amb.intensity = 0.75; L.sun.intensity = 1.6; L.sun.color.set('#ffffff'); L.sun.position.set(6 + 20, 5 - 30, 2 + 38);
  app.renderer.toneMappingExposure = 1; app.renderer.shadowMap.enabled = false;
  app.renderer.setClearColor(bg, 1);
}
function leaveOrtho() {
  const L = lights(), s = saved; saved = null;
  for (const c of s.hid) c.visible = true;
  app.scene.fog = s.fog; L.sun.intensity = s.sun[0]; L.sun.position.copy(s.sun[1]); L.sun.color.copy(s.sun[2]); L.hemi.intensity = s.hemi; L.amb.intensity = s.amb;
  app.renderer.toneMappingExposure = s.exp; app.renderer.shadowMap.enabled = s.shadows; app.renderer.setClearColor(s.bg, s.ba);
}
let hiddenGround = [];
function cutawayHide(on) {   // 'hidden' ground: the plain ground, the scenery ground and the plants are not drawn in the 3D pane
  if (on) { for (const n of ['ground', 'sceneryGround', 'nature', 'contactShadow']) { const o = n === 'ground' ? app.ground : n === 'contactShadow' ? app.lighting.contact() : app.scene.getObjectByName(n); if (o && o.visible) { hiddenGround.push(o); o.visible = false; } } }
  else { for (const o of hiddenGround) o.visible = true; hiddenGround = []; }
}
function draw3d(p, scissor) {
  const R = app.renderer, r = p.rect;
  if (scissor) { R.setScissorTest(true); R.setViewport(r.x, stageSize.h - r.y - r.h, r.w, r.h); R.setScissor(r.x, stageSize.h - r.y - r.h, r.w, r.h); }
  else { R.setScissorTest(false); R.setViewport(0, 0, stageSize.w, stageSize.h); }
  SEC.beginPane('3d'); SEC.showHandles(true);
  const hide = app.ground && app.ground.getCutaway && app.ground.getCutaway() === 'hidden'; if (hide) cutawayHide(true);
  R.render(app.scene, app.camera);
  if (hide) cutawayHide(false);
  SEC.showHandles(false);
  stats.drawCalls = R.info.render.calls; stats.frameCalls += R.info.render.calls; stats.frameTris += R.info.render.triangles; stats.framePanes++;
}
function drawOrtho(p) {
  const R = app.renderer, r = p.rect, y = stageSize.h - r.y - r.h;
  updateCamera(p);
  R.setScissorTest(true); R.setViewport(r.x, y, r.w, r.h); R.setScissor(r.x, y, r.w, r.h);
  enterOrtho(p); SEC.beginPane(p.id);
  R.render(app.scene, p.camera);
  leaveOrtho();
  syncPaneUi(p); drawOverlay(p);
  stats.drawCalls = R.info.render.calls; stats.frameCalls += R.info.render.calls; stats.frameTris += R.info.render.triangles; stats.framePanes++;
}
export function render(all, only3d) {
  const R = app.renderer;
  R.info.autoReset = true; stats.frameCalls = 0; stats.frameTris = 0; stats.framePanes = 0;
  if (layout === 'single' && !maximised) {   // the page as it always was: one pane, no scissor
    const p = panes.get('3d'); if (needClear) { R.setScissorTest(false); needClear = false; }
    draw3d(p, false); stats.drawn['3d']++; stats.frames++; p.dirty = false; return;
  }
  if (needClear) { R.setScissorTest(false); R.setClearColor('#0b1019', 1); R.clear(); needClear = false; all = true; for (const p of panes.values()) p.dirty = true; }
  const ids = activeIds(); const order = ids.slice().sort((a, b) => (a === '3d' ? -1 : 0) - (b === '3d' ? -1 : 0));   // the 3D pane first: it draws the shadow map
  for (const id of order) {
    const p = panes.get(id);
    if (!(all || p.dirty || (only3d && id === '3d'))) continue;
    if (id === '3d') draw3d(p, true); else drawOrtho(p);
    stats.drawn[id]++; p.dirty = false;
  }
  R.setScissorTest(false); stats.frames++;
}
export function invalidate(id) { if (id) { const p = panes.get(id); if (p) p.dirty = true; app.requestRender('panes'); } else { for (const p of panes.values()) p.dirty = true; app.requestRender(); } }
export function onSection() { for (const p of panes.values()) { p.dirty = true; if (p.ortho && p.ui.cutCb) syncPaneUi(p); } }

// ---------- descriptors ----------
function describe(p) {
  const k = KINDS[p.id], on = activeIds().includes(p.id);
  const d = { id: p.id, label: k.label, projection: p.ortho ? 'orthographic' : 'perspective', visible: on && (layout === 'quad' || p.id === '3d'), rect: { ...p.rect }, maximised: maximised === p.id };
  if (p.ortho) {
    d.view = { centre: p.view.c.map(v => +v.toFixed(3)), pixelsPerMetre: +p.view.ppm.toFixed(3), scale: '1:' + Math.round(nice(3779.5 / p.view.ppm)) };
    d.axes = k.ax; d.northUp = p.id === 'plan' ? northUp : undefined;
    const f = facadeNormal(p.id); d.facade = f ? facadeOf(...f) : null;
    const c = app.section && app.section.get()[k.cut]; d.cut = c ? { axis: k.cut, enabled: c.enabled, position: c.position } : null;
    d.lookDirection = p.id === 'plan' ? [0, 0, -1] : p.id === 'front' ? [0, 1, 0] : [-1, 0, 0];
  } else {
    d.view = { fovDeg: app.camera.fov, position: app.camera.position.toArray().map(v => +v.toFixed(2)), mode: app.mode };
    d.cut = app.section && app.section.get().applyTo3D ? { axes: ['x', 'y', 'z'].filter(a => app.section.get()[a].enabled) } : null;
  }
  return d;
}
const panes_ = () => activeIds().map(id => describe(panes.get(id)));

// ---------- layout ----------
export function setLayout(l) {
  if (l !== 'single' && l !== 'quad') throw new Error('layout must be "single" or "quad".');
  if (l === layout) return layout;
  layout = l; if (l === 'single') { maximised = null; autoMax = false; }
  readStage(); computeRects(); applyRects();
  if (l === 'quad') for (const p of panes.values()) if (p.ortho && !p.fitted) fit(p.id);
  SEC.layoutChanged(l);
  for (const p of panes.values()) p.dirty = true;
  app.requestRender(); app.emit('layout', { layout, maximised, panes: panes_() }); return layout;
}
export function maximise(id) {
  if (id != null && !KINDS[id]) throw new Error('Unknown pane "' + id + '". Panes: plan, front, side, 3d.');
  if (layout !== 'quad') { if (id == null || id === '3d') return null; throw new Error('Only the quad layout has panes to maximise. Call setLayout("quad") first.'); }
  autoMax = false;
  maximised = id || null; readStage(); computeRects(); applyRects(); fit();
  app.requestRender(); app.emit('layout', { layout, maximised, panes: panes_() }); return maximised;
}
export function setView(id, o = {}) {
  const p = panes.get(id); if (!p || !p.ortho) throw new Error('"' + id + '" is not an orthographic pane (plan, front, side).');
  if (Array.isArray(o.centre)) { if (o.centre.length < 2 || o.centre.some(v => !Number.isFinite(+v))) throw new Error('centre must be [a, b] in metres.'); p.view.c = [+o.centre[0], +o.centre[1]]; }
  if (o.pixelsPerMetre != null) { const v = +o.pixelsPerMetre; if (!(v > 0)) throw new Error('pixelsPerMetre must be positive.'); p.view.ppm = Math.min(MAX_PPM, Math.max(MIN_PPM, v)); }
  p.fitted = true; paneChanged(p, 'view'); return describe(p);
}
export function zoomBy(id, f) { const p = panes.get(id); if (!p || !p.ortho) throw new Error('not an orthographic pane'); zoomAt(p, f, p.rect.w / 2, p.rect.h / 2); paneChanged(p, 'view'); return describe(p); }
export function setNorthUp(on) { northUp = !!on; const p = panes.get('plan'); setBasis(p); fit('plan'); paneChanged(p, 'north'); return northUp; }
export function setBackground(c) { bg = c; invalidate(); }
export function project(id, w) {
  const p = panes.get(id); if (!p) throw new Error('Unknown pane "' + id + '".');
  if (!p.ortho) { app.camera.updateMatrixWorld(true); const v = new THREE.Vector3(...w).project(app.camera); return { x: p.rect.x + (v.x + 1) / 2 * p.rect.w, y: p.rect.y + (1 - v.y) / 2 * p.rect.h }; }
  const a = id === 'plan' ? [w[0], w[1]] : id === 'front' ? [w[0], w[2]] : [w[1], w[2]], dx = a[0] - p.view.c[0], dy = a[1] - p.view.c[1];
  return { x: p.rect.x + p.rect.w / 2 + (dx * p.u[0] + dy * p.u[1]) * p.view.ppm, y: p.rect.y + p.rect.h / 2 - (dx * p.v[0] + dy * p.v[1]) * p.view.ppm };
}

// ---------- picking ----------
const deepVisible = o => { for (; o; o = o.parent) if (!o.visible) return false; return true; };
export function paneAt(cx, cy) {
  const r = stage.getBoundingClientRect(), x = cx - r.left, y = cy - r.top;
  for (const id of activeIds()) { const q = panes.get(id).rect; if (x >= q.x && x < q.x + q.w && y >= q.y && y < q.y + q.h) return id; }
  return null;
}
export function pick(cx, cy, paneId) {
  const id = paneId || paneAt(cx, cy); if (!id) return null;
  const p = panes.get(id), sr = stage.getBoundingClientRect(), r = p.rect;
  const nx = ((cx - sr.left - r.x) / r.w) * 2 - 1, ny = -(((cy - sr.top - r.y) / r.h) * 2 - 1);
  const cam = p.ortho ? p.camera : app.camera; if (p.ortho) updateCamera(p); else app.camera.updateMatrixWorld(true);
  ray.setFromCamera(new THREE.Vector2(nx, ny), cam);
  const planes = SEC.planesOf(p.kind === '3d' ? '3d' : p.kind);
  const hit = SEC.withPickPlanes(planes, () => pickCore());
  return hit ? { ...hit, pane: id } : null;
}
function pickCore() {
  let best = null;
  for (const h of ray.intersectObject(app.house, true)) {
    const o = h.object;
    if (o.isSprite || o.userData.storey != null || !deepVisible(o)) continue;
    if ((o.userData.alpha ?? 1) <= 0.2) continue;
    let q = o, guid = null, pid = null;
    while (q) { if (q.userData.pickId && !pid) pid = q.userData.pickId; if (q.userData.guid) { guid = q.userData.guid; break; } q = q.parent; }
    if (pid) { best = { id: pid, dist: h.distance, pick: true, point: h.point.toArray() }; break; }
    if (guid && app.nodeByGuid.has(guid)) { best = { id: guid, dist: h.distance, point: h.point.toArray() }; break; }
  }
  if (app.pickables && app.pickables.size) {
    const hs = ray.intersectObjects([...app.pickables.keys()], true).filter(h => deepVisible(h.object));
    if (hs.length && (!best || hs[0].distance < best.dist)) { let q = hs[0].object; while (q && !q.userData.pickId) q = q.parent; if (q) best = { id: q.userData.pickId, dist: hs[0].distance, pick: true, point: hs[0].point.toArray() }; }
  }
  return best;
}

// ---------- selection: frame it in the orthographic panes ----------
function frameSelection(e) {
  if (!e || !e.id) return;
  let obj = e.object || app.meshByGuid.get(e.id); if (!obj && app.pickables) for (const [o, q] of app.pickables) if (q.id === e.id) { obj = o; break; }
  if (!obj) return; const box = new THREE.Box3().setFromObject(obj); if (box.isEmpty()) return;
  for (const p of panes.values()) {
    if (!p.ortho) continue;
    const c = box.getCenter(new THREE.Vector3()), s = box.getSize(new THREE.Vector3());
    const ctr = p.id === 'plan' ? [c.x, c.y] : p.id === 'front' ? [c.x, c.z] : [c.y, c.z];
    const ext = p.id === 'plan' ? [s.x, s.y] : p.id === 'front' ? [s.x, s.z] : [s.y, s.z];
    const a = unproject(p, p.rect.w / 2, p.rect.h / 2); void a;
    const q = unproject(p, 0, 0), q2 = unproject(p, p.rect.w, p.rect.h);
    const inView = ctr[0] > Math.min(q[0], q2[0]) + ext[0] / 2 && ctr[0] < Math.max(q[0], q2[0]) - ext[0] / 2 && ctr[1] > Math.min(q[1], q2[1]) + ext[1] / 2 && ctr[1] < Math.max(q[1], q2[1]) - ext[1] / 2;
    if (inView) continue;   // already in the picture: leave the pane as it is
    p.view.c = ctr;
    const need = Math.min(p.rect.w * 0.7 / Math.max(ext[0], 0.2), p.rect.h * 0.7 / Math.max(ext[1], 0.2));
    if (need < p.view.ppm) p.view.ppm = Math.max(MIN_PPM, need);
    paneChanged(p, 'frame');
  }
}

// ---------- input ----------
function setupInput() {
  const act = { pane: null, id: null, down: null, pts: new Map(), pinch: null, handle: null };
  const sRect = () => stage.getBoundingClientRect();
  const local = (e, p) => { const r = sRect(); return [e.clientX - r.left - p.rect.x, e.clientY - r.top - p.rect.y]; };
  const onUi = e => e.target && e.target.closest && e.target.closest('.vp-max,.vp-cut');
  // the 3D pane's rectangle for code that asks the canvas (OrbitControls, ui.js picking): the canvas answers with the pane
  const orig = canvas.getBoundingClientRect.bind(canvas);
  canvas.getBoundingClientRect = () => {
    const p = panes.get('3d'), r = orig(); if (!p || !activeIds().includes('3d') || (p.rect.w === stageSize.w && p.rect.h === stageSize.h)) return r;
    return new DOMRect(r.left + p.rect.x, r.top + p.rect.y, p.rect.w, p.rect.h);
  };
  stage.addEventListener('pointerdown', e => {
    if (layout === 'single' && !(app.section && app.section.get().applyTo3D && app.section.get().handles)) return;
    if (onUi(e)) return;
    const id = paneAt(e.clientX, e.clientY); if (!id) return;
    act.id = id; act.pane = panes.get(id);
    if (id === '3d') {   // the draggable section plane
      if (app.mode === 'orbit' && e.button === 0 && SEC.isOn()) {
        const p = act.pane, r = sRect(); ray.setFromCamera(new THREE.Vector2(((e.clientX - r.left - p.rect.x) / p.rect.w) * 2 - 1, -(((e.clientY - r.top - p.rect.y) / p.rect.h) * 2 - 1)), app.camera);
        const ax = SEC.hitHandle(ray); if (ax) { act.handle = ax; e.stopPropagation(); e.preventDefault(); try { canvas.setPointerCapture(e.pointerId); } catch { /* ignore */ } }
      }
      return;
    }
    e.stopPropagation(); e.preventDefault();
    try { canvas.setPointerCapture(e.pointerId); } catch { /* ignore */ }
    act.pts.set(e.pointerId, [e.clientX, e.clientY]);
    if (act.pts.size === 1) act.down = { x: e.clientX, y: e.clientY, moved: 0, button: e.button, t: performance.now() };
    else if (act.pts.size === 2) { const [a, b] = [...act.pts.values()]; act.pinch = { d: Math.hypot(a[0] - b[0], a[1] - b[1]), mx: (a[0] + b[0]) / 2, my: (a[1] + b[1]) / 2 }; act.down = null; }
  }, true);
  stage.addEventListener('pointermove', e => {
    if (act.handle) { const p = act.pane, r = sRect(); ray.setFromCamera(new THREE.Vector2(((e.clientX - r.left - p.rect.x) / p.rect.w) * 2 - 1, -(((e.clientY - r.top - p.rect.y) / p.rect.h) * 2 - 1)), app.camera); SEC.dragTo(act.handle, ray); e.stopPropagation(); return; }
    if (act.pane && act.pane.ortho && act.pts.has(e.pointerId)) {
      e.stopPropagation(); const p = act.pane, prev = act.pts.get(e.pointerId); act.pts.set(e.pointerId, [e.clientX, e.clientY]);
      if (act.pts.size >= 2 && act.pinch) {
        const [a, b] = [...act.pts.values()], d = Math.hypot(a[0] - b[0], a[1] - b[1]), mx = (a[0] + b[0]) / 2, my = (a[1] + b[1]) / 2, r = sRect();
        zoomAt(p, d / act.pinch.d, mx - r.left - p.rect.x, my - r.top - p.rect.y); panPixels(p, mx - act.pinch.mx, my - act.pinch.my);
        act.pinch = { d, mx, my }; paneChanged(p, 'view'); return;
      }
      const dx = e.clientX - prev[0], dy = e.clientY - prev[1]; if (act.down) act.down.moved += Math.abs(dx) + Math.abs(dy);
      if (act.down && act.down.moved > 4) { panPixels(p, dx, dy); paneChanged(p, 'view'); }
      return;
    }
    hoverAt(e.clientX, e.clientY);
  }, true);
  const up = e => {
    if (act.handle) { act.handle = null; act.id = null; act.pane = null; e.stopPropagation(); return; }
    if (!act.pane || !act.pane.ortho) { act.id = null; act.pane = null; return; }
    e.stopPropagation(); const d = act.down; act.pts.delete(e.pointerId);
    if (act.pts.size < 2) act.pinch = null;
    if (act.pts.size === 0) {
      if (d && d.moved <= 4 && d.button === 0 && e.type === 'pointerup') {
        const hit = pick(e.clientX, e.clientY, act.id);
        if (hit && app.select) { try { app.select(hit.id); } catch (err) { console.warn(err.message); } }
      }
      act.down = null; act.pane = null; act.id = null;
    }
  };
  stage.addEventListener('pointerup', up, true); stage.addEventListener('pointercancel', up, true);
  stage.addEventListener('wheel', e => {
    if (layout === 'single' || onUi(e)) return;
    const id = paneAt(e.clientX, e.clientY); if (!id || id === '3d') return;
    e.preventDefault(); e.stopPropagation();
    const p = panes.get(id), [px, py] = local(e, p); zoomAt(p, Math.exp(-e.deltaY * (e.deltaMode ? 0.05 : 0.0015)), px, py); paneChanged(p, 'view');
  }, { capture: true, passive: false });
  stage.addEventListener('dblclick', e => {
    if (layout === 'single') return; const id = paneAt(e.clientX, e.clientY); if (!id || id === '3d' || onUi(e)) return;
    e.stopPropagation(); const hit = pick(e.clientX, e.clientY, id);
    if (hit && app.getState(hit.id) && ['door', 'window'].includes((app.stateList().find(s => s.id === hit.id) || {}).kind)) app.toggleState(hit.id);
  }, true);
  stage.addEventListener('contextmenu', e => { if (layout !== 'single' && paneAt(e.clientX, e.clientY) !== '3d') e.preventDefault(); });
  // hover: what is under the pointer, once per frame
  let hv = null, pending = null;
  function hoverAt(cx, cy) {
    pending = [cx, cy]; if (hv) return;
    hv = requestAnimationFrame(() => {
      hv = null; if (!pending || app.mode === 'walk' || app.mode === 'fly') return; const [x, y] = pending; pending = null;
      const hit = pick(x, y); const key = hit ? hit.pane + ':' + hit.id : '';
      if (key !== hoverAt.last) {
        hoverAt.last = key; const nd = hit && app.nodeByGuid.get(hit.id);
        const label = hit ? (nd ? nd.l : (app.svc && app.svc.thingFor(hit.id) ? app.svc.thingFor(hit.id).d.label : hit.id)) : null;
        stage.style.cursor = hit ? 'pointer' : ''; api.hovered = hit ? { pane: hit.pane, id: hit.id, label } : null;
        app.emit('hover', api.hovered || { pane: null, id: null, label: null });
      }
    });
  }
}

// ---------- start ----------
let api = null;
export function initViewport() {
  if (api) return api;
  canvas = app.renderer.domElement; stage = canvas.parentElement;
  host = $el('div', 'position:absolute;inset:0;pointer-events:none;z-index:3'); host.id = 'vpPanes'; stage.insertBefore(host, canvas.nextSibling);
  const p3 = { id: '3d', kind: '3d', ortho: false, rect: { x: 0, y: 0, w: 100, h: 100 }, dirty: true, ui: {} };
  readStage();
  for (const id of ORDER) panes.set(id, id === '3d' ? Object.assign(makePane('3d'), { camera: app.camera }) : makePane(id));
  void p3;
  SEC.initSection();
  api = { setLayout, getLayout: () => layout, maximise, getMaximised: () => maximised, panes: panes_, pane: id => describe(panes.get(id)), setView, fit, zoomBy, setNorthUp, setBackground, project, pick, paneAt, invalidate, stats: () => ({ frames: stats.frames, drawn: { ...stats.drawn }, drawCalls: stats.drawCalls, lastFrame: { calls: stats.frameCalls, triangles: stats.frameTris, panes: stats.framePanes } }),
    render, resize, onSection, facadeOf, hovered: null, get active() { return true; }, bounds: () => { const b = bounds(); return { min: b.lo.toArray(), max: b.hi.toArray() }; }, _panes: panes };
  app.viewport = api;
  computeRects(); applyRects(); setupInput();
  app.on('select', frameSelection);
  app.on('site', () => { for (const p of panes.values()) if (p.ortho) { syncPaneUi(p); p.dirty = true; } app.requestRender(); });
  // walking or flying in a quad: the 3D pane takes the whole viewport, and gives it back afterwards
  app.on('mode', e => {
    if (layout !== 'quad') return;
    if ((e.mode === 'walk' || e.mode === 'fly') && maximised !== '3d') { maximise('3d'); autoMax = true; }
    else if (e.mode === 'orbit' && autoMax) { maximise(null); }
  });
  installCamera(); initGround(); initRooms();
  app.reset = reset; app.on('interactive', captureDefaults); if (app.interactive) captureDefaults(); app.on('ready', captureLayers); if (app.ready) captureLayers();
  return api;
}

// ---------- camera modes: app.camera.setMode('orbit' | 'walk' | 'fly') ----------
// app.camera is the THREE camera; these are added to it. orbit -> fly keeps the pose; fly -> walk puts the walker on the floor below the
// camera (the nearest free spot); walk -> fly keeps the eye position and direction; anything -> orbit looks at the house from where it is.
//   camera.setMode(mode, {position: [x, y, z], look: [dx, dy, dz]})  -> Promise of {mode, position, yaw, pitch}
//   camera.getMode()  camera.pose() -> {position, yaw, pitch}    (event 'mode' {mode} on every change)
function installCamera() {
  const cam = app.camera; let flyP = null;
  app.loadFly = () => flyP || (flyP = import('./fly.js').then(m => { m.initFly(); return app.fly; }));
  const poseNow = () => { const f = new THREE.Vector3(0, 0, -1).applyQuaternion(cam.quaternion); return { position: cam.position.toArray().map(v => +v.toFixed(3)), yaw: +Math.atan2(-f.x, f.y).toFixed(4), pitch: +Math.asin(Math.max(-1, Math.min(1, f.z))).toFixed(4) }; };
  const yawOf = d => Math.atan2(-d[0], d[1]), pitchOf = d => Math.atan2(d[2], Math.hypot(d[0], d[1]));
  cam.getMode = () => app.mode; cam.pose = poseNow;
  cam.setMode = async (mode, o = {}) => {
    if (!['orbit', 'walk', 'fly'].includes(mode)) throw new Error('Camera mode must be orbit, walk or fly.');
    if (app.animateCam && app.animateCam.cancel) app.animateCam.cancel();   // a flight to a selection must not carry on moving the camera
    const from = app.mode, pos = Array.isArray(o.position) ? o.position.map(Number) : null, dir = Array.isArray(o.look) ? [+o.look[0], +o.look[1], +(o.look[2] || 0)] : null;
    if (pos && (pos.length < 2 || pos.some(v => !Number.isFinite(v)))) throw new Error('position must be [x, y, z] in metres.');
    if (dir && !Math.hypot(...dir)) throw new Error('look must be a direction [dx, dy, dz].');
    if (mode === 'fly') {
      const fly = await app.loadFly(); let p = null;
      if (from === 'walk') { p = poseNow(); await app.walk.leave(); cam.position.set(...p.position); }
      fly.enter({ position: pos ? [pos[0], pos[1], pos[2] ?? cam.position.z] : (p ? p.position : undefined), yaw: dir ? yawOf(dir) : (p ? p.yaw : undefined), pitch: dir ? pitchOf(dir) : (p ? p.pitch : undefined) });
    } else if (mode === 'walk') {
      let x, y, storey = 0, yaw = dir ? yawOf(dir) : undefined;
      const ups = app.graph.storeys.length > 1 ? +app.graph.storeys[1].el : 2.7;
      if (pos) { x = pos[0]; y = pos[1]; storey = (pos[2] ?? 0) > ups + 0.5 ? 1 : 0; }
      else if (from === 'fly') { const p = poseNow(); x = p.position[0]; y = p.position[1]; storey = p.position[2] > ups + 0.8 ? 1 : 0; if (yaw === undefined) yaw = p.yaw; }
      const w = await app.loadWalk();
      if (from === 'fly') { app.fly.leave({ silent: true }); }
      if (x === undefined) { w.enter(yaw !== undefined ? { yaw } : {}); }
      else { [x, y] = w.findFree(x, y, storey); w.enter({ x, y, storey, yaw: yaw ?? 0 }); }
    } else {
      if (from === 'walk') await app.walk.leave(); else if (from === 'fly') app.fly.leave();
      if (pos) { const z = pos[2] ?? cam.position.z; cam.position.set(pos[0], pos[1], z); const t = dir ? new THREE.Vector3(...pos.slice(0, 2), z).addScaledVector(new THREE.Vector3(...dir).normalize(), 8) : app.controls.target.clone(); app.controls.target.copy(t); cam.lookAt(t); app.controls.update(); }
    }
    app.requestRender(); return { mode: app.mode, ...poseNow() };
  };
}
