// States: doors and windows are split into parts (frame, leaf, glass panes, handles)
// and the moving parts hang on hinge pivots. A small generic store holds the state of
// anything that has one (doors, windows, and what other modules register).
import * as THREE from 'three';
import { mergeGeometries } from 'three/addons/utils/BufferGeometryUtils.js';
import { app } from './app.js';

const WORLD_Z = new THREE.Vector3(0, 0, 1);

// ---------- connected components of a mesh ----------
export function components(geom) {
  const P = geom.attributes.position, I = geom.index;
  const nt = I ? I.count / 3 : P.count / 3;
  const ids = new Map(), vid = new Int32Array(P.count);
  for (let i = 0; i < P.count; i++) {
    const k = Math.round(P.getX(i) * 1000) + ',' + Math.round(P.getY(i) * 1000) + ',' + Math.round(P.getZ(i) * 1000);
    let id = ids.get(k); if (id === undefined) { id = ids.size; ids.set(k, id); } vid[i] = id;
  }
  const par = Array.from({ length: ids.size }, (_, i) => i);
  const find = a => { while (par[a] !== a) { par[a] = par[par[a]]; a = par[a]; } return a; };
  const idx = (t, q) => I ? I.getX(t * 3 + q) : t * 3 + q;
  for (let t = 0; t < nt; t++) { const a = find(vid[idx(t, 0)]), b = find(vid[idx(t, 1)]), c = find(vid[idx(t, 2)]); par[b] = a; par[find(c)] = find(a); }
  const by = new Map();
  for (let t = 0; t < nt; t++) { const r = find(vid[idx(t, 0)]); if (!by.has(r)) by.set(r, []); by.get(r).push(t); }
  return [...by.values()].map(tris => {
    const box = new THREE.Box3(), v = new THREE.Vector3();
    tris.forEach(t => { for (let q = 0; q < 3; q++) { v.fromBufferAttribute(P, idx(t, q)); box.expandByPoint(v); } });
    const size = box.getSize(new THREE.Vector3());
    return { tris, box, size, center: box.getCenter(new THREE.Vector3()), minDim: Math.min(size.x, size.y, size.z), maxDim: Math.max(size.x, size.y, size.z), vol: size.x * size.y * size.z };
  });
}
function subGeometry(geom, comps, shift) {
  const P = geom.attributes.position, N = geom.attributes.normal, I = geom.index;
  const idx = (t, q) => I ? I.getX(t * 3 + q) : t * 3 + q;
  const tris = comps.flatMap(c => c.tris);
  const pos = new Float32Array(tris.length * 9), nor = N ? new Float32Array(tris.length * 9) : null;
  tris.forEach((t, k) => {
    for (let q = 0; q < 3; q++) {
      const s = idx(t, q), d = (k * 3 + q) * 3;
      pos[d] = P.getX(s) - shift.x; pos[d + 1] = P.getY(s) - shift.y; pos[d + 2] = P.getZ(s) - shift.z;
      if (nor) { nor[d] = N.getX(s); nor[d + 1] = N.getY(s); nor[d + 2] = N.getZ(s); }
    }
  });
  const g = new THREE.BufferGeometry();
  g.setAttribute('position', new THREE.BufferAttribute(pos, 3));
  if (nor) g.setAttribute('normal', new THREE.BufferAttribute(nor, 3)); else g.computeVertexNormals();
  g.computeBoundingBox(); g.computeBoundingSphere();
  return g;
}

// ---------- the state store ----------
const store = new Map();        // id -> {id,label,kind,values,state,onChange}
const openables = new Map();    // guid -> articulation
let animOff = null;

function emitState(rec, prev) { app.emit('state', { id: rec.id, state: rec.state, prev, kind: rec.kind, label: rec.label }); }

export function registerStateful(id, { values, initial, kind = 'thing', label = id, onChange } = {}) {
  if (!Array.isArray(values) || !values.length) throw new Error('registerStateful needs values');
  const rec = { id, label, kind, values: values.slice(), state: initial ?? values[0], onChange };
  store.set(id, rec);
  return rec.state;
}
export function getState(id) { const r = store.get(id); return r ? r.state : undefined; }
export function states(id) { const r = store.get(id); return r ? r.values.slice() : []; }
export function allStates() { const o = {}; store.forEach((r, id) => { o[id] = r.state; }); return o; }
export function stateList() { return [...store.values()].map(r => ({ id: r.id, label: r.label, kind: r.kind, state: r.state, values: r.values.slice() })); }
export function setState(id, state, opts = {}) {
  const r = store.get(id);
  if (!r) throw new Error('No stateful thing with id ' + id + '.');
  if (!r.values.includes(state)) throw new Error(`"${state}" is not a state of ${r.label} (allowed: ${r.values.join(', ')}).`);
  if (r.state === state) return state;
  const prev = r.state; r.state = state;
  const a = openables.get(id);
  if (a) { a.tgt = state === 'open' ? { o: 1, t: 0 } : state === 'tilted' ? { o: 0, t: 1 } : { o: 0, t: 0 }; if (opts.instant) { a.cur = { ...a.tgt }; applyPose(a); } else kick(); }
  if (r.onChange) try { r.onChange(state, prev); } catch (e) { console.error(e); }
  emitState(r, prev);
  app.requestRender();
  return state;
}
export function toggleState(id) {
  const r = store.get(id); if (!r) throw new Error('No stateful thing with id ' + id + '.');
  if (r.values.length < 2) return r.state;   // a fixed window
  const i = r.values.indexOf(r.state);
  const next = (r.kind === 'door' || r.kind === 'window') ? (r.state === 'closed' ? 'open' : 'closed') : r.values[(i + 1) % r.values.length];
  return setState(id, next);
}
// 0..1: how far the door/window is open (tilt counts as a little)
export function openness(id) { const a = openables.get(id); return a ? Math.max(a.cur.o, a.cur.t * 0.3) : 0; }
export function settleStates() { openables.forEach(a => { a.cur = { ...a.tgt }; applyPose(a); }); app.requestRender(); }

const ease = s => s * s * (3 - 2 * s);
function applyPose(a) {
  const o = ease(a.cur.o), t = ease(a.cur.t);
  for (const l of a.leaves) {
    if (l.slide) l.pOpen.position.copy(l.slide.home).addScaledVector(l.slide.v, l.slide.t * o);   // a sliding panel moves along its track, no rotation
    else { l.pOpen.rotation.z = l.signO * l.maxO * o; l.pTilt.rotation[l.axisT] = l.signT * l.maxT * t; }
    l.pOpen.updateMatrixWorld(true);
  }
  if (app.poseMoving) app.poseMoving(a.group);   // the batches draw from copies of the leaves: move those too
}
function kick() {
  if (animOff) return;
  animOff = app.addTicker(dt => {
    let moving = false;
    openables.forEach(a => {
      let mv = false;
      for (const k of ['o', 't']) {
        const d = a.tgt[k] - a.cur[k];
        if (Math.abs(d) > 1e-4) { a.cur[k] += Math.sign(d) * Math.min(Math.abs(d), dt / 0.75); mv = true; }
      }
      if (mv) { applyPose(a); moving = true; }
    });
    if (!moving) { animOff = null; return false; }
  });
}

// ---------- articulation ----------
const dirOf = (ax) => ax === 'x' ? new THREE.Vector3(1, 0, 0) : new THREE.Vector3(0, 1, 0);

function swingSide(node, mid, tAx) {
  // which way (+1/-1 along the thickness axis) the leaf swings: into a room
  const g = app.graph; let target = null;
  const rooms = (node.bd || []).map(i => app.roomByGuid.get(g.nodes[i].n)).filter(Boolean);
  if (rooms.length === 1) target = rooms[0];
  else if (rooms.length > 1) {
    const doors = r => g.nodes.filter(n => n.c === 'IfcDoor' && (n.bd || []).includes(r.node.i)).length;
    const area = r => { const s = r.box.getSize(new THREE.Vector3()); return s.x * s.y; };
    target = rooms.slice().sort((a, b) => doors(a) - doors(b) || area(a) - area(b))[0];
  }
  const c = target ? target.center : app.HOUSE_CENTER;
  return Math.sign(c[tAx] - mid) || 1;
}

// ---------- sliding doors and window sashes ----------
const slidingInfo = new Map();     // guid -> {dir, travel, panels:[{box, moving}]}
// panels of a sliding unit: wide, tall, thin
const isPanel = (c, wAx, tAx) => c.size[wAx] > 0.4 && c.size.z > 1.2 && c.size[tAx] >= 0.04 && c.size[tAx] < 0.1;   // glazing bars are as big but thinner
const partRole = (c, hasFrame) => {
  if (c.maxDim < 0.3 && c.tris.length >= 28) return 'handle';
  if (hasFrame && c.tris.length <= 12 && c.minDim < 0.03) return 'bar';
  if (hasFrame && c.minDim < 0.06) return 'glass';
  return 'leaf';
};
// a thin sash ring around a pane (4 small boxes), local to the pivot `shift`; the GLB has no separate sash
function sashGeometry(pane, wAx, tAx, shift) {
  const rw = 0.04, d = 0.04, ov = 0.012, parts = [];
  const lo = pane.box.min, hi = pane.box.max, mid = (lo[tAx] + hi[tAx]) / 2;
  const add = (w0, w1, z0, z1) => {
    const g = new THREE.BoxGeometry(wAx === 'x' ? w1 - w0 : d, wAx === 'x' ? d : w1 - w0, z1 - z0);
    g.translate(wAx === 'x' ? (w0 + w1) / 2 : mid, wAx === 'x' ? mid : (w0 + w1) / 2, (z0 + z1) / 2); parts.push(g.toNonIndexed());
  };
  const w0 = lo[wAx], w1 = hi[wAx], z0 = lo.z, z1 = hi.z;
  add(w0 - rw, w1 + rw, z0 - rw, z0 + ov); add(w0 - rw, w1 + rw, z1 - ov, z1 + rw); add(w0 - rw, w0 + ov, z0 + ov, z1 - ov); add(w1 - ov, w1 + rw, z0 + ov, z1 - ov);
  const g = mergeGeometries(parts.map(p => { p.deleteAttribute('uv'); return p; })); g.translate(-shift.x, -shift.y, -shift.z); return g;
}
function boxGeo(w, d, h, cx, cy, cz, shift) { const g = new THREE.BoxGeometry(w, d, h).toNonIndexed(); g.deleteAttribute('uv'); g.translate(cx - shift.x, cy - shift.y, cz - shift.z); return g; }

function articulate(node, mesh) {
  const kind = node.c === 'IfcDoor' ? 'door' : 'window';
  const guid = node.n, geom = mesh.geometry;
  const comps = components(geom).sort((a, b) => b.vol - a.vol);
  const whole = new THREE.Box3().setFromObject(mesh), ws = whole.getSize(new THREE.Vector3());
  const wAx = ws.x >= ws.y ? 'x' : 'y', tAx = wAx === 'x' ? 'y' : 'x';
  const W = dirOf(wAx);
  const od = (app.systems && Array.isArray(app.systems.openings)) ? app.systems.openings.find(o => o.guid === guid) : null;

  // 1. which components stay (frame) and which move (leaf units)
  let frame = [], units = [], fixedParts = [], slide = null;
  const panelsAll = comps.filter(c => isPanel(c, wAx, tAx)).sort((a, b) => a.center[wAx] - b.center[wAx]);
  const sliding = kind === 'door' && ((od && od.operation === 'sliding') || (!od && panelsAll.length >= 2 && ws[wAx] > 1.5));
  if (sliding && panelsAll.length >= 1) {
    // the unit: the panel at the start of the slide moves, the others stay (a three-panel unit moves one panel by `travel`)
    const dirv = od && od.slide ? new THREE.Vector3(od.slide[0], od.slide[1], 0).normalize() : W.clone();
    const sgn = Math.sign(dirv[wAx]) || 1, moving = sgn > 0 ? panelsAll[0] : panelsAll[panelsAll.length - 1];
    const travel = od && od.travel ? od.travel : moving.size[wAx];
    const owned = new Map(); panelsAll.forEach(p => owned.set(p, []));
    const rest = [];
    for (const c of comps) {
      if (panelsAll.includes(c)) continue;
      const owner = panelsAll.find(p => p.box.clone().expandByScalar(0.03).containsPoint(c.center));
      if (owner) owned.get(owner).push(c); else rest.push(c);
    }
    frame = rest;
    const movingComps = [moving, ...owned.get(moving)];
    units = [{ comps: movingComps, roles: movingComps.map(c => c === moving ? 'glass' : partRole(c, true)) }];
    for (const p of panelsAll) if (p !== moving) { fixedParts.push({ c: p, role: 'glass' }); owned.get(p).forEach(c => fixedParts.push({ c, role: partRole(c, true) })); }
    const pb = new THREE.Box3(); panelsAll.forEach(p => pb.union(p.box));
    slide = { dir: dirv, travel, panels: panelsAll.map(p => ({ box: p.box.clone(), moving: p === moving })), side: Math.sign(moving.center[tAx] - (frame[0] || comps[0]).center[tAx]) || 1 };
  } else if (kind === 'window') {
    const big = comps[0], rest = comps.slice(1);
    frame = [big];
    const panes = rest.filter(c => c.minDim < 0.05 && big.box.containsPoint(c.center));
    rest.forEach(c => { if (!panes.includes(c)) frame.push(c); });
    panes.sort((a, b) => a.center[wAx] - b.center[wAx]);
    units = panes.map(c => ({ comps: [c], roles: ['glass'], pane: c }));
  } else {
    let leafComps = comps;
    const big = comps[0], rest = comps.slice(1);
    if (rest.length && rest.every(c => big.box.clone().expandByScalar(0.12).containsPoint(c.center)) && big.size[wAx] > 1.2) { frame = [big]; leafComps = rest; }
    units = [{ comps: leafComps, roles: leafComps.map(c => partRole(c, frame.length > 0)) }];
  }
  const grp = new THREE.Group(); grp.name = mesh.name; grp.userData.guid = guid; grp.userData.kind = kind;
  const zero = new THREE.Vector3();
  // A two-leaf window here is a Stulpfenster: no fixed mullion. The IFC mesh is one solid, outer frame and centre bar together, so the
  // triangles of the frame that are not on the outer ring (the meeting stile between the leaves) are cut out and move with the nearer leaf.
  const stile = units.map(() => []);
  if (kind === 'window' && units.length >= 2 && !(od && od.operation === 'fixed') && frame.length) {
    const P = geom.attributes.position, I = geom.index, tri = (t, q) => I ? I.getX(t * 3 + q) : t * 3 + q;
    const lo = whole.min[wAx], hi = whole.max[wAx], pb = units.map(u => u.pane.box);
    const fwS = Math.max(0.03, pb[0].min[wAx] - lo), fwE = Math.max(0.03, hi - pb[pb.length - 1].max[wAx]), fwB = Math.max(0.03, Math.min(...pb.map(b => b.min.z)) - whole.min.z), fwT = Math.max(0.03, whole.max.z - Math.max(...pb.map(b => b.max.z))), m = 0.012;
    const keep = [], mids = units.map(u => u.pane.center[wAx]);
    for (const c of frame) for (const t of c.tris) {
      let cw = 0, cz = 0; for (let q = 0; q < 3; q++) { const i = tri(t, q); cw += P.getComponent(i, wAx === 'x' ? 0 : 1); cz += P.getZ(i); } cw /= 3; cz /= 3;
      const ring = cw < lo + fwS + m || cw > hi - fwE - m || cz < whole.min.z + fwB + m || cz > whole.max.z - fwT - m;
      if (ring) keep.push(t); else { let k = 0; for (let j = 1; j < mids.length; j++) if (Math.abs(cw - mids[j]) < Math.abs(cw - mids[k])) k = j; stile[k].push(t); }
    }
    frame = [{ tris: keep, center: frame[0].center, box: frame[0].box, size: frame[0].size }];
  }
  if (frame.length) {
    const fm = new THREE.Mesh(subGeometry(geom, frame, zero), new THREE.MeshStandardMaterial());
    fm.userData.role = 'frame'; fm.userData.part = 'frame'; fm.name = mesh.name; grp.add(fm);
  }
  // fixed panels of a sliding unit stay in the wall, each role its own mesh
  for (const role of ['glass', 'bar', 'handle', 'leaf']) {
    const cs = fixedParts.filter(f => f.role === role).map(f => f.c); if (!cs.length) continue;
    const fm = new THREE.Mesh(subGeometry(geom, cs, zero), new THREE.MeshStandardMaterial());
    fm.userData.role = role === 'leaf' ? 'frame' : role; fm.userData.part = 'fixed-' + role; fm.name = mesh.name; grp.add(fm);
  }

  // 2. hinges (or the track of a sliding panel)
  const leaves = [];
  const frameCenter = (frame[0] || comps[0]).center;
  units.forEach((u, ui) => {
    const box = new THREE.Box3(); u.comps.forEach((c, k) => { if (u.roles[k] !== 'handle') box.union(c.box); });
    if (box.isEmpty()) u.comps.forEach(c => box.union(c.box));
    const lo = box.min[wAx], hi = box.max[wAx], mid = (box.min[tAx] + box.max[tAx]) / 2;
    const swing = swingSide(node, mid, tAx);
    let hingeEnd;   // 'lo' | 'hi'
    if (kind === 'door') {
      const handles = u.comps.filter((c, k) => u.roles[k] === 'handle');
      if (handles.length) { const m = handles.reduce((s, c) => s + c.center[wAx], 0) / handles.length; hingeEnd = (m - lo) < (hi - m) ? 'hi' : 'lo'; }
      else hingeEnd = wallScore(node, box, wAx, tAx, swing, mid, 'lo') >= wallScore(node, box, wAx, tAx, swing, mid, 'hi') ? 'lo' : 'hi';
    } else hingeEnd = units.length > 1 ? (u.comps[0].center[wAx] < frameCenter[wAx] ? 'lo' : 'hi') : 'lo';
    const H = new THREE.Vector3(); H[wAx] = hingeEnd === 'lo' ? lo : hi; H[tAx] = mid; H.z = box.min.z;
    const d = dirOf(wAx).multiplyScalar(hingeEnd === 'lo' ? 1 : -1);          // hinge -> free edge
    let signO = 1, maxO = (kind === 'door' ? 92 : 78) * Math.PI / 180;
    const swingV = dirOf(tAx).multiplyScalar(swing);
    signO = (-d.y * swingV.x + d.x * swingV.y) > 0 ? 1 : -1;
    // the data of the openings: a door's hinge, a window's leaf (the one whose hinge is nearest)
    const lf = od && Array.isArray(od.leaves) ? od.leaves.map(l => ({ l, dist: Math.hypot(l.hinge[0] - H.x, l.hinge[1] - H.y) })).sort((a, b) => a.dist - b.dist)[0] : null;
    let source = 'heuristic';
    if (!slide) {
      if (od && od.hinge && kind === 'door') { H.set(od.hinge[0], od.hinge[1], od.hinge[2] ?? H.z); if (od.closed) d.set(od.closed[0], od.closed[1], 0); signO = od.opens < 0 ? -1 : 1; if (od.maxAngle) maxO = od.maxAngle * Math.PI / 180; source = 'data'; }
      else if (lf && lf.dist < 0.35) { H.set(lf.l.hinge[0], lf.l.hinge[1], lf.l.hinge[2] ?? H.z); d.set(lf.l.closed[0], lf.l.closed[1], 0); signO = lf.l.opens < 0 ? -1 : 1; if (od.maxAngle) maxO = od.maxAngle * Math.PI / 180; source = 'data'; }
      else if (od && od.hinge && Math.hypot(od.hinge[0] - H.x, od.hinge[1] - H.y) < 0.35) { H.set(od.hinge[0], od.hinge[1], od.hinge[2] ?? H.z); if (od.closed) d.set(od.closed[0], od.closed[1], 0); signO = od.opens < 0 ? -1 : 1; if (od.maxAngle) maxO = od.maxAngle * Math.PI / 180; source = 'data'; }
    } else { H.copy(box.min); H[tAx] = mid; source = od ? 'data' : 'heuristic'; }
    // tilt: about the bottom edge, along the width axis; the top moves toward the swing side
    const T = new THREE.Vector3(); T[wAx] = (lo + hi) / 2; T[tAx] = mid; T.z = box.min.z;
    const wz = new THREE.Vector3().crossVectors(W, WORLD_Z);
    const signT = wz.dot(swingV) > 0 ? 1 : -1;
    const pOpen = new THREE.Group(), pTilt = new THREE.Group();
    const tiltPivot = kind === 'window' ? T : H;
    pOpen.position.copy(H); pTilt.position.copy(tiltPivot).sub(H); pOpen.add(pTilt);
    const mk = (g, role, part) => { const m = new THREE.Mesh(g, new THREE.MeshStandardMaterial()); m.userData.role = role; m.userData.part = part; m.userData.moving = true; m.name = mesh.name; pTilt.add(m); };
    u.comps.forEach((c, k) => mk(subGeometry(geom, [c], tiltPivot), u.roles[k], 'leaf' + ui + '-' + k));
    if (u.pane) {   // a casement: the sash ring around the pane and a handle, so the whole thing opens, not just the glass
      mk(sashGeometry(u.pane, wAx, tAx, tiltPivot), 'sash', 'sash' + ui);
      if (stile[ui] && stile[ui].length) mk(subGeometry(geom, [{ tris: stile[ui] }], tiltPivot), 'frame', 'stile' + ui);   // its half of the meeting stile
      const active = units.length < 2 || ui === 0;   // two leaves: the first one has the handle and is the one that tilts; the other is held shut
      u.active = active;
      const free = hingeEnd === 'lo' ? u.pane.box.max[wAx] : u.pane.box.min[wAx], hc = new THREE.Vector3();
      hc[wAx] = free + (hingeEnd === 'lo' ? -0.05 : 0.05); hc[tAx] = (u.pane.box.min[tAx] + u.pane.box.max[tAx]) / 2 + swing * 0.03; hc.z = u.pane.center.z;
      const hg = wAx === 'x' ? boxGeo(0.1, 0.014, 0.022, hc.x, hc.y, hc.z, tiltPivot) : boxGeo(0.014, 0.1, 0.022, hc.x, hc.y, hc.z, tiltPivot);
      if (active) mk(hg, 'handle', 'handle' + ui);
    }
    grp.add(pOpen);
    leaves.push({ pOpen, pTilt, signO, maxO: slide ? 0 : maxO, axisT: wAx, signT, maxT: kind === 'window' && u.active !== false ? 13 * Math.PI / 180 : 0, hingeEnd, hinge: H.clone(), dirClosed: d.clone(), swing: swingV.clone(), slide: slide ? { v: slide.dir.clone(), t: slide.travel, home: H.clone().addScaledVector(dirOf(tAx), slide.side * 0.003) } : null, source });   // a sliding panel runs on a slightly offset track: the panels never share a plane
  });

  mesh.parent.add(grp); mesh.parent.remove(mesh);
  mesh.geometry.dispose();
  app.meshByGuid.set(guid, grp);
  // the states come from the data (statevars.js): a fixed window has one, "closed", and cannot be opened
  const values = (app.statevars && app.statevars.openingValues(guid, kind, od && od.operation)) || (kind === 'door' ? ['closed', 'open'] : ['closed', 'tilted', 'open']);
  const a = { guid, kind, node, group: grp, leaves, cur: { o: 0, t: 0 }, tgt: { o: 0, t: 0 }, wAx, tAx, box: whole.clone(), slide: !!slide, hingeSource: leaves.map(l => l.source) };
  openables.set(guid, a);
  if (slide) slidingInfo.set(guid, { dir: slide.dir.clone(), travel: slide.travel, panels: slide.panels });
  registerStateful(guid, { values, initial: 'closed', kind, label: node.l });
}

// 1 if a wall meets the main wall near that end on the swing side (the open leaf lies against it)
function wallScore(node, box, wAx, tAx, swing, mid, end) {
  const e = end === 'lo' ? box.min[wAx] : box.max[wAx], out = end === 'lo' ? -1 : 1;
  const probe = { [wAx]: e + out * 0.18, [tAx]: mid + swing * 0.6 };
  for (const n of app.graph.nodes) {
    if (n.c !== 'IfcWallStandardCase') continue;
    const m = app.meshByGuid.get(n.n); if (!m) continue;
    const b = m.userData.closedBox || (m.userData.closedBox = new THREE.Box3().setFromObject(m));
    if (b.min[wAx] - 0.02 <= probe[wAx] && probe[wAx] <= b.max[wAx] + 0.02 && b.min[tAx] <= probe[tAx] && probe[tAx] <= b.max[tAx]) {
      if (b.min.z < 1 && b.max.z > 1.5) return 1;
    }
  }
  return 0;
}

export function initStates() {
  Object.assign(app, { getState, setState, states, allStates, stateList, toggleState, registerStateful, openness, settleStates,
    openables: () => [...openables.values()], slidingInfo: id => slidingInfo.get(id) || null });
}

// Called after the GLB is in the scene and meshByGuid is filled.
export function buildStates() {
  app.house.updateMatrixWorld(true);
  const list = app.graph.nodes.filter(n => (n.c === 'IfcDoor' || n.c === 'IfcWindow') && app.meshByGuid.get(n.n) && app.meshByGuid.get(n.n).isMesh);
  list.forEach(n => articulate(n, app.meshByGuid.get(n.n)));
  // default states from the systems file, if it has them
  const ops = app.systems && Array.isArray(app.systems.openings) ? app.systems.openings : [];
  ops.forEach(o => { if (o.state && store.has(o.guid) && store.get(o.guid).values.includes(o.state)) setState(o.guid, o.state, { instant: true }); });
}
