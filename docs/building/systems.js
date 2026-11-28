// The building-services layer: a designed example (not part of the IFC) drawn over the
// house. This module builds the parts, keeps the states and what is live, traces supply,
// runs scenarios, x-ray and isolate. The panels are in systems-ui.js, the simulation in
// thermal.js; both and the WebMCP tools use only app.svc, which this module fills.
import * as THREE from 'three';
import { app } from './app.js';
import { buildNode, tubeGeometry, runRadius, loopPath, mat, BODY, CEILING, FURNITURE } from './fixtures.js';
import { BatchSet, PROXY } from './batch.js';
import { structureNodeGroup, initStructure } from './structure.js';

// types whose off/closed state cuts everything downstream of them
const GATING = new Set(['grid-connection', 'rcd', 'breaker', 'valve', 'heat-pump-outdoor', 'heat-pump-indoor', 'water-heater', 'floor-loop', 'hrv-unit', 'router', 'net-switch', 'access-point']);
const INDOOR_LIGHTS = new Set(['light-ceiling', 'light-spot', 'light-wall']);
const LEDS = new Set(['smoke-detector', 'access-point', 'router', 'net-switch', 'fibre-entry', 'hrv-unit', 'dishwasher', 'washing-machine', 'ac-indoor']);
export const COL = { select: 0xffd54a, trace: 0x33d6ff, scen: 0xff5a5a, scenOff: 0xffa13d, result: 0x33d6ff };
const NO_WORD = { electrical: 'no power', lighting: 'no power', safety: 'no power', network: 'no connection', 'water-cold': 'no water', 'water-hot': 'no hot water', drainage: 'no drainage', heating: 'no heat', ventilation: 'no air', cooling: 'no cooling' };
const TYPE_WORD = { breaker: 'breaker', rcd: 'RCD', valve: 'valve', 'grid-connection': 'grid connection', router: 'router', 'net-switch': 'network switch', 'heat-pump-outdoor': 'heat pump', 'heat-pump-indoor': 'heat pump', 'hrv-unit': 'ventilation unit', 'water-heater': 'water heater', 'floor-loop': 'floor loop', 'access-point': 'access point' };

let svc = null;

export function initSystems(data) {
  const sysList = data.systems, sysById = new Map(sysList.map(s => [s.id, s]));
  const nodeById = new Map(data.nodes.map(n => [n.id, n])), runById = new Map(data.runs.map(r => [r.id, r])), groupById = new Map(data.groups.map(g => [g.id, g]));
  const parents = new Map(), children = new Map();
  for (const [a, b] of data.feeds) { (children.get(a) || children.set(a, []).get(a)).push(b); (parents.get(b) || parents.set(b, []).get(b)).push(a); }
  const controls = new Map(), controlledBy = new Map();
  for (const [a, b] of data.controls) { (controls.get(a) || controls.set(a, []).get(a)).push(b); (controlledBy.get(b) || controlledBy.set(b, []).get(b)).push(a); }
  const groupsOf = new Map();
  for (const g of data.groups) for (const m of g.members) (groupsOf.get(m) || groupsOf.set(m, []).get(m)).push(g);
  const roomLabel = new Map(data.rooms.map(r => [r.guid, r.label]));
  const things = new Map(), nodes = [], runs = [];
  const sysGroup = new Map();
  const report = { shifted: [] };
  const bootLights = app.getInteriorLights();

  // ---------- geometry: where each thing really sits ----------
  app.house.updateMatrixWorld(true);
  const targets = []; app.nodeByGuid.forEach((n, g) => { if (n.k === 'Space' || n.c === 'IfcOpeningElement') return; const o = app.meshByGuid.get(g); if (o) targets.push(o); });
  const ray = new THREE.Raycaster();
  const cast = (o, d, far) => {
    ray.set(o, d); ray.far = far; const h = ray.intersectObjects(targets, true)[0]; if (!h || !h.face) return null;
    return { t: h.distance, point: h.point.clone(), n: h.face.normal.clone().transformDirection(h.object.matrixWorld) };
  };
  const Z = new THREE.Vector3(0, 0, 1);
  // fixtures whose coordinates sit on an installation wall are exact: they are not pulled onto the wall behind it
  const prewalls = data.nodes.filter(n => n.type === 'pre-wall');
  const mountedOn = n => n.type === 'pre-wall' || prewalls.some(w => {
    const hx = (w.dir ? Math.abs(w.dir[0]) : 1) * w.size[1] / 2 + (w.dir ? Math.abs(w.dir[1]) : 0) * w.size[0] / 2, hy = (w.dir ? Math.abs(w.dir[0]) : 1) * w.size[0] / 2 + (w.dir ? Math.abs(w.dir[1]) : 0) * w.size[1] / 2;
    return Math.abs(n.p[0] - w.p[0]) <= hx + 0.45 && Math.abs(n.p[1] - w.p[1]) <= hy + 0.45 && n.p[2] <= w.p[2] + w.size[2] / 2 + 0.3;
  });
  function place(node) {
    const p = new THREE.Vector3(...node.p), type = node.type;
    const sz = node.size; let dir = node.dir ? new THREE.Vector3(node.dir[0], node.dir[1], 0).normalize() : null;
    const q = new THREE.Quaternion(), res = { pos: p.clone(), quat: q, how: 'free' };
    const yaw = d => new THREE.Quaternion().setFromAxisAngle(Z, Math.atan2(d.y, d.x));
    if (CEILING.has(type) && !dir) {
      const h = cast(p.clone().add(new THREE.Vector3(0, 0, -0.3)), Z, 0.75);
      if (h && h.n.z < -0.3) { res.pos.copy(h.point); q.setFromUnitVectors(Z, h.n.clone().negate()); res.how = 'ceiling'; }
      return res;
    }
    if (type === 'floor-loop' || (type === 'drain' && p.z < 0.12) || type === 'vent-stack' || type === 'sewer-connection') { if (dir) q.copy(yaw(dir)); return res; }
    if (!dir) {   // a module in the board, a meter: the nearest wall tells which way it faces
      let best = null;
      for (const d of [[1, 0], [-1, 0], [0, 1], [0, -1]]) { const h = cast(p, new THREE.Vector3(d[0], d[1], 0), 0.6); if (h && Math.abs(h.n.z) < 0.3 && (!best || h.t < best.t)) best = h; }
      if (best) { dir = new THREE.Vector3(best.n.x, best.n.y, 0).normalize(); res.pos.copy(best.point); res.pos.z = p.z; res.how = 'wall'; q.copy(yaw(dir)); }
      return res;
    }
    q.copy(yaw(dir));
    if (mountedOn(node)) return res;
    if (BODY.has(type) || sz) {        // a body around its point: put its back on the wall if it stands off by a few cm
      const d = (sz || [0, 0.1])[1], h = cast(p, dir.clone().negate(), 1.2);
      if (h && h.n.dot(dir) > 0.5) {
        const gap = h.t - d / 2;
        const standoff = type === 'heat-pump-outdoor' || type === 'ac-outdoor';   // outdoor units stand off their wall on brackets
        if (!standoff && Math.abs(gap) > 0.003 && gap > -0.15 && gap < 0.3) { res.pos.addScaledVector(dir, -gap); res.how = 'body'; if (Math.abs(gap) > 0.05) report.shifted.push({ id: node.id, type, gapM: +gap.toFixed(3) }); }
      }
      return res;
    }
    const h = cast(p.clone().addScaledVector(dir, 0.4), dir.clone().negate(), 0.9);   // a plate: back on the wall face
    if (h && h.n.dot(dir) > 0.5) { res.pos.copy(h.point).addScaledVector(dir, 0.001); res.pos.z = p.z; res.how = 'wall'; /* 1 mm off the wall face: no shared plane */ const off = res.pos.distanceTo(p); if (off > 0.05) report.shifted.push({ id: node.id, type, offM: +off.toFixed(3) }); }
    return res;
  }

  // ---------- build ----------
  const markerMat = (() => {
    const c = document.createElement('canvas'); c.width = c.height = 64; const g = c.getContext('2d');
    g.fillStyle = 'rgba(20,24,34,.85)'; g.beginPath(); g.arc(32, 32, 28, 0, 7); g.fill(); g.strokeStyle = '#ff6b5b'; g.lineWidth = 6; g.stroke();
    g.beginPath(); g.moveTo(21, 21); g.lineTo(43, 43); g.moveTo(43, 21); g.lineTo(21, 43); g.stroke();
    const t = new THREE.CanvasTexture(c); t.colorSpace = THREE.SRGBColorSpace; return new THREE.SpriteMaterial({ map: t, transparent: true, depthTest: false, depthWrite: false, fog: false });
  })();
  for (const s of sysList) { const g = new THREE.Group(); g.name = 'svc-' + s.id; sysGroup.set(s.id, g); }
  const lampThings = [], moved = new Map();
  const prewallGroup = new THREE.Group(); prewallGroup.name = 'svc-prewall';   // shown while the drainage or the cold-water layer is
  const addMarker = t => { const s = new THREE.Sprite(markerMat); s.scale.set(0.16, 0.16, 1); s.renderOrder = 8; s.visible = false; const c = new THREE.Box3().setFromObject(t.obj).getCenter(new THREE.Vector3()); s.position.copy(t.obj.worldToLocal(c.clone().add(new THREE.Vector3(0, 0, 0.18)))); t.obj.add(s); t.marker = s; };

  for (const node of data.nodes) {
    const color = (sysById.get(node.system) || {}).color || '#8895aa';
    const isStruct = node.system === 'structure';   // drawn at true size in world coordinates (structure.js)
    const pl = isStruct ? { pos: new THREE.Vector3(), quat: new THREE.Quaternion(), how: 'structure' } : place(node);
    const t = { id: node.id, kind: 'node', d: node, system: node.system, live: true, tint: null, meshes: [], dyn: {}, room: node.room ? roomLabel.get(node.room) || null : null, why: null };
    if (!isStruct && pl.pos.distanceTo(new THREE.Vector3(...node.p)) > 0.01) moved.set(node.id, { old: new THREE.Vector3(...node.p), now: pl.pos.clone() });
    let obj;
    if (node.type === 'floor-loop') {
      obj = new THREE.Group(); const geo = tubeGeometry(loopPath(node), 0.008, 5), pair = mat('sys', color);
      const m = new THREE.Mesh(geo, pair.live); m.userData.mats = pair; obj.add(m); t.meshes.push(m);
      const [w, d] = node.size, hit = new THREE.Mesh(new THREE.BoxGeometry(d, w, 0.05), new THREE.MeshBasicMaterial({ visible: false }));
      hit.position.set(node.p[0], node.p[1], node.p[2] + 0.02); hit.rotation.z = Math.atan2(node.dir[1], node.dir[0]); obj.add(hit);
    } else {
      const b = isStruct ? structureNodeGroup(node) : buildNode(node, color); obj = new THREE.Group(); obj.add(b.group); obj.position.copy(pl.pos); obj.quaternion.copy(pl.quat);
      if (CEILING.has(node.type) && node.dir) b.group.rotation.y = -Math.PI / 2;   // an air valve on a wall faces out of it
      b.meshes.forEach(m => t.meshes.push(m)); t.unknown = b.unknown; t.dyn = b.dyn; if (b.dyn.lamp) { b.dyn.lamp.userData.lamp = true; lampThings.push(t); }
    }
    obj.name = node.id; t.obj = obj; t.place = pl.how;
    obj.userData.thingId = node.id;
    (node.type === 'pre-wall' ? prewallGroup : sysGroup.get(node.system))?.add(obj);
    things.set(node.id, t); nodes.push(t);
    addMarker(t);
  }
  for (const run of data.runs) {
    const color = (sysById.get(run.system) || {}).color || '#8895aa', pair = mat('sys', color);
    const pts = run.path.map(p => new THREE.Vector3(...p));
    const ma = moved.get(run.from), mb = moved.get(run.to);   // a fixture that was set against its wall: the pipe reaches it
    if (ma && pts[0].distanceTo(ma.old) < 0.02) pts.unshift(ma.now.clone());
    if (mb && pts[pts.length - 1].distanceTo(mb.old) < 0.02) pts.push(mb.now.clone());
    const geo = tubeGeometry(pts, runRadius(run), 7); if (!geo) continue;
    const m = new THREE.Mesh(geo, pair.live); m.userData.mats = pair;
    const obj = new THREE.Group(); obj.name = run.id; obj.add(m);
    const hit = new THREE.Mesh(tubeGeometry(pts, 0.03, 5), new THREE.MeshBasicMaterial({ visible: false })); obj.add(hit);
    const t = { id: run.id, kind: 'run', d: run, system: run.system, live: true, tint: null, meshes: [m], dyn: {}, obj, room: null, why: null };
    sysGroup.get(run.system)?.add(obj); things.set(run.id, t); runs.push(t);
  }

  // ---------- one batch for the whole layer ----------
  // Every mesh made above becomes an element of one BatchSet (batch.js): a handful of draw calls for about 500
  // fixtures and 160 runs. The meshes stay as invisible proxies: picking, boxes and ray casts use them.
  const sb = new BatchSet({ name: 'svc', parent: app.house, shadow: false });
  const sysIdx = new Map(sysList.map((s, i) => [s.id, i]));
  for (const t of things.values()) for (const m of t.meshes) {
    const pr = m.userData.mats, lv = pr.live;
    m.updateWorldMatrix(true, false);
    const lever = !!(t.dyn && t.dyn.lever === m);
    const el = sb.element(m, { dynamic: lever, src: m });
    sb.addPart(el, m.geometry, sb.localMatrix(m), { map: null, color: lv.color.clone(), rough: lv.roughness, metal: lv.metalness, alpha: lv.transparent ? lv.opacity : 1, side: THREE.FrontSide, env: !!lv.envMap, envBase: lv.envMapIntensity || 0.3, cast: false, receive: true, offset: t.unknown ? 7 : t.kind === 'run' ? 4 + (sysIdx.get(t.system) || 0) % 3 : m.userData.zi || 0 });   // runs sit behind fixtures; unknown boxes behind everything
    m.userData.el = el; m.material = PROXY; m.castShadow = false;
  }
  app.house.add(prewallGroup);
  sb.build();
  // the pre-wall is part of the house look: it shows while the drainage or the cold water is on
  const prewallOn = () => { const v = ['drainage', 'water-cold'].some(id => layers.get(id) && layers.get(id).isVisible()); if (prewallGroup.visible !== v) { prewallGroup.visible = v; app.requestRender(); } };
  prewallGroup.visible = false;
  app.addBeforeRender(prewallOn);

  // ---------- states ----------
  let busy = false;
  const OFFS = t => app.states(t.id).includes('off') ? 'off' : 'closed';   // the values the thing registered (structural elements have theirs from the state variables)
  for (const t of nodes) {
    const vals = (app.statevars && app.statevars.deviceValues(t.id, t.d.type)) || data.states[t.d.type]; if (!vals) continue;
    t.hasState = true;
    app.registerStateful(t.id, { values: vals, initial: t.d.state || vals[0], kind: t.d.type, label: t.d.label, onChange: s => onState(t, s) });
  }
  const stateOf = t => (t.hasState ? app.getState(t.id) : null);
  // what blocks the supply, per device type, from rule-supply of the data (statevars.js); the fixed list when the data has no rules
  const BLOCK = app.statevars ? app.statevars.supplyBlocking() : null;
  const enabled = t => {
    if (BLOCK) { const b = BLOCK[t.d.type]; if (!b) return true; const s = stateOf(t); return s == null || !b.includes(s); }
    return !GATING.has(t.d.type) || (() => { const s = stateOf(t); return s === 'on' || s === 'open' || s == null; })();
  };
  function onState(t, s) {
    if (busy) { dynFor(t); return; }
    busy = true;
    try {
      const type = t.d.type;
      if (type === 'switch') { for (const l of controls.get(t.id) || []) if (things.get(l)?.hasState) app.setState(l, s); for (const sw of syncSwitches(controls.get(t.id) || [])) app.setState(sw, s); }
      else if (INDOOR_LIGHTS.has(type) || type === 'light-outdoor') { for (const sw of controlledBy.get(t.id) || []) { const lights = controls.get(sw) || []; const any = lights.some(l => app.getState(l) === 'on'); if (app.getState(sw) !== (any ? 'on' : 'off')) app.setState(sw, any ? 'on' : 'off'); } }
    } finally { busy = false; }
    dynFor(t); recompute();
  }
  const syncSwitches = lights => { const out = new Set(); for (const l of lights) for (const sw of controlledBy.get(l) || []) out.add(sw); return [...out].filter(sw => things.get(sw)); };

  // ---------- what is live ----------
  let memo = new Map();
  const liveOf = id => {
    if (memo.has(id)) return memo.get(id);
    memo.set(id, true);                       // a cycle does not block itself
    const t = things.get(id); let ok = !t || enabled(t);
    if (ok) for (const p of parents.get(id) || []) if (!liveOf(p)) { ok = false; break; }
    memo.set(id, ok); return ok;
  };
  function culprit(id, seen = new Set()) {
    if (seen.has(id)) return null; seen.add(id);
    const t = things.get(id); if (t && !enabled(t)) return id;
    for (const p of parents.get(id) || []) { if (!liveOf(p)) { const c = culprit(p, seen); if (c) return c; } }
    return null;
  }
  const whyText = t => {
    const c = culprit(t.id); if (!c) return null; const ct = things.get(c), s = stateOf(ct);
    const word = NO_WORD[ct.system] || 'not live', kind = TYPE_WORD[ct.d.type] || ct.d.type;
    const lab = /^(breaker|RCD)$/.test(kind) ? `${kind} ${shortLabel(ct)}` : (ct.d.label.length > 34 ? ct.d.label.slice(0, 34) + '…' : ct.d.label);
    return { text: `${word}: ${lab} is ${s}`, by: c };
  };
  const shortLabel = t => { const m = /^(?:cb|rcd)-(\d+)/.exec(t.id); return m ? (t.d.type === 'breaker' ? 'C' : '') + m[1] : t.d.label; };
  function recompute() {
    memo = new Map(); const changed = [];
    for (const t of nodes) { const l = liveOf(t.id); if (l !== t.live) changed.push(t); t.live = l; }
    for (const r of runs) { const l = liveOf(r.d.from) && liveOf(r.d.to); if (l !== r.live) changed.push(r); r.live = l; }
    for (const t of nodes) t.why = t.live ? null : whyText(t);
    for (const t of runs) t.why = t.live ? null : (things.get(t.d.to)?.why || things.get(t.d.from)?.why || null);
    for (const t of changed) { refresh(t); if (t.marker) t.marker.visible = !t.live && t.kind === 'node'; }
    for (const t of nodes) dynFor(t);
    updateLights(); app.requestRender(); app.emit('svclive', { changed: changed.length });
  }

  // ---------- look: live, dead, highlight, fade ----------
  let iso = null;
  // The look of a thing is written into its batch: live or dead colour, a tint (selected, traced, scenario), faded
  // (another system is isolated), translucent (x-ray on the pre-wall) or glowing (a lamp that is on).
  const cA = new THREE.Color(), cB = new THREE.Color(), cE = new THREE.Color();
  function refresh(t) {
    for (const m of t.meshes) {
      const el = m.userData.el, pr = m.userData.mats; if (!el) continue;
      const base = t.live ? pr.live.color : pr.dead.color;
      let color = base, alpha = null, emissive = null;
      if (t.tint != null && !m.userData.lamp) { color = cA.copy(base).lerp(cB.setHex(t.tint), 0.35); emissive = cE.setHex(t.tint).multiplyScalar(0.85); }
      else if (iso && t.system !== iso) alpha = 0.07;
      else if (m.userData.glow > 0 && pr.glowColor) emissive = cE.set(pr.glowColor).multiplyScalar(m.userData.glow);
      if (t.d.type === 'pre-wall' && xray && !(iso && t.system !== iso)) alpha = xrayLevel;
      if (t.modeHex != null && t.tint == null && !m.userData.lamp) { color = cA.copy(base).lerp(cB.setHex(t.modeHex), t.glow ? 0.6 : 0.9); if (t.glow) emissive = cE.setHex(t.modeHex).multiplyScalar(0.6); }   // structure colour modes, load paths, what lost its support (structure.js)
      sb.setLook(el, { color, alpha, emissive });
    }
  }
  function dynFor(t) {
    const s = stateOf(t), type = t.d.type, d = t.dyn;
    if (d.lever && d.lever.userData.ls !== s) {
      d.lever.userData.ls = s;
      if (type === 'valve') d.lever.rotation.z = s === 'open' ? 0 : Math.PI / 2; else d.lever.position.z = s === 'on' ? 0.02 : -0.02;
      if (d.lever.userData.el) sb.pose(d.lever.userData.el);
    }
    if (d.lamp) {
      const pair = d.lamp.userData.mats;
      const lit = t.live && (INDOOR_LIGHTS.has(type) || type === 'light-outdoor' ? s === 'on' : type === 'smoke-detector' ? true : LEDS.has(type) ? (s == null || s === 'on') : type === 'cooker-hood' ? s === 'on' : false);
      const glow = lit ? (type === 'smoke-detector' ? (s === 'alarm' ? 3 : 0.6) : INDOOR_LIGHTS.has(type) || type === 'light-outdoor' ? 2.2 : 1.6) : 0;
      t.lit = lit;
      if (d.lamp.userData.glow !== glow) { d.lamp.userData.glow = glow; refresh(t); }
    }
  }
  const tints = { select: new Set(), trace: new Set(), result: new Set(), scen: new Map() };
  function retint() {
    for (const t of things.values()) {
      const id = t.id; let c = null;
      if (tints.select.has(id)) c = COL.select; else if (tints.trace.has(id)) c = COL.trace; else if (tints.scen.has(id)) c = tints.scen.get(id); else if (tints.result.has(id)) c = COL.result;
      if (c !== t.tint) { t.tint = c; refresh(t); }
    }
    app.requestRender();
  }
  const setTint = (kind, ids, col) => { const m = tints[kind]; m.clear?.(); ids.forEach(i => (m instanceof Map ? m.set(i, col(i)) : m.add(i))); retint(); };

  // ---------- real light: a point light per room, driven by the fixtures that are on ----------
  const lights = new THREE.Group(); lights.name = 'svcLights'; app.scene.add(lights);
  const roomLights = new Map();
  for (const l of app.interiorLightGroup.children) { const pl = new THREE.PointLight(0xffd9a8, 0, l.distance, l.decay); pl.userData.base = l.userData.base; pl.position.copy(l.position); pl.userData.home = l.position.clone(); lights.add(pl); roomLights.set(l.userData.room, pl); }
  app.interiorLightGroup.visible = false;
  const roomName = new Map(); app.roomByGuid.forEach((r, g) => roomName.set(r.label, g));
  function updateLights() {
    const acc = new Map();
    for (const t of nodes) {
      if (!INDOOR_LIGHTS.has(t.d.type) || !t.d.room) continue;
      const a = acc.get(t.d.room) || acc.set(t.d.room, { tot: 0, on: 0, c: new THREE.Vector3() }).get(t.d.room), w = t.d.props?.powerW || 10;
      a.tot += w; if (t.lit) { a.on += w; a.c.x += t.d.p[0] * w; a.c.y += t.d.p[1] * w; a.c.z += t.d.p[2] * w; }
    }
    roomLights.forEach((pl, guid) => {
      const a = acc.get(guid);
      if (!a || !a.on) { pl.intensity = 0; return; }
      pl.position.set(a.c.x / a.on, a.c.y / a.on, a.c.z / a.on - 0.3);
      pl.intensity = pl.userData.base * (0.35 + 0.65 * a.on / a.tot);
    });
    const cb = document.getElementById('lightsCb'); if (cb) cb.checked = anyIndoorOn();
    if (app.updateGlass) app.updateGlass();   // the windows glow at night while a light is on
    app.requestRender();
  }
  const anyIndoorOn = () => nodes.some(t => INDOOR_LIGHTS.has(t.d.type) && t.hasState && stateOf(t) === 'on');
  function driveInterior(on) {
    busy = true;
    try { for (const t of nodes) if (INDOOR_LIGHTS.has(t.d.type) && t.hasState && stateOf(t) !== (on ? 'on' : 'off')) app.setState(t.id, on ? 'on' : 'off'); for (const sw of nodes.filter(t => t.d.type === 'switch')) { const l = controls.get(sw.id) || []; const any = l.some(x => app.getState(x) === 'on'); if (app.getState(sw.id) !== (any ? 'on' : 'off')) app.setState(sw.id, any ? 'on' : 'off'); } }
    finally { busy = false; }
    nodes.forEach(dynFor); recompute();
  }
  const origSetInterior = app.setInteriorLights;
  app.setInteriorLights = (on, auto) => { origSetInterior(on, auto); driveInterior(!!on); };
  app.getInteriorLights = anyIndoorOn;
  app.on('lights', e => driveInterior(e.on));
  let lastNight = null;
  const outdoor = () => {
    const night = app.sun.get().elevation < 4; if (night === lastNight) return; lastNight = night;
    busy = true; try { for (const t of nodes) if (t.d.type === 'light-outdoor' && t.hasState) app.setState(t.id, night ? 'on' : 'off'); } finally { busy = false; }
    nodes.forEach(dynFor); recompute();
  };
  app.on('time', outdoor); app.on('site', outdoor);

  // ---------- pick, select ----------
  const hintOf = t => () => {
    if (t.d.type === 'switch') return (app.getState(t.id) === 'on' ? 'Switch off ' : 'Switch on ') + 'the light';
    if (t.hasState && t.kind === 'node' && ['breaker', 'rcd', 'valve'].includes(t.d.type)) return (app.getState(t.id) === 'on' || app.getState(t.id) === 'open' ? 'Switch off ' : 'Switch on ') + t.d.label.split(',')[0];
    return 'Use ' + t.d.label.split(',')[0];
  };
  for (const t of things.values()) app.registerPickable(t.obj, t.id, { label: t.d.label, kind: t.kind === 'run' ? t.d.kind : t.d.type, system: t.system, hint: hintOf(t) });
  app.on('interact', e => { const t = things.get(e.id); if (t && t.d.type === 'switch') app.toggleState(t.id); });
  app.on('select', e => {
    const t = e.kind === 'pickable' ? things.get(e.id) : null;
    tints.select = new Set(t ? [t.id] : []); retint();
    if (t) {
      const lay = layers.get(t.system); if (lay && !lay.isVisible()) lay.setVisible(true);
      if (t.room && app.mode === 'orbit' && !xray) setXrayUser(true);   // a thing inside a room is only seen through the walls
    }
  });
  app.on('state', () => {});

  // ---------- layers ----------
  const layers = new Map();
  const counts = id => nodes.filter(t => t.system === id).length + runs.filter(t => t.system === id).length;
  // (the layer rows are made by systems-ui.js; this is the model)
  function setLayer(sysId, vis) { const l = layers.get(sysId); if (!l) throw new Error('No system "' + sysId + '". Systems: ' + sysList.map(s => s.id).join(', ') + '.'); l.setVisible(!!vis); return l.isVisible(); }
  function layerState() { return sysList.map(s => ({ id: s.id, label: s.label, color: s.color, visible: layers.get(s.id) ? layers.get(s.id).isVisible() : false, nodes: nodes.filter(t => t.system === s.id).length, runs: runs.filter(t => t.system === s.id).length })); }

  // ---------- x-ray and isolate ----------
  let xray = false, xrayLevel = 0.08, xrayWanted = false;
  const shell = new Set(['IfcWallStandardCase', 'IfcSlab']);
  const origPaint = app.paint;
  const isShell = o => { let p = o; while (p && !p.userData.guid) p = p.parent; const n = p && app.nodeByGuid.get(p.userData.guid); return n && (iso ? n.k !== 'Space' && n.c !== 'IfcOpeningElement' : shell.has(n.c)); };
  app.paint = (obj, style) => {
    if (xray && isShell(obj) && !(style && (style.color != null || style.emissive != null))) style = { ...(style || {}), opacity: Math.min(xrayLevel, style && style.opacity != null ? style.opacity : 1) };
    return origPaint(obj, style);
  };
  function setXray(on) {
    xray = !!on; xrayLevel = iso ? 0.04 : 0.08;
    if (app.roomFloors) app.roomFloors.children.forEach(f => { const el = f.userData.els && f.userData.els[0]; if (el) el.set.setStyle(el, xray ? { opacity: 0.2 } : null); });
    nodes.forEach(t => { if (t.d.type === 'pre-wall') refresh(t); });
    app.restyle && app.restyle(); app.requestRender(); app.emit('xray', { on: xray }); return xray;
  }
  function isolate(sysId) {
    if (sysId != null && !sysById.has(sysId)) throw new Error('No system "' + sysId + '". Systems: ' + sysList.map(s => s.id).join(', ') + '.');
    iso = sysId || null;
    if (iso) { const l = layers.get(iso); if (l && !l.isVisible()) l.setVisible(true); }
    for (const [id, l] of layers) { if (iso && id !== iso && !l.isVisible()) l.setVisible(true); }   // faded systems stay in the picture
    nodes.forEach(refresh); runs.forEach(refresh);
    setXray(xrayWanted || !!iso);
    app.emit('isolate', { system: iso }); return iso;
  }
  const setXrayUser = on => { xrayWanted = !!on; return setXray(xrayWanted || !!iso); };

  // ---------- trace ----------
  function trace(id) {
    const t = things.get(id); if (!t) throw new Error('No service thing "' + id + '". Use list_devices to find one.');
    const startId = t.kind === 'run' ? t.d.from : id;
    const sys = sysById.get(things.get(startId)?.system), goals = new Set(sys ? sys.sources : []);
    const prev = new Map([[startId, null]]), q = [startId]; let found = null;
    while (q.length) {
      const x = q.shift(); if (goals.has(x) && x !== startId) { found = x; break; }
      for (const p of parents.get(x) || []) if (!prev.has(p)) { prev.set(p, x); q.push(p); }
    }
    if (!found) { // no goal reachable: follow to a root
      let x = startId; const seen = new Set([x]); while ((parents.get(x) || []).length) { const p = parents.get(x).find(y => !seen.has(y)); if (!p) break; prev.set(p, x); seen.add(p); x = p; } found = x;
    }
    const path = []; for (let x = found; x != null; x = prev.get(x)) path.push(x);   // from the source down to the start
    path.reverse(); const fromStart = path.slice().reverse();                        // start ... source
    const runIds = [];
    for (let i = 0; i + 1 < fromStart.length; i++) { const a = fromStart[i], b = fromStart[i + 1]; for (const r of runs) if ((r.d.from === a && r.d.to === b) || (r.d.from === b && r.d.to === a)) runIds.push(r.id); }
    if (t.kind === 'run' && !runIds.includes(id)) runIds.unshift(id);
    return { id, path: fromStart, source: found, runs: runIds, labels: fromStart.map(x => things.get(x)?.d.label || x) };
  }
  let lastTrace = null;
  function showTrace(id) {
    const tr = trace(id); lastTrace = tr;
    setTint('trace', [...tr.path, ...tr.runs]); app.emit('trace', tr);
    const objs = [...tr.path, ...tr.runs].map(x => things.get(x)?.obj).filter(Boolean);
    for (const x of tr.path) { const l = layers.get(things.get(x)?.system); if (l && !l.isVisible()) l.setVisible(true); }
    if (app.mode === 'orbit') app.focus(objs); return tr;
  }
  const clearTrace = () => { lastTrace = null; setTint('trace', []); app.emit('trace', null); };

  // ---------- scenarios ----------
  const scenById = new Map(data.scenarios.map(s => [s.id, s]));
  const defaults = new Map(); for (const t of nodes) if (t.hasState) defaults.set(t.id, t.d.state || app.states(t.id)[0]);
  const openingDefaults = (data.openings || []).map(o => [o.guid, o.state || 'closed']);
  let active = null;
  function resetStates({ openings = true } = {}) {
    busy = true;
    try { for (const [id, s] of defaults) if (app.getState(id) !== s) app.setState(id, s); if (openings) for (const [g, s] of openingDefaults) { try { if (app.getState(g) && app.getState(g) !== s) app.setState(g, s); } catch { /* fixed windows */ } } }
    finally { busy = false; }
    if (app.structure) app.structure.clearScenario();
    nodes.forEach(dynFor); recompute(); active = null; setTint('scen', [], null); app.emit('scenario', null); return true;
  }
  function applyScenario(id) {
    const s = scenById.get(id); if (!s) throw new Error('No scenario "' + id + '". Scenarios: ' + data.scenarios.map(x => x.id).join(', ') + '.');
    resetStates({ openings: false });
    if (s.kind === 'structure') {   // removing a wall, a lintel, a purlin: the structure module computes what loses its support (same result shape)
      const r = app.structure.applyScenario(id); active = r; return r;
    }
    busy = true; try { for (const o of s.off) { const t = things.get(o); if (t && t.hasState) app.setState(o, OFFS(t)); } } finally { busy = false; }
    nodes.forEach(dynFor); recompute();
    const affected = nodes.filter(t => !t.live && !s.off.includes(t.id)).map(t => t.id);
    const expect = new Set(s.affected), got = new Set(affected);
    const result = { id, label: s.label, description: s.description, off: s.off, affected, affectedRuns: runs.filter(r => !r.live).map(r => r.id), expected: s.affected.length, same: expect.size === got.size && [...expect].every(x => got.has(x)), sparql: s.sparql };
    active = result;
    const offSet = new Set(s.off);
    setTint('scen', [...affected, ...s.off, ...result.affectedRuns], i => (offSet.has(i) ? COL.scenOff : COL.scen));
    for (const x of [...affected, ...s.off]) { const l = layers.get(things.get(x)?.system); if (l && !l.isVisible()) l.setVisible(true); }
    app.emit('scenario', result); return result;
  }
  const byRoom = ids => { const g = {}; for (const id of ids) { const r = things.get(id)?.room || 'Outside or no room'; (g[r] = g[r] || []).push(id); } return g; };
  async function runScenarioQuery(id) {
    const s = scenById.get(id); if (!s) throw new Error('No scenario "' + id + '".');
    const { runSparql, toResult } = await import('./queries.js');
    const j = await runSparql(s.sparql);
    const base = data.base, ids = new Set(); for (const r of j.rows) { const t = String(r.thing || ''); if (t.startsWith('<' + base)) ids.add(t.slice(base.length + 1, -1)); }
    if (s.kind === 'structure') {   // the query is an approximation (everything that bears on the removed element, a superset): the exact set is the stored list
      const model = new Set(app.structure.state().unsupported.map(u => u.id)), contains = [...model].every(x => ids.has(x));
      return { graphCount: ids.size, modelCount: model.size, same: contains, approximate: true, note: 'The graph query lists everything that bears on the removed element; the exact set is the things with no path left.', result: toResult(s.label + ' (graph)', j), rows: j.rows.length, graphUrl: app.graphSource || null };
    }
    const model = new Set(nodes.filter(t => !t.live && !s.off.includes(t.id)).map(t => t.id));
    const same = ids.size === model.size && [...ids].every(x => model.has(x));
    return { graphCount: ids.size, modelCount: model.size, same, result: toResult(s.label + ' (graph)', j), rows: j.rows.length, graphUrl: app.graphSource || null };
  }

  // ---------- queries over things ----------
  const info = t => {
    const d = t.d, o = { id: t.id, label: d.label, system: t.system, kind: t.kind };
    if (t.kind === 'node' && t.system === 'structure' && app.structure) { try { o.structure = app.structure.info(t.id); } catch { /* not an element */ } }
    if (t.kind === 'node') { Object.assign(o, { type: d.type, ifc: d.ifc, room: t.room, storey: d.storey, position: d.p.map(v => +v.toFixed(3)) }); if (t.hasState) { o.state = stateOf(t); o.allowed = app.states(t.id); } o.live = t.live; if (!t.live && t.why) o.why = t.why.text; }
    else { Object.assign(o, { runKind: d.kind, from: d.from, to: d.to, diameterM: d.d, live: t.live }); if (!t.live && t.why) o.why = t.why.text; }
    o.props = d.props || {};
    return o;
  };
  function find({ system, type, room, text, kind } = {}) {
    const txt = text ? String(text).toLowerCase() : null, rm = room ? String(room).toLowerCase() : null;
    const pool = kind === 'runs' ? runs : kind === 'nodes' ? nodes : [...nodes, ...runs];
    return pool.filter(t => (!system || t.system === system) && (!type || t.d.type === type || t.d.kind === type) && (!rm || (t.room && t.room.toLowerCase().startsWith(rm)) || t.d.room === room) && (!txt || t.d.label.toLowerCase().includes(txt) || t.id.includes(txt)));
  }

  svc = {
    data, things, nodes, runs, sys: sysById, sysList, parents, children, controls, controlledBy, groupsOf, groupById, nodeById, runById, roomLabel, layers, sysGroup, report,
    GATING, INDOOR_LIGHTS, FURNITURE, lampThings,
    isLive: id => { const t = things.get(id); if (!t) throw new Error('No service thing "' + id + '".'); return t.live; },
    why: id => things.get(id)?.why || null, getState: stateOf, info, find, byRoom,
    setState(id, s) { const t = things.get(id); if (!t || !t.hasState) throw new Error(`"${id}" has no state to set.` + (t ? ` (${t.d.type})` : '')); app.setState(id, s); return { id, state: app.getState(id), live: t.live, why: t.why && t.why.text }; },
    toggle(id) { const t = things.get(id); if (!t || !t.hasState) throw new Error(`"${id}" has no state to toggle.`); return app.toggleState(id); },
    stateOf, recompute, refresh, retint, setTint, tints,
    trace, showTrace, clearTrace, get lastTrace() { return lastTrace; },
    scenarios: data.scenarios, applyScenario, resetStates, runScenarioQuery, byRoomOf: byRoom, get activeScenario() { return active; },
    setLayer, layerState, counts, setXray: setXrayUser, isXray: () => xray, isolate, get isolated() { return iso; },
    get xrayWanted() { return xrayWanted; },
    updateLights, driveInterior, has: id => things.has(id), batch: sb,
    thingFor: id => things.get(id),
    pickIds: () => [...things.keys()],
  };
  app.svc = svc;
  if (data.structure) initStructure(svc, data);
  recompute(); nodes.forEach(dynFor); updateLights();
  outdoor();
  if (bootLights) driveInterior(true);
  return svc;
}
export const services = () => svc;
