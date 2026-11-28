// Walk mode: first person, pointer lock (desktop) or a joystick (touch).
//
// Collision is a 2D obstacle list per storey, built once from the model:
//  - every wall is a box, cut at each door opening (so a doorway is a gap)
//  - every door is a box over its opening that blocks while the door is not open
//  - railings are boxes, the spiral stair is a circle; windows stay part of the wall
//    (the sill is at 0.8 m, an open window is not a way through)
// The walker is a circle of 0.25 m, moved in steps of at most 4 cm, x then y, so it
// slides along walls. The floor is the storey level; on the upper storey a ray down
// must hit the floor slab (the stair void has none), and a ray up must leave 1.75 m
// below the roof. The stair is a trigger: E near it plays a short climb.
import * as THREE from 'three';
import { app } from './app.js';
import { rect3d } from './shared.js';

const R = 0.25, EYE = 1.65, STEP = 0.04;
const obstacles = [];
let stair = null, storeyZ = [0.001, 2.701];
let slabUpper = [], roofSlabs = [], solid = [];
const W = { active: false, x: 0, y: 0, storey: 0, yaw: 0, pitch: 0, locked: false, climbing: false };
const keys = new Set();
const stick = { x: 0, y: 0, id: null };
let saved = null, ui = null, hintAt = 0, targetNow = null, lockWanted = false;
const ray = new THREE.Raycaster();

const clamp = (v, a, b) => Math.min(b, Math.max(a, v));
const nf = fn => (app.section ? app.section.noFilter(fn) : fn());   // a section cut does not change where a wall stands
const isTyping = () => { const a = document.activeElement; return a && /^(INPUT|TEXTAREA|SELECT)$/.test(a.tagName); };

// ---------- building the obstacles ----------
function boxOf(obj) { const b = new THREE.Box3().setFromObject(obj); return b; }
const ov = (a0, a1, b0, b1) => Math.min(a1, b1) - Math.max(a0, b0);

export function buildWalk() {
  obstacles.length = 0; solid = []; slabUpper = []; roofSlabs = [];
  const g = app.graph;
  storeyZ = g.storeys.map(s => (+s.el) + 0.001);
  const nodes = g.nodes;
  const openings = nodes.filter(n => n.c === 'IfcOpeningElement').map(n => app.meshByGuid.get(n.n)).filter(Boolean).map(boxOf);
  // each door's footprint: its own box plus the opening that holds it
  const doors = nodes.filter(n => n.c === 'IfcDoor').map(n => {
    const o = app.meshByGuid.get(n.n); if (!o) return null;
    const b = boxOf(o);
    openings.forEach(ob => { if (ov(ob.min.x, ob.max.x, b.min.x, b.max.x) > 0.02 && ov(ob.min.y, ob.max.y, b.min.y, b.max.y) > 0.02 && ob.min.z < 0.9 + b.min.z) b.union(ob); });
    return { node: n, box: b };
  }).filter(Boolean);
  nodes.forEach(n => {
    const o = app.meshByGuid.get(n.n); if (!o) return;
    if (n.k === 'Space' || n.c === 'IfcOpeningElement') return;
    solid.push(o);
    if (n.c === 'IfcSlab') { if (/^Dach/.test(n.l)) roofSlabs.push(o); else if (n.st === 1) slabUpper.push(o); }
    if (n.c === 'IfcWallStandardCase') {
      const b = boxOf(o), sx = b.max.x - b.min.x, sy = b.max.y - b.min.y;
      const a = sx >= sy ? 'x' : 'y', p = a === 'x' ? 'y' : 'x';
      const gaps = [];
      doors.forEach(d => { if (ov(d.box.min[p], d.box.max[p], b.min[p], b.max[p]) > 0.03 && ov(d.box.min[a], d.box.max[a], b.min[a], b.max[a]) > 0.2 && ov(d.box.min.z, d.box.max.z, b.min.z, b.max.z) > 1) gaps.push([d.box.min[a], d.box.max[a]]); });
      gaps.sort((m, k) => m[0] - k[0]);
      let from = b.min[a];
      const push = (u, v) => { if (v - u < 0.01) return; obstacles.push(a === 'x' ? { x0: u, x1: v, y0: b.min.y, y1: b.max.y, z0: b.min.z, z1: b.max.z, what: n.l } : { x0: b.min.x, x1: b.max.x, y0: u, y1: v, z0: b.min.z, z1: b.max.z, what: n.l }); };
      gaps.forEach(([u, v]) => { push(from, Math.max(from, u)); from = Math.max(from, v); });
      push(from, b.max[a]);
    } else if (n.c === 'IfcRailing') {
      const b = boxOf(o); obstacles.push({ x0: b.min.x - 0.03, x1: b.max.x + 0.03, y0: b.min.y - 0.03, y1: b.max.y + 0.03, z0: b.min.z, z1: b.max.z, what: n.l });
    } else if (n.c === 'IfcStair') {
      const b = boxOf(o); const c = b.getCenter(new THREE.Vector3());
      stair = { guid: n.n, obj: o, cx: c.x, cy: c.y, r: Math.min(b.max.x - b.min.x, b.max.y - b.min.y) / 2, z0: b.min.z, z1: b.max.z };
      obstacles.push({ circle: true, cx: c.x, cy: c.y, r: stair.r, z0: b.min.z, z1: b.max.z, what: n.l });
      stairAngles(o);
    }
  });
  doors.forEach(d => {
    const sl = app.slidingInfo && app.slidingInfo(d.node.n);
    if (sl) {   // a sliding door: every panel blocks; the moving one blocks where it stands, so only the cleared part is a way through
      for (const p of sl.panels) obstacles.push({ x0: p.box.min.x, x1: p.box.max.x, y0: p.box.min.y, y1: p.box.max.y, z0: p.box.min.z, z1: p.box.max.z, what: d.node.l, slide: p.moving ? { id: d.node.n, vx: sl.dir.x, vy: sl.dir.y, t: sl.travel } : null });
      return;
    }
    obstacles.push({ x0: d.box.min.x, x1: d.box.max.x, y0: d.box.min.y, y1: d.box.max.y, z0: d.box.min.z, z1: d.box.max.z, door: d.node.n, what: d.node.l });
  });
  Object.assign(app, { walkObstacles: obstacles });
}
function stairAngles(o) {
  const lo = [], hi = [];
  o.traverse(m => {
    if (!m.isMesh) return; const P = m.geometry.attributes.position;
    for (let i = 0; i < P.count; i++) { const z = P.getZ(i); if (z < 0.3) lo.push([P.getX(i), P.getY(i)]); else if (z > 2.45 && z < 2.75) hi.push([P.getX(i), P.getY(i)]); }
  });
  const ang = pts => { if (!pts.length) return null; let sx = 0, sy = 0; pts.forEach(p => { sx += p[0] - stair.cx; sy += p[1] - stair.cy; }); return Math.atan2(sy, sx); };
  stair.a0 = ang(lo) ?? Math.PI; stair.a1 = ang(hi) ?? 0;
}

// ---------- collision ----------
function floorZ(storey) { return storeyZ[storey] ?? 0; }
function hitsObstacle(x, y, storey) {
  const z0 = floorZ(storey) + 0.15, z1 = floorZ(storey) + 1.7;
  for (const o of obstacles) {
    if (o.z1 <= z0 || o.z0 >= z1) continue;
    if (o.door && app.openness(o.door) >= 0.5) continue;
    if (o.circle) { if (Math.hypot(x - o.cx, y - o.cy) < o.r + R) return o; continue; }
    let x0 = o.x0, x1 = o.x1, y0 = o.y0, y1 = o.y1;
    if (o.slide) { const sh = app.openness(o.slide.id) * o.slide.t; x0 += o.slide.vx * sh; x1 += o.slide.vx * sh; y0 += o.slide.vy * sh; y1 += o.slide.vy * sh; }
    const dx = x - clamp(x, x0, x1), dy = y - clamp(y, y0, y1);
    if (dx * dx + dy * dy < R * R) return o;
  }
  return null;
}
export function blocked(x, y, storey = W.storey) {
  const P = app.plot;   // the plot (the fence of the scenery stands on its edge)
  if (P ? (x < P.x0 + 0.7 || x > P.x1 - 0.7 || y < P.y0 + 0.7 || y > P.y1 - 0.7) : Math.hypot(x - 6, y - 5) > 40) return true;
  if (hitsObstacle(x, y, storey)) return true;
  if (storey >= 1) {
    const fz = floorZ(storey);
    ray.set(new THREE.Vector3(x, y, fz + 1.0), new THREE.Vector3(0, 0, -1)); ray.far = 1.4;
    const down = nf(() => ray.intersectObjects(slabUpper, false));
    if (!down.length || Math.abs(down[0].point.z - fz) > 0.25) return true;
    ray.set(new THREE.Vector3(x, y, fz + 0.3), new THREE.Vector3(0, 0, 1)); ray.far = 4;
    const up = nf(() => ray.intersectObjects(roofSlabs, false));
    if (up.length && up[0].point.z < fz + 1.75) return true;
  }
  return false;
}
// move by (dx,dy) with sliding; returns the distance really moved
export function step(dx, dy) {
  const len = Math.hypot(dx, dy); if (!len) return 0;
  const n = Math.max(1, Math.ceil(len / STEP)), sx = dx / n, sy = dy / n;
  const x0 = W.x, y0 = W.y;
  for (let i = 0; i < n; i++) {
    if (!blocked(W.x + sx, W.y, W.storey)) W.x += sx;
    if (!blocked(W.x, W.y + sy, W.storey)) W.y += sy;
  }
  return Math.hypot(W.x - x0, W.y - y0);
}
export function roomAt(x = W.x, y = W.y, storey = W.storey) {
  const fz = floorZ(storey); let best = null, ba = 1e9;
  app.roomByGuid.forEach(r => {
    const b = r.box;
    if (x >= b.min.x && x <= b.max.x && y >= b.min.y && y <= b.max.y && b.min.z <= fz + 0.6 && b.max.z >= fz + 1.2) {
      const a = (b.max.x - b.min.x) * (b.max.y - b.min.y); if (a < ba) { ba = a; best = r; }
    }
  });
  return best;
}
// nearest free spot to (x,y) on a storey, for placing the walker in a room
export function findFree(x, y, storey = 0) {
  if (!blocked(x, y, storey)) return [x, y];
  for (let r = 0.2; r < 3; r += 0.2) for (let a = 0; a < 360; a += 30) {
    const px = x + Math.cos(a * Math.PI / 180) * r, py = y + Math.sin(a * Math.PI / 180) * r;
    if (!blocked(px, py, storey)) return [px, py];
  }
  return [x, y];
}

// ---------- the camera ----------
function placeCamera() {
  const c = app.camera;
  c.position.set(W.x, W.y, floorZ(W.storey) + EYE + (W.zOff || 0));
  c.rotation.order = 'ZXY'; c.rotation.set(Math.PI / 2 + W.pitch, 0, W.yaw);
}
export function teleport(x, y, o = {}) {
  W.x = x; W.y = y; if (o.storey != null) W.storey = o.storey; if (o.yaw != null) W.yaw = o.yaw; if (o.pitch != null) W.pitch = o.pitch;
  W.zOff = 0; if (W.active) { placeCamera(); app.requestRender(); }
}
function spawnPoint() {
  const ds = app.graph.nodes.filter(n => n.c === 'IfcDoor');
  const n = ds.find(d => /haus/i.test(d.l)) || ds.find(d => (d.bd || []).length === 1) || ds[0];
  const a = n && app.openables().find(o => o.guid === n.n);
  if (!a) return { x: -2.2, y: 5, yaw: -Math.PI / 2, storey: 0 };
  const b = new THREE.Box3().setFromObject(a.group), c = b.getCenter(new THREE.Vector3());
  const room = (n.bd || []).map(i => app.roomByGuid.get(app.graph.nodes[i].n)).filter(Boolean)[0];
  const out = room ? -Math.sign(room.center[a.tAx] - c[a.tAx]) : -1;
  const p = { x: c.x, y: c.y }; p[a.tAx] += out * 2.4;
  const f = { x: 0, y: 0 }; f[a.tAx] = -out;
  return { x: p.x, y: p.y, yaw: Math.atan2(-f.x, f.y), storey: 0 };
}

// ---------- interaction ----------
function aim() {
  ray.setFromCamera(new THREE.Vector2(0, 0), app.camera); ray.far = 2.6;
  const list = solid.filter(o => o.visible).concat(app.pickables ? [...app.pickables.keys()] : []);
  const hits = nf(() => ray.intersectObjects(list, true));
  for (const h of hits) {
    let o = h.object, guid = null, pick = null;
    while (o) { if (o.userData.pickId) pick = pick || o.userData.pickId; if (o.userData.guid) { guid = o.userData.guid; break; } o = o.parent; }
    if (pick) return { kind: 'pick', id: pick, dist: h.distance };
    if (!guid) continue;
    const n = app.nodeByGuid.get(guid);
    if (n && (n.c === 'IfcDoor' || n.c === 'IfcWindow')) return { kind: n.c === 'IfcDoor' ? 'door' : 'window', id: guid, node: n, dist: h.distance };
    if (n && n.c === 'IfcStair') return { kind: 'stair', id: guid, node: n, dist: h.distance };
    return null;   // a wall or something else is in the way
  }
  return null;
}
function nearStair() { return stair && Math.hypot(W.x - stair.cx, W.y - stair.cy) < stair.r + 1.1; }
export function interact() {
  if (!W.active || W.climbing) return null;
  const t = aim();
  if (t && (t.kind === 'door' || t.kind === 'window')) { app.toggleState(t.id); return { did: 'toggle', id: t.id, state: app.getState(t.id) }; }
  if (t && t.kind === 'pick') { app.select(t.id); app.emit('interact', { id: t.id }); return { did: 'pick', id: t.id }; }
  if ((t && t.kind === 'stair') || nearStair()) { climb(); return { did: 'climb' }; }
  return null;
}

// ---------- the stair ----------
export function climb() {
  if (!stair || W.climbing) return false;
  const up = W.storey === 0, to = up ? 1 : 0;
  const a0 = up ? stair.a0 : stair.a1, a1 = up ? stair.a1 : stair.a0;
  let sweep = (((a1 - a0) % (2 * Math.PI)) + 2 * Math.PI) % (2 * Math.PI);
  if (sweep < Math.PI) sweep += 2 * Math.PI;
  if (!up) sweep = -sweep;
  // where we step off
  const aE = up ? stair.a1 : stair.a0;
  let exit = null;
  for (const da of [0, 0.5, -0.5, 1, -1, 1.5, -1.5, 2, -2, 2.6, -2.6, Math.PI]) for (const rr of [1.15, 1.35, 1.6]) {
    const x = stair.cx + Math.cos(aE + da) * rr, y = stair.cy + Math.sin(aE + da) * rr;
    if (!exit && !blocked(x, y, to)) exit = { x, y, a: aE + da };
  }
  if (!exit) return false;
  W.climbing = true;
  const z0 = floorZ(W.storey), z1 = floorZ(to), rad = Math.max(0.45, stair.r - 0.3);
  let t = 0;
  app.addTicker(dt => {
    t = Math.min(1, t + dt / 3.0);
    const k = t * t * (3 - 2 * t), a = a0 + sweep * k;
    const c = app.camera;
    c.position.set(stair.cx + Math.cos(a) * rad, stair.cy + Math.sin(a) * rad, z0 + (z1 - z0) * k + EYE);
    W.yaw = Math.sign(sweep) > 0 ? a : a + Math.PI; W.pitch = 0.05;
    c.rotation.order = 'ZXY'; c.rotation.set(Math.PI / 2 + W.pitch, 0, W.yaw);
    if (t >= 1) {
      W.storey = to; W.x = exit.x; W.y = exit.y; W.yaw = exit.a - Math.PI / 2; W.climbing = false;
      placeCamera(); app.emit('walk', walkInfo()); return false;
    }
  }, { scope: '3d' });
  return true;
}

// ---------- enter / leave ----------
export function walkInfo() {
  const r = W.active ? roomAt() : null;
  return { active: W.active, x: +W.x.toFixed(2), y: +W.y.toFixed(2), z: +(floorZ(W.storey) + EYE).toFixed(2), storey: W.storey, yaw: +W.yaw.toFixed(2), room: r ? r.label : null, roomId: r ? r.guid : null };
}
export function enterWalk(o = {}) {
  if (W.active) { if (o.x != null) teleport(o.x, o.y, o); return walkInfo(); }
  const c = app.camera;
  saved = { pos: c.position.clone(), target: app.controls.target.clone(), fov: c.fov, quat: c.quaternion.clone() };
  const sp = (o.x != null) ? { x: o.x, y: o.y, yaw: o.yaw ?? 0, storey: o.storey ?? 0 } : spawnPoint();
  W.active = true; W.climbing = false; W.pitch = 0;
  W.x = sp.x; W.y = sp.y; W.yaw = sp.yaw; W.storey = sp.storey; W.zOff = 0;
  app.mode = 'walk'; app.controls.enabled = false;
  c.fov = 70; c.updateProjectionMatrix();
  placeCamera();
  document.body.classList.add('walking');
  if (ui) ui.root.hidden = false;
  setHelp();
  if (o.lock !== false && !('ontouchstart' in window && matchMedia('(pointer:coarse)').matches)) { lockWanted = true; try { app.renderer.domElement.requestPointerLock?.(); } catch { /* not allowed */ } }
  app.addTicker(tick, { scope: '3d' });
  app.emit('mode', { mode: 'walk' });
  return walkInfo();
}
export function leaveWalk() {
  if (!W.active) return walkInfo();
  W.active = false; W.climbing = false; lockWanted = false;
  if (document.pointerLockElement) try { document.exitPointerLock(); } catch { /* ignore */ }
  const c = app.camera;
  c.fov = saved.fov; c.updateProjectionMatrix(); c.up.set(0, 0, 1);
  c.position.copy(saved.pos); app.controls.target.copy(saved.target); c.lookAt(saved.target);
  app.mode = 'orbit'; app.controls.enabled = true; app.controls.update();
  document.body.classList.remove('walking');
  if (ui) ui.root.hidden = true;
  keys.clear(); stick.x = stick.y = 0;
  app.requestRender();
  app.emit('mode', { mode: 'orbit' });
  return walkInfo();
}
export const toggleWalk = () => W.active ? leaveWalk() : enterWalk();

// ---------- per-frame ----------
function tick(dt) {
  if (!W.active) return false;
  if (!W.climbing) {
    let f = (keys.has('w') || keys.has('arrowup') ? 1 : 0) - (keys.has('s') || keys.has('arrowdown') ? 1 : 0) - stick.y;
    let s = (keys.has('d') || keys.has('arrowright') ? 1 : 0) - (keys.has('a') || keys.has('arrowleft') ? 1 : 0) + stick.x;
    const m = Math.hypot(f, s);
    if (m > 1) { f /= m; s /= m; }
    if (m > 0.01) {
      const v = (keys.has('shift') ? 3.4 : 1.7) * dt;
      const fx = -Math.sin(W.yaw), fy = Math.cos(W.yaw), rx = Math.cos(W.yaw), ry = Math.sin(W.yaw);
      step((fx * f + rx * s) * v, (fy * f + ry * s) * v);
    }
    // turn with arrow-left/right + q/e is left out on purpose: left/right strafe like A/D
    placeCamera();
  }
  const now = performance.now();
  if (now - hintAt > 120) { hintAt = now; setHelp(); }
  return true;
}
function setHelp() {
  if (!ui) return;
  const info = walkInfo();
  ui.room.textContent = info.room ? info.room : (W.storey ? 'Upper floor' : 'Outside');
  const t = W.climbing ? null : aim();
  targetNow = t;
  let hint = '';
  if (t && (t.kind === 'door' || t.kind === 'window')) hint = `${app.getState(t.id) === 'closed' ? 'Open' : 'Close'} ${t.node.l}`;
  else if (t && t.kind === 'stair' || (!W.climbing && nearStair())) hint = W.storey === 0 ? 'Climb the stair' : 'Go down the stair';
  else if (t && t.kind === 'pick') { let h = 'Use'; for (const p of app.pickables.values()) if (p.id === t.id && p.meta.hint) { h = typeof p.meta.hint === 'function' ? p.meta.hint() : p.meta.hint; break; } hint = h; }
  ui.target.textContent = hint ? (matchMedia('(pointer:coarse)').matches ? hint : 'E  ' + hint) : '';
  ui.target.style.opacity = hint ? 1 : 0;
  ui.cross.classList.toggle('on', !!hint);
  ui.use.style.display = hint ? '' : 'none';
}

// ---------- input and overlay ----------
export function initWalk() {
  const stage = document.getElementById('stage');
  const root = document.createElement('div'); root.id = 'walkUi'; root.hidden = true;
  root.innerHTML = `<div class="wk-room"></div><div class="wk-cross"></div><div class="wk-target"></div>
    <div class="wk-help">W A S D move · Shift run · mouse look · E, Space or click use · F or Esc leave</div>
    <button class="wk-exit btn">Leave</button><button class="wk-use btn" style="display:none">Use</button>
    <div class="wk-stick"><i></i></div>`;
  stage.appendChild(root);
  ui = { root, room: root.querySelector('.wk-room'), cross: root.querySelector('.wk-cross'), target: root.querySelector('.wk-target'), use: root.querySelector('.wk-use'), stick: root.querySelector('.wk-stick') };
  if (matchMedia('(pointer:coarse)').matches) root.querySelector('.wk-help').textContent = 'Left stick moves · drag on the right to look · Use opens doors';
  ui.stick.style.display = matchMedia('(pointer:coarse)').matches ? 'block' : 'none';
  root.querySelector('.wk-exit').onclick = leaveWalk;
  ui.use.onclick = interact;

  addEventListener('keydown', e => {
    if (isTyping() || e.ctrlKey || e.metaKey || e.altKey) return;
    const k = e.key.toLowerCase();
    if (k === 'f') { e.preventDefault(); toggleWalk(); return; }
    if (!W.active) return;
    if (k === 'escape') { leaveWalk(); return; }
    if (k === 'e' || k === ' ') { e.preventDefault(); interact(); return; }
    if (['w', 'a', 's', 'd', 'arrowup', 'arrowdown', 'arrowleft', 'arrowright', 'shift'].includes(k)) { keys.add(k); e.preventDefault(); }
  });
  addEventListener('keyup', e => keys.delete(e.key.toLowerCase()));
  addEventListener('blur', () => { keys.clear(); stick.x = stick.y = 0; });

  const cv = app.renderer.domElement;
  document.addEventListener('pointerlockchange', () => {
    const was = W.locked; W.locked = document.pointerLockElement === cv;
    if (was && !W.locked && W.active) leaveWalk();      // Esc while locked
  });
  document.addEventListener('mousemove', e => {
    if (!W.active || !W.locked || W.climbing) return;
    W.yaw -= e.movementX * 0.0022; W.pitch = clamp(W.pitch - e.movementY * 0.0022, -1.4, 1.4); placeCamera(); app.requestRender();
  });
  // pointer: mouse drag looks when not locked, a click uses; touch: left half stick, right half look
  let look = null;
  cv.addEventListener('pointerdown', e => {
    if (!W.active) return;
    if (W.locked) { if (e.button === 0) interact(); return; }
    const r = rect3d(cv);   // the 3D pane in the four-view layout
    if (e.clientX < r.left || e.clientX > r.right || e.clientY < r.top || e.clientY > r.bottom) return;
    if (e.pointerType === 'touch' && e.clientX - r.left < r.width * 0.4) { stick.id = e.pointerId; stick.ox = e.clientX; stick.oy = e.clientY; cv.setPointerCapture(e.pointerId); return; }
    look = { id: e.pointerId, x: e.clientX, y: e.clientY, moved: 0, touch: e.pointerType === 'touch' }; cv.setPointerCapture(e.pointerId);
  });
  cv.addEventListener('pointermove', e => {
    if (!W.active) return;
    if (stick.id === e.pointerId) {
      const dx = e.clientX - stick.ox, dy = e.clientY - stick.oy, m = Math.min(1, Math.hypot(dx, dy) / 55), a = Math.atan2(dy, dx);
      stick.x = Math.cos(a) * m; stick.y = Math.sin(a) * m;
      const k = ui.stick.querySelector('i'); k.style.transform = `translate(${Math.cos(a) * m * 40}px,${Math.sin(a) * m * 40}px)`; return;
    }
    if (look && look.id === e.pointerId && !W.climbing) {
      const dx = e.clientX - look.x, dy = e.clientY - look.y; look.x = e.clientX; look.y = e.clientY; look.moved += Math.abs(dx) + Math.abs(dy);
      const s = look.touch ? 0.005 : 0.004;
      W.yaw -= dx * s; W.pitch = clamp(W.pitch - dy * s, -1.4, 1.4); placeCamera(); app.requestRender();
    }
  });
  const up = e => {
    if (stick.id === e.pointerId) { stick.id = null; stick.x = stick.y = 0; ui.stick.querySelector('i').style.transform = ''; return; }
    if (look && look.id === e.pointerId) {
      const tap = look.moved < 6; look = null;
      if (tap && W.active) { if (!matchMedia('(pointer:coarse)').matches && lockWanted) { try { cv.requestPointerLock?.(); } catch { /* ignore */ } } else interact(); }
    }
  };
  cv.addEventListener('pointerup', up); cv.addEventListener('pointercancel', up);

  const api = { enter: enterWalk, leave: leaveWalk, toggle: toggleWalk, info: walkInfo, teleport, step, blocked, interact, climb, roomAt, findFree, get active() { return W.active; }, get state() { return W; } };
  app.walk = api;
}
