// Boot. The order is: the house on screen first, everything else after.
//   1. renderer and scene (scene.js), the index and the GLB are fetched in parallel
//   2. the GLB is decoded: the house is added with one plain material and drawn (first frame)
//   3. after that frame: textures, the doors and windows, the batches, the panels, the sun
//   4. the services layer, the WebMCP tools, the scenery. `ready` is the end of all of it.
// Walk mode and the thermal model are not loaded here: the first use loads them (app.loadWalk,
// app.loadThermal). See README.md.
import * as THREE from 'three';
import { GLTFLoader } from 'three/addons/loaders/GLTFLoader.js';
import { DRACOLoader } from 'three/addons/loaders/DRACOLoader.js';
import { app } from './app.js';
import { initScene } from './scene.js';
import './logs.js';   // the log starts with the page (errors, loads); the shell's tabs come later

const DATA = 'https://data.graphplaza.com/cad/fzk-viewer.json';
// Optional systems layer, on R2; 404 until it is uploaded. Nothing is kept in docs/.
export const SYSTEMS_JSON = 'https://data.graphplaza.com/cad/fzk-haus-systems-2026-11-28.json';
export const SYSTEMS_RETE = 'https://data.graphplaza.com/cad/fzk-haus-systems-2026-11-28.rete';
app.SYSTEMS_JSON = SYSTEMS_JSON; app.SYSTEMS_RETE = SYSTEMS_RETE;

let resolveReady;
app.readyPromise = new Promise(r => { resolveReady = r; });
app.whenReady = () => app.readyPromise;
app.timing = {};
const mark = k => { app.timing[k] = Math.round(performance.now()); };

const lmsg = document.getElementById('lmsg');
const say = m => { if (lmsg) lmsg.textContent = m; };
const later = () => new Promise(r => setTimeout(r, 0));
const paint = () => new Promise(r => requestAnimationFrame(() => setTimeout(r, 0)));

// the index is fetched like the model is: a failed request is tried again (twice) before the page gives up
async function fetchJson(url, tries = 3) {
  for (let a = 0; ; a++) {
    try { const r = await fetch(url); if (!r.ok) throw new Error('HTTP ' + r.status + ' for ' + url); return await r.json(); }
    catch (e) { if (a >= tries - 1) throw e; await new Promise(r => setTimeout(r, 500 * (a + 1))); }
  }
}

async function fetchSystems() {
  try {
    const r = await fetch(SYSTEMS_JSON);
    if (!r.ok) return null;                 // 404 is the normal case until the systems layer exists
    return await r.json();
  } catch { return null; }
}

// ---------- modules that load on first use ----------
let walkP = null, thermalP = null, sceneryP = null;
// A stand-in for app.walk until walk.js is loaded: reading it is free, using it loads the module.
function walkStub() {
  const load = () => app.loadWalk();
  const key = e => {
    if (e.key.toLowerCase() !== 'f' || e.ctrlKey || e.metaKey || e.altKey) return;
    const a = document.activeElement; if (a && /^(INPUT|TEXTAREA|SELECT)$/.test(a.tagName)) return;
    e.preventDefault(); load().then(w => w.enter());
  };
  addEventListener('keydown', key);
  app._walkKey = key;
  app.walk = {
    stub: true, active: false, info: () => ({ active: false }),
    enter: async o => (await load()).enter(o), leave: async () => (await load()).leave(), toggle: async () => (await load()).toggle(),
    teleport: async (...a) => (await load()).teleport(...a), findFree: async (...a) => (await load()).findFree(...a),
  };
}
app.loadWalk = () => walkP || (walkP = import('./walk.js').then(m => {
  removeEventListener('keydown', app._walkKey);
  m.initWalk(); m.buildWalk(); mark('walk');
  return app.walk;
}));
app.loadThermal = () => thermalP || (thermalP = (async () => {
  if (!app.svc) throw new Error('The building-services layer is not loaded.');
  const [TH, TU] = await Promise.all([import('./thermal.js'), import('./thermal-ui.js')]);
  const th = TH.initThermal(app.svc); TU.initThermalUi(app.svc, th); mark('thermal');
  return th;
})());
app.findCoplanar = () => import('./zfight.js').then(m => m.findCoplanar());   // the z-fighting diagnostic, see zfight.js
app.loadScenery = () => sceneryP || (sceneryP = import('./scenery.js').then(m => { m.initScenery(); mark('scenery'); return app.scenery; }));

async function bootServices(systems) {
  const [S, UI, W] = await Promise.all([import('./systems.js'), import('./systems-ui.js'), import('./webmcp.js')]);
  const svc = S.initSystems(systems);
  UI.initSystemsUi(svc);
  // the thermal day: the section is here, its code loads when it is first opened
  // (the shell has the Thermal panel in the page; opening it loads the model, see shell-init.js)
  if (app.shell && app.shell.state().side === 'thermal') app.loadThermal().catch(e => console.error('[building] thermal failed to load', e));
  mark('services');
  return W;
}

async function boot() {
  const stage = document.getElementById('stage');
  const dataP = fetchJson(DATA);
  const systemsP = fetchSystems();
  initScene(stage);
  // not needed for the first frame: fetched in parallel with the model, run after it
  const modsP = Promise.all([import('./materials.js'), import('./states.js'), import('./sun.js'), import('./ui.js'), import('./shell-init.js').catch(e => { console.error('[building] the shell failed to load', e); return null; })]);   // the house works without the shell
  modsP.catch(() => {});
  const graph = await dataP;
  app.graph = graph;
  graph.nodes.forEach((n, i) => { n.i = i; app.nodeByGuid.set(n.n, n); });

  const loader = new GLTFLoader().setDRACOLoader(new DRACOLoader().setDecoderPath('https://www.gstatic.com/draco/versioned/decoders/1.5.7/'));
  let gltf = null;
  for (let a = 0; a < 3 && !gltf; a++) {
    try { gltf = await loader.loadAsync(graph.glb, e => { if (e.total) document.querySelector('#bar>i').style.width = (100 * e.loaded / e.total) + '%'; }); }
    catch (e) { if (a === 2) { say('could not load the 3D model.'); console.warn(e); return; } await new Promise(r => setTimeout(r, 500 * (a + 1))); }
  }

  // ---- first frame: the house, one plain material, voids and room volumes hidden as they are by default
  const plain = new THREE.MeshStandardMaterial({ color: '#e9e3d6', roughness: 0.9 });
  gltf.scene.traverse(o => {
    if (!o.isMesh) return;
    o.userData.guid = o.name;
    app.meshByGuid.set(o.name, o);
    const n = app.nodeByGuid.get(o.name);
    if (n && (n.k === 'Space' || n.c === 'IfcOpeningElement')) o.visible = false;
    o.material = plain;
  });
  app.house.add(gltf.scene);
  app.house.updateMatrixWorld(true);
  app.houseBox = new THREE.Box3().setFromObject(app.house);
  const hb = app.houseBox; app.plot = { x0: hb.min.x - 14, x1: hb.max.x + 14, y0: hb.min.y - 12, y1: hb.max.y + 12 };   // the walker and the fence stay inside
  app.frameAll(true);
  app.applySunDefault();
  document.getElementById('loading').style.display = 'none';
  app.renderNow(); mark('firstFrame');
  document.getElementById('count').textContent = `(${graph.nodes.length})`;
  await paint();

  // ---- everything the first frame did not need
  const [M, S, SUN, UI, SHELL] = await modsP;
  const systems = await systemsP;          // started with the page; the door hinges are in it
  app.systems = systems;
  if (systems) { try { (await import('./statevars.js')).initStateVars(systems); } catch (e) { console.error('[building] the state variables failed to start', e); } }   // before the doors and windows take their states
  SUN.initSun(); M.initMaterials(); S.initStates();
  SUN.siteFromSystems(systems);
  walkStub();
  try { (await import('./viewport.js')).initViewport(); mark('viewport'); } catch (e) { console.error('[building] the viewport failed to start', e); }   // layouts, sections, camera modes, reset
  app.buildEnv();
  await later();

  const rooms = [];
  graph.nodes.filter(n => n.k === 'Space').forEach(n => {
    const m = app.meshByGuid.get(n.n); if (!m) return;
    const box = new THREE.Box3().setFromObject(m);
    const el = graph.storeys[n.st] ? +graph.storeys[n.st].el : box.min.z;
    const r = { guid: n.n, label: n.l, storey: n.st, box, center: box.getCenter(new THREE.Vector3()), floorZ: el + 0.001, node: n };
    app.roomByGuid.set(n.n, r); rooms.push(r);
  });
  S.buildStates();          // splits doors and windows into parts, hangs the leaves on hinges
  await later();
  M.applyMaterials(rooms);  // textures, UVs, the batches, floors and name sprites
  app.setRoomLights(rooms);
  await later();
  UI.initUI();
  try { if (SHELL) { const t0 = performance.now(); SHELL.initShell(); app.timing.shellMs = Math.round(performance.now() - t0); } } catch (e) { console.error('[building] the shell failed to start', e); }
  import('./clock.js').then(c => c.initClock());
  app.settleStates();
  app.sun.set({});          // light the scene for the real date and time
  app.renderNow();
  app.interactive = true; mark('interactive');
  app.emit('interactive', {});
  await paint();

  // ---- the services layer, the tools and the scenery
  let W = null;
  if (systems) { try { W = await bootServices(systems); } catch (e) { console.error('[building] the services layer failed to start', e); } }
  if (!W) W = await import('./webmcp.js');
  W.wireWebMcp();
  if (app.svc) W.addTools(W.systemsTools());
  W.addTools(W.perfTools());
  await W.addExtraTools();   // the shell's tools, and the rendering side's when its module is there
  mark('tools');
  try { await app.loadScenery(); } catch (e) { console.error('[building] the scenery failed to start', e); }
  document.getElementById('loading').style.display = 'none';
  app.renderNow();
  app.ready = true; mark('ready');
  app.emit('ready', { systems: !!systems, services: !!app.svc });
  resolveReady();
}

boot().catch(e => { console.error(e); say('Something went wrong: ' + (e.message || e)); });
