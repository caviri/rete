// Parts of the building-services layer, built from three.js primitives at true size.
//
// Local frame of every fixture: x = out of the surface it is mounted on (the wall normal,
// the node's `dir`), y = along the surface, z = up. Wall plates have their back at x = 0
// (the wall face); bodies with a `size` ([width, depth, height]) are centred on the node's
// point. Ceiling fixtures have the ceiling at z = 0 and hang toward -z. Everything of one
// fixture that is static is merged into one mesh per material, so a fixture costs 1 to 4
// draw calls; levers, lamps and fans are separate small meshes that change with the state.
import * as THREE from 'three';
import { mergeGeometries } from 'three/addons/utils/BufferGeometryUtils.js';
import { app } from './app.js';

// name: [colour, roughness, metalness]
const PAL = {
  plate: ['#efece3', 0.5, 0], dark: ['#23262c', 0.6, 0.1], rubber: ['#17181b', 0.9, 0], metal: ['#a4abb2', 0.35, 0.85],
  steel: ['#cdd2d6', 0.35, 0.6], brass: ['#c9a14a', 0.35, 0.85], white: ['#f3f2ee', 0.45, 0], ceramic: ['#f6f7f8', 0.16, 0],
  cab: ['#e6e4dd', 0.55, 0], grey: ['#8e959e', 0.6, 0.2], graphite: ['#4b5058', 0.5, 0.3], red: ['#c9302c', 0.5, 0],
  yellow: ['#e6b422', 0.5, 0], green: ['#3aa05a', 0.5, 0], blue: ['#2e72c6', 0.5, 0], tank: ['#d3dbe6', 0.45, 0.1],
  plaster: ['#ebe6da', 0.9, 0], wood: ['#b88a58', 0.6, 0], chrome: ['#dfe3e8', 0.12, 1], louvre: ['#b9bec4', 0.5, 0.4],
};
const cache = new Map();
const tmpC = new THREE.Color();
// -> {live, dead}: the normal material and the grey one for things without power or water
export function mat(name, sys) {
  const key = name === 'sys' ? 'sys' + sys : name;
  let p = cache.get(key); if (p) return p;
  let live;
  if (name === 'glass') live = new THREE.MeshStandardMaterial({ color: '#bfe3f2', roughness: 0.05, metalness: 0, transparent: true, opacity: 0.26, depthWrite: false, side: THREE.DoubleSide });
  else {
    const s = name === 'sys' ? [sys || '#8895aa', 0.45, 0.15] : (PAL[name] || PAL.grey);
    live = new THREE.MeshStandardMaterial({ color: s[0], roughness: s[1], metalness: s[2] });
  }
  if (live.metalness > 0.3 && app.envTexture) { live.envMap = app.envTexture; live.envMapIntensity = 0.3; }
  const c = tmpC.set(live.color), l = c.r * 0.3 + c.g * 0.59 + c.b * 0.11;
  const dead = live.clone(); dead.color.setRGB(l * 0.55 + 0.12, l * 0.55 + 0.12, l * 0.55 + 0.13); dead.metalness = Math.min(live.metalness, 0.2); dead.envMap = null;
  p = { live, dead }; cache.set(key, p); return p;
}
// a lamp or LED: its own material, so it can glow on its own
export function lampMat(color = '#fff1d0') {
  const live = new THREE.MeshStandardMaterial({ color: '#f4f0e6', emissive: color, emissiveIntensity: 0, roughness: 0.4 });
  const dead = new THREE.MeshStandardMaterial({ color: '#8c8f94', roughness: 0.8 });
  return { live, dead, glowColor: color };
}
const variants = new Map();   // material -> {hi:Map, fade}
// highlight (emissive tint) and fade (isolate) variants of a material, made once
export function variant(m, kind, hex) {
  let v = variants.get(m); if (!v) { v = { hi: new Map(), fade: null }; variants.set(m, v); }
  if (kind === 'fade') {
    if (!v.fade) { v.fade = m.clone(); v.fade.transparent = true; v.fade.opacity = 0.07; v.fade.depthWrite = false; v.fade.emissive?.setHex(0); v.fade.emissiveIntensity = 0; }
    return v.fade;
  }
  let h = v.hi.get(hex);
  if (!h) { h = m.clone(); h.emissive = new THREE.Color(hex); h.emissiveIntensity = 0.85; h.color.lerp(tmpC.setHex(hex), 0.35); v.hi.set(hex, h); }
  return h;
}

// ---------- the kit: collects geometry by material ----------
class Kit {
  constructor(sys) { this.sys = sys; this.by = new Map(); this.dyn = {}; this.group = new THREE.Group(); }
  put(m, g, x = 0, y = 0, z = 0) { g.translate(x, y, z); if (!this.by.has(m)) this.by.set(m, []); this.by.get(m).push(g); }
  box(m, d, w, h, x = 0, y = 0, z = 0) { this.put(m, new THREE.BoxGeometry(d, w, h), x, y, z); }
  cyl(m, r, len, axis = 'z', x = 0, y = 0, z = 0, r2 = r, seg = 18) {
    const g = new THREE.CylinderGeometry(r2, r, len, seg);
    if (axis === 'z') g.rotateX(Math.PI / 2); else if (axis === 'x') g.rotateZ(Math.PI / 2);
    this.put(m, g, x, y, z);
  }
  sph(m, r, x = 0, y = 0, z = 0, sx = 1, sy = 1, sz = 1) { const g = new THREE.SphereGeometry(r, 14, 10); g.scale(sx, sy, sz); this.put(m, g, x, y, z); }
  torus(m, r, t, axis = 'x', x = 0, y = 0, z = 0) { const g = new THREE.TorusGeometry(r, t, 6, 24); if (axis === 'x') g.rotateY(Math.PI / 2); else if (axis === 'y') g.rotateX(Math.PI / 2); this.put(m, g, x, y, z); }
  // a separate mesh with its own material pair (a lamp, a lever): geometry g at (x,y,z)
  part(name, pair, g, x = 0, y = 0, z = 0) {
    const mesh = new THREE.Mesh(g, pair.live); mesh.position.set(x, y, z); mesh.userData.mats = pair; mesh.userData.zi = -1; this.group.add(mesh); this.dyn[name] = mesh; return mesh;
  }
  finish() {
    let zi = 0;
    for (const [name, gs] of this.by) {
      const geo = gs.length === 1 ? gs[0] : mergeGeometries(gs, false);
      const pair = typeof name === 'string' ? mat(name, this.sys) : name;
      const mesh = new THREE.Mesh(geo, pair.live); mesh.userData.mats = pair; mesh.userData.zi = zi++ % 4; this.group.add(mesh);   // the material groups of one fixture get different polygon offsets (batch.js)
    }
    // keep material names as strings: a key may be 'sys', resolved at put time
    return this.group;
  }
}
// Kit keys are material pairs: wrap helper methods so builders can say 'plate' / 'sys'
function wrap(k) {
  const raw = { box: k.box, cyl: k.cyl, sph: k.sph, torus: k.torus };
  const pair = n => (typeof n === 'string' ? mat(n, k.sys) : n);
  k.box = (m, ...a) => raw.box.call(k, pair(m), ...a);
  k.cyl = (m, ...a) => raw.cyl.call(k, pair(m), ...a);
  k.sph = (m, ...a) => raw.sph.call(k, pair(m), ...a);
  k.torus = (m, ...a) => raw.torus.call(k, pair(m), ...a);
  return k;
}
const FRONT = 0.215;     // modules sit on the front of the distribution board (0.21 m deep)

// ---------- one builder per node type ----------
// D = {w, d, h}: from node.size ([width, depth, height]) or the default. k.dyn holds parts that change.
const plate = (k, w = 0.08, h = 0.08, t = 0.012) => k.box('plate', t, w, h, t / 2, 0, 0);
const BUILD = {
  'grid-connection': (k, n, D) => { k.box('metal', D.d, D.w, D.h); k.box('graphite', 0.012, D.w * 0.88, D.h * 0.88, D.d / 2 + 0.006); k.box('yellow', 0.014, 0.09, 0.05, D.d / 2 + 0.007, 0, D.h * 0.3); k.box('brass', 0.016, 0.03, 0.06, D.d / 2 + 0.008, D.w * 0.3, -0.05); },
  panel: (k, n, D) => { k.box('cab', D.d, D.w, D.h); k.box('grey', 0.008, D.w * 0.94, D.h * 0.95, D.d / 2 + 0.004); k.box('dark', 0.004, D.w * 0.8, D.h * 0.45, D.d / 2 + 0.009, 0, D.h * 0.12); k.box('metal', 0.012, 0.025, 0.07, D.d / 2 + 0.01, D.w * 0.4, -D.h * 0.3); },
  meter: (k) => { k.box('white', 0.06, 0.17, 0.14, FRONT + 0.03); k.box('dark', 0.004, 0.11, 0.035, FRONT + 0.062, 0, 0.03); k.cyl('red', 0.012, 0.004, 'x', FRONT + 0.062, 0, -0.035); },
  breaker: (k, n) => { const main = n.props && n.props.main; const w = main ? 0.08 : 0.055;
    k.box('white', 0.05, w, 0.095, FRONT + 0.025); k.box('sys', 0.004, w * 0.8, 0.01, FRONT + 0.052, 0, 0.032);
    k.part('lever', mat('dark'), new THREE.BoxGeometry(0.012, w * 0.3, 0.026), FRONT + 0.056, 0, 0.0); },
  rcd: (k) => { k.box('white', 0.05, 0.075, 0.095, FRONT + 0.025); k.box('yellow', 0.006, 0.02, 0.014, FRONT + 0.052, 0.02, 0.03);
    k.part('lever', mat('dark'), new THREE.BoxGeometry(0.012, 0.024, 0.026), FRONT + 0.056, -0.015, 0); },
  socket: (k) => { plate(k); k.cyl('white', 0.029, 0.006, 'x', 0.015); k.cyl('dark', 0.004, 0.01, 'x', 0.016, 0.0095, 0); k.cyl('dark', 0.004, 0.01, 'x', 0.016, -0.0095, 0); },
  'socket-appliance': (k) => { plate(k, 0.09, 0.09); k.cyl('white', 0.03, 0.006, 'x', 0.015); k.torus('yellow', 0.031, 0.003, 'x', 0.017); k.cyl('dark', 0.004, 0.01, 'x', 0.016, 0.0095, 0); k.cyl('dark', 0.004, 0.01, 'x', 0.016, -0.0095, 0); },
  switch: (k) => { plate(k); k.box('white', 0.008, 0.05, 0.056, 0.016); },
  dimmer: (k) => { plate(k); k.cyl('white', 0.02, 0.012, 'x', 0.018); },
  thermostat: (k) => { plate(k, 0.09, 0.09); k.box('dark', 0.003, 0.05, 0.03, 0.0135, 0, 0.012); k.cyl('white', 0.014, 0.01, 'x', 0.017, 0, -0.022); },
  doorbell: (k) => { k.box('metal', 0.014, 0.05, 0.1, 0.007); k.cyl('white', 0.013, 0.01, 'x', 0.017, 0, 0.012); k.torus('yellow', 0.014, 0.002, 'x', 0.016, 0, 0.012); },
  'data-outlet': (k) => { plate(k); k.box('dark', 0.004, 0.022, 0.022, 0.014, 0.017, 0); k.box('dark', 0.004, 0.022, 0.022, 0.014, -0.017, 0); k.box('blue', 0.003, 0.05, 0.005, 0.0135, 0, 0.03); },
  'bonding-bar': (k) => { k.box('plaster', 0.01, 0.34, 0.12, 0.005); k.box('brass', 0.012, 0.3, 0.02, 0.016); for (let i = -2; i <= 2; i++) k.cyl('steel', 0.006, 0.02, 'x', 0.026, i * 0.06, 0); },   // the main equipotential bonding bar: a brass bar on a plate
  junction: (k) => { k.box('white', 0.04, 0.1, 0.1, 0.02); k.cyl('dark', 0.009, 0.02, 'z', 0, 0.03, 0.058); k.cyl('dark', 0.009, 0.02, 'z', 0, -0.03, 0.058); },
  'light-ceiling': (k, n) => {
    k.cyl('white', 0.165, 0.03, 'z', 0, 0, -0.015); k.part('lamp', lampMat(), new THREE.CylinderGeometry(0.15, 0.15, 0.02, 24).rotateX(Math.PI / 2), 0, 0, -0.04);
    const drop = n.props && +n.props.dropM;   // a pendant: a thin cord up to the roof underside, with a small canopy
    if (drop > 0.05) { k.cyl('dark', 0.004, drop, 'z', 0, 0, drop / 2); k.cyl('white', 0.05, 0.02, 'z', 0, 0, drop - 0.01); }
  },
  // an installation wall: a plain plaster box, part of the house look
  'pre-wall': (k, n, D) => { k.box('plaster', D.d, D.w, D.h); },
  'light-spot': (k) => { k.cyl('metal', 0.048, 0.03, 'z', 0, 0, -0.015); k.part('lamp', lampMat(), new THREE.CylinderGeometry(0.034, 0.034, 0.01, 16).rotateX(Math.PI / 2), 0, 0, -0.032); },
  'light-wall': (k) => { k.box('plate', 0.012, 0.1, 0.22, 0.006); k.box('white', 0.09, 0.12, 0.18, 0.057); k.part('lamp', lampMat(), new THREE.BoxGeometry(0.08, 0.1, 0.012), 0.057, 0, -0.096); },
  'light-outdoor': (k) => { k.box('graphite', 0.1, 0.18, 0.24, 0.05); k.box('graphite', 0.14, 0.2, 0.02, 0.07, 0, 0.13); k.part('lamp', lampMat('#fff4dc'), new THREE.BoxGeometry(0.008, 0.15, 0.2), 0.101, 0, 0); },
  'smoke-detector': (k) => { k.cyl('white', 0.055, 0.035, 'z', 0, 0, -0.0175); k.cyl('plate', 0.04, 0.008, 'z', 0, 0, -0.039);
    k.part('lamp', lampMat('#ff3b30'), new THREE.SphereGeometry(0.006, 8, 6), 0.03, 0, -0.037); },
  'access-point': (k) => { k.cyl('white', 0.1, 0.03, 'z', 0, 0, -0.015); k.part('lamp', lampMat('#3d8bff'), new THREE.TorusGeometry(0.06, 0.004, 6, 24), 0, 0, -0.032); },
  'vent-supply': (k) => { k.cyl('white', 0.07, 0.014, 'z', 0, 0, -0.007); k.cyl('white', 0.06, 0.012, 'z', 0, 0, -0.02, 0.03); k.torus('sys', 0.066, 0.004, 'z', 0, 0, -0.014); },
  'vent-extract': (k) => { k.cyl('white', 0.07, 0.014, 'z', 0, 0, -0.007); k.cyl('grey', 0.045, 0.012, 'z', 0, 0, -0.02, 0.045); k.torus('sys', 0.066, 0.004, 'z', 0, 0, -0.014); },
  'fibre-entry': (k) => { k.box('white', 0.05, 0.14, 0.1, 0.025); k.box('dark', 0.004, 0.1, 0.03, 0.052, 0, 0.02); k.cyl('yellow', 0.012, 0.03, 'x', 0.06, -0.04, -0.025); k.part('lamp', lampMat('#35e07a'), new THREE.SphereGeometry(0.004, 8, 6), 0.053, 0.04, 0.02); },
  router: (k) => { k.box('white', 0.04, 0.22, 0.14, 0.02); k.box('dark', 0.004, 0.18, 0.012, 0.042, 0, 0.04); k.part('lamp', lampMat('#35e07a'), new THREE.BoxGeometry(0.004, 0.1, 0.006), 0.044, 0, 0.0); },
  'net-switch': (k) => { k.box('graphite', 0.05, 0.3, 0.045, 0.025); k.box('dark', 0.004, 0.24, 0.02, 0.052); k.part('lamp', lampMat('#35e07a'), new THREE.BoxGeometry(0.004, 0.2, 0.006), 0.054, 0, 0.0); },
  'water-entry': (k) => { k.cyl('metal', 0.034, 0.012, 'x', 0.006); k.cyl('sys', 0.0165, 0.14, 'x', 0.07); },
  'water-meter': (k) => { k.cyl('brass', 0.04, 0.1, 'z', 0.07); k.cyl('white', 0.05, 0.03, 'z', 0.07, 0, 0.065); k.cyl('dark', 0.036, 0.004, 'z', 0.07, 0, 0.082); k.cyl('sys', 0.0165, 0.14, 'x', 0.07, 0, 0, 0.0165, 12); },
  valve: (k) => { k.cyl('sys', 0.0165, 0.14, 'x', 0.07); k.sph('brass', 0.032, 0.07); k.cyl('brass', 0.011, 0.07, 'z', 0.07, 0, 0.05);
    k.part('lever', mat('sys', k.sys), new THREE.BoxGeometry(0.1, 0.014, 0.014), 0.07, 0, 0.09); },
  'outdoor-tap': (k) => { k.box('metal', 0.012, 0.07, 0.07, 0.006); k.cyl('brass', 0.011, 0.09, 'x', 0.05, 0, 0.01); k.cyl('brass', 0.007, 0.05, 'z', 0.07, 0, 0.03); k.box('red', 0.012, 0.05, 0.012, 0.07, 0, 0.056); },
  drain: (k, n) => {
    if (n.p[2] < 0.12) { k.cyl('chrome', 0.07, 0.012, 'z', 0, 0, 0.006); k.cyl('dark', 0.05, 0.004, 'z', 0, 0, 0.013); return; }
    k.cyl('sys', 0.028, 0.07, 'x', 0.035); k.cyl('metal', 0.034, 0.012, 'x', 0.006);
  },
  stack: (k) => { k.cyl('sys', 0.055, 0.3, 'z', 0.06, 0, 0.1); k.cyl('sys', 0.055, 0.14, 'x', 0.04, 0, -0.05); k.cyl('metal', 0.07, 0.02, 'z', 0.06, 0, 0.12); },
  'vent-stack': (k) => { k.cyl('sys', 0.055, 0.5, 'z', 0, 0, 0); k.cyl('graphite', 0.07, 0.04, 'z', 0, 0, 0.27, 0.06); },
  'sewer-connection': (k) => { k.cyl('sys', 0.05, 0.4, 'x', 0.2); k.cyl('metal', 0.34, 0.02, 'z', 0.2, 0, 1.0); k.torus('dark', 0.3, 0.012, 'z', 0.2, 0, 1.0); },
  basin: (k, n, D) => { // wall-hung, p is the centre of the bounding box; rim at the top
    k.box('ceramic', D.d, D.w, D.h * 0.7, 0, 0, -D.h * 0.15); k.box('ceramic', D.d * 0.5, D.w * 0.96, D.h * 0.3, -D.d * 0.2, 0, D.h * 0.2);
    k.box('dark', D.d * 0.7, D.w * 0.72, 0.006, 0.02, 0, D.h * 0.2 + 0.004);
    k.cyl('chrome', 0.014, 0.1, 'z', -D.d / 2 + 0.06, 0, D.h * 0.2 + 0.05); k.cyl('chrome', 0.008, 0.07, 'x', -D.d / 2 + 0.1, 0, D.h * 0.2 + 0.095); },
  wc: (k, n, D) => { // wall-hung bowl; the bounding box is 0.2 above the floor
    const z0 = -D.h / 2;
    const g = new THREE.CylinderGeometry(0.17, 0.17, D.h * 0.55, 24); g.rotateX(Math.PI / 2); g.scale(1.45, 1, 1); k.put(mat('ceramic'), g, 0.02, 0, z0 + D.h * 0.275);
    k.box('ceramic', 0.14, 0.34, D.h * 0.55, -D.d / 2 + 0.07, 0, z0 + D.h * 0.275);
    const s = new THREE.CylinderGeometry(0.16, 0.16, 0.025, 24); s.rotateX(Math.PI / 2); s.scale(1.4, 1, 1); k.put(mat('white'), s, 0.02, 0, z0 + D.h * 0.55 + 0.012);
    k.box('white', 0.04, 0.3, 0.34, -D.d / 2 + 0.02, 0, z0 + D.h * 0.55 + 0.17);
    k.box('chrome', 0.01, 0.2, 0.14, -D.d / 2 - 0.005 + 0.0, 0, z0 + D.h + 0.5); },
  shower: (k, n) => { // mixer on the wall, tray on the floor, screen on the open sides
    const zf = -n.p[2];
    k.cyl('chrome', 0.03, 0.02, 'x', 0.01); k.cyl('chrome', 0.012, 0.1, 'x', 0.06); k.box('chrome', 0.03, 0.012, 0.1, 0.09, 0, 0);
    k.cyl('chrome', 0.008, 0.9, 'z', 0.02, 0, 0.45); k.cyl('chrome', 0.1, 0.012, 'z', 0.2, 0, 0.9); k.cyl('chrome', 0.008, 0.18, 'x', 0.11, 0, 0.9);
    k.box('white', 0.9, 0.9, 0.03, 0.45, 0, zf + 0.015); k.cyl('chrome', 0.05, 0.004, 'z', 0.45, 0, zf + 0.031);
    k.box('glass', 0.008, 0.9, 1.95, 0.9, 0, zf + 0.03 + 0.975); k.box('glass', 0.9, 0.008, 1.95, 0.45, -0.45, zf + 0.03 + 0.975);
    k.box('chrome', 0.014, 0.014, 1.95, 0.9, -0.45, zf + 1.005); k.box('chrome', 0.014, 0.9, 0.014, 0.9, 0, zf + 1.98); k.box('chrome', 0.9, 0.014, 0.014, 0.45, -0.45, zf + 1.98); },
  sink: (k, n, D) => { // kitchen sink unit: cabinet, worktop, basin, tap
    k.box('cab', D.d, D.w, D.h - 0.05, 0, 0, -0.025); k.box('wood', D.d + 0.02, D.w, 0.04, 0.01, 0, D.h / 2 - 0.02);
    k.box('steel', D.d * 0.6, D.w * 0.55, 0.004, 0.02, -D.w * 0.1, D.h / 2 + 0.002); k.box('dark', D.d * 0.5, D.w * 0.45, 0.002, 0.02, -D.w * 0.1, D.h / 2 + 0.005);
    k.cyl('chrome', 0.016, 0.26, 'z', -D.d / 2 + 0.08, -D.w * 0.1, D.h / 2 + 0.13); k.cyl('chrome', 0.009, 0.13, 'x', -D.d / 2 + 0.14, -D.w * 0.1, D.h / 2 + 0.25);
    k.box('graphite', 0.01, D.w * 0.9, 0.02, D.d / 2 + 0.005, 0, D.h / 2 - 0.1); },
  dishwasher: (k, n, D) => { k.box('steel', D.d, D.w, D.h); k.box('graphite', 0.012, D.w * 0.92, D.h * 0.1, D.d / 2 + 0.006, 0, D.h / 2 - 0.08); k.box('chrome', 0.03, D.w * 0.7, 0.02, D.d / 2 + 0.02, 0, D.h / 2 - 0.2);
    k.part('lamp', lampMat('#35e07a'), new THREE.BoxGeometry(0.004, 0.03, 0.006), D.d / 2 + 0.013, D.w * 0.35, D.h / 2 - 0.08); },
  'washing-machine': (k, n, D) => { k.box('white', D.d, D.w, D.h); k.cyl('chrome', 0.24, 0.012, 'x', D.d / 2 + 0.006, 0, -0.04); k.cyl('graphite', 0.2, 0.03, 'x', D.d / 2 + 0.015, 0, -0.04);
    k.cyl('glass', 0.18, 0.02, 'x', D.d / 2 + 0.02, 0, -0.04); k.box('graphite', 0.012, D.w * 0.9, 0.09, D.d / 2 + 0.006, 0, D.h / 2 - 0.07); k.cyl('white', 0.03, 0.02, 'x', D.d / 2 + 0.02, -D.w * 0.3, D.h / 2 - 0.07);
    k.part('lamp', lampMat('#35e07a'), new THREE.BoxGeometry(0.004, 0.04, 0.008), D.d / 2 + 0.013, D.w * 0.25, D.h / 2 - 0.07); },
  'cooker-hood': (k, n, D) => { k.box('steel', D.d, D.w, D.h); k.box('graphite', D.d * 0.8, D.w * 0.9, 0.01, 0.03, 0, -D.h / 2 - 0.005); k.box('steel', 0.22, 0.22, 0.62, -D.d / 2 + 0.115, 0, D.h / 2 + 0.31);
    k.part('lamp', lampMat('#fff1d0'), new THREE.BoxGeometry(0.08, 0.5, 0.004), 0.05, 0, -D.h / 2 - 0.012); },
  'towel-radiator': (k, n, D) => { for (const s of [-1, 1]) k.cyl('white', 0.016, D.h, 'z', 0, s * (D.w / 2 - 0.016)); for (let i = 0; i < 9; i++) k.cyl('white', 0.012, D.w - 0.03, 'y', 0, 0, -D.h / 2 + 0.06 + i * (D.h - 0.12) / 8); k.box('sys', 0.02, 0.03, 0.03, 0, D.w / 2 - 0.016, -D.h / 2 + 0.04); },
  manifold: (k, n, D) => { k.box('cab', D.d, D.w, D.h); k.box('graphite', 0.004, D.w * 0.94, D.h * 0.9, D.d / 2 + 0.002);
    for (const z of [0.1, -0.08]) { k.cyl('brass', 0.016, D.w * 0.84, 'y', D.d / 2 + 0.03, 0, z); for (let i = 0; i < 9; i++) k.cyl(z > 0 ? 'red' : 'blue', 0.01, 0.06, 'x', D.d / 2 + 0.03, -D.w * 0.38 + i * D.w * 0.095, z + (z > 0 ? 0.04 : -0.04)); } },
  'heat-pump-outdoor': (k, n, D) => {
    k.box('cab', D.d, D.w, D.h); k.box('graphite', D.d + 0.01, D.w + 0.01, 0.025, 0, 0, D.h / 2 - 0.0125); k.box('dark', 0.05, D.w * 0.9, 0.04, 0, 0, -D.h / 2 - 0.02);
    const fx = D.d / 2 + 0.002, r = Math.min(D.w, D.h) * 0.4; k.cyl('dark', r + 0.03, 0.01, 'x', fx, 0, 0);
    for (const f of [0.35, 0.65, 1]) k.torus('metal', r * f, 0.004, 'x', fx + 0.006, 0, 0);
    for (let a = 0; a < 6; a++) { const g = new THREE.BoxGeometry(0.004, 0.004, r * 2); g.rotateX(a * Math.PI / 6); k.put(mat('metal'), g, fx + 0.006); }
    for (let i = 0; i < 8; i++) k.box('metal', 0.004, 0.012, D.h * 0.82, -D.d / 2 - 0.002, -D.w / 2 + 0.06 + i * (D.w - 0.12) / 7, 0);
    const fan = k.part('fan', mat('graphite'), mergeGeometries([0, 1, 2, 3, 4].map(i => { const g = new THREE.BoxGeometry(0.004, r * 0.45, r * 0.9); g.translate(0, r * 0.25, 0); g.rotateX(i * Math.PI * 2 / 5); return g; })), fx - 0.015, 0, 0); fan.userData.spin = 'x'; },
  'heat-pump-indoor': (k, n, D) => { k.box('white', D.d, D.w, D.h); k.box('graphite', 0.01, D.w * 0.5, 0.2, D.d / 2 + 0.005, 0, D.h * 0.3); k.box('dark', 0.004, 0.16, 0.05, D.d / 2 + 0.012, 0, D.h * 0.3 + 0.03);
    k.cyl('red', 0.014, 0.12, 'z', -D.d * 0.2, D.w * 0.25, D.h / 2 + 0.06); k.cyl('blue', 0.014, 0.12, 'z', -D.d * 0.2, D.w * 0.1, D.h / 2 + 0.06); },
  'buffer-tank': (k, n, D) => { k.cyl('tank', Math.min(D.w, D.d) / 2, D.h, 'z'); k.cyl('tank', Math.min(D.w, D.d) / 2, 0.06, 'z', 0, 0, D.h / 2 + 0.02, 0.18); k.cyl('red', 0.014, 0.12, 'x', Math.min(D.w, D.d) / 2, 0, D.h * 0.3); k.cyl('blue', 0.014, 0.12, 'x', Math.min(D.w, D.d) / 2, 0, -D.h * 0.3); k.cyl('graphite', 0.26, 0.04, 'z', 0, 0, -D.h / 2 + 0.02); },
  'water-heater': (k, n, D) => { const r = Math.min(D.w, D.d) / 2; k.cyl('white', r, D.h, 'z'); k.cyl('white', r, 0.08, 'z', 0, 0, D.h / 2 + 0.03, r * 0.6); k.cyl('blue', 0.014, 0.1, 'z', r * 0.4, 0, D.h / 2 + 0.1); k.cyl('red', 0.014, 0.1, 'z', -r * 0.4, 0, D.h / 2 + 0.1); k.cyl('graphite', r + 0.01, 0.05, 'z', 0, 0, -D.h / 2 + 0.025); k.box('dark', 0.01, 0.1, 0.05, r, 0, D.h * 0.25); },
  'hrv-unit': (k, n, D) => { k.box('cab', D.d, D.w, D.h); k.box('grey', 0.01, D.w * 0.9, D.h * 0.6, D.d / 2 + 0.005, 0, -0.04); k.box('dark', 0.004, 0.12, 0.04, D.d / 2 + 0.012, 0, D.h * 0.3);
    for (const [y, x] of [[-0.28, -0.15], [-0.1, 0.15], [0.1, -0.15], [0.28, 0.15]]) k.cyl('metal', 0.075, 0.07, 'z', x, y, D.h / 2 + 0.035); k.part('lamp', lampMat('#35e07a'), new THREE.SphereGeometry(0.006, 8, 6), D.d / 2 + 0.01, D.w * 0.4, D.h * 0.3); },
  'air-intake': (k) => { louvre(k, 0.3, 0.2); }, 'air-exhaust': (k) => { louvre(k, 0.3, 0.2); },
  'ac-indoor': (k, n, D) => { k.box('white', D.d, D.w, D.h); k.box('white', D.d, D.w, 0.004, 0, 0, D.h / 2 + 0.0); k.box('graphite', 0.02, D.w * 0.86, 0.04, D.d / 2 - 0.01, 0, -D.h / 2 + 0.04); k.box('dark', 0.004, 0.06, 0.016, D.d / 2 + 0.002, D.w * 0.36, D.h * 0.1);
    k.part('lamp', lampMat('#35e07a'), new THREE.SphereGeometry(0.005, 8, 6), D.d / 2 + 0.004, D.w * 0.4, D.h * 0.1); },
  'ac-outdoor': (k, n, D) => { k.box('cab', D.d, D.w, D.h); k.box('graphite', D.d + 0.01, D.w + 0.01, 0.02, 0, 0, D.h / 2 - 0.01); const fx = D.d / 2 + 0.002, r = Math.min(D.w, D.h) * 0.36;
    k.cyl('dark', r + 0.03, 0.01, 'x', fx, 0, 0); for (const f of [0.4, 0.7, 1]) k.torus('metal', r * f, 0.004, 'x', fx + 0.006, 0, 0);
    for (let a = 0; a < 4; a++) { const g = new THREE.BoxGeometry(0.004, 0.004, r * 2); g.rotateX(a * Math.PI / 4); k.put(mat('metal'), g, fx + 0.006); }
    k.box('graphite', 0.1, 0.05, D.h * 0.9, 0, -D.w / 2 - 0.0, 0); for (const y of [-0.38, 0.38]) k.box('dark', 0.45, 0.06, 0.04, 0, y, -D.h / 2 - 0.02); },
};
function louvre(k, w, h) { k.box('graphite', 0.02, w, h, 0.01); for (let i = 0; i < 6; i++) k.box('louvre', 0.03, w * 0.88, 0.012, 0.025, 0, -h / 2 + 0.03 + i * (h - 0.06) / 5); }
const DEFAULTS = { 'grid-connection': [0.35, 0.2, 0.5], panel: [0.9, 0.21, 1.4], basin: [0.6, 0.45, 0.2], wc: [0.36, 0.54, 0.4], sink: [0.8, 0.6, 0.9], dishwasher: [0.6, 0.6, 0.85], 'washing-machine': [0.6, 0.6, 0.85], 'cooker-hood': [0.6, 0.45, 0.1],
  'towel-radiator': [0.5, 0.1, 1.2], manifold: [0.6, 0.15, 0.5], 'heat-pump-outdoor': [1, 0.45, 0.8], 'heat-pump-indoor': [0.75, 0.5, 1.8], 'buffer-tank': [0.5, 0.5, 1], 'water-heater': [0.6, 0.6, 1.4], 'hrv-unit': [0.8, 0.55, 0.5], 'ac-indoor': [0.9, 0.2, 0.3], 'ac-outdoor': [0.9, 0.35, 0.7] };

// types that are drawn as a body around their point (so the point is a centre, not a wall contact)
export const BODY = new Set(Object.keys(DEFAULTS).filter(t => t !== 'grid-connection' && t !== 'panel'));
BODY.add('grid-connection'); BODY.add('panel');
// types that hang from the ceiling (no dir)
export const CEILING = new Set(['light-ceiling', 'light-spot', 'smoke-detector', 'access-point', 'vent-supply', 'vent-extract', 'co-detector']);
// room furniture: visible while walking whenever their layer is on
export const FURNITURE = new Set(['socket', 'socket-appliance', 'switch', 'dimmer', 'thermostat', 'light-ceiling', 'light-spot', 'light-wall', 'sink', 'dishwasher', 'washing-machine', 'wc', 'basin', 'shower', 'cooker-hood', 'towel-radiator', 'smoke-detector', 'data-outlet', 'doorbell']);

// Build one node. Returns {group, dyn, meshes, extent}: group is in the local frame, positioned later.
const warned = new Set();
function warnOnce(type, e) { if (warned.has(type)) return; warned.add(type); console.warn('[building] no drawing for the node type "' + type + '"' + (e ? ': ' + e.message : '') + '; drawn as a small box'); }
export function buildNode(node, sysColor) {
  const k = wrap(new Kit(sysColor));
  const sz = node.size || DEFAULTS[node.type];
  const D = sz ? { w: sz[0], d: sz[1], h: sz[2] } : { w: 0.1, d: 0.1, h: 0.1 };
  const fn = BUILD[node.type];
  let kit = k, unknown = false;
  if (fn) { try { fn(k, node, D); } catch (e) { warnOnce(node.type, e); kit = wrap(new Kit(sysColor)); kit.box('sys', 0.06, 0.1, 0.1, 0.03); unknown = true; } }
  else { warnOnce(node.type); unknown = true; k.box('sys', 0.06, 0.1, 0.1, 0.03); }   // a type the kit does not know: a small box
  const g = kit.finish();
  k.dyn = kit.dyn;
  const meshes = []; g.traverse(o => { if (o.isMesh) meshes.push(o); });
  const box = new THREE.Box3().setFromObject(g); const sizeV = box.getSize(new THREE.Vector3());
  // a larger invisible box so thin plates are easy to click and to aim at
  const m = new THREE.Vector3(Math.max(0.12, sizeV.x), Math.max(0.14, sizeV.y), Math.max(0.14, sizeV.z));
  const hit = new THREE.Mesh(new THREE.BoxGeometry(m.x, m.y, m.z), new THREE.MeshBasicMaterial({ visible: false }));
  hit.position.copy(box.getCenter(new THREE.Vector3())); hit.name = 'hit'; g.add(hit);
  return { group: g, dyn: k.dyn, meshes, extent: Math.max(sizeV.x, sizeV.y, sizeV.z), D, unknown };
}

// ---------- tubes along a polyline ----------
const UP = new THREE.Vector3(0, 1, 0);
function segGeo(a, b, r, seg) {
  const len = a.distanceTo(b); if (len < 1e-4) return null;
  const g = new THREE.CylinderGeometry(r, r, len, seg, 1, false);
  const q = new THREE.Quaternion().setFromUnitVectors(UP, b.clone().sub(a).normalize());
  g.applyQuaternion(q); const m = a.clone().add(b).multiplyScalar(0.5); g.translate(m.x, m.y, m.z);
  return g;
}
export function tubeGeometry(pts, r, seg = 8) {
  const gs = [];
  for (let i = 0; i + 1 < pts.length; i++) { const g = segGeo(pts[i], pts[i + 1], r, seg); if (g) gs.push(g); }
  for (let i = 1; i + 1 < pts.length; i++) { const s = new THREE.SphereGeometry(r, seg, 4); s.translate(pts[i].x, pts[i].y, pts[i].z); gs.push(s); }
  return gs.length ? mergeGeometries(gs, false) : null;
}
// radius drawn for a run: cables are exaggerated so they read from a distance
export function runRadius(run) { return run.kind === 'cable' ? Math.max(run.d / 2 * 1.25, 0.0075) : Math.max(run.d / 2, 0.006); }

// serpentine of an underfloor loop inside its rectangle (size [width, depth, h], depth along dir)
export function loopPath(node) {
  const [w, d] = node.size, s = (node.props && node.props.spacingM) || 0.15;
  const dir = node.dir ? new THREE.Vector2(node.dir[0], node.dir[1]).normalize() : new THREE.Vector2(1, 0), lat = new THREE.Vector2(-dir.y, dir.x);
  const z = node.p[2], c = new THREE.Vector2(node.p[0], node.p[1]);
  const pass = Math.max(2, Math.floor((w - 0.1) / s) + 1), pts = [];
  const at = (u, v) => { const p = c.clone().addScaledVector(dir, u).addScaledVector(lat, v); return new THREE.Vector3(p.x, p.y, z); };
  const margin = 0.12, U = d / 2 - margin;
  for (let i = 0; i < pass; i++) {
    const v = -w / 2 + margin + i * ((w - 2 * margin) / (pass - 1)), fwd = i % 2 === 0, vn = v + (w - 2 * margin) / (pass - 1);
    pts.push(at(fwd ? -U : U, v), at(fwd ? U : -U, v));
    if (i < pass - 1) { // a half circle to the next pass
      const R = (vn - v) / 2, sgn = fwd ? 1 : -1;
      for (let a = 1; a < 5; a++) { const t = a / 5 * Math.PI; pts.push(at(sgn * (U + Math.sin(t) * R), v + R - Math.cos(t) * R)); }
    }
  }
  return pts;
}
