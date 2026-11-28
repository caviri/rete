// Section cuts: one clipping plane per model axis, shared by every house and services material.
//
// API (app.section, events 'section'):
//   set(axis, {enabled, position, flip})   axis 'x' | 'y' | 'z' (model axes, metres); any of the three fields; returns get()[axis]
//   get()                                  {x, y, z: {enabled, position, flip}, applyTo3D, applyTo3DSetting, handles, storey}
//   reset()                                all cuts off, default positions, applyTo3D auto, storey 0
//   setApplyTo3D(true | false | null)      null = auto: the 3D pane is cut in the single layout, not in the quad layout
//   setStorey(i)                           z cut at 1.2 m above the floor of storey i (the plan pane's cut); returns the position
//   storeys()                              [{index, label, floorZ, cutZ}]
//   setHandles(bool)                       the draggable translucent plane in the 3D pane (on by default)
//   anyEnabled(), clipped(point, panePlanes?)
// Which side is kept (flip inverts it):  z: z <= position (the roof and what is above the cut go);
//   y: y >= position (the part nearer the front elevation goes); x: x <= position (the part nearer the side elevation goes).
// Which pane obeys which cut: the plan pane the z cut, the front elevation the y cut, the side elevation the x cut,
// the 3D pane all enabled cuts when applyTo3D is on.
//
// How a cut looks. The materials of the merged batches carry the three planes (renderer.localClippingEnabled). The IFC
// solids and the pipes are closed, so when a cut removes the near part, the camera sees the inside of the solid: its back
// faces. While a cut is on, the batch materials draw those back faces as a flat, darker, hatched colour of the material
// (batch.js, uCap): a cut wall reads as solid, not hollow, and each material class has its own cut colour. This is not a
// stencil cap: the cut face is drawn at the depth of the far inside face, which only matters for a perspective camera
// close to the cut. Picking ignores clipped geometry (Mesh.raycast is filtered, the proxies are double sided meanwhile).
import * as THREE from 'three';
import { app } from './app.js';

const AX = ['x', 'y', 'z'];
const NEUTRAL = 1e6;
const state = { x: { enabled: false, position: 6, flip: false }, y: { enabled: false, position: 5, flip: false }, z: { enabled: false, position: 1.2, flip: false }, applyTo3D: null, handles: true, storey: 0, auto: false };
let handleGroup = null, handleMeshes = {}, pickOverride = null, cache3d = null;
const tmp = new THREE.Vector3();

const floorZ = i => { const s = app.graph && app.graph.storeys[i]; return s ? +s.el : 0; };
const clampPos = (axis, v) => { const b = app.houseBox; return b ? Math.min(b.max[axis] + 3, Math.max(b.min[axis] - 3, v)) : v; };
function defaults() {
  const b = app.houseBox; const c = b ? b.getCenter(new THREE.Vector3()) : new THREE.Vector3(6, 5, 2);
  return { x: c.x, y: c.y, z: floorZ(0) + 1.2 };
}
export const isOn = () => AX.some(a => state[a].enabled);
// the planes a pane obeys: 'plan' | 'front' | 'side' | '3d' -> [x, y, z] booleans
function activeAxes(kind) {
  if (kind === 'plan') return [false, false, state.z.enabled];
  if (kind === 'front') return [false, state.y.enabled, false];
  if (kind === 'side') return [state.x.enabled, false, false];
  return resolved3D() ? AX.map(a => state[a].enabled) : [false, false, false];
}
export function resolved3D() { return state.applyTo3D == null ? (app.viewport ? app.viewport.getLayout() === 'single' : true) : !!state.applyTo3D; }
function planeValues(axis, s) {   // normal and constant of the kept side
  const k = s.flip ? -1 : 1;
  if (axis === 'z') return [0, 0, -k, s.position * k];
  if (axis === 'y') return [0, k, 0, -s.position * k];
  return [-k, 0, 0, s.position * k];
}
// write the planes of one pane into the shared array; returns true when this pane is cut
export function beginPane(kind) {
  const on = activeAxes(kind), P = app.clipPlanes; let any = false;
  AX.forEach((a, i) => {
    const p = P[i];
    if (on[i]) { const v = planeValues(a, state[a]); p.normal.set(v[0], v[1], v[2]); p.constant = v[3]; any = true; }
    else { p.normal.set(0, 0, 1); p.constant = NEUTRAL; }
  });
  setCap(any); return any;
}
// the batch materials: cut faces on or off
function setCap(on) {
  for (const set of app.batchSetList || []) for (const b of set.batches.values()) {
    if (!b.mats) continue;
    for (const k of ['opaque', 'trans']) {
      const m = b.mats[k]; if (m.userData.side0 == null) m.userData.side0 = m.side;
      if (k === 'opaque') { m.userData.cap.value = on ? 1 : 0; m.side = on ? THREE.DoubleSide : m.userData.side0; }
    }
  }
}
// planes of one pane as a list of THREE.Plane copies (for picking and tools)
export function planesOf(kind) {
  const on = activeAxes(kind);
  return AX.map((a, i) => on[i] ? new THREE.Plane(new THREE.Vector3(...planeValues(a, state[a]).slice(0, 3)), planeValues(a, state[a])[3]) : null).filter(Boolean);
}
export function clipped(p, planes) {
  for (const pl of planes || planesOf('3d')) if (pl.distanceToPoint(p) < 0) return true;
  return false;
}
// while pickOverride is an array, Mesh.raycast drops the hits in the cut away; null: the cut of the 3D pane
export function withPickPlanes(planes, fn) { const was = pickOverride; pickOverride = planes; try { return fn(); } finally { pickOverride = was; } }
export function noFilter(fn) { return withPickPlanes([], fn); }
let patched = false;
function patchRaycast() {
  if (patched) return; patched = true;
  const orig = THREE.Mesh.prototype.raycast;
  THREE.Mesh.prototype.raycast = function (raycaster, intersects) {
    const planes = pickOverride || (cache3d || (cache3d = planesOf('3d')));
    if (!planes.length || this.userData.noClip) return orig.call(this, raycaster, intersects);
    const mat = this.material, was = mat && mat.side;
    const box = mat === app.PROXY; if (box) mat.side = THREE.DoubleSide;   // the near face may be cut away: the far face of the solid is what is seen
    const hits = [];
    try { orig.call(this, raycaster, hits); } finally { if (box) mat.side = was; }
    for (const h of hits) if (!clipped(h.point, planes)) intersects.push(h);
  };
}

// ---------- the draggable plane in the 3D pane ----------
function buildHandles() {
  if (handleGroup) return;
  handleGroup = new THREE.Group(); handleGroup.name = 'sectionHandles'; handleGroup.visible = false;
  const cols = { x: 0xe5484d, y: 0x3fb950, z: 0x3b82f6 };
  for (const a of AX) {
    const m = new THREE.Mesh(new THREE.PlaneGeometry(1, 1), new THREE.MeshBasicMaterial({ color: cols[a], transparent: true, opacity: 0.16, side: THREE.DoubleSide, depthWrite: false, fog: false, toneMapped: false }));
    m.userData.axis = a; m.userData.noClip = true; m.renderOrder = 20; m.visible = false;
    const e = new THREE.LineSegments(new THREE.EdgesGeometry(m.geometry), new THREE.LineBasicMaterial({ color: cols[a], transparent: true, opacity: 0.8, fog: false, toneMapped: false }));
    m.add(e); handleMeshes[a] = m; handleGroup.add(m);
  }
  app.scene.add(handleGroup);
}
function placeHandles() {
  if (!handleGroup || !app.houseBox) return;
  const b = app.houseBox, c = b.getCenter(new THREE.Vector3()), s = b.getSize(new THREE.Vector3());
  for (const a of AX) {
    const m = handleMeshes[a], st = state[a];
    m.visible = st.enabled && resolved3D() && state.handles;
    if (!m.visible) continue;
    if (a === 'z') { m.position.set(c.x, c.y, st.position); m.rotation.set(0, 0, 0); m.scale.set(s.x + 3, s.y + 3, 1); }
    else if (a === 'x') { m.position.set(st.position, c.y, c.z); m.rotation.set(0, Math.PI / 2, 0); m.scale.set(s.z + 3, s.y + 3, 1); }
    else { m.position.set(c.x, st.position, c.z); m.rotation.set(Math.PI / 2, 0, 0); m.scale.set(s.x + 3, s.z + 3, 1); }
    m.updateMatrixWorld(true);
  }
}
export function showHandles(on) { if (handleGroup) handleGroup.visible = !!on; }
// the handle under a ray (Raycaster), or null
export function hitHandle(raycaster) {
  if (!handleGroup || !state.handles || !resolved3D()) return null;
  const ms = AX.map(a => handleMeshes[a]).filter(m => m.visible);
  handleGroup.visible = true;
  const hs = raycaster.intersectObjects(ms, false); return hs.length ? hs[0].object.userData.axis : null;
}
// move the plane of `axis` to where the ray passes closest to the axis line through the centre of the house
export function dragTo(axis, raycaster) {
  const c = app.houseBox.getCenter(new THREE.Vector3()), a = new THREE.Vector3(); a[axis] = 1;
  const o = raycaster.ray.origin, d = raycaster.ray.direction, w = o.clone().sub(c);
  const b = d.dot(a), dd = d.dot(w), ee = a.dot(w), den = 1 - b * b;
  if (Math.abs(den) < 1e-4) return null;
  const t = (ee - b * dd) / den;   // parameter along the axis line
  return set(axis, { enabled: true, position: +(c[axis] + t).toFixed(3) });
}

// ---------- the API ----------
const view = a => ({ enabled: state[a].enabled, position: +state[a].position.toFixed(3), flip: state[a].flip });
export function get() {
  return { x: view('x'), y: view('y'), z: view('z'), applyTo3D: resolved3D(), applyTo3DSetting: state.applyTo3D, handles: state.handles, storey: state.storey, kept: { x: 'x <= position', y: 'y >= position', z: 'z <= position', note: 'flip keeps the other side' } };
}
function changed(axis) {
  cache3d = null; placeHandles(); if (app.shadowDirty) app.shadowDirty();
  if (app.viewport) app.viewport.onSection();
  app.requestRender(); app.emit('section', get()); return axis ? view(axis) : get();
}
export function set(axis, o = {}) {
  if (!AX.includes(axis)) throw new Error(`Unknown axis "${axis}". Axes: x, y, z.`);
  const s = state[axis];
  if (o.position != null) { const v = +o.position; if (!Number.isFinite(v)) throw new Error('position must be a number (metres, model frame).'); s.position = clampPos(axis, v); }
  if (o.enabled != null) { s.enabled = !!o.enabled; if (axis === 'z') state.auto = false; }
  if (o.flip != null) s.flip = !!o.flip;
  if (o.applyTo3D !== undefined) state.applyTo3D = o.applyTo3D == null ? null : !!o.applyTo3D;
  return changed(axis);
}
export function setApplyTo3D(v) { state.applyTo3D = v == null ? null : !!v; return changed(); }
export function setHandles(v) { state.handles = !!v; return changed(); }
export function storeys() { return (app.graph ? app.graph.storeys : []).map((s, i) => ({ index: i, label: s.l, floorZ: +s.el, cutZ: +(+s.el + 1.2).toFixed(3) })); }
export function setStorey(i) {
  i = Number(i); const n = app.graph ? app.graph.storeys.length : 1;
  if (!Number.isInteger(i) || i < 0 || i >= n) throw new Error('storey must be 0..' + (n - 1) + '.');
  state.storey = i; state.z.position = +(floorZ(i) + 1.2).toFixed(3); changed('z'); return state.z.position;
}
export function reset() {
  const d = defaults(), was = get();
  for (const a of AX) { state[a].enabled = false; state[a].flip = false; state[a].position = +d[a].toFixed(3); }
  state.applyTo3D = null; state.handles = true; state.storey = 0; state.auto = false;
  changed(); return was;
}
// the quad layout turns the z cut on for the plan pane; leaving it undoes that, unless the person has touched the cut since
export function layoutChanged(layout) {
  if (layout === 'quad' && !state.z.enabled) { state.z.enabled = true; state.auto = true; state.z.position = +(floorZ(state.storey) + 1.2).toFixed(3); }
  else if (layout === 'single' && state.auto) { state.z.enabled = false; state.auto = false; }
  changed();
}
export function initSection() {
  patchRaycast(); buildHandles();
  const d = defaults(); state.x.position = +d.x.toFixed(3); state.y.position = +d.y.toFixed(3); state.z.position = +d.z.toFixed(3);
  app.section = { set, get, reset, setApplyTo3D, setHandles, setStorey, storeys, anyEnabled: isOn, clipped: (p, k) => clipped(Array.isArray(p) ? new THREE.Vector3(...p) : p, k ? planesOf(k) : null), planesOf, hitHandle, dragTo, noFilter, withPickPlanes };
  app.PROXY = app.PROXY || null;
  placeHandles();
  return app.section;
}
export const _internal = { beginPane, showHandles, placeHandles, layoutChanged, state };
