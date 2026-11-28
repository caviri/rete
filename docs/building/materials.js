// Materials: procedural canvas textures, what each element is made of, the flat class
// colours ("class" mode), room floors and labels, and paint(): the one place that
// tints/dims an element for selection and queries.
//
// Nothing here makes a THREE material per element any more. specFor() describes what an
// element is made of; batch.js draws all elements that share a texture as one mesh and
// keeps colour, alpha and glow per vertex. See batch.js and README.md ("Draw calls").
import * as THREE from 'three';
import { app } from './app.js';
import { colOf } from './classes.js';
import { BatchSet, PROXY } from './batch.js';

// ---------- small helpers ----------
function rng(seed) { let s = seed >>> 0; return () => (s = (Math.imul(s, 1664525) + 1013904223) >>> 0) / 4294967296; }
function hash(str) { let h = 2166136261; for (let i = 0; i < str.length; i++) { h ^= str.charCodeAt(i); h = Math.imul(h, 16777619); } return h >>> 0; }
function jitter(hex, key, amt = 0.04, sat = 0.03) {
  const r = rng(hash(key)), c = new THREE.Color(hex), hsl = {};
  c.getHSL(hsl);
  c.setHSL(hsl.h + (r() - .5) * 0.02, Math.min(1, Math.max(0, hsl.s + (r() - .5) * sat * 2)), Math.min(1, Math.max(0, hsl.l + (r() - .5) * amt * 2)));
  return '#' + c.getHexString();
}
function canvasTex(w, h, draw, size) {
  const c = document.createElement('canvas'); c.width = w; c.height = h;
  draw(c.getContext('2d'), w, h);
  const t = new THREE.CanvasTexture(c);
  t.wrapS = t.wrapT = THREE.RepeatWrapping; t.colorSpace = THREE.SRGBColorSpace; t.anisotropy = 1;
  t.userData.size = size;   // metres per tile (u, v); the UVs of a part are scaled by 1 / size when it is batched
  return t;
}

// ---------- the textures (made after the first frame) ----------
// 256 px where the pattern is fine grain, 512 for planks and tiles that are seen up close.
const TX = {};
let texturesMade = false;
function makeTextures() {
  if (texturesMade) return; texturesMade = true;
  const R = rng(11);
  TX.plaster = canvasTex(256, 256, (g, w, h) => {
    g.fillStyle = '#f4efe6'; g.fillRect(0, 0, w, h);
    for (let i = 0; i < 2500; i++) { g.fillStyle = `rgba(${R() < .5 ? '90,80,60' : '255,255,255'},${R() * 0.05})`; g.fillRect(R() * w, R() * h, 1 + R() * 3, 1 + R() * 3); }
    for (let i = 0; i < 30; i++) { const x = R() * w, y = R() * h, r = 20 + R() * 40, gr = g.createRadialGradient(x, y, 0, x, y, r); gr.addColorStop(0, 'rgba(120,100,70,.035)'); gr.addColorStop(1, 'rgba(120,100,70,0)'); g.fillStyle = gr; g.fillRect(x - r, y - r, 2 * r, 2 * r); }
  }, 2.2);
  TX.concrete = canvasTex(256, 256, (g, w, h) => {
    g.fillStyle = '#a3a29d'; g.fillRect(0, 0, w, h);
    for (let i = 0; i < 4000; i++) { const v = 100 + R() * 90; g.fillStyle = `rgba(${v},${v},${v - 4},${0.05 + R() * 0.12})`; g.fillRect(R() * w, R() * h, 1 + R() * 2, 1 + R() * 2); }
  }, 3);
  TX.lawn = canvasTex(256, 256, (g, w, h) => {
    g.fillStyle = '#5f7b44'; g.fillRect(0, 0, w, h);
    for (let i = 0; i < 6000; i++) { g.fillStyle = `hsla(${86 + R() * 28},${34 + R() * 22}%,${28 + R() * 30}%,${0.25 + R() * 0.4})`; g.fillRect(R() * w, R() * h, 1 + R() * 2, 1 + R() * 3); }
  }, 4);
  // wood grain on white: the colour of the wood is the colour of the part
  TX.grain = (size = 1.2) => canvasTex(256, 256, (g, w, h) => {
    g.fillStyle = '#ffffff'; g.fillRect(0, 0, w, h);
    for (let i = 0; i < 160; i++) {
      const y0 = R() * h, a = 0.05 + R() * 0.14, amp = 1 + R() * 3, f = 0.01 + R() * 0.03, ph = R() * 6;
      g.strokeStyle = `rgba(70,40,15,${a})`; g.lineWidth = 0.6 + R() * 1.4; g.beginPath();
      for (let x = 0; x <= w; x += 8) { const y = y0 + Math.sin(x * f + ph) * amp; x ? g.lineTo(x, y) : g.moveTo(x, y); }
      g.stroke();
    }
    for (let i = 0; i < 6; i++) { const x = R() * w, y = R() * h; g.fillStyle = 'rgba(70,40,15,.14)'; g.beginPath(); g.ellipse(x, y, 3 + R() * 4, 1.5 + R() * 2, 0, 0, 7); g.fill(); }
  }, size);
  TX.planks = (base, size = 1.2) => canvasTex(512, 512, (g, w, h) => {
    const n = 8, pw = w / n; const c = new THREE.Color(base);
    for (let i = 0; i < n; i++) {
      const split = (i * 37 % 100) / 100 * 0.5 + 0.25;
      [[0, split * h], [split * h, h]].forEach(([a, b]) => {
        const col = c.clone().multiplyScalar(0.86 + R() * 0.26); g.fillStyle = '#' + col.getHexString(); g.fillRect(i * pw, a, pw, b - a);
        for (let q = 0; q < 14; q++) { const x = i * pw + R() * pw; g.strokeStyle = `rgba(50,28,10,${0.05 + R() * 0.1})`; g.lineWidth = 0.7; g.beginPath(); g.moveTo(x, a); g.bezierCurveTo(x + R() * 4 - 2, a + (b - a) * .3, x + R() * 4 - 2, a + (b - a) * .7, x + R() * 3 - 1.5, b); g.stroke(); }
        g.fillStyle = 'rgba(30,16,6,.45)'; g.fillRect(i * pw, a, pw, 1.2);
      });
      g.fillStyle = 'rgba(30,16,6,.5)'; g.fillRect(i * pw, 0, 1.4, h);
    }
  }, size);
  TX.tiles = (base, grout = '#8f8c85', size = 1.2, n = 4) => canvasTex(512, 512, (g, w, h) => {
    const c = new THREE.Color(base), tw = w / n, gw = 4;
    g.fillStyle = grout; g.fillRect(0, 0, w, h);
    for (let i = 0; i < n; i++) for (let j = 0; j < n; j++) {
      const k = 0.94 + R() * 0.1; g.fillStyle = '#' + c.clone().multiplyScalar(k).getHexString();
      g.fillRect(i * tw + gw / 2, j * tw + gw / 2, tw - gw, tw - gw);
      const gr = g.createLinearGradient(i * tw, j * tw, (i + 1) * tw, (j + 1) * tw); gr.addColorStop(0, 'rgba(255,255,255,.08)'); gr.addColorStop(1, 'rgba(0,0,0,.07)');
      g.fillStyle = gr; g.fillRect(i * tw + gw / 2, j * tw + gw / 2, tw - gw, tw - gw);
    }
  }, size);
  TX.roof = canvasTex(256, 256, (g, w, h) => {
    const cols = 6, rows = 8, tw = w / cols, th = h / rows;
    g.fillStyle = '#4a2418'; g.fillRect(0, 0, w, h);
    for (let r = 0; r < rows; r++) {
      for (let c = -1; c < cols; c++) {
        const x = c * tw + (r % 2 ? tw / 2 : 0), y = r * th;
        const k = R(), col = `hsl(${12 + k * 8},${52 + k * 12}%,${33 + R() * 14}%)`;
        const gr = g.createLinearGradient(0, y, 0, y + th * 1.25); gr.addColorStop(0, 'rgba(255,220,190,.18)'); gr.addColorStop(0.55, 'rgba(0,0,0,0)'); gr.addColorStop(1, 'rgba(20,6,0,.5)');
        g.fillStyle = col; g.beginPath(); g.roundRect(x + 1, y, tw - 2, th * 1.3, 4); g.fill();
        g.fillStyle = gr; g.beginPath(); g.roundRect(x + 1, y, tw - 2, th * 1.3, 4); g.fill();
        g.fillStyle = 'rgba(20,6,0,.35)'; g.fillRect(x + tw / 2 - 0.8, y, 1.6, th * 1.1);
      }
    }
  }, [1.32, 1.6]);
}
// the generators make one canvas per call: remember them by arguments
function memoise() { ['grain', 'planks', 'tiles'].forEach(k => { const f = TX[k], cache = new Map(); TX[k] = (...a) => { const id = JSON.stringify(a); if (!cache.has(id)) cache.set(id, f(...a)); return cache.get(id); }; }); }

// ---------- what an element is made of ----------
// A part description: { map (a base texture or null), color (hex), rough, metal, alpha, side, glass, env, envBase }.
// The geometry carries UVs in metres; the batch scales them by 1 / (tile size in metres) of the map.
const D = (map, color = '#ffffff', o = {}) => ({ map, color, rough: 0.85, metal: 0, alpha: 1, side: THREE.FrontSide, ...o });
const metalD = (color = '#3a3d41') => D(null, color, { rough: 0.38, metal: 0.85, env: true, envBase: 1.2 });
const glassD = key => D(null, jitter('#a8cfe0', key, 0.02), { rough: 0.04, metal: 0.15, alpha: 0.3, side: THREE.DoubleSide, env: true, envBase: 1.6, glass: true });
const CENTER = new THREE.Vector2(6, 5);

// What a mesh is made of. Returns {parts, classify(faceNormal, faceCentre) -> index, roofUV}.
function specFor(node, obj) {
  const c = node ? node.c : '';
  const name = node ? node.l : '';
  const key = obj.userData.guid + (obj.userData.part || '');
  const role = obj.userData.role;
  const one = d => ({ parts: [d], classify: () => 0 });
  if (!node) return one(D(TX.lawn, '#ffffff', { rough: 1 }));   // the site plate under the house
  if (c === 'IfcWallStandardCase') {
    const ext = /^Wand-Ext/.test(name);
    if (ext) {
      return { parts: [D(TX.plaster, jitter('#ece1c8', key, 0.025)), D(TX.plaster, jitter('#f6f2ea', key, 0.02))],
        classify: (n, p) => (Math.abs(n.z) > 0.5 || (n.x * (p.x - CENTER.x) + n.y * (p.y - CENTER.y)) > 0) ? 0 : 1 };
    }
    return one(D(TX.plaster, jitter('#f4f0e8', key, 0.03)));
  }
  if (c === 'IfcSlab') {
    if (/^Dach/.test(name)) return { parts: [D(TX.roof, '#ffffff', { rough: 0.78 }), D(TX.grain(1.4), '#cfa877')], classify: n => n.z > 0.15 ? 0 : 1, roofUV: true };
    if (/^Bodenplatte/.test(name)) return one(D(TX.concrete, '#ffffff', { rough: 0.95 }));
    return { parts: [D(TX.planks('#b98a58', 1.2), '#ffffff', { rough: 0.6 }), D(TX.plaster, jitter('#f6f2ea', key, 0.02))], classify: n => n.z > 0.7 ? 0 : 1 };
  }
  if (c === 'IfcMember') return one(D(TX.grain(0.9), jitter('#b98a5a', key, 0.07), { rough: 0.7 }));
  if (c === 'IfcBeam') return one(D(TX.grain(1.0), jitter('#8a5f3a', key, 0.05), { rough: 0.65 }));
  if (c === 'IfcRailing') return one(metalD('#2f3236'));
  if (c === 'IfcStair') return one(D(TX.planks('#c79a68', 1.0), '#ffffff', { rough: 0.55 }));
  if (c === 'IfcSpace') return one(D(null, colOf(c), { alpha: 0.16, side: THREE.DoubleSide }));
  if (c === 'IfcOpeningElement') return one(D(null, colOf(c), { alpha: 0.4, side: THREE.DoubleSide }));
  if (c === 'IfcDoor') {
    const front = /Haus/i.test(name);
    if (role === 'glass') return one(glassD(key));
    if (role === 'handle') return one(metalD('#b9bcc0'));
    if (role === 'frame' || role === 'bar') return one(D(null, jitter('#f1efe9', key, 0.01), { rough: 0.5 }));
    if (front) return one(D(TX.grain(0.8), '#5a4332', { rough: 0.5 }));
    return one(D(TX.grain(0.8), jitter('#c9a074', key, 0.05), { rough: 0.55 }));
  }
  if (c === 'IfcWindow') {
    if (role === 'glass') return one(glassD(key));
    if (role === 'handle') return one(metalD('#b9bcc0'));
    return one(D(null, jitter('#f3f1ec', key, 0.008), { rough: 0.45 }));
  }
  return one(D(null, colOf(c), { rough: 0.8 }));
}
const clsD = (node) => D(null, colOf(node ? node.c : ''), { rough: 0.82, metal: 0.03, flat: true, alpha: node && node.k === 'Space' ? 0.16 : 0.92, side: THREE.DoubleSide, cls: true });

// ---------- geometry: non-indexed, with metre UVs and optional material groups ----------
function prepGeometry(src, classify, roofUV) {
  const g = src.index ? src.toNonIndexed() : src.clone();
  const P = g.attributes.position, N = g.attributes.normal;
  const nt = P.count / 3;
  const info = new Array(nt);
  const a = new THREE.Vector3(), b = new THREE.Vector3(), c = new THREE.Vector3(), fn = new THREE.Vector3(), fc = new THREE.Vector3();
  for (let t = 0; t < nt; t++) {
    a.fromBufferAttribute(P, t * 3); b.fromBufferAttribute(P, t * 3 + 1); c.fromBufferAttribute(P, t * 3 + 2);
    fn.subVectors(c, b).cross(a.clone().sub(b)).normalize();
    fc.copy(a).add(b).add(c).multiplyScalar(1 / 3);
    info[t] = { n: fn.clone(), c: fc.clone(), g: classify ? classify(fn, fc) : 0 };
  }
  const order = info.map((_, i) => i).sort((p, q) => info[p].g - info[q].g || p - q);
  const pos = new Float32Array(nt * 9), nor = new Float32Array(nt * 9), uv = new Float32Array(nt * 6);
  const groups = []; let gi = -1, gs = 0;
  order.forEach((t, k) => {
    const f = info[t];
    if (f.g !== gi) { if (gi >= 0) groups.push({ start: gs * 3, count: (k - gs) * 3, materialIndex: gi }); gi = f.g; gs = k; }
    const ax = Math.abs(f.n.x), ay = Math.abs(f.n.y), az = Math.abs(f.n.z);
    let tx = null;
    if (roofUV && Math.abs(f.n.x) < 0.35 && az > 0.2) { const l = Math.hypot(f.n.z, f.n.y) || 1; tx = [0, f.n.z / l, -f.n.y / l]; }
    for (let v = 0; v < 3; v++) {
      const si = t * 3 + v, di = k * 3 + v;
      const x = P.getX(si), y = P.getY(si), z = P.getZ(si);
      pos[di * 3] = x; pos[di * 3 + 1] = y; pos[di * 3 + 2] = z;
      if (N) { nor[di * 3] = N.getX(si); nor[di * 3 + 1] = N.getY(si); nor[di * 3 + 2] = N.getZ(si); }
      else { nor[di * 3] = f.n.x; nor[di * 3 + 1] = f.n.y; nor[di * 3 + 2] = f.n.z; }
      let u, w;
      if (tx) { u = x; w = y * tx[1] + z * tx[2]; }
      else if (az >= ax && az >= ay) { u = x; w = y; }
      else if (ax >= ay) { u = y; w = z; }
      else { u = x; w = z; }
      uv[di * 2] = u; uv[di * 2 + 1] = w;
    }
  });
  if (gi >= 0) groups.push({ start: gs * 3, count: (nt - gs) * 3, materialIndex: gi });
  const out = new THREE.BufferGeometry();
  out.setAttribute('position', new THREE.BufferAttribute(pos, 3));
  out.setAttribute('normal', new THREE.BufferAttribute(nor, 3));
  out.setAttribute('uv', new THREE.BufferAttribute(uv, 2));
  groups.forEach(x => out.addGroup(x.start, x.count, x.materialIndex));
  out.computeBoundingBox(); out.computeBoundingSphere();
  return out;
}

// ---------- the batches ----------
let mode = 'realistic';
let floors = null, labels = null;
let real = null, cls = null;
const envMats = [];
let aniso = 1;
const ccol = h => new THREE.Color(h);
// Faces of different elements that lie in one plane would flicker. The element with the lower offset wins, always
// (polygon offset, in the batch key). Frames beat walls, glass sits behind frames, bars and handles in front of glass,
// roof slabs beat the rafters and walls beat the floor slabs; the lawn plate is under everything.
function offsetFor(node, o, d) {
  if (!node) return 4;
  const c = node.c, role = o.userData.role;
  if (c === 'IfcWindow' || c === 'IfcDoor') { if (d.glass) return o.userData.moving ? 1 : 2; return role === 'bar' || role === 'handle' || role === 'sash' ? -2 : -1; }
  if (c === 'IfcSlab') return /^Dach/.test(node.l) ? -2 : 3;
  if (c === 'IfcBeam') return -1;
  return 0;
}
const specOf = (d, node, obj, soft) => {
  const sz = d.map ? d.map.userData.size : null, [su, sv] = sz ? (Array.isArray(sz) ? sz : [sz, sz]) : [1, 1];
  return { map: d.map, uvScale: [1 / su, 1 / sv], color: ccol(d.color), rough: d.rough, metal: d.metal, alpha: d.alpha, side: d.side, flat: d.flat, env: d.env, envBase: d.envBase,
    glass: d.glass, cast: !!node && !soft && !d.glass, receive: !soft, offset: d.cls ? 0 : offsetFor(node, obj, d) };
};
function onMaterial(m, s) { if (s.env) envMats.push(m); }
const newSet = name => new BatchSet({ name, parent: app.house, onMaterial, anisotropy: aniso });

export function initMaterials() {
  app.envMaterials = envMats;
  Object.assign(app, { lookOf, setColourMode, getColourMode: () => mode, paint, updateRoomDecor, updateGlass, setAnisotropy, poseMoving, batchSets: () => [real, cls].filter(Boolean), PROXY });
  app.on('lights', updateGlass);
  app.on('time', updateGlass);
}

// After the doors and windows have been split into parts: describe every element, make the batches.
export function applyMaterials(rooms) {
  makeTextures(); memoise();
  app.house.updateMatrixWorld(true);
  real = newSet('house');
  app.meshByGuid.forEach((top, guid) => {
    const node = app.nodeByGuid.get(guid);
    top.traverse(o => {
      if (!o.isMesh) return;
      o.userData.guid = guid;
      const spec = specFor(node, o);
      const geom = prepGeometry(o.geometry, spec.classify, spec.roofUV);
      o.geometry.dispose(); o.geometry = geom; o.material = PROXY; o.castShadow = o.receiveShadow = false;
      const soft = node && (node.k === 'Space' || node.c === 'IfcOpeningElement');
      const moving = !!o.userData.moving;
      o.userData.alpha = spec.parts[0].alpha; o.userData.descs = { spec, soft, node };
      const el = real.element(o, { dynamic: moving, src: o });
      const M = real.localMatrix(o);
      for (const g of geom.groups) real.addPart(el, geom, M, specOf(spec.parts[g.materialIndex] || spec.parts[0], node, o, soft), { start: g.start, count: g.count });
      o.userData.els = [el];
    });
  });
  buildRoomDecor(rooms || []);
  real.build();
  setColourMode(mode, true);
  updateGlass();
}

// "By class" is a second set of batches, made the first time it is asked for.
function ensureCls() {
  if (cls) return;
  cls = newSet('house-cls');
  real.elements.forEach(rel => {
    const o = rel.owner; if (!o.userData.descs) return;
    const { node, soft } = o.userData.descs, geom = o.geometry;
    const el = cls.element(o, { dynamic: rel.dynamic, src: o }), M = cls.localMatrix(o);
    for (const g of geom.groups) cls.addPart(el, geom, M, specOf(clsD(node), node, o, soft), { start: g.start, count: g.count });
    el.hidden = rel.hidden; o.userData.els[1] = el;
  });
  cls.build();
  real.elements.forEach(rel => { const o = rel.owner; if (o.userData.pstyle && o.userData.els[1]) cls.setStyle(o.userData.els[1], o.userData.pstyle); });
}

export function setColourMode(m, silent) {
  mode = m === 'class' ? 'class' : 'realistic';
  if (mode === 'class') ensureCls();
  if (real) real.root.visible = mode === 'realistic';
  if (cls) cls.root.visible = mode === 'class';
  if (floors) floors.visible = mode === 'realistic';
  app.requestRender();
  if (!silent) app.emit('colourmode', { mode });
}

export function setAnisotropy(n) {
  aniso = n;
  for (const set of [real, cls]) if (set) { set.opts.anisotropy = n; set.batches.forEach(b => { const t = b.spec.map; if (t && t.anisotropy !== n) { t.anisotropy = n; t.needsUpdate = true; } }); }
  app.requestRender();
}

// the batches draw from copies of door leaves and window sashes: write them again after they moved
function poseMoving(group) { group.traverse(o => { if (o.userData.moving && o.userData.els) for (const el of o.userData.els) if (el) el.set.pose(el); }); }

// what is drawn for a mesh now (the first vertex of its first part): colour, alpha, emissive. For tests and tools.
function lookOf(mesh) {
  const el = (mesh.userData.els && mesh.userData.els.find(e => e && e.set.root.visible)) || mesh.userData.els?.[0] || mesh.userData.el; if (!el) return null;
  const p = el.parts[0], b = p.batch, v = p.v0, c = b.col.array, e = b.em.array;
  return { color: [c[v * 3], c[v * 3 + 1], c[v * 3 + 2]], alpha: e[v * 4 + 3], emissive: [e[v * 4], e[v * 4 + 1], e[v * 4 + 2]] };
}

// windows glow while a light is on at night
function updateGlass() {
  if (!real || !real.built) return;
  const night = app.getInteriorLights && app.getInteriorLights() && app.sun && app.sun.get().elevation < 4;
  const col = night ? 0xffd9a0 : 0x000000, ei = night ? 0.45 : 0;
  for (const el of real.elements) if (el.parts.some(p => p.glass)) real.setBaseEmissive(el, col, ei);
  app.requestRender();
}

// paint(obj, style): style null restores; {color, opacity, emissive} override (undefined = base).
// The look of an element is a set of per-vertex values in its batch (see batch.js).
export function paint(obj, style) {
  obj.traverse(o => {
    if (!o.isMesh || !o.userData.els) return;
    o.userData.pstyle = style || null;
    o.userData.alpha = style && style.opacity != null ? style.opacity : o.userData.descs.spec.parts[0].alpha;
    for (const el of o.userData.els) if (el) el.set.setStyle(el, style);
  });
}

// ---------- room floors and labels ----------
const FLOOR = {
  Wohnen: ['planks', '#b8884f'], Buero: ['planks', '#a9784a'], Schlafzimmer: ['planks', '#cfa56f'],
  Küche: ['tiles', '#d9cdb6'], Bad: ['tiles', '#a9bcc4', '#7f8c91'], Flur: ['tiles', '#bda58a'],
};
export function buildRoomDecor(rooms) {
  floors = new THREE.Group(); floors.name = 'roomFloors';
  labels = new THREE.Group(); labels.name = 'roomLabels';
  for (const r of rooms) {
    const size = r.box.getSize(new THREE.Vector3());
    if (r.storey === 0) {
      const spec = FLOOR[r.label] || ['planks', '#b8884f'];
      const base = spec[0] === 'planks' ? TX.planks(spec[1], 1.2) : TX.tiles(spec[1], spec[2] || '#8f8c85', 1.2);
      const f = new THREE.Mesh(new THREE.PlaneGeometry(size.x - 0.03, size.y - 0.03), PROXY);   // 1.5 cm short of each wall, so neighbouring floors never overlap
      f.position.set((r.box.min.x + r.box.max.x) / 2, (r.box.min.y + r.box.max.y) / 2, r.floorZ + 0.012);
      f.userData.storey = r.storey; f.name = 'floor-' + r.label;
      floors.add(f);
      f.updateMatrixWorld(true);
    }
  }
  app.house.add(floors, labels);
  app.house.updateMatrixWorld(true);
  for (const r of rooms) {
    const size = r.box.getSize(new THREE.Vector3());
    if (r.storey === 0) {
      const f = floors.children.find(x => x.name === 'floor-' + r.label);
      const spec = FLOOR[r.label] || ['planks', '#b8884f'];
      const base = spec[0] === 'planks' ? TX.planks(spec[1], 1.2) : TX.tiles(spec[1], spec[2] || '#8f8c85', 1.2);
      const el = real.element(f, { src: f });
      real.addPart(el, f.geometry, real.localMatrix(f), { map: base, uvScale: [size.x / 1.2, size.y / 1.2], color: ccol('#ffffff'), rough: spec[0] === 'planks' ? 0.5 : 0.35, metal: 0, alpha: 1, side: THREE.FrontSide, cast: false, receive: true, offset: -3 });   // a decal on the slab: it wins
      f.userData.els = [el]; f.userData.descs = { spec: { parts: [{ alpha: 1 }] }, soft: false, node: null };
    }
    labels.add(makeLabel(r));
  }
  floors.visible = mode === 'realistic';
  app.roomFloors = floors; app.roomLabels = labels;
}
function makeLabel(r) {
  const c = document.createElement('canvas'); c.width = 320; c.height = 96;
  const g = c.getContext('2d'); g.font = '600 44px system-ui,sans-serif'; g.textAlign = 'center'; g.textBaseline = 'middle';
  const w = Math.min(310, g.measureText(r.label).width + 36);
  g.fillStyle = 'rgba(12,17,28,.78)'; g.beginPath(); g.roundRect((320 - w) / 2, 14, w, 68, 20); g.fill();
  g.fillStyle = '#e6ecf5'; g.fillText(r.label, 160, 49);
  const t = new THREE.CanvasTexture(c); t.colorSpace = THREE.SRGBColorSpace;
  const s = new THREE.Sprite(new THREE.SpriteMaterial({ map: t, transparent: true, depthWrite: false, fog: false }));
  s.scale.set(1.5, 0.45, 1);
  s.position.set((r.box.min.x + r.box.max.x) / 2, (r.box.min.y + r.box.max.y) / 2, r.floorZ + (r.storey ? 1.9 : 1.5));
  s.userData.room = r.guid; s.renderOrder = 5;
  return s;
}
// ui calls this when classes/storeys/room-name toggles change
export function updateRoomDecor({ slabsOn = true, storeyOn = [], labelsOn = true } = {}) {
  if (floors) floors.children.forEach(f => { f.visible = slabsOn && storeyOn[f.userData.storey] !== false; });
  if (labels) labels.visible = labelsOn && app.mode !== 'walk';
  app.requestRender();
}
