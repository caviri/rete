// State variables: what a thing can be in, from the data (`stateVariables`, `stateValues`, `rules` of the systems file).
// One source for: the states a door, window or device may take (states.js and systems.js read them from here), numeric variables
// with range, unit and step, levels with colours (including the bands that are offsets from the room set point), the current value
// of a variable per thing, and the 3D colouring of rooms and surfaces by a variable.
//
// API (app.statevars; absent when the systems file is missing; events 'value' {variable, thing, value, prev, pinned, source},
//      'values' {source, count}, 'colour' {variable} | {variable: null}):
//   variables(filter?)           [{id, label, kind: 'numeric'|'enumerated', appliesTo, unit, unitLabel, min, max, step, setBy, default, relativeTo, levels | values}]
//                                filter: {kind: 'room'|'surface'|'device'|'opening'|'structure'} or {thing: id}
//   variable(id)                 one of them, or throws
//   thingKind(id)                'room' | 'surface' | 'device' | 'opening' | 'structure' | null
//   appliesTo(thingId)           ids of the variables that apply to a thing
//   things(kind)                 [{id, label}]
//   getValue(variableId, thingId)            the current value (live state for enumerated ones)
//   setValue(variableId, thingId, value)     validates; throws a clear error for an unknown thing, a thing the variable does not apply to,
//                                            a number outside min..max or a value that is not one of the allowed states.
//                                            A number written for a variable the simulation owns (room air temperature) pins it:
//                                            the thermal model then holds that room at the value. unpin(variableId, thingId) releases it.
//   levelOf(variableId, thingId, value?)     the level (band) of a value: {id, label, color, order, min, max} (bands that are offsets
//                                            from the set point are applied to value - set point of that thing); null without levels
//   allowedStates(thingId)       the states a door, window, device or structural element can take, default first
//   colourBy(variableId | null)  tint rooms (floor and volume) or surfaces (the wall, roof and window faces) by level; returns legend()
//   legend()                     null, or {variable, label, unit, unitLabel, relativeTo, items: [{id, label, color, min, max, text}], things: [{id, label, value, level}]}
//   rules()                      the declarative rules of the data;  supplyBlocking() the blocking state of each device type that gates supply
//   pinned()                     [{variable, thing, value}];  resetValues() initial values again, no pins, no colouring
import * as THREE from 'three';
import { app } from './app.js';

let data = null, vars = [], varById = new Map(), values = new Map(), initial = new Map(), pins = new Map();
let things = null, colourVar = null, layer = null, xrayWas = null;
const key = (v, t) => v + '|' + t;
const num = x => typeof x === 'number' && Number.isFinite(x);

function buildThings() {
  const m = new Map();
  for (const r of data.rooms || []) m.set(r.guid, { id: r.guid, kind: 'room', label: r.label, room: r });
  for (const s of (data.thermal && data.thermal.surfaces) || []) m.set(s.id, { id: s.id, kind: 'surface', label: `${s.kind} of ${((data.rooms || []).find(r => r.guid === s.room) || {}).label || '?'} (${s.boundary})`, surface: s });
  for (const o of data.openings || []) m.set(o.guid, { id: o.guid, kind: 'opening', label: o.label, opening: o });
  for (const n of data.nodes || []) m.set(n.id, { id: n.id, kind: n.system === 'structure' ? 'structure' : 'device', label: n.label, node: n });
  for (const e of (data.structure && data.structure.elements) || []) { const id = e.guid || e.id; if (!m.has(id)) m.set(id, { id, kind: 'structure', label: e.name || id, element: e }); }
  return m;
}
const thingOf = id => things.get(id);
const roomLabelOf = guid => ((data.rooms || []).find(r => r.guid === guid) || {}).label;

function applies(v, t) {
  const a = v.appliesTo; if (!a || a.kind !== t.kind) return false;
  if (t.kind === 'surface') { const s = t.surface; return (!a.surfaceKinds || a.surfaceKinds.includes(s.kind)) && (!a.boundary || a.boundary === s.boundary); }
  if (t.kind === 'device') return !a.nodeTypes || a.nodeTypes.includes(t.node.type);
  if (t.kind === 'opening') { const o = t.opening; return (!a.openingKind || a.openingKind === o.kind) && (!a.operation || a.operation === o.operation); }
  return true;
}
const sorted = v => (v.values || []).slice().sort((a, b) => a.order - b.order).map(x => x.value);

// the allowed states of a thing, default first: the most specific enumerated variable that applies
export function allowedStates(id) {
  const t = thingOf(id); if (!t) return null;
  const cands = vars.filter(v => v.variableKind === 'enumerated' && applies(v, t));
  if (!cands.length) return null;
  cands.sort((a, b) => (b.appliesTo.operation ? 1 : 0) - (a.appliesTo.operation ? 1 : 0));   // a fixed window has its own variable
  const v = cands[0], out = sorted(v), d = v.default;
  return d && out.includes(d) ? [d, ...out.filter(x => x !== d)] : out;
}
export const openingValues = (guid, kind, operation) => {
  const t = thingOf(guid); const r = t ? allowedStates(guid) : null; if (r) return r;
  const v = vars.find(x => x.variableKind === 'enumerated' && x.appliesTo.kind === 'opening' && x.appliesTo.openingKind === kind && (!operation || !x.appliesTo.operation || x.appliesTo.operation === operation));
  return v ? sorted(v) : null;
};
export const deviceValues = (id, type) => allowedStates(id) || (() => { const v = vars.find(x => x.variableKind === 'enumerated' && x.appliesTo.nodeTypes && x.appliesTo.nodeTypes.includes(type)); return v ? sorted(v) : null; })();

const setpointOf = t => {
  const room = t.kind === 'room' ? t.id : t.kind === 'surface' ? t.surface.room : null; if (!room) return null;
  const v = values.get(key('room-setpoint', room)); if (num(v)) return v;
  const z = data.thermal && data.thermal.zones.find(z => z.room === room); return z ? z.setpointC : null;
};
function defaultOf(v, t) {
  const d = v.default;
  if (num(d)) return d;
  if (d === 'setpoint') return setpointOf(t);
  const z = t.kind === 'room' && data.thermal && data.thermal.zones.find(z => z.room === t.id);
  if (d === 'zone') return z ? z.airChangesPerHour : null;
  if (d === 'designOutdoor') return data.thermal && data.thermal.weather ? data.thermal.weather.designOutdoorC : null;
  return null;
}
export function getValue(vid, tid) {
  const v = varOf(vid), t = thingOf(tid);
  if (!t) throw new Error(`No thing "${tid}".`);
  if (!applies(v, t)) throw new Error(`"${v.id}" does not apply to ${t.kind} "${t.label}".`);
  if (v.variableKind === 'enumerated') {
    if (t.kind === 'structure') return app.structure ? app.structure.stateOf(tid) : 'intact';
    return app.getState ? (app.getState(tid) ?? v.default) : v.default;
  }
  const x = values.get(key(vid, tid)); return x !== undefined ? x : defaultOf(v, t);
}
function varOf(id) { const v = varById.get(id); if (!v) throw new Error(`Unknown state variable "${id}". Known: ${vars.map(x => x.id).join(', ')}.`); return v; }

// ---------- levels ----------
export function levelOf(vid, tid, value) {
  const v = varOf(vid), t = thingOf(tid); if (!t) throw new Error(`No thing "${tid}".`);
  if (!v.levels || !v.levels.length) return null;
  let x = value !== undefined ? value : getValue(vid, tid); if (!num(x)) return null;
  if (v.relativeTo) { const sp = setpointOf(t); if (!num(sp)) return null; x -= sp; }
  const L = v.levels, n = L.length;
  for (let i = 0; i < n; i++) { const l = L[i]; if (x >= l.min && (x < l.max || (i === n - 1 && x <= l.max))) return { id: l.id, label: l.label, color: l.color, order: l.order, index: i, min: l.min, max: l.max }; }
  return null;
}

// ---------- writing ----------
export function setValue(vid, tid, value, opts = {}) {
  const v = varOf(vid), t = thingOf(tid);
  if (!t) throw new Error(`No thing "${tid}". Rooms, surfaces, devices, openings and structural elements have ids (list_state_variables, list_devices).`);
  if (!applies(v, t)) throw new Error(`"${v.label}" does not apply to ${t.kind} "${t.label}" (it applies to ${v.appliesTo.kind}s${v.appliesTo.nodeTypes ? ' of type ' + v.appliesTo.nodeTypes.join(', ') : ''}).`);
  const source = opts.source || 'person';
  if (v.variableKind === 'enumerated') {
    const ok = allowedStates(tid) || sorted(v);
    if (!ok.includes(value)) throw new Error(`"${value}" is not a state of ${t.label} (allowed: ${ok.join(', ')}).`);
    const prev = getValue(vid, tid);
    if (t.kind === 'structure') { if (value === 'removed') app.structure.remove(tid); else app.structure.restore(tid); }
    else app.setState(tid, value);
    app.emit('value', { variable: vid, thing: tid, value, prev, pinned: false, source }); return { variable: vid, thing: tid, value, prev };
  }
  if (!num(value)) throw new Error(`${v.label}: the value must be a number (got ${JSON.stringify(value)}).`);
  if (value < v.min || value > v.max) throw new Error(`${v.label}: ${value} is outside the range ${v.min} to ${v.max}${v.unitLabel ? ' ' + v.unitLabel : ''}.`);
  const k = key(vid, tid), prev = getValue(vid, tid); values.set(k, value);
  const pinned = source === 'person' && v.setBy === 'simulation'; if (pinned) pins.set(k, value);
  if (vid === 'room-setpoint' || pinned) { if (app.thermal) app.thermal.invalidate(); }
  app.emit('value', { variable: vid, thing: tid, value, prev, pinned, source });
  refreshColour(); return { variable: vid, thing: tid, value, prev, pinned };
}
export function unpin(vid, tid) { const k = key(vid, tid), was = pins.delete(k); if (was) { if (app.thermal) app.thermal.invalidate(); app.emit('value', { variable: vid, thing: tid, value: getValue(vid, tid), pinned: false, source: 'person' }); } return was; }
export const pinned = () => [...pins].map(([k, value]) => { const [variable, thing] = k.split('|'); return { variable, thing, value }; });
// the simulation writes (no pin, clamped to the range); values: [[thingId, number]]
export function writeSim(vid, list) {
  const v = varOf(vid); let n = 0;
  for (const [tid, val] of list) { if (!num(val) || !thingOf(tid)) continue; values.set(key(vid, tid), Math.min(v.max, Math.max(v.min, val))); n++; }
  app.emit('values', { source: 'simulation', variable: vid, count: n }); refreshColour(); return n;
}

// ---------- the 3D colouring ----------
const SIM_NEEDS = new Set(['surface-sunlit-fraction', 'surface-irradiance']);
function clipMat(color) {
  const m = new THREE.MeshBasicMaterial({ color, transparent: true, opacity: 0.8, depthWrite: false, side: THREE.DoubleSide, fog: false, toneMapped: false, polygonOffset: true, polygonOffsetFactor: -3, polygonOffsetUnits: -3 });
  m.clippingPlanes = app.clipPlanes; return m;
}
function ensureLayer() {
  if (layer) return layer;
  const g = new THREE.Group(); g.name = 'statevarColour'; app.house.add(g); layer = { group: g, rooms: new Map(), surfaces: new Map(), offsets: new Map() };
  for (const r of data.rooms || []) {
    const sh = new THREE.Shape(r.polygon.map(p => new THREE.Vector2(p[0], p[1])));
    const floor = new THREE.Mesh(new THREE.ShapeGeometry(sh), clipMat('#888')); floor.position.z = (app.roomByGuid.get(r.guid) ? app.roomByGuid.get(r.guid).floorZ : r.z0) + 0.025; floor.renderOrder = 6;
    const vol = new THREE.Mesh(new THREE.ExtrudeGeometry(sh, { depth: Math.max(0.2, r.z1 - r.z0 - 0.05), bevelEnabled: false }), clipMat('#888')); vol.position.z = r.z0 + 0.02; vol.material.opacity = 0.2; vol.renderOrder = 5;
    const edge = new THREE.LineSegments(new THREE.EdgesGeometry(vol.geometry, 35), new THREE.LineBasicMaterial({ color: '#888', fog: false, toneMapped: false })); edge.position.copy(vol.position); edge.material.clippingPlanes = app.clipPlanes; edge.renderOrder = 8;
    floor.userData.kind = 'floor'; vol.userData.kind = 'vol'; edge.userData.kind = 'outline'; vol.userData.room = r.guid;
    for (const m of [floor, vol, edge]) { m.raycast = () => {}; m.visible = false; m.userData.noClip = true; g.add(m); }
    layer.rooms.set(r.guid, { floor, vol, edge });
  }
  return layer;
}
function surfaceMesh(s) {
  const L = ensureLayer(); let m = L.surfaces.get(s.id); if (m) return m;
  const P = s.polygon.map(p => new THREE.Vector3(...p)), n = new THREE.Vector3(...s.normal).normalize();
  let off = s.boundary === 'outside' ? 0.25 : -0.012;   // an outside surface is drawn on the outer face of its wall
  if (s.boundary === 'outside') {
    const obj = app.meshByGuid.get(s.element);
    if (obj) {
      const c = P.reduce((a, p) => a.add(p), new THREE.Vector3()).multiplyScalar(1 / P.length), rc = new THREE.Raycaster(); rc.set(c.clone().addScaledVector(n, 1.5), n.clone().negate()); rc.far = 1.8;
      const h = app.section ? app.section.noFilter(() => rc.intersectObject(obj, true)) : rc.intersectObject(obj, true);
      if (h.length) off = 1.5 - h[0].distance + 0.012;
    }
  }
  const pos = []; for (let i = 1; i + 1 < P.length; i++) for (const q of [P[0], P[i], P[i + 1]]) pos.push(q.x + n.x * off, q.y + n.y * off, q.z + n.z * off);
  const geo = new THREE.BufferGeometry(); geo.setAttribute('position', new THREE.Float32BufferAttribute(pos, 3));
  m = new THREE.Mesh(geo, clipMat('#888')); m.renderOrder = 7; m.raycast = () => {}; m.visible = false; m.userData.noClip = true; L.group.add(m); L.surfaces.set(s.id, m); return m;
}
function refreshColour() {
  if (!layer || !colourVar) return;
  const v = varById.get(colourVar);
  for (const [guid, o] of layer.rooms) { o.floor.visible = o.vol.visible = o.edge.visible = false; o.vol.userData.want = false; }
  for (const m of layer.surfaces.values()) m.visible = false;
  const set = (mesh, level, floor) => { if (!level) { mesh.visible = false; return; } mesh.material.color.set(level.color); mesh.visible = true; if (floor) mesh.material.opacity = 0.85; };
  const off = app.rooms && app.rooms.getStyle() === 'off';   // rooms are tinted on the floor and by their border; the volume only in the fill style (rooms.js)
  if (v.appliesTo.kind === 'room') for (const r of data.rooms) { const lv = levelOf(colourVar, r.guid), o = layer.rooms.get(r.guid); set(o.floor, lv, true); set(o.vol, lv); o.vol.visible = false; o.vol.userData.want = !!lv; if (lv) { o.edge.material.color.set(lv.color); o.edge.visible = !off; } o.floor.visible = o.floor.visible && !off; }
  else if (v.appliesTo.kind === 'surface') for (const t of things.values()) if (t.kind === 'surface' && applies(v, t) && values.has(key(colourVar, t.id))) set(surfaceMesh(t.surface), levelOf(colourVar, t.id));
  app.requestRender();
}
export function legend() {
  if (!colourVar) return null;
  const v = varById.get(colourVar), rel = v.relativeTo;
  const sg = x => (rel && x > 0 ? '+' : '') + x, u = ' ' + v.unitLabel + (rel ? ' from the set point' : '');
  const text = l => (l.min <= -100 ? 'below ' + sg(l.max) : l.max >= 100 ? 'above ' + sg(l.min) : sg(l.min) + ' to ' + sg(l.max)) + u;
  const items = v.levels.map(l => ({ id: l.id, label: l.label, color: l.color, min: l.min, max: l.max, text: text(l) }));
  const ts = []; for (const t of things.values()) if (t.kind === v.appliesTo.kind && applies(v, t) && (t.kind !== 'surface' || values.has(key(colourVar, t.id)))) { const val = getValue(colourVar, t.id), lv = levelOf(colourVar, t.id); ts.push({ id: t.id, label: t.label, value: num(val) ? +val.toFixed(3) : val, level: lv ? lv.label : null, color: lv ? lv.color : null }); }
  return { variable: colourVar, label: v.label, unit: v.unit, unitLabel: v.unitLabel, relativeTo: rel || null, items, things: ts };
}
export function colourBy(vid) {
  if (vid == null) {
    const was = colourVar; colourVar = null;
    if (layer) { for (const o of layer.rooms.values()) { o.floor.visible = o.vol.visible = o.edge.visible = false; o.vol.userData.want = false; } for (const m of layer.surfaces.values()) m.visible = false; }
    if (xrayWas === false && app.svc && app.svc.isXray()) app.svc.setXray(false); xrayWas = null;
    app.requestRender(); if (was) app.emit('colour', { variable: null }); return null;
  }
  const v = varOf(vid);
  if (v.variableKind !== 'numeric' || !v.levels || !v.levels.length || !['room', 'surface'].includes(v.appliesTo.kind)) throw new Error(`"${vid}" cannot colour the building: only numeric variables with levels, of rooms or surfaces, can (e.g. room-air-temperature, surface-sunlit-fraction).`);
  ensureLayer(); colourVar = vid;
  if (app.svc && !app.svc.isXray()) { xrayWas = false; app.svc.setXray(true); }   // the walls go translucent: the tint is inside them
  refreshColour(); app.emit('colour', { variable: vid });
  if ((SIM_NEEDS.has(vid) || vid === 'room-air-temperature' || vid === 'room-air-change-rate') && app.svc) {
    (app.thermal ? Promise.resolve(app.thermal) : app.loadThermal()).then(th => { th.publish && th.publish({ surfaces: SIM_NEEDS.has(vid) }); refreshColour(); }).catch(e => console.warn('[building] thermal values for colouring:', e.message));
  }
  return legend();
}

// ---------- supply rule ----------
// The blocking states per device type, from `rule-supply`. A type that is in its blocking state at rest (a switch or an air conditioner
// that is "off" by default) is a load or a controller, not a gate of the supply: left out, so the
// reach of a tripped breaker is what the stored scenarios say.
export function supplyBlocking() {
  const r = (data.rules || []).find(x => x.kind === 'supply-propagation'); if (!r) return null;
  const out = {};
  for (const [type, bad] of Object.entries(r.blockingStates || {})) {
    const first = (data.states || {})[type] ? data.states[type][0] : null;
    if (first && bad.includes(first)) continue;
    out[type] = bad.slice();
  }
  return out;
}

export function resetValues() {
  values = new Map(initial); pins.clear(); colourBy(null);
  if (app.thermal) app.thermal.invalidate();
  app.emit('values', { source: 'reset', count: values.size }); return true;
}
const describeVar = v => ({ id: v.id, label: v.label, kind: v.variableKind, appliesTo: v.appliesTo, unit: v.unit, unitLabel: v.unitLabel, min: v.min, max: v.max, step: v.step, setBy: v.setBy, default: v.default, relativeTo: v.relativeTo || null,
  ...(v.levels ? { levels: v.levels.map(l => ({ id: l.id, label: l.label, min: l.min, max: l.max, color: l.color })) } : {}), ...(v.values ? { values: sorted(v) } : {}) });
export function initStateVars(d) {
  data = d; vars = d.stateVariables || []; if (!vars.length) return null;
  varById = new Map(vars.map(v => [v.id, v])); things = buildThings();
  values = new Map(); initial = new Map();
  for (const [tid, o] of Object.entries(d.stateValues || {})) for (const [vid, val] of Object.entries(o)) if (typeof val === 'number') values.set(key(vid, tid), val);
  initial = new Map(values);
  app.statevars = {
    variables: f => vars.filter(v => !f || ((!f.kind || v.appliesTo.kind === f.kind) && (!f.thing || (thingOf(f.thing) && applies(v, thingOf(f.thing)))))).map(describeVar),
    variable: id => describeVar(varOf(id)), thingKind: id => (thingOf(id) || {}).kind || null,
    appliesTo: id => { const t = thingOf(id); if (!t) throw new Error(`No thing "${id}".`); return vars.filter(v => applies(v, t)).map(v => v.id); },
    things: kind => [...things.values()].filter(t => !kind || t.kind === kind).map(t => ({ id: t.id, label: t.label, kind: t.kind })),
    getValue, setValue, unpin, pinned, writeSim, levelOf, allowedStates, openingValues, deviceValues, colourBy, legend, colouring: () => colourVar,
    rules: () => JSON.parse(JSON.stringify(d.rules || [])), supplyBlocking, resetValues, setpointOf: id => setpointOf(thingOf(id)), thingLabel: id => (thingOf(id) || {}).label, roomLabelOf,
  };
  app.on('roomstyle', () => refreshColour());
  app.on('time', () => { if (colourVar && SIM_NEEDS.has(colourVar) && app.thermal && app.thermal.publish) app.thermal.publish({ surfaces: true }); else if (app.thermal && app.thermal.last()) app.thermal.publish({ surfaces: false }); });
  return app.statevars;
}
