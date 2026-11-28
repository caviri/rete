// Renderer, camera, lights, base sky, ground, time of day, quality and the render loop.
// The page renders on demand: requestRender() marks it dirty, tickers keep it
// drawing while something moves. Nothing renders when nothing changes.
//
// Lights are never added or removed: a light that is off has intensity 0, so the shader
// programs stay the same when the sun sets, the lights go on or a layer is toggled.
// The shadow map is not redrawn every frame (autoUpdate is off): it is redrawn when the sun
// moves, when a door or window moves, or when something in the house is shown or hidden.
import * as THREE from 'three';
import { OrbitControls } from 'three/addons/controls/OrbitControls.js';
import { app } from './app.js';

export const HOUSE_CENTER = new THREE.Vector3(6, 5, 2.2);
const DEFAULT_BOX = new THREE.Box3(new THREE.Vector3(-0.5, -0.5, -0.1), new THREE.Vector3(12.8, 10.8, 6.4));

// quality levels: pixel ratio cap, shadow map size (0 = off), texture anisotropy, scenery density and its animation rate (0 = still)
export const LEVELS = {
  low: { pr: 1, shadow: 0, aniso: 1, density: 0.35, fps: 0 },
  medium: { pr: 1.5, shadow: 1024, aniso: 4, density: 0.7, fps: 24 },
  high: { pr: 2, shadow: 2048, aniso: 8, density: 1, fps: 60 },
};

let renderer, scene, camera, controls, stage;
let dirty = true, dirtyAll = true, dirty3d = false, last = performance.now(), shadowDirty = true;
const tickers = new Set(), beforeRender = new Set();
let sun, moon, hemi, amb, sky, ground, contact, fog;
let sunElev = 30, quality = 'high', lightsOn = false, lightsManual = false, qualityAuto = true;
let envTex = null;
const interiorGroup = new THREE.Group();
const sunDir = new THREE.Vector3(0, 0, 1);
const perf = { renderMs: 0, frameMs: 0, slow: 0 };

const cl = (v, a, b) => Math.min(b, Math.max(a, v));
const smooth = (a, b, x) => { const t = cl((x - a) / (b - a), 0, 1); return t * t * (3 - 2 * t); };

// ---------- the first guess at a quality level ----------
// Small screen, few cores or little memory lower it; ?quality=low|medium|high decides it.
export function detectQuality() {
  try { const q = new URLSearchParams(location.search).get('quality'); if (LEVELS[q]) { qualityAuto = false; return q; } } catch { /* no URL */ }
  const small = matchMedia('(max-width:860px)').matches || Math.min(screen.width, screen.height) < 600;
  const cores = navigator.hardwareConcurrency || 4, mem = navigator.deviceMemory || 4;
  if (small || cores <= 2 || mem <= 2) return 'low';       // a phone, or a device with little to spare
  if (cores <= 4 || mem <= 4) return 'medium';
  return 'high';
}

// ---------- sky keyframes: sun elevation (deg) -> zenith colour, horizon colour
const SKY = [
  [-14, '#03050c', '#0a1124'],
  [-5, '#0a1230', '#1a2a52'],
  [0, '#2a3b78', '#ef8a5c'],
  [6, '#4a6cb4', '#f4b98c'],
  [18, '#4784d2', '#a9cdee'],
  [40, '#3b7ad4', '#b8d7f2'],
];
const tmpA = new THREE.Color(), tmpB = new THREE.Color();
export function skyAt(e) {
  let i = 0;
  while (i < SKY.length - 2 && e > SKY[i + 1][0]) i++;
  const a = SKY[i], b = SKY[i + 1];
  const t = cl((e - a[0]) / (b[0] - a[0]), 0, 1);
  return {
    top: tmpA.set(a[1]).lerp(tmpB.set(b[1]), t).clone(),
    hor: new THREE.Color(a[2]).lerp(tmpB.set(b[2]), t),
  };
}

function skyMaterial() {
  return new THREE.ShaderMaterial({
    side: THREE.BackSide, depthWrite: false, fog: false, toneMapped: false,
    uniforms: {
      top: { value: new THREE.Color('#3b7ad4') }, hor: { value: new THREE.Color('#b8d7f2') },
      bot: { value: new THREE.Color('#6d7a6a') }, sunDir: { value: new THREE.Vector3(0, 0, 1) },
      sunCol: { value: new THREE.Color('#fff2d8') }, glow: { value: 1 },
    },
    vertexShader: 'varying vec3 vP; void main(){ vP=position; vec4 p=projectionMatrix*modelViewMatrix*vec4(position,1.); gl_Position=p.xyww; }',   // the sky is at infinity: it never needs the far plane
    fragmentShader: `#include <common>
      varying vec3 vP; uniform vec3 top,hor,bot,sunDir,sunCol; uniform float glow;
      void main(){ vec3 d=normalize(vP); float h=d.z;
        vec3 c = h>0. ? mix(hor, top, pow(h,0.55)) : mix(hor, bot, smoothstep(0.,0.35,-h));
        float s=max(dot(d,normalize(sunDir)),0.);
        c += sunCol*(smoothstep(.9988,.9992,s)*2.4 + pow(s,40.)*0.35*glow + pow(s,24.)*0.15*glow);
        gl_FragColor=vec4(c,1.);
        #include <colorspace_fragment>
      }`,
  });
}

export function initScene(stageEl) {
  stage = stageEl;
  const w = stage.clientWidth || 800, h = stage.clientHeight || 600;
  quality = detectQuality();
  scene = new THREE.Scene();
  camera = new THREE.PerspectiveCamera(45, w / h, 0.05, 3000);
  camera.up.set(0, 0, 1);                       // IFC is Z-up
  camera.position.set(18, -16, 12);

  // preserveDrawingBuffer: the panes of the four-view layout are drawn one by one, a pane that did not change is left as it is (viewport.js)
  renderer = new THREE.WebGLRenderer({ antialias: true, powerPreference: 'high-performance', preserveDrawingBuffer: true });
  renderer.localClippingEnabled = true;
  // the three section planes shared by every house and services material; a plane that is off keeps everything (section.js)
  app.clipPlanes = [0, 1, 2].map(() => new THREE.Plane(new THREE.Vector3(0, 0, 1), 1e6));
  renderer.setPixelRatio(Math.min(devicePixelRatio || 1, LEVELS[quality].pr));
  renderer.setSize(w, h);
  renderer.outputColorSpace = THREE.SRGBColorSpace;
  renderer.toneMapping = THREE.ACESFilmicToneMapping;
  renderer.toneMappingExposure = 1.0;
  renderer.shadowMap.type = THREE.PCFSoftShadowMap;
  renderer.shadowMap.enabled = LEVELS[quality].shadow > 0;
  renderer.shadowMap.autoUpdate = false;
  stage.insertBefore(renderer.domElement, stage.firstChild);
  renderer.domElement.style.touchAction = 'none';

  const house = new THREE.Group(); house.name = 'house'; scene.add(house);
  scene.add(interiorGroup); interiorGroup.name = 'interiorLights';

  // base sky dome (the scenery module puts a richer one in front of it)
  sky = new THREE.Mesh(new THREE.SphereGeometry(1000, 24, 12), skyMaterial());
  sky.renderOrder = -10; sky.frustumCulled = false; sky.name = 'baseSky'; scene.add(sky);

  // lights: these five are the only lights besides the room lamps, for the whole life of the page
  hemi = new THREE.HemisphereLight(0xcfe3ff, 0x4a4538, 0.8); scene.add(hemi);
  amb = new THREE.AmbientLight(0xfff2e0, 0.3); scene.add(amb);
  sun = new THREE.DirectionalLight(0xfff1dc, 3);
  sun.castShadow = LEVELS[quality].shadow > 0;
  sun.shadow.mapSize.set(LEVELS[quality].shadow || 1024, LEVELS[quality].shadow || 1024);
  const sc = sun.shadow.camera; sc.near = 1; sc.far = 110;
  sun.shadow.bias = -0.0006; sun.shadow.normalBias = 0.04; sun.shadow.radius = 3;
  sun.target.position.copy(HOUSE_CENTER);
  scene.add(sun, sun.target);
  moon = new THREE.DirectionalLight(0x7f9bff, 0);
  const md = new THREE.Vector3(-0.5, -0.6, 0.62).normalize();
  moon.position.copy(HOUSE_CENTER).addScaledVector(md, 60); moon.target.position.copy(HOUSE_CENTER);
  scene.add(moon, moon.target);

  // ground + contact shadow
  ground = new THREE.Mesh(new THREE.CircleGeometry(700, 48), new THREE.MeshStandardMaterial({ color: 0x5d7a43, roughness: 1, metalness: 0 }));
  ground.position.z = -0.03; ground.receiveShadow = true; ground.name = 'ground';
  scene.add(ground);
  contact = new THREE.Mesh(new THREE.PlaneGeometry(21, 19),
    new THREE.MeshBasicMaterial({ map: contactTexture(), transparent: true, depthWrite: false, polygonOffset: true, polygonOffsetFactor: -2, polygonOffsetUnits: -2, fog: false }));
  contact.position.set(6, 5, -0.025); contact.renderOrder = 1; scene.add(contact);

  fog = new THREE.Fog(0xb8d7f2, 120, 600); scene.fog = fog;

  controls = new OrbitControls(camera, renderer.domElement);
  controls.enableDamping = true; controls.dampingFactor = 0.08;
  controls.screenSpacePanning = true;
  controls.maxPolarAngle = Math.PI * 0.495; controls.minDistance = 0.5; controls.maxDistance = 120;
  controls.mouseButtons = { LEFT: THREE.MOUSE.ROTATE, MIDDLE: THREE.MOUSE.DOLLY, RIGHT: THREE.MOUSE.PAN };
  controls.touches = { ONE: THREE.TOUCH.ROTATE, TWO: THREE.TOUCH.DOLLY_PAN };
  controls.addEventListener('change', () => requestRender('3d'));
  renderer.domElement.addEventListener('contextmenu', e => e.preventDefault());
  addEventListener('keydown', e => { if (e.key === 'Shift') controls.mouseButtons.LEFT = THREE.MOUSE.PAN; });
  addEventListener('keyup', e => { if (e.key === 'Shift') controls.mouseButtons.LEFT = THREE.MOUSE.ROTATE; });
  addEventListener('resize', onResize);
  if (typeof ResizeObserver !== 'undefined') new ResizeObserver(onResize).observe(stage);

  Object.assign(app, { scene, camera, renderer, controls, house, HOUSE_CENTER, sky, ground, perf });
  Object.assign(app, {
    requestRender, renderNow, addTicker, animateCam, focus, buildEnv, addBeforeRender,
    applySun, setQuality, getQuality: () => quality, qualityLevels: () => LEVELS, getQualityAuto: () => qualityAuto,
    setInteriorLights, getInteriorLights: () => lightsOn,
    setRoomLights, interiorLightGroup: interiorGroup,
    getSunDirection: () => sunDir.clone(), frameAll, isAnimating: () => tickers.size > 0, shadowDirty: () => { shadowDirty = true; },
    resetLights: () => { lightsManual = false; app.setInteriorLights(sunElev < 4, true); },
    frameCount: 0, lighting: { sun: () => sun, hemi: () => hemi, amb: () => amb, ground: () => ground, contact: () => contact, fog: () => fog },
  });
  app.applySunDefault = () => applySun(new THREE.Vector3(0.2, -0.45, 0.87).normalize(), 55);
  app.applySunDefault();
  requestAnimationFrame(frame);
}

// environment map for glass and metal: a daylight sky, baked once (after the first frame)
export function buildEnv() {
  if (envTex) return envTex;
  try {
    const pm = new THREE.PMREMGenerator(renderer);
    const es = new THREE.Scene();
    const m = skyMaterial(); m.uniforms.sunDir.value.set(0.4, -0.5, 0.6);
    const dome = new THREE.Mesh(new THREE.SphereGeometry(50, 24, 12), m); es.add(dome);
    envTex = pm.fromScene(es, 0.02).texture; pm.dispose(); dome.geometry.dispose(); m.dispose();
    app.envTexture = envTex;
  } catch (e) { console.warn('env map failed', e); }
  return envTex;
}

function contactTexture() {
  const c = document.createElement('canvas'); c.width = 128; c.height = 128;
  const g = c.getContext('2d'); g.clearRect(0, 0, 128, 128);
  g.shadowColor = 'rgba(0,0,0,.85)'; g.shadowBlur = 15; g.fillStyle = '#000';
  g.fillRect(26, 28, 76, 72);   // ~ house footprint inside the 21 x 19 m plane
  const t = new THREE.CanvasTexture(c); t.colorSpace = THREE.SRGBColorSpace; return t;
}

function onResize() {
  if (!stage) return;
  const w = stage.clientWidth, h = stage.clientHeight; if (!w || !h) return;
  if (!(app.viewport && app.viewport.resize())) { camera.aspect = w / h; camera.updateProjectionMatrix(); }
  renderer.setSize(w, h);
  requestRender();
}

// ---------- render loop ----------
// requestRender(): something in the scene changed, every pane is drawn again. requestRender('3d'): only the camera of the
// 3D pane moved, the orthographic panes keep what they show (viewport.js).
// requestRender('panes'): a pane marked itself dirty (viewport.js), nothing else changed.
export function requestRender(scope) { dirty = true; if (scope === '3d') dirty3d = true; else if (scope !== 'panes') dirtyAll = true; }
// fn(now) runs just before every render (batches upload their changes, scenery sets its clock)
export function addBeforeRender(fn) { beforeRender.add(fn); return () => beforeRender.delete(fn); }
export function renderNow() {
  const t0 = performance.now();
  dirtyAll = true;
  drawFrame(t0);
}
function drawFrame(t0) {
  const all = dirtyAll, only3d = !all && dirty3d; dirtyAll = dirty3d = false;
  updateClip();
  for (const fn of beforeRender) fn(t0);
  for (const set of app.batchSetList || []) if (set.sync()) shadowDirty = true;
  if (shadowDirty && renderer.shadowMap.enabled) { renderer.shadowMap.needsUpdate = true; shadowDirty = false; }
  if (app.viewport) app.viewport.render(all, only3d); else renderer.render(scene, camera);
  app.frameCount++;
  perf.renderMs = performance.now() - t0;
  app.emit('render');
}
// A ticker is fn(dt, now). Return false to remove it; anything else keeps it (and keeps rendering).
// opts.fps caps how often it is called: between calls it neither runs nor asks for a render.
export function addTicker(fn, opts = {}) {
  const t = { fn, every: opts.fps ? 1000 / opts.fps : 0, next: 0, last: performance.now(), scope: opts.scope || 'all' };   // scope '3d': it only moves the 3D camera
  tickers.add(t); dirty = true; if (t.scope !== '3d') dirtyAll = true; else dirty3d = true;
  return () => tickers.delete(t);
}
let frames = 0, slowAcc = 0, lastRender = 0;
function frame(now) {
  requestAnimationFrame(frame);
  const dt = Math.min(0.1, (now - last) / 1000); last = now;
  let must = dirty; dirty = false;
  for (const t of [...tickers]) {
    if (t.every && now < t.next) continue;
    const td = t.every ? Math.min(0.25, (now - t.last) / 1000) : dt; t.last = now; t.next = now + t.every * 0.92;
    if (t.fn(td, now) === false) tickers.delete(t); else { must = true; if (t.scope === '3d') dirty3d = true; else dirtyAll = true; }
  }
  if (app.mode === 'orbit' && controls.update()) { must = true; dirty3d = true; }
  if (must) {
    if (lastRender && now - lastRender < 250) { perf.frameMs = perf.frameMs ? perf.frameMs * 0.9 + (now - lastRender) * 0.1 : now - lastRender; autoDowngrade(now - lastRender); } else slowAcc = frames = 0;
    lastRender = now; drawFrame(performance.now());
  } else lastRender = 0;
}
// A device that keeps missing 20 fps steps down one level (not under WebDriver: tests stay deterministic).
function autoDowngrade(ms) {
  if (!qualityAuto || navigator.webdriver || quality === 'low' || document.hidden) return;
  frames++; slowAcc += ms;
  if (frames >= 45) { const avg = slowAcc / frames; frames = 0; slowAcc = 0; if (avg > 50) setQuality(quality === 'high' ? 'medium' : 'low'); }
}

// ---------- depth precision ----------
// The near plane decides how finely depth is resolved (the far plane hardly matters), so it follows the camera: far from the
// house it can be far (2 m at most, so the trees next to the camera are not cut), close to it, small. Walk mode is
// at eye height inside rooms: 8 cm. The sky is drawn at infinity (gl_Position.xyww) and does not set the far plane.
const clipBox = new THREE.Box3();
function updateClip() {
  let near, far;
  if (app.mode === 'walk') { near = 0.08; far = 450; }
  else if (app.mode === 'fly') { near = 0.1; far = 1100; }
  else { const b = app.houseBox || DEFAULT_BOX; near = cl(0.35 * b.distanceToPoint(camera.position), 0.1, 2); far = 1100; }
  if (Math.abs(near - camera.near) > camera.near * 0.08 || far !== camera.far) { camera.near = near; camera.far = far; camera.updateProjectionMatrix(); }
}
// ---------- camera ----------
function animateCam(pos, target, ms = 600) {
  const a = { p0: camera.position.clone(), p1: pos.clone(), t0: controls.target.clone(), t1: target.clone(), s: performance.now(), d: ms };
  const off = addTicker(() => {
    const k = Math.min(1, (performance.now() - a.s) / a.d), e = k < .5 ? 2 * k * k : 1 - Math.pow(-2 * k + 2, 2) / 2;
    camera.position.lerpVectors(a.p0, a.p1, e); controls.target.lerpVectors(a.t0, a.t1, e);
    camera.lookAt(controls.target);
    if (k >= 1) return false;
  }, { scope: '3d' });
  animateCam.cancel && animateCam.cancel();
  animateCam.cancel = off;
}
const boxOf = o => {
  if (!o) return new THREE.Box3();
  if (o.isBox3) return o.clone();
  if (Array.isArray(o)) { const b = new THREE.Box3(); o.forEach(x => b.union(boxOf(x))); return b; }
  return new THREE.Box3().setFromObject(o);
};
// focus(object3D | Box3 | array of them, {snap, distance}) : move the orbit camera so it is framed.
export function focus(target, opts = {}) {
  const box = boxOf(target); if (box.isEmpty()) return false;
  if (app.mode === 'walk' || app.mode === 'fly') return false;
  const c = box.getCenter(new THREE.Vector3()), s = box.getSize(new THREE.Vector3()).length() || 0.5;
  const dir = new THREE.Vector3(0.8, -0.9, 0.5).normalize();
  const dist = opts.distance || Math.max(s * 2.0, 2.5);
  const pos = c.clone().add(dir.multiplyScalar(dist));
  if (opts.snap) { camera.position.copy(pos); controls.target.copy(c); camera.lookAt(c); controls.update(); requestRender(); }
  else animateCam(pos, c);
  return true;
}
export function frameAll(snap) {
  const box = app.houseBox || new THREE.Box3().setFromObject(app.house); if (box.isEmpty()) return;
  const c = box.getCenter(new THREE.Vector3()), size = box.getSize(new THREE.Vector3());
  const dist = (Math.max(size.x, size.y, size.z) / 2) / Math.tan(camera.fov * Math.PI / 360) * 1.55;
  const dir = new THREE.Vector3(0.85, -0.95, 0.5).normalize();
  const pos = c.clone().add(dir.multiplyScalar(dist));
  if (snap) { camera.position.copy(pos); controls.target.copy(c); camera.lookAt(c); controls.update(); requestRender(); }
  else animateCam(pos, c);
}

// ---------- the shadow camera, fitted to the house ----------
const corner = new THREE.Vector3(), look = new THREE.Vector3();
function fitShadow() {
  const b = app.houseBox || DEFAULT_BOX, sc = sun.shadow.camera;
  sun.updateMatrixWorld(); sun.target.updateMatrixWorld();
  sc.position.setFromMatrixPosition(sun.matrixWorld); sc.lookAt(look.setFromMatrixPosition(sun.target.matrixWorld));
  sc.updateMatrixWorld(); sc.matrixWorldInverse.copy(sc.matrixWorld).invert();
  let x0 = 1e9, x1 = -1e9, y0 = 1e9, y1 = -1e9, z0 = 1e9, z1 = -1e9;
  for (let i = 0; i < 8; i++) {
    corner.set(i & 1 ? b.max.x + 0.3 : b.min.x - 0.3, i & 2 ? b.max.y + 0.3 : b.min.y - 0.3, i & 4 ? b.max.z + 0.3 : b.min.z - 0.3).applyMatrix4(sc.matrixWorldInverse);
    x0 = Math.min(x0, corner.x); x1 = Math.max(x1, corner.x); y0 = Math.min(y0, corner.y); y1 = Math.max(y1, corner.y); z0 = Math.min(z0, corner.z); z1 = Math.max(z1, corner.z);
  }
  sc.left = x0; sc.right = x1; sc.bottom = y0; sc.top = y1; sc.near = Math.max(0.1, -z1 - 1); sc.far = -z0 + 1;
  sc.updateProjectionMatrix();
}

// ---------- the sun (sun.js computes where it is; this lights the scene) ----------
// dir: unit vector from the house toward the sun, in the model frame. e: elevation in degrees.
export function applySun(dir, e) {
  sunDir.copy(dir);
  const day = smooth(-1, 9, e), warm = smooth(0, 32, e);
  sun.position.copy(HOUSE_CENTER).addScaledVector(sunDir.clone().setZ(Math.max(sunDir.z, 0.06)).normalize(), 55);
  sun.color.set('#ff8c45').lerp(tmpB.set('#fff0da'), warm);
  sun.intensity = 3.4 * day;
  moon.intensity = 0.55 * (1 - smooth(-12, 2, e));
  const k = skyAt(e);
  const u = sky.material.uniforms;
  u.top.value.copy(k.top); u.hor.value.copy(k.hor); u.bot.value.copy(k.hor).multiplyScalar(0.55);
  u.sunDir.value.copy(sunDir); u.sunCol.value.copy(sun.color); u.glow.value = smooth(-4, 6, e);
  fog.color.copy(k.hor);
  hemi.color.copy(k.top).lerp(tmpB.set('#ffffff'), 0.45);
  hemi.groundColor.set('#3c372c').lerp(k.hor, 0.15);
  hemi.intensity = 0.1 + 1.25 * smooth(-10, 20, e);
  amb.intensity = 0.04 + 0.4 * day;
  ground.material.color.set('#5d7a43').multiplyScalar(0.18 + 0.82 * smooth(-12, 14, e));
  contact.material.opacity = (renderer.shadowMap.enabled ? 0.3 : 0.55) * (0.4 + 0.6 * day);
  renderer.toneMappingExposure = 1 + 0.25 * (1 - day);
  if (app.envMaterials) for (const m of app.envMaterials) m.envMapIntensity = (m.userData.envBase ?? 1) * (0.12 + 0.88 * day);
  sunElev = e;
  if (sun.castShadow && e > 0.5) { fitShadow(); shadowDirty = true; }
  if (!lightsManual) setInteriorLights(e < 4, true);
  app.lastSun = { dir: sunDir.clone(), elevation: e, color: sun.color.clone(), day, sky: k };
  app.emit('sunlight', app.lastSun);
  requestRender();
}

// ---------- quality ----------
export function setQuality(q, silent) {
  q = q === 'high' || q === 'medium' ? q : q === 'low' ? 'low' : (q ? 'high' : quality);
  const L = LEVELS[q], wasShadow = renderer.shadowMap.enabled;
  quality = q;
  renderer.setPixelRatio(Math.min(devicePixelRatio || 1, L.pr));
  renderer.setSize(stage.clientWidth || 800, stage.clientHeight || 600);
  renderer.shadowMap.enabled = L.shadow > 0;
  sun.castShadow = L.shadow > 0;
  if (L.shadow && sun.shadow.mapSize.x !== L.shadow) { sun.shadow.mapSize.set(L.shadow, L.shadow); if (sun.shadow.map) { sun.shadow.map.dispose(); sun.shadow.map = null; } }
  if (wasShadow !== renderer.shadowMap.enabled) scene.traverse(o => { if (!o.material) return; (Array.isArray(o.material) ? o.material : [o.material]).forEach(m => { m.needsUpdate = true; }); });
  if (app.setAnisotropy) app.setAnisotropy(L.aniso);
  applySun(sunDir.clone(), sunElev);   // refits the shadow camera and the contact shadow
  shadowDirty = true; requestRender();
  if (!silent) app.emit('quality', { quality });
  return quality;
}

// ---------- interior lights ----------
// One point light per room, near the ceiling. The systems pass replaces these
// with real fixtures: hide or clear app.interiorLightGroup and call setInteriorLights as before.
export function setRoomLights(rooms) {
  interiorGroup.clear();
  for (const r of rooms) {
    const size = r.box.getSize(new THREE.Vector3());
    const area = size.x * size.y;
    const z = Math.min(r.box.max.z - 0.3, r.floorZ + 2.3);
    const pl = new THREE.PointLight(0xffd9a8, 0, Math.max(size.x, size.y) * 1.1 + 2, 1.6);
    pl.userData.base = 1.5 + Math.min(area, 40) * 0.12;
    pl.position.set((r.box.min.x + r.box.max.x) / 2, (r.box.min.y + r.box.max.y) / 2, z);
    pl.userData.room = r.guid;
    interiorGroup.add(pl);
  }
  setInteriorLights(lightsOn, true);
}
export function setInteriorLights(on, auto) {
  if (!auto) lightsManual = true;
  const changed = lightsOn !== !!on;
  lightsOn = !!on;
  interiorGroup.children.forEach(l => { l.intensity = lightsOn ? l.userData.base : 0; });
  if (changed) { requestRender(); app.emit('lights', { on: lightsOn }); }
}
