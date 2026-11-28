// The structure layer: foundations, slab layers, ring beams, lintels, wall heads, trimmer, posts, collar ties and wind bracing
// drawn at true size from the 11th system of the systems file, plus the load model: roles, load paths, "remove this element".
//
// The parts are nodes of the services layer (systems.js builds them with structureNodeGroup() and draws them through its batch), so
// layers, isolate, x-ray, picking and the info panel work for them as for every other thing. Elements with `embedded: true` (they lie
// inside a wall or slab) are drawn only while x-ray, isolate, a section cut or a load path shows them.
//
// API (app.structure; event 'structure' {removed, unsupported}):
//   list({role, loadBearing, fromIfc, embedded})  elements: [{id, label, role, fromIfc, loadBearing, embedded, bearsOn, state, ifc: guid | null}]
//   setColourMode('material' | 'role' | 'load-bearing' | 'from-ifc'), getColourMode(), legend()   tints the house and the structure parts
//   tracePath(idOrGuid, 'down' | 'up')   down: everything this element bears on, to the ground; up: everything that bears on it.
//                                        Highlights it (x-ray on, embedded parts shown). Returns {id, direction, ids, labels, path}.  clearPath()
//   remove(idOrGuid)                     state 'removed': hidden; everything with no complete bearsOn path to the ground through intact
//                                        elements is marked unsupported (red tint, a marker, a reason). Returns state().
//   restore(idOrGuid?)                   back to 'intact' (all of them without an argument)
//   state()                              {removed: [ids], unsupported: [{id, label, reason}]};  stateOf(id) 'intact' | 'removed'
//   unsupportedFor(removedIds)           the exact set for a hypothetical removal (does not change anything)
//   applyScenario(id)                    a scenario with kind 'structure': removes its `off` elements, returns the same shape as svc.applyScenario
//   info(id)                             what to show about an element (role, load path counts, state, reason)
// The rule (rule-bearing): an element is supported when at least one complete bearsOn path to `ground` exists through intact elements.
import * as THREE from 'three';
import { app } from './app.js';

const ROLE_COL = {
  'load-bearing-wall': '#c0504d', 'non-bearing-wall': '#8fb3d9', slab: '#8c8f94', 'roof-covering': '#7d4a3a', rafter: '#d9a441', 'eaves-purlin': '#c88a2b', 'ridge-purlin': '#b5731c', beam: '#a05a2c', stair: '#8d6fb5',
  ground: '#6b5a43', 'strip-foundation': '#6c7076', 'pad-foundation': '#5c6168', 'slab-layer': '#b3a78c', 'perimeter-insulation': '#5fb7ac', 'earth-electrode': '#e07b39', 'ring-beam': '#3d7ea6', lintel: '#2f9e8f', 'wall-head': '#b8a88a', trimmer: '#3d6fa6', post: '#7cb342', 'collar-tie': '#f2c14e', bracing: '#d84a8b',
};
const NOT_BEARING = new Set(['non-bearing-wall', 'roof-covering', 'stair', 'perimeter-insulation', 'earth-electrode', 'ground']);
// what the part is made of (colour, roughness, metalness) by type or layer
const MATERIAL = { ground: ['#6f5a40', 1, 0], 'strip-foundation': ['#8f9096', 0.95, 0], 'pad-foundation': ['#8f9096', 0.95, 0], 'perimeter-insulation': ['#8fd0c9', 0.8, 0], 'earth-electrode': ['#b87333', 0.4, 0.8], 'ring-beam': ['#7f858d', 0.95, 0], lintel: ['#7f858d', 0.95, 0], 'wall-head': ['#c9c2b3', 0.9, 0], trimmer: ['#7f858d', 0.95, 0], post: ['#b98a5a', 0.7, 0], 'collar-tie': ['#b98a5a', 0.7, 0], bracing: ['#7c868f', 0.4, 0.8] };
const LAYER = { 'layer-gravel': ['#b3a78c', 1, 0], 'layer-blinding': ['#a6a8a6', 0.95, 0], 'layer-membrane': ['#26292f', 0.6, 0], 'layer-xps': ['#8fd0c9', 0.8, 0], 'layer-impact': ['#e8d27a', 0.8, 0] };
const TRACE = '#33d6ff', AMBER = '#ffa13d', RED = '#ff5a5a', DIM = 0.3;
const pairs = new Map();
function pairFor(spec) {   // {live, dead}, like fixtures.js mat()
  const k = spec.join('|'); if (pairs.has(k)) return pairs.get(k);
  const live = new THREE.MeshStandardMaterial({ color: spec[0], roughness: spec[1], metalness: spec[2] });
  const c = new THREE.Color(spec[0]), l = c.r * 0.3 + c.g * 0.59 + c.b * 0.11, dead = live.clone(); dead.color.setRGB(l * 0.55 + 0.12, l * 0.55 + 0.12, l * 0.55 + 0.13);
  const p = { live, dead }; pairs.set(k, p); return p;
}
// ---------- the drawing of a node (world coordinates; the object stays at the origin) ----------
export function structureNodeGroup(node) {
  const spec = LAYER[node.id] || MATERIAL[node.type] || ['#8d99ae', 0.8, 0], pair = pairFor(spec);
  let geo;
  if (node.type === 'bracing' && node.a && node.b) {
    const a = new THREE.Vector3(...node.a), b = new THREE.Vector3(...node.b), k = b.clone().sub(a), len = k.length(); k.normalize();
    const z = new THREE.Vector3(0, 0, 1), h0 = z.clone().addScaledVector(k, -z.dot(k)).normalize(), r = node.roll || 0;
    const kh = new THREE.Vector3().crossVectors(k, h0), h = h0.clone().multiplyScalar(Math.cos(r)).addScaledVector(kh, Math.sin(r)), bb = new THREE.Vector3().crossVectors(k, h);   // the section axes after the roll
    const [sb, sh] = node.section || [0.04, 0.005];
    geo = new THREE.BoxGeometry(sh, sb, len);   // local x: along h, y: along b, z: along the member (h x b = k: right handed)
    geo.applyMatrix4(new THREE.Matrix4().makeBasis(h, bb, k).setPosition(a.clone().add(b).multiplyScalar(0.5)));
  } else {
    const s = node.size || [0.1, 0.1, 0.1]; geo = new THREE.BoxGeometry(s[0], s[1], s[2]); geo.translate(node.p[0], node.p[1], node.p[2]);
  }
  const mesh = new THREE.Mesh(geo, pair.live); mesh.userData.mats = pair; mesh.userData.zi = ({ 'strip-foundation': 1, 'pad-foundation': 4, 'slab-layer': 3, 'perimeter-insulation': 2 })[node.type] ?? (node.embedded ? 5 : 2);   // parts that touch get different polygon offsets (batch.js)
  const g = new THREE.Group(); g.add(mesh);
  return { group: g, meshes: [mesh], dyn: {}, extent: 1, unknown: false };
}

// ---------- the model ----------
let data = null, svc = null, S = null;
const els = new Map(), byGuid = new Map();
let mode = 'material', pathSet = null, pathInfo = null, xrayWas = null, forceEmbedded = false, scenarioSet = null;
let marks = null;
const key = e => e.guid || e.id;

function build() {
  els.clear(); byGuid.clear();
  for (const e of S.elements) {
    const node = e.id ? data.nodes.find(n => n.id === e.id) : null;
    const rec = { id: key(e), guid: e.guid || null, label: e.name || (node && node.label) || e.id, role: e.role, fromIfc: !!e.fromIfc, embedded: !!(e.embedded || (node && node.embedded)) || key(e) === 'ground', bearsOn: (e.bearsOn || []).slice(), props: e.props || {}, state: 'intact', node: !!node, ifcClass: e.ifcClass || null,
      loadBearing: !NOT_BEARING.has(e.role) && !(e.role === 'slab-layer' && false) };
    els.set(rec.id, rec); if (rec.guid) byGuid.set(rec.guid, rec);
  }
  // `ground` is a node of the structure system too (it is not in the element list)
  if (!els.has('ground')) { const g = data.nodes.find(n => n.id === 'ground'); if (g) els.set('ground', { id: 'ground', guid: null, label: g.label, role: 'ground', fromIfc: false, embedded: true, bearsOn: [], props: g.props || {}, state: 'intact', node: true, loadBearing: false }); }
}
const resolve = x => { const k = String(x); const e = els.get(k) || byGuid.get(k); if (e) return e; const lc = k.toLowerCase(); const by = [...els.values()].filter(e => e.label.toLowerCase() === lc); if (by.length === 1) return by[0]; throw new Error(`No structural element "${x}". Use list_structure to find one.`); };

// the exact rule
function supportedSet(removed) {
  const rem = new Set(removed), memo = new Map();
  const ok = (id, stack) => {
    if (id === 'ground') return true; if (rem.has(id)) return false; if (memo.has(id)) return memo.get(id);
    if (stack.has(id)) return false; stack.add(id); const e = els.get(id); let r = false;
    if (e) for (const b of e.bearsOn) if (ok(b, stack)) { r = true; break; }
    stack.delete(id); memo.set(id, r); return r;
  };
  return id => ok(id, new Set());
}
export function unsupportedFor(removed) {
  const sup = supportedSet(removed), rem = new Set(removed);
  return [...els.keys()].filter(id => id !== 'ground' && !rem.has(id) && !sup(id));
}
const closure = (id, dir) => {   // dir 'down': what it bears on; 'up': what bears on it
  const out = new Set(), q = [id];
  const up = new Map(); if (dir === 'up') for (const e of els.values()) for (const b of e.bearsOn) (up.get(b) || up.set(b, []).get(b)).push(e.id);
  while (q.length) { const x = q.pop(); const nx = dir === 'down' ? (els.get(x) ? els.get(x).bearsOn : []) : (up.get(x) || []); for (const n of nx) if (!out.has(n)) { out.add(n); q.push(n); } }
  return out;
};
function reasonFor(id, removedIds) {
  const via = [...closure(id, 'down')].filter(x => removedIds.includes(x)).map(x => els.get(x).label);
  return via.length ? `No complete load path to the ground: it bears on ${via.join(', ')}, which is removed.` : 'No complete load path to the ground.';
}
const removedIds = () => [...els.values()].filter(e => e.state === 'removed').map(e => e.id);
let unsup = [];

// ---------- looks ----------
const thingOf = id => svc && svc.thingFor(id);
const objOfIfc = guid => app.meshByGuid.get(guid);
function colourFor(e) {   // {hex, glow, opacity} for one element under the current mode, path and cut
  if (e.state === 'removed') return null;
  const u = unsup.find(x => x.id === e.id); if (u) return { hex: RED, glow: true };
  if (pathSet && pathSet.has(e.id)) return { hex: TRACE, glow: true };
  if (scenarioSet && scenarioSet.has(e.id)) return { hex: AMBER, glow: true };
  if (mode === 'role') return { hex: ROLE_COL[e.role] || '#8d99ae' };
  if (mode === 'load-bearing') return { hex: e.loadBearing ? '#d9822b' : '#5b8fd1' };
  if (mode === 'from-ifc') return { hex: e.fromIfc ? '#4f8fd9' : '#e0a030' };
  return null;
}
const tint = new Map();   // guid -> {hex, opacity}
function applyTints() {
  tint.clear();
  const dim = mode !== 'material' && !pathSet;
  for (const e of els.values()) {
    const c = colourFor(e);
    if (e.guid) { if (c) tint.set(e.guid, { hex: c.hex, opacity: undefined }); }
    else { const t = thingOf(e.id); if (t) { t.modeHex = c ? new THREE.Color(c.hex).getHex() : null; t.glow = !!(c && c.glow); svc.refresh(t); } }
  }
  void dim;
  if (app.restyle) app.restyle(); else for (const [g] of tint) app.paint(objOfIfc(g), null);
  syncEmbedded(); updateMarks(); app.requestRender();
}
// the wrapper of paint(): tint, dim the rest in a colour mode, hide removed elements. Installed once.
function wrapPaint() {
  const orig = app.paint;
  app.paint = (obj, style) => {
    const g = obj && obj.userData && obj.userData.guid, e = g && byGuid.get(g);
    if (e && e.state === 'removed') { orig(obj, { opacity: 0 }); obj.traverse(o => { if (o.isMesh) { o.userData.alpha = 0; o.userData.removedStructure = true; } }); return; }
    const t = g && tint.get(g), n = g && app.nodeByGuid.get(g);
    if (t && !(style && style.color != null)) {
      const xr = app.svc && app.svc.isXray() && n && (n.c === 'IfcWallStandardCase' || n.c === 'IfcSlab');
      style = { ...(style || {}), color: t.hex, ...(xr ? { opacity: Math.min(0.14, style && style.opacity != null ? style.opacity : 1) } : {}) };
    } else if (!t && g && n && mode !== 'material' && !pathSet && n.k !== 'Space' && n.c !== 'IfcOpeningElement' && !(style && style.opacity != null) && !e) style = { ...(style || {}), opacity: DIM };
    return orig(obj, style);
  };
}
// an embedded part is drawn in x-ray, with isolate, in a load path or a scenario, and where a section cut passes through it (the cut exposes it)
const cutThrough = n => {
  if (!app.section || !n) return false; const g = app.section.get(), sz = n.size || [0, 0, 0];
  return ['x', 'y', 'z'].some((a, i) => g[a].enabled && Math.abs(g[a].position - n.p[i]) <= sz[i] / 2 + 0.002);
};
function syncEmbedded() {
  const on = !!(svc.isXray() || svc.isolated || forceEmbedded);
  for (const e of els.values()) { const t = thingOf(e.id); if (t) t.obj.visible = e.state !== 'removed' && (!e.embedded || on || cutThrough(t.d)); }
  svc.batch.changed = true; app.requestRender();
}
// markers over what has lost its support
function updateMarks() {
  if (!marks) { marks = new THREE.Group(); marks.name = 'structureExtras'; app.scene.add(marks); }
  marks.clear();
  for (const u of unsup) {
    const e = els.get(u.id), box = new THREE.Box3(), o = e.guid ? objOfIfc(e.guid) : (thingOf(e.id) || {}).obj; if (!o) continue;
    o.updateWorldMatrix(true, true); box.setFromObject(o); if (box.isEmpty()) continue;
    const c = box.getCenter(new THREE.Vector3()); c.z = box.max.z + 0.12;
    const s = new THREE.Sprite(markerMat()); s.scale.set(0.22, 0.22, 1); s.position.copy(c); s.renderOrder = 9; s.userData.noClip = true; marks.add(s);
  }
}
let mm = null;
function markerMat() {
  if (mm) return mm; const c = document.createElement('canvas'); c.width = c.height = 64; const g = c.getContext('2d');
  g.fillStyle = 'rgba(20,24,34,.9)'; g.beginPath(); g.arc(32, 32, 28, 0, 7); g.fill(); g.strokeStyle = '#ff5a5a'; g.lineWidth = 6; g.stroke(); g.fillStyle = '#ff5a5a'; g.font = '700 38px system-ui'; g.textAlign = 'center'; g.fillText('!', 32, 46);
  const t = new THREE.CanvasTexture(c); t.colorSpace = THREE.SRGBColorSpace; mm = new THREE.SpriteMaterial({ map: t, transparent: true, depthTest: false, depthWrite: false, fog: false }); return mm;
}

// ---------- actions ----------
function recompute() {
  const rem = removedIds(); unsup = unsupportedFor(rem).map(id => ({ id, label: els.get(id).label, reason: reasonFor(id, rem) }));
  applyTints(); app.emit('structure', state()); return state();
}
export function remove(x) { const e = resolve(x); if (e.id === 'ground') throw new Error('The ground cannot be removed.'); e.state = 'removed'; return afterChange(); }
export function restore(x) { if (x == null) els.forEach(e => { e.state = 'intact'; }); else resolve(x).state = 'intact'; return afterChange(); }
function afterChange() {
  for (const e of els.values()) {
    if (e.guid) { const o = objOfIfc(e.guid); if (o) { if (e.state === 'removed') { o.visible = false; } else { o.visible = true; o.traverse(m => { if (m.isMesh) m.userData.removedStructure = false; }); } } }
  }
  if (app.applyVisibility) app.applyVisibility();   // gives back the class and storey visibility of what was restored
  for (const e of els.values()) if (e.guid && e.state === 'removed') { const o = objOfIfc(e.guid); if (o) o.visible = false; }
  const r = recompute(); if (app.shadowDirty) app.shadowDirty(); return r;
}
export const state = () => ({ removed: removedIds(), unsupported: unsup.map(u => ({ ...u })) });
export const stateOf = id => resolve(id).state;
export function setColourMode(m) {
  if (!['material', 'role', 'load-bearing', 'from-ifc'].includes(m)) throw new Error('Colour mode must be material, role, load-bearing or from-ifc.');
  mode = m; applyTints(); app.emit('structurecolour', { mode, legend: legend() }); return mode;
}
export function legend() {
  if (mode === 'role') return { mode, items: Object.keys(ROLE_COL).map(r => ({ label: r, color: ROLE_COL[r] })) };
  if (mode === 'load-bearing') return { mode, items: [{ label: 'load-bearing', color: '#d9822b' }, { label: 'not load-bearing', color: '#5b8fd1' }, { label: 'other parts of the house', color: '#9aa0a6' }] };
  if (mode === 'from-ifc') return { mode, items: [{ label: 'from the IFC', color: '#4f8fd9' }, { label: 'designed here', color: '#e0a030' }, { label: 'other parts of the house', color: '#9aa0a6' }] };
  return null;
}
export function tracePath(x, dir = 'down') {
  if (dir !== 'down' && dir !== 'up') throw new Error('direction must be "down" or "up".');
  const e = resolve(x), c = closure(e.id, dir); pathSet = new Set([e.id, ...c]);
  // one chain to the ground for 'down': the shortest
  let path = [e.id];
  if (dir === 'down') { const prev = new Map([[e.id, null]]), q = [e.id]; while (q.length) { const a = q.shift(); if (a === 'ground') break; for (const b of els.get(a) ? els.get(a).bearsOn : []) if (!prev.has(b)) { prev.set(b, a); q.push(b); } } path = []; for (let a = prev.has('ground') ? 'ground' : null; a != null; a = prev.get(a)) path.unshift(a); }   // the element first, the ground last
  pathInfo = { id: e.id, direction: dir, ids: [...pathSet], labels: [...pathSet].map(i => els.get(i).label), path };
  if (!svc.isXray()) { xrayWas = false; svc.setXray(true); }
  const lay = svc.layers.get('structure'); if (lay && !lay.isVisible()) lay.setVisible(true);
  forceEmbedded = true; applyTints();
  const objs = pathInfo.ids.map(i => { const q = els.get(i); return q.guid ? objOfIfc(q.guid) : (thingOf(i) || {}).obj; }).filter(Boolean);
  if (app.mode === 'orbit' && objs.length) app.focus(objs);
  app.emit('structurepath', pathInfo); return pathInfo;
}
export function clearPath() {
  if (!pathSet) return; pathSet = null; pathInfo = null; forceEmbedded = false;
  if (xrayWas === false && svc.isXray()) svc.setXray(false); xrayWas = null;
  applyTints(); app.emit('structurepath', null);
}
export function applyScenario(id) {
  const s = data.scenarios.find(x => x.id === id && x.kind === 'structure'); if (!s) throw new Error(`No structural scenario "${id}".`);
  restore(); s.off.forEach(o => { resolve(o).state = 'removed'; });
  scenarioSet = new Set(s.off.map(o => resolve(o).id));
  const off = s.off.map(o => resolve(o).id); afterChange();
  const affected = unsup.map(u => u.id), exp = new Set(s.affected), got = new Set(affected);
  const result = { id, kind: 'structure', label: s.label, description: s.description, off, affected, affectedRuns: [], expected: s.affected.length, same: exp.size === got.size && [...exp].every(x => got.has(x)), sparql: s.sparql, reasons: unsup.map(u => ({ id: u.id, reason: u.reason })) };
  forceEmbedded = true; svc.setXray(true); const lay = svc.layers.get('structure'); if (lay && !lay.isVisible()) lay.setVisible(true); syncEmbedded();
  app.emit('scenario', result); return result;
}
export function clearScenario() { scenarioSet = null; forceEmbedded = false; restore(); }
export function list(f = {}) {
  return [...els.values()].filter(e => (!f.role || e.role === f.role) && (f.loadBearing == null || e.loadBearing === !!f.loadBearing) && (f.fromIfc == null || e.fromIfc === !!f.fromIfc) && (f.embedded == null || e.embedded === !!f.embedded))
    .map(e => ({ id: e.id, label: e.label, role: e.role, fromIfc: e.fromIfc, loadBearing: e.loadBearing, embedded: e.embedded, bearsOn: e.bearsOn.slice(), state: e.state, ifc: e.guid }));
}
export function info(id) { const e = resolve(id), u = unsup.find(x => x.id === e.id); return { id: e.id, label: e.label, role: e.role, fromIfc: e.fromIfc, loadBearing: e.loadBearing, embedded: e.embedded, state: e.state, bearsOn: e.bearsOn.map(b => (els.get(b) || { label: b }).label), carries: closure(e.id, 'up').size, supportsDown: closure(e.id, 'down').size, unsupported: !!u, reason: u ? u.reason : null }; }

export function initStructure(service, d) {
  S = d.structure; if (!S || !Array.isArray(S.elements)) return null;
  data = d; svc = service; build(); wrapPaint();
  // the model state goes with the page: x-ray, isolate and a cut show what is embedded
  for (const ev of ['xray', 'isolate', 'section']) app.on(ev, () => syncEmbedded());
  app.structure = { list, setColourMode, getColourMode: () => mode, legend, tracePath, clearPath, remove, restore, state, stateOf, unsupportedFor, applyScenario, clearScenario, info, groundZ: S.groundZ, foundationBaseZ: S.foundationBaseZ, roles: () => Object.keys(ROLE_COL), loads: () => S.loads || null };
  svc.structure = app.structure; syncEmbedded();
  return app.structure;
}
