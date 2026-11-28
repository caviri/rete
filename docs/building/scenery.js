// The world around the house: deliberately stylised and cheap, so the realistic house sits in a low-fi landscape.
//
//   sky dome   one shader: gradient that follows the sun, warm horizon at sunrise and sunset, a sun disc with glow,
//              a moon, stars, drifting clouds from layered noise. Ordered (Bayer 8x8) dither on the colours, anchored to
//              screen pixels so it does not crawl when the camera moves.
//   ground     a large disc: grass patches in a few dithered tones, a stone path to the front door, a paved strip
//              along the walls, fading into the horizon haze (the scene fog).
//   nature     trees (two kinds), bushes, rocks, grass tufts, flowers and a fence: InstancedMesh, one or two materials,
//              placed from a fixed seed; the house, the way to the front door and the view from the doors stay clear.
//   animation  trees and tufts sway in the vertex shader, clouds drift, birds circle far away, fireflies at night.
//              No CPU work per object. The animation asks for frames at a capped rate (see LEVELS in scene.js).
//
// Budget: 11 draw calls (dome, ground, 2 trees, bushes, rocks, tufts, flowers, fence, birds, fireflies).
// Not pickable, not solid (the walker stays inside the plot, see walk.js), not in the thermal ray casts unless
// "trees shade the house" is on. Hidden in x-ray and isolate views.
import * as THREE from 'three';
import { mergeGeometries } from 'three/addons/utils/BufferGeometryUtils.js';
import { app } from './app.js';

const U = {
  uTime: { value: 0 }, uWind: { value: 0.4 }, uDither: { value: 0.55 }, uCell: { value: 1 }, uCloud: { value: 0 }, uNight: { value: 0 },
  uPath: { value: [new THREE.Vector4(), new THREE.Vector4(), new THREE.Vector4()] }, uHouse: { value: new THREE.Vector4(0, 0, 12, 10) }, uPR: { value: 1 },
};
const state = { on: true, dither: 0.55, wind: 0.4, animate: true, treesShade: false };
let group, nature, dome, groundMesh, fireflies, birds, built = false;
let animOff = null, onScreen = true, reduced = false, T = 0, lastT = 0, cloudT = 0, level = 'high';
const inst = {};                       // name -> InstancedMesh
let nearCounts = { treeA: 0, treeB: 0 }, totals = { treeA: 0, treeB: 0, bush: 0, rock: 0, tuft: 0, flower: 0, fence: 0 };
let shadeProxy = null;

const cl = (v, a, b) => Math.min(b, Math.max(a, v));
const smooth = (a, b, x) => { const t = cl((x - a) / (b - a), 0, 1); return t * t * (3 - 2 * t); };
function rng(seed) { let a = seed >>> 0; return () => { a = (a + 0x6D2B79F5) >>> 0; let t = Math.imul(a ^ (a >>> 15), 1 | a); t = (t + Math.imul(t ^ (t >>> 7), 61 | t)) ^ t; return ((t ^ (t >>> 14)) >>> 0) / 4294967296; }; }

// ---------- GLSL shared by everything here ----------
const GLSL_COMMON = `
uniform float uTime, uWind, uDither, uCell, uCloud, uNight;
float bayer8(){
  ivec2 q = ivec2(mod(floor(gl_FragCoord.xy / max(uCell, 1.)), 8.));
  int x = q.x ^ q.y, y = q.y;
  int v = ((y & 1) << 5) | ((x & 1) << 4) | ((y & 2) << 2) | ((x & 2) << 1) | ((y & 4) >> 1) | ((x & 4) >> 2);
  return (float(v) + 0.5) / 64.;
}
// a few levels per channel with an ordered dither; uDither 0 leaves the colour alone
vec3 ditherQ(vec3 c){
  if (uDither < 0.004) return c;
  float L = mix(96., 6., pow(uDither, 0.65));
  return clamp(floor(c * (L - 1.) + bayer8()) / (L - 1.), 0., 1.);
}
float h21(vec2 p){ p = fract(p * vec2(123.34, 456.21)); p += dot(p, p + 45.32); return fract(p.x * p.y); }
float vn(vec2 p){ vec2 i = floor(p), f = fract(p); f = f * f * (3. - 2. * f);
  return mix(mix(h21(i), h21(i + vec2(1, 0)), f.x), mix(h21(i + vec2(0, 1)), h21(i + vec2(1, 1)), f.x), f.y); }
float fbm(vec2 p){ float a = .5, s = 0.; for (int i = 0; i < 3; i++) { s += a * vn(p); p = p * 2.03 + vec2(17., 3.); a *= .5; } return s; }
`;

// ---------- the sky ----------
function skyMaterial() {
  return new THREE.ShaderMaterial({
    side: THREE.BackSide, depthWrite: false, depthTest: false, fog: false, toneMapped: false,
    uniforms: { ...U, uTop: { value: new THREE.Color('#3b7ad4') }, uHor: { value: new THREE.Color('#b8d7f2') }, uSunDir: { value: new THREE.Vector3(0, 0, 1) },
      uSunCol: { value: new THREE.Color('#fff2d8') }, uDay: { value: 1 }, uCover: { value: 0.46 } },
    vertexShader: 'varying vec3 vD; void main(){ vD = position; vec4 p = projectionMatrix * modelViewMatrix * vec4(position, 1.); gl_Position = p.xyww; }',
    fragmentShader: `#include <common>
      varying vec3 vD; uniform vec3 uTop, uHor, uSunDir, uSunCol; uniform float uDay, uCover;
      ${GLSL_COMMON}
      float h31(vec3 p){ return fract(sin(dot(p, vec3(12.9898, 78.233, 37.719))) * 43758.5453); }
      void main(){
        vec3 d = normalize(vD); float h = d.z;
        vec3 c = mix(uHor, uTop, pow(clamp(h, 0., 1.), .5));
        if (h < 0.) c = mix(uHor, uHor * .6, smoothstep(0., .3, -h));
        // the warm band along the horizon on the side of the sun, strongest when the sun is low
        vec2 sxy = normalize(uSunDir.xy + 1e-4);
        float az = max(dot(normalize(d.xy + 1e-4), sxy), 0.);
        float low = smoothstep(.5, 0., abs(uSunDir.z));
        float band = exp(-abs(h) * 5.) * pow(az, 3.) * low;
        c += uSunCol * vec3(1., .5, .28) * band * .6;
        // sun disc and glow
        float s = dot(d, uSunDir), above = step(-.02, h);
        c += uSunCol * (smoothstep(.9988, .9992, s) * 2.4 + pow(max(s, 0.), 40.) * .55 + pow(max(s, 0.), 6.) * .12 * (.4 + .6 * uDay)) * above;
        // moon and stars (the moon stands opposite the sun)
        float night = 1. - uDay;
        vec3 md = vec3(-uSunDir.xy, -uSunDir.z);
        float m = dot(d, md);
        c += vec3(.86, .9, 1.) * (smoothstep(.99905, .99925, m) * .95 + pow(max(m, 0.), 120.) * .2) * night * above;
        if (night > .02 && h > -.02) {
          vec3 q = d * 80.; vec3 id = floor(q); vec3 f = fract(q) - .5; float r = h31(id);
          float star = step(.982, r) * smoothstep(.26, .02, length(f)) * (.55 + .45 * sin(uTime * (1. + r * 4.) + r * 60.));
          c += vec3(.8, .85, 1.) * star * night * night * smoothstep(-.02, .22, h);
        }
        // clouds: layered noise on a plane overhead
        if (h > .0) {
          vec2 uv = d.xy / (h + .16) * .5 + vec2(uCloud, uCloud * .31);
          float n = fbm(uv * 1.3), n2 = fbm(uv * 1.3 + sxy * .07);
          float cov = smoothstep(1. - uCover - .16, 1. - uCover + .14, n) * smoothstep(.0, .2, h);
          float lit = clamp(.55 + (n - n2) * 5., 0., 1.);
          float lum = .06 + .94 * uDay;
          vec3 cc = mix(vec3(.58, .63, .74), vec3(1., .98, .94), lit) * lum + uSunCol * vec3(1., .55, .35) * band * .5 * lit;
          c = mix(c, cc, cov * .88);
        }
        gl_FragColor = vec4(c, 1.);
        #include <colorspace_fragment>
        gl_FragColor.rgb = ditherQ(gl_FragColor.rgb);
      }`,
  });
}

// ---------- lit scenery materials: sway, ground patches, dither ----------
function litMaterial({ sway = 0, ground = false, side = THREE.FrontSide, flat = true, colors = true }) {
  const m = new THREE.MeshStandardMaterial({ vertexColors: colors, flatShading: flat, roughness: 1, metalness: 0, side });
  m.onBeforeCompile = s => {
    for (const k of ['uTime', 'uWind', 'uDither', 'uCell', 'uNight']) s.uniforms[k] = U[k];
    if (ground) { s.uniforms.uPath = U.uPath; s.uniforms.uHouse = U.uHouse; }
    s.vertexShader = s.vertexShader
      .replace('#include <common>', '#include <common>\nuniform float uTime, uWind; varying vec3 vWp;')
      .replace('#include <begin_vertex>', `#include <begin_vertex>
        ${sway ? `{ float hh = max(position.z, 0.);
          #ifdef USE_INSTANCING
            vec2 io = instanceMatrix[3].xy;
          #else
            vec2 io = vec2(0.);
          #endif
          float ph = io.x * .37 + io.y * .53;
          float sw = sin(uTime * 1.6 + ph) * .5 + sin(uTime * 2.7 + ph * 1.7) * .5;
          float amp = uWind * ${sway.toFixed(3)} * hh * hh;
          transformed.x += sw * amp; transformed.y += cos(uTime * 1.3 + ph) * amp * .6; }` : ''}
        #ifdef USE_INSTANCING
          vWp = (modelMatrix * instanceMatrix * vec4(transformed, 1.)).xyz;
        #else
          vWp = (modelMatrix * vec4(transformed, 1.)).xyz;
        #endif`);
    let fs = s.fragmentShader
      .replace('#include <common>', `#include <common>\nvarying vec3 vWp;\n${GLSL_COMMON}${ground ? 'uniform vec4 uPath[3]; uniform vec4 uHouse;' : ''}`)
      .replace('#include <dithering_fragment>', 'gl_FragColor.rgb = ditherQ(gl_FragColor.rgb);');
    if (ground) fs = fs.replace('#include <color_fragment>', `
      { vec2 p = vWp.xy;
        float dc = length(p - cameraPosition.xy);
        float t = fbm(p * .045) * .62 + fbm(p * .33 + 7.) * .26 + vn(p * 1.7) * .12;
        t = mix(t, .5, smoothstep(50., 180., dc) * .75);
        float sc = clamp(t * 1.15 - .08, 0., .999) * 3.;
        float lo = floor(sc), f = sc - lo;
        float pick = mix(f, step(bayer8(), f), uDither);
        float k = lo + pick;
        vec3 c0 = vec3(.10, .21, .06), c1 = vec3(.17, .32, .09), c2 = vec3(.27, .43, .12), c3 = vec3(.42, .50, .17);
        vec3 g = k < 1. ? mix(c0, c1, k) : k < 2. ? mix(c1, c2, k - 1.) : mix(c2, c3, k - 2.);
        // dry earth patches
        float dry = smoothstep(.66, .8, vn(p * .09 + 31.) * .6 + vn(p * .3 + 5.) * .4);
        g = mix(g, vec3(.34, .27, .15), dry * .55);
        // the path to the front door: three segments, a stone colour with lighter joints
        float pd = 1e3;
        for (int i = 0; i < 3; i++) {
          vec4 sg = uPath[i]; if (sg.z == sg.x && sg.w == sg.y) continue;
          vec2 a = sg.xy, b = sg.zw, ab = b - a;
          float tt = clamp(dot(p - a, ab) / max(dot(ab, ab), 1e-4), 0., 1.);
          pd = min(pd, length(p - a - ab * tt));
        }
        float onPath = smoothstep(.95, .75, pd + .06 * (vn(p * 3.) - .5));
        float stones = step(.12, vn(p * 5.5)) * .12;
        g = mix(g, vec3(.52, .48, .42) * (.88 + stones) , onPath);
        diffuseColor.rgb *= g;
      }`);
    s.fragmentShader = fs;
  };
  m.customProgramCacheKey = () => 'scenery-' + (sway ? 'sway' + sway : 'still') + (ground ? '-ground' : '') + (side === THREE.DoubleSide ? '-ds' : '');
  return m;
}

// ---------- geometry ----------
function paint(g, hex, jit = 0, r) {
  const n = g.attributes.position.count, a = new Float32Array(n * 3), c = new THREE.Color(hex);
  for (let i = 0; i < n; i++) { const k = 1 + (jit ? (r() - .5) * jit : 0); a[i * 3] = c.r * k; a[i * 3 + 1] = c.g * k; a[i * 3 + 2] = c.b * k; }
  g.setAttribute('color', new THREE.BufferAttribute(a, 3)); return g;
}
function plain(g) { g = g.index ? g.toNonIndexed() : g; g.deleteAttribute('uv'); return g; }
function blob(r, detail, x, y, z, sx, sy, sz, hex, rand) {
  let g = new THREE.IcosahedronGeometry(r, detail); g = plain(g);
  const P = g.attributes.position, v = new THREE.Vector3(), memo = new Map();
  for (let i = 0; i < P.count; i++) {   // the same corner moves the same way in every triangle
    v.fromBufferAttribute(P, i); const k = v.x.toFixed(3) + v.y.toFixed(3) + v.z.toFixed(3);
    let j = memo.get(k); if (j === undefined) { j = 0.86 + rand() * 0.28; memo.set(k, j); }
    P.setXYZ(i, v.x * j * sx, v.y * j * sy, v.z * j * sz + z); P.setX(i, P.getX(i) + x); P.setY(i, P.getY(i) + y);
  }
  g.computeVertexNormals(); return paint(g, hex, 0.12, rand);
}
function cyl(r0, r1, h, seg, z, hex, rand) { const g = plain(new THREE.CylinderGeometry(r1, r0, h, seg, 1)); g.rotateX(Math.PI / 2); g.translate(0, 0, z + h / 2); return paint(g, hex, 0.1, rand); }
function cone(r, h, seg, z, hex, rand) { const g = plain(new THREE.ConeGeometry(r, h, seg, 1)); g.rotateX(Math.PI / 2); g.translate(0, 0, z + h / 2); return paint(g, hex, 0.12, rand); }
function box(x, y, z, w, d, h, hex, rand) { const g = plain(new THREE.BoxGeometry(w, d, h)); g.translate(x, y, z + h / 2); return paint(g, hex, 0.08, rand); }

function buildGeometries(R) {
  const treeA = mergeGeometries([cyl(0.16, 0.1, 1.9, 6, 0, '#6b4a2e', R),
    blob(1.35, 1, 0, 0, 2.7, 1, 1, 0.9, '#4f9140', R), blob(1.0, 1, 0.55, 0.2, 3.5, 1, 1, 0.9, '#5ea348', R), blob(0.95, 1, -0.5, -0.25, 3.2, 1, 1, 0.9, '#448638', R)]);
  const treeB = mergeGeometries([cyl(0.14, 0.09, 1.1, 6, 0, '#5d4029', R), cone(1.45, 2.0, 8, 0.9, '#2f6b3c', R), cone(1.15, 1.8, 8, 2.2, '#397a45', R), cone(0.8, 1.6, 8, 3.5, '#438a4f', R)]);
  const bush = mergeGeometries([blob(0.6, 1, 0, 0, 0.42, 1.2, 1, 0.85, '#4b8a3a', R), blob(0.42, 1, 0.5, 0.1, 0.34, 1, 1, 0.8, '#5a9a44', R), blob(0.4, 1, -0.45, -0.1, 0.32, 1, 1, 0.8, '#3f7a33', R)]);
  const rock = mergeGeometries([blob(0.5, 0, 0, 0, 0.18, 1.3, 1, 0.65, '#8b8a85', R), blob(0.28, 0, 0.45, 0.2, 0.1, 1.2, 1, 0.7, '#77766f', R)]);
  // a tuft: five thin blades, each a tilted triangle
  const tuft = (() => {
    const pos = [], col = [], c0 = new THREE.Color('#2f5f1c'), c1 = new THREE.Color('#9bc255');
    for (let b = 0; b < 5; b++) {
      const a = b / 5 * Math.PI * 2 + R() * 0.6, tilt = 0.18 + R() * 0.22, h = 0.28 + R() * 0.22, w = 0.028;
      const dx = Math.cos(a), dy = Math.sin(a), px = -dy, py = dx, ox = dx * 0.05, oy = dy * 0.05;
      pos.push(ox - px * w, oy - py * w, 0, ox + px * w, oy + py * w, 0, ox + dx * tilt * h, oy + dy * tilt * h, h);
      col.push(c0.r, c0.g, c0.b, c0.r, c0.g, c0.b, c1.r, c1.g, c1.b);
    }
    const g = new THREE.BufferGeometry(); g.setAttribute('position', new THREE.Float32BufferAttribute(pos, 3)); g.setAttribute('color', new THREE.Float32BufferAttribute(col, 3)); g.computeVertexNormals(); return g;
  })();
  const flower = (() => {
    const stem = cyl(0.01, 0.01, 0.22, 4, 0, '#3f7a2a', R), head = blob(0.045, 0, 0, 0, 0.25, 1, 1, 0.6, '#ffffff', R);
    return mergeGeometries([stem, head]);
  })();
  // one fence section, 1.8 m: two posts and two rails
  const fence = mergeGeometries([box(-0.88, 0, 0, 0.08, 0.08, 0.95, '#8a6a46', R), box(0.88, 0, 0, 0.08, 0.08, 0.95, '#8a6a46', R), box(0, 0, 0.28, 1.8, 0.04, 0.07, '#9a7a52', R), box(0, 0, 0.66, 1.8, 0.04, 0.07, '#9a7a52', R)]);
  return { treeA, treeB, bush, rock, tuft, flower, fence };
}

// ---------- where things go ----------
function layout(R) {
  const hb = app.houseBox, plot = app.plot || { x0: hb.min.x - 14, x1: hb.max.x + 14, y0: hb.min.y - 12, y1: hb.max.y + 12 };
  const cx = (hb.min.x + hb.max.x) / 2, cy = (hb.min.y + hb.max.y) / 2;
  // exterior doors: where the walk comes in, and the views from them
  const doors = [];
  const ops = app.systems && Array.isArray(app.systems.openings) ? app.systems.openings.filter(o => o.kind === 'door' && o.exterior) : [];
  const fronts = ops.length ? ops.map(o => ({ x: o.hinge[0], y: o.hinge[1], label: o.label })) : app.graph.nodes.filter(n => n.c === 'IfcDoor' && /Haus/i.test(n.l)).map(n => { const b = new THREE.Box3().setFromObject(app.meshByGuid.get(n.n)).getCenter(new THREE.Vector3()); return { x: b.x, y: b.y, label: n.l }; });
  for (const f of fronts) {
    const dx = f.x - cx, dy = f.y - cy; let ox = 0, oy = 0;
    if (Math.abs(dx) / (hb.max.x - hb.min.x) > Math.abs(dy) / (hb.max.y - hb.min.y)) ox = Math.sign(dx) || 1; else oy = Math.sign(dy) || 1;
    doors.push({ x: f.x, y: f.y, ox, oy, front: /Haus/i.test(f.label || '') });
  }
  if (!doors.length) doors.push({ x: hb.min.x, y: cy, ox: -1, oy: 0, front: true });
  const front = doors.find(d => d.front) || doors[0];
  // the path: from the door out, a gentle bend, to the plot edge
  const edgeX = front.ox < 0 ? plot.x0 : front.ox > 0 ? plot.x1 : front.x, edgeY = front.oy < 0 ? plot.y0 : front.oy > 0 ? plot.y1 : front.y;
  const p0 = new THREE.Vector2(front.x + front.ox * 0.3, front.y + front.oy * 0.3), p1 = new THREE.Vector2(front.x + front.ox * 7, front.y + front.oy * 7), p2 = new THREE.Vector2(edgeX, edgeY);
  p1.x += front.oy * 1.8; p1.y += front.ox * -1.8; p2.x += front.oy * 3; p2.y += front.ox * -3;
  U.uPath.value[0].set(p0.x, p0.y, p1.x, p1.y); U.uPath.value[1].set(p1.x, p1.y, p2.x, p2.y); U.uPath.value[2].set(0, 0, 0, 0);
  U.uHouse.value.set(hb.min.x, hb.min.y, hb.max.x - hb.min.x, hb.max.y - hb.min.y);
  const segs = [[p0, p1], [p1, p2]];
  const dseg = (p, a, b) => { const ab = b.clone().sub(a), t = cl(p.clone().sub(a).dot(ab) / ab.lengthSq(), 0, 1); return p.distanceTo(a.clone().addScaledVector(ab, t)); };
  const houseDist = (x, y) => { const qx = Math.max(hb.min.x - x, 0, x - hb.max.x), qy = Math.max(hb.min.y - y, 0, y - hb.max.y); return Math.hypot(qx, qy); };
  const v = new THREE.Vector2();
  const nearPath = (x, y, w) => { v.set(x, y); return segs.some(s => dseg(v, s[0], s[1]) < w); };
  const inCorridor = (x, y, w, len) => doors.some(d => { const ax = (x - d.x) * d.ox + (y - d.y) * d.oy, lat = Math.abs((x - d.x) * d.oy - (y - d.y) * d.ox); return ax > -1 && ax < len && lat < w; });
  // the view south from the living side: a wide sector in front of the south face
  const inView = (x, y) => y < hb.min.y && y > hb.min.y - 13 && Math.abs(x - cx) < (hb.max.x - hb.min.x) / 2 + (hb.min.y - y) * 0.45;
  const free = (x, y, kind) => {
    const hd = houseDist(x, y);
    if (kind === 'tree') return hd > 5 && !nearPath(x, y, 3) && !inCorridor(x, y, 3.2, 14) && !inView(x, y);
    if (kind === 'bush') return hd > 2 && !nearPath(x, y, 1.8) && !inCorridor(x, y, 2, 8) && !(inView(x, y) && hd < 6);
    if (kind === 'flower') return hd > 1.5 && hd < 5 && !nearPath(x, y, 1.4) && !inCorridor(x, y, 1.6, 8);
    if (kind === 'tuft') return hd > 1.5 && !nearPath(x, y, 1.1);
    return hd > 1.5 && !nearPath(x, y, 1.6);
  };
  const within = (m = 0) => [plot.x0 + m + R() * (plot.x1 - plot.x0 - 2 * m), plot.y0 + m + R() * (plot.y1 - plot.y0 - 2 * m)];
  const pick = (n, kind, margin, minDist, out, tries = 40) => {
    for (let i = 0; i < n; i++) for (let k = 0; k < tries; k++) {
      const [x, y] = within(margin);
      if (!free(x, y, kind)) continue;
      if (minDist && out.some(o => Math.hypot(o[0] - x, o[1] - y) < minDist)) continue;
      out.push([x, y]); break;
    }
    return out;
  };
  const L = { plot, hb, doors, front, nearTrees: [], bushes: [], rocks: [], flowers: [], tufts: [], fence: [], far: [] };
  pick(16, 'tree', 2, 5.5, L.nearTrees);
  pick(34, 'bush', 1.5, 2.2, L.bushes);
  pick(14, 'rock', 1.5, 3, L.rocks);
  // flowers in small clusters near the house
  for (let c = 0; c < 9; c++) { const [x, y] = within(0); if (!free(x, y, 'flower')) { c--; if (R() < 0.02) break; continue; } for (let k = 0; k < 7; k++) { const fx = x + (R() - .5) * 1.6, fy = y + (R() - .5) * 1.6; if (free(fx, fy, 'flower')) L.flowers.push([fx, fy]); } }
  pick(900, 'tuft', 0.6, 0, L.tufts, 3);
  // a forest ring outside the plot
  const rMin = Math.hypot(plot.x1 - plot.x0, plot.y1 - plot.y0) / 2 + 4;
  for (let i = 0; i < 130; i++) { const a = R() * Math.PI * 2, r = rMin + Math.pow(R(), 1.4) * 70; L.far.push([cx + Math.cos(a) * r, cy + Math.sin(a) * r * 0.9]); }
  // the fence around the plot, with a gap where the path leaves
  const seg = 1.8, side = (x0, y0, x1, y1) => { const len = Math.hypot(x1 - x0, y1 - y0), n = Math.round(len / seg), ang = Math.atan2(y1 - y0, x1 - x0); for (let i = 0; i < n; i++) { const t = (i + 0.5) / n, x = x0 + (x1 - x0) * t, y = y0 + (y1 - y0) * t; if (v.set(x, y).distanceTo(p2) < 2.6) continue; L.fence.push([x, y, ang, len / n / seg]); } };
  side(plot.x0, plot.y0, plot.x1, plot.y0); side(plot.x1, plot.y0, plot.x1, plot.y1); side(plot.x1, plot.y1, plot.x0, plot.y1); side(plot.x0, plot.y1, plot.x0, plot.y0);
  return L;
}

// ---------- building ----------
function build() {
  const R = rng(20261128), L = layout(R); app.scenery_layout = L;
  const G = buildGeometries(R);
  const m1 = litMaterial({ sway: 0.02 }), m2 = litMaterial({ sway: 0.9, side: THREE.DoubleSide, flat: false });
  group = new THREE.Group(); group.name = 'scenery'; nature = new THREE.Group(); nature.name = 'nature';
  const M = new THREE.Matrix4(), Q = new THREE.Quaternion(), S = new THREE.Vector3(), P = new THREE.Vector3(), Z = new THREE.Vector3(0, 0, 1), col = new THREE.Color();
  const make = (name, geo, mat, list, { scale = [0.8, 1.3], tilt = 0, palette = null, z = 0, stretch = null } = {}) => {
    const n = list.length, im = new THREE.InstancedMesh(geo, mat, Math.max(1, n));
    for (let i = 0; i < n; i++) {
      const e = list[i], s = scale[0] + R() * (scale[1] - scale[0]);
      Q.setFromAxisAngle(Z, e[2] != null ? e[2] : R() * Math.PI * 2); S.set(s * (stretch ? e[3] : 1), s, s); P.set(e[0], e[1], z);
      M.compose(P, Q, S); im.setMatrixAt(i, M);
      if (palette) col.set(palette[Math.floor(R() * palette.length)]); else col.setScalar(0.85 + R() * 0.3);
      im.setColorAt(i, col);
    }
    im.count = n; im.instanceMatrix.needsUpdate = true; if (im.instanceColor) im.instanceColor.needsUpdate = true;
    im.frustumCulled = false; im.castShadow = false; im.receiveShadow = false; im.name = name; im.raycast = () => {};
    nature.add(im); inst[name] = im; totals[name] = n; return im;
  };
  // trees: kind A and B, the near ones first (the thermal model can ray cast just those)
  const nearA = L.nearTrees.filter((_, i) => i % 5 < 3), nearB = L.nearTrees.filter((_, i) => i % 5 >= 3);
  const farA = L.far.filter((_, i) => i % 2 === 0), farB = L.far.filter((_, i) => i % 2 === 1);
  nearCounts = { treeA: nearA.length, treeB: nearB.length };
  make('treeA', G.treeA, m1, [...nearA, ...farA], { scale: [0.85, 1.35] });
  make('treeB', G.treeB, m1, [...nearB, ...farB], { scale: [0.85, 1.4] });
  make('bush', G.bush, m1, L.bushes, { scale: [0.8, 1.5] });
  make('rock', G.rock, m1, L.rocks, { scale: [0.7, 1.6] });
  make('tuft', G.tuft, m2, L.tufts, { scale: [0.8, 1.7] });
  make('flower', G.flower, m2, L.flowers, { scale: [0.9, 1.4], palette: ['#f4d03f', '#ffffff', '#e85d9a', '#9b6fe0', '#ff8a3d'] });
  make('fence', G.fence, m1, L.fence, { scale: [1, 1], stretch: true });
  // birds: every instance flies a circle, in the vertex shader
  const birdGeo = new THREE.BufferGeometry();
  birdGeo.setAttribute('position', new THREE.Float32BufferAttribute([0, 0.5, 0, -0.14, -0.1, 0, 0.14, -0.1, 0, -1.2, -0.25, 0, -0.14, 0.15, 0, -0.1, -0.1, 0, 1.2, -0.25, 0, 0.14, 0.15, 0, 0.1, -0.1, 0], 3));
  const nb = 7, ab = new Float32Array(nb * 4), ah = new Float32Array(nb * 2);
  for (let i = 0; i < nb; i++) { ab.set([L.plot.x0 / 2 + L.plot.x1 / 2 + (R() - .5) * 60, L.plot.y0 / 2 + L.plot.y1 / 2 + (R() - .5) * 60, 55 + R() * 70, R() * 6.28], i * 4); ah.set([28 + R() * 22, (R() < .5 ? -1 : 1) * (0.05 + R() * 0.05)], i * 2); }
  const bg = new THREE.InstancedBufferGeometry(); bg.index = null; bg.setAttribute('position', birdGeo.attributes.position); bg.setAttribute('aB', new THREE.InstancedBufferAttribute(ab, 4)); bg.setAttribute('aH', new THREE.InstancedBufferAttribute(ah, 2)); bg.instanceCount = nb;
  birds = new THREE.Mesh(bg, new THREE.ShaderMaterial({
    uniforms: { uTime: U.uTime, uNight: U.uNight }, transparent: true, depthWrite: false, side: THREE.DoubleSide, fog: false,
    vertexShader: `attribute vec4 aB; attribute vec2 aH; uniform float uTime; void main(){
      float ang = uTime * aH.y + aB.w; vec3 c = vec3(aB.x + cos(ang) * aB.z, aB.y + sin(ang) * aB.z, aH.x + sin(ang * 3.) * 3.);
      vec3 fwd = vec3(-sin(ang), cos(ang), 0.) * sign(aH.y), rad = vec3(cos(ang), sin(ang), 0.);
      float flap = sin(uTime * 7. + aB.w * 3.) * abs(position.x) * .55;
      vec3 p = c + rad * position.x * 1.6 + fwd * position.y * 1.6 + vec3(0., 0., position.z + flap * 1.6);
      gl_Position = projectionMatrix * viewMatrix * vec4(p, 1.); }`,
    fragmentShader: 'uniform float uNight; void main(){ gl_FragColor = vec4(vec3(.07, .08, .1), .85 * (1. - uNight)); }',
  }));
  birds.frustumCulled = false; birds.name = 'birds'; birds.raycast = () => {}; nature.add(birds);
  // fireflies at night
  const nf = 80, fp = new Float32Array(nf * 3), fs = new Float32Array(nf);
  for (let i = 0; i < nf; i++) { const a = R() * 6.28, r = 3 + R() * 16; fp.set([(L.hb.min.x + L.hb.max.x) / 2 + Math.cos(a) * r, (L.hb.min.y + L.hb.max.y) / 2 + Math.sin(a) * r, 0.4 + R() * 2.4], i * 3); fs[i] = R() * 6.28; }
  const fg = new THREE.BufferGeometry(); fg.setAttribute('position', new THREE.BufferAttribute(fp, 3)); fg.setAttribute('aS', new THREE.BufferAttribute(fs, 1));
  fireflies = new THREE.Points(fg, new THREE.ShaderMaterial({
    uniforms: { uTime: U.uTime, uNight: U.uNight, uPR: U.uPR }, transparent: true, depthWrite: false, blending: THREE.AdditiveBlending, fog: false,
    vertexShader: `attribute float aS; uniform float uTime, uPR, uNight; varying float vA; void main(){
      vec3 p = position + vec3(sin(uTime * .6 + aS * 3.), cos(uTime * .5 + aS * 5.), sin(uTime * .8 + aS) * .5) * 1.4;
      vA = uNight * (.55 + .45 * sin(uTime * 2.2 + aS * 9.));
      vec4 mv = modelViewMatrix * vec4(p, 1.); gl_Position = projectionMatrix * mv; gl_PointSize = (4. + 3. * vA) * uPR; }`,
    fragmentShader: 'varying float vA; void main(){ float d = length(gl_PointCoord - .5); float a = smoothstep(.5, .0, d) * vA; gl_FragColor = vec4(vec3(.85, 1., .4) * a, a); }',
  }));
  fireflies.frustumCulled = false; fireflies.name = 'fireflies'; fireflies.raycast = () => {}; nature.add(fireflies);

  // sky and ground
  dome = new THREE.Mesh(new THREE.SphereGeometry(1400, 32, 16), skyMaterial()); dome.renderOrder = -9; dome.frustumCulled = false; dome.name = 'skyDome'; dome.raycast = () => {};
  groundMesh = new THREE.Mesh(new THREE.CircleGeometry(900, 48), litMaterial({ ground: true, colors: false, flat: false }));
  groundMesh.position.z = -0.03; groundMesh.receiveShadow = true; groundMesh.name = 'sceneryGround'; groundMesh.raycast = () => {};
  group.add(dome, groundMesh, nature); app.scene.add(group);
  built = true;
}

// ---------- state ----------
const levelFps = () => app.qualityLevels()[level].fps;       // 0: still, 60: every frame, else a cap
function animating() { return state.on && state.animate && state.wind > 0 && levelFps() > 0 && !reduced && onScreen && !document.hidden; }
function syncAnim() {
  const want = animating();
  if (want && !animOff) { const f = levelFps(); animOff = app.addTicker(() => true, f >= 60 ? {} : { fps: f }); }
  else if (!want && animOff) { animOff(); animOff = null; }
}
function applyLevel() {
  level = app.getQuality();
  const L = app.qualityLevels()[level], d = L.density;
  for (const [k, im] of Object.entries(inst)) {
    const n = totals[k] || 0;
    im.count = k === 'fence' ? n : k === 'treeA' || k === 'treeB' ? Math.max(Math.min(n, nearCounts[k]), Math.round(n * d)) : Math.round(n * d);
  }
  if (birds) birds.visible = level !== 'low';
  U.uPR.value = app.renderer.getPixelRatio();
  U.uCell.value = Math.max(1, Math.floor(app.renderer.getPixelRatio()));
  syncAnim(); app.requestRender();
}
function onSun(e) {
  if (!built) return;
  const u = dome.material.uniforms;
  u.uTop.value.copy(e.sky.top); u.uHor.value.copy(e.sky.hor); u.uSunDir.value.copy(e.dir); u.uSunCol.value.copy(e.color);
  u.uDay.value = smooth(-6, 4, e.elevation);
  u.uCover.value = 0.46;
  U.uNight.value = smooth(2, -8, e.elevation);
  if (fireflies) fireflies.visible = state.on && U.uNight.value > 0.02 && !nature.userData.hidden;
  const gc = groundMesh.material; gc.color.setScalar(0.2 + 0.8 * smooth(-12, 14, e.elevation));
}
function setNatureHidden() {
  const hide = !!((app.svc && (app.svc.isXray() || app.svc.isolated)));
  nature.userData.hidden = hide; nature.visible = state.on && !hide;
  if (fireflies) fireflies.visible = state.on && U.uNight.value > 0.02 && !hide;
  app.requestRender();
}
function apply() {
  if (!built) return;
  // the plain lawn plate under the house of the GLB is covered by the scenery ground
  app.meshByGuid.forEach((o, g) => { if (!app.nodeByGuid.has(g) && o.userData.els) o.userData.els.forEach(e => { if (e) e.set.setHidden(e, state.on); }); });
  group.visible = state.on; app.sky.visible = !state.on; app.ground.visible = !state.on;
  U.uDither.value = state.dither; U.uWind.value = state.wind;
  setNatureHidden(); syncAnim(); app.requestRender();
}

function get() { return { on: state.on, dither: state.dither, wind: state.wind, animate: state.animate, treesShade: state.treesShade, animating: !!animOff, reducedMotion: reduced, drawCalls: state.on ? 11 : 0, fps: animOff ? (levelFps() >= 60 ? 'every frame' : levelFps()) : 0, trees: { near: nearCounts.treeA + nearCounts.treeB, total: (inst.treeA ? inst.treeA.count : 0) + (inst.treeB ? inst.treeB.count : 0) } }; }
function set(o = {}) {
  if (o.on != null) state.on = !!o.on;
  if (o.dither != null) state.dither = cl(+o.dither, 0, 1);
  if (o.wind != null) state.wind = cl(+o.wind, 0, 1);
  if (o.animate != null) state.animate = !!o.animate;
  if (o.treesShade != null && !!o.treesShade !== state.treesShade) { state.treesShade = !!o.treesShade; if (app.thermal) app.thermal.invalidate(); app.emit('scenery', { treesShade: state.treesShade }); }
  syncControls(); apply(); app.emit('scenery', get());
  return get();
}
// the instances the thermal model casts rays against: the trees near the house, only when asked
function shadeTargets() {
  if (!state.on || !state.treesShade || !built) return [];
  if (!shadeProxy) {
    shadeProxy = ['treeA', 'treeB'].map(k => { const src = inst[k], im = new THREE.InstancedMesh(src.geometry, new THREE.MeshBasicMaterial(), nearCounts[k]); im.instanceMatrix = src.instanceMatrix; im.count = nearCounts[k]; im.frustumCulled = false; im.updateMatrixWorld(); return im; });
  }
  return shadeProxy;
}

function syncControls() {
  const q = id => document.getElementById(id);
  if (q('scenCb')) q('scenCb').checked = state.on;
  if (q('scenDither')) q('scenDither').value = Math.round(state.dither * 100);
  if (q('scenWind')) q('scenWind').value = Math.round(state.wind * 100);
  if (q('scenShade')) q('scenShade').checked = state.treesShade;
}

export function initScenery() {
  if (built) return app.scenery;
  const mq = matchMedia('(prefers-reduced-motion: reduce)'); reduced = mq.matches;
  mq.addEventListener && mq.addEventListener('change', e => { reduced = e.matches; syncAnim(); app.requestRender(); });
  build();
  app.scenery = { set, get, shadeTargets, on: () => state.on, layout: () => app.scenery_layout };
  app.on('sunlight', onSun);
  app.on('quality', applyLevel);
  app.on('xray', setNatureHidden); app.on('isolate', setNatureHidden);
  addEventListener('resize', () => { U.uCell.value = Math.max(1, Math.floor(app.renderer.getPixelRatio())); U.uPR.value = app.renderer.getPixelRatio(); });
  document.addEventListener('visibilitychange', syncAnim);
  if (typeof IntersectionObserver !== 'undefined') new IntersectionObserver(es => { onScreen = es[es.length - 1].isIntersecting; syncAnim(); }).observe(app.renderer.domElement);
  // the clock of the animation: moves only while the animation runs, so a still scene stays still
  app.addBeforeRender(now => {
    const dt = Math.min(0.1, (now - lastT) / 1000); lastT = now;
    if (animOff) { T += dt; cloudT += dt * (0.003 + 0.02 * state.wind); }
    U.uTime.value = T; U.uCloud.value = cloudT;
    dome.position.copy(app.camera.position);
  });
  // controls in the Look panel
  const q = id => document.getElementById(id);
  if (q('scenCb')) q('scenCb').onchange = e => set({ on: e.target.checked });
  if (q('scenDither')) q('scenDither').oninput = e => set({ dither: +e.target.value / 100 });
  if (q('scenWind')) q('scenWind').oninput = e => set({ wind: +e.target.value / 100 });
  if (q('scenShade')) q('scenShade').onchange = e => set({ treesShade: e.target.checked });
  applyLevel();
  if (app.lastSun) onSun(app.lastSun);
  apply();
  // compile every program now (the fireflies too), so a night or a toggle never adds one
  const fv = fireflies.visible, bv = birds.visible; fireflies.visible = birds.visible = true; nature.visible = group.visible = true;
  app.renderer.compile(app.scene, app.camera);
  fireflies.visible = fv; birds.visible = bv; apply();
  return app.scenery;
}
