// app.reset(scope): put things back as they were on a fresh load.
//
//   app.reset('view')        camera, camera mode, layout and maximised pane, sections, ground cutaway, structure and variable colouring, class and
//                            storey visibility, x-ray and isolate, selection and query result, quality back to the automatic level
//   app.reset('states')      every door and window, device, structural element and pinned value back to the default of the data (the
//                            scenario that changed them is cleared with them)
//   app.reset('time')        date and time back to what the page showed when it loaded (the real date and time), the sun stops playing,
//                            the lights follow the sun again
//   app.reset('simulation')  the scenario, traces, load paths, highlights, thermal results and the values the simulation wrote
//   app.reset('all')         all four
// Each returns {scope, changed: [what was different]}.  app.resetDefaults() is what "fresh" means: the visibility and time of the first
// interactive moment.
import { app } from './app.js';
import { detectQuality } from './scene.js';

const D = { vis: null, time: null, layers: null };
const eq = (a, b) => JSON.stringify(a) === JSON.stringify(b);
export function captureLayers() { if (app.svc) D.layers = app.svc.layerState().map(l => [l.id, l.visible]); }   // the services layers as the page starts (isolate switches the others on)
export function captureDefaults() {
  if (app.getVisibility) D.vis = app.getVisibility();
  if (app.sun) { const g = app.sun.get(); D.time = { date: g.date, time: g.time }; }
}
function view(ch) {
  const c = app.camera;
  if (app.mode !== 'orbit') { c.setMode('orbit'); ch.push('camera mode'); }
  if (app.viewport && (app.viewport.getLayout() !== 'single' || app.viewport.getMaximised())) { app.viewport.setLayout('single'); ch.push('layout'); }
  if (app.viewport) app.viewport.setNorthUp(false);
  if (app.section) { const was = app.section.get(), changed = ['x', 'y', 'z'].some(a => was[a].enabled) || was.applyTo3DSetting != null || was.storey; app.section.reset(); if (changed) ch.push('sections'); }
  if (app.rooms && app.rooms.getStyle() !== 'outline') { app.rooms.setStyle('outline'); ch.push('room style'); }
  if (app.ground && app.ground.getCutaway && app.ground.getCutaway() !== 'none') { app.ground.setCutaway('none'); ch.push('ground cutaway'); }
  if (app.structure && app.structure.getColourMode() !== 'material') { app.structure.setColourMode('material'); ch.push('structure colours'); }
  if (app.structure) app.structure.clearPath();
  if (app.statevars && app.statevars.colouring()) { app.statevars.colourBy(null); ch.push('variable colouring'); }
  if (app.svc) { if (D.layers) for (const [id, v] of D.layers) { const l = app.svc.layers.get(id); if (l && l.isVisible() !== v) { l.setVisible(v); if (!ch.includes('layers')) ch.push('layers'); } } }
  if (app.svc) { if (app.svc.isolated) { app.svc.isolate(null); ch.push('isolate'); } if (app.svc.isXray() || app.svc.xrayWanted) { app.svc.setXray(false); ch.push('x-ray'); } }
  if (app.clearResult && app.lastQueryInfo && app.lastQueryInfo()) { app.clearResult(); ch.push('query result'); }
  if (app.selectedInfo && app.selectedInfo()) { app.clearSelection(); ch.push('selection'); }
  if (D.vis && app.getVisibility && !eq(app.getVisibility(), D.vis)) { app.setVisibility(D.vis); ch.push('visibility'); }
  if (app.getColourMode && app.getColourMode() !== 'realistic') { app.setColourMode('realistic'); ch.push('colour mode'); }
  const q = detectQuality(); if (app.getQuality() !== q) { app.setQuality(q); ch.push('quality'); }
  if (app.animateCam && app.animateCam.cancel) app.animateCam.cancel();
  app.frameAll(true); app.requestRender();
}
function states(ch) {
  const ops = (app.systems && app.systems.openings) || [];
  for (const o of ops) { try { if (app.getState(o.guid) && app.getState(o.guid) !== (o.state || 'closed')) { app.setState(o.guid, o.state || 'closed'); ch.push('state of ' + o.label); } } catch { /* a fixed window has one state */ } }
  if (!ops.length) for (const s of app.stateList()) if ((s.kind === 'door' || s.kind === 'window') && s.state !== 'closed') { app.setState(s.id, 'closed'); ch.push('state of ' + s.label); }
  if (app.svc) { const before = app.svc.nodes.filter(t => t.hasState && app.getState(t.id) !== (t.d.state || app.systems.states[t.d.type][0])).length; app.svc.resetStates({ openings: false }); if (before) ch.push(before + ' devices'); }
  if (app.structure) { const n = app.structure.state().removed.length; if (n) { app.structure.restore(); ch.push(n + ' removed structural elements'); } }
  if (app.statevars) { const n = app.statevars.pinned().length; app.statevars.resetValues(); if (n) ch.push(n + ' pinned values'); }
  if (app.settleStates) app.settleStates();
}
function time(ch) {
  if (!app.sun) return;
  if (app.sun.isPlaying()) { app.sun.playDay(false); ch.push('play'); }
  const g = app.sun.get();
  if (D.time && (g.date !== D.time.date || Math.abs(g.time - D.time.time) > 1e-6)) { app.sun.set(D.time); ch.push('date and time'); }
  if (app.resetLights) app.resetLights();
}
function simulation(ch) {
  if (app.svc) { if (app.svc.activeScenario) { app.svc.resetStates({ openings: false }); ch.push('scenario'); } if (app.svc.lastTrace) { app.svc.clearTrace(); ch.push('trace'); } }
  if (app.structure) { app.structure.clearScenario(); app.structure.clearPath(); }
  if (app.thermal && app.thermal.last()) { app.thermal.invalidate(); ch.push('thermal results'); }
  if (app.statevars) { if (app.statevars.colouring()) { app.statevars.colourBy(null); ch.push('variable colouring'); } }
  app.emit('simulationreset', {}); app.requestRender();
}
export function reset(scope = 'all') {
  if (!['view', 'states', 'time', 'simulation', 'all'].includes(scope)) throw new Error('scope must be view, states, time, simulation or all.');
  const ch = [];
  if (scope === 'simulation' || scope === 'all') simulation(ch);
  if (scope === 'states' || scope === 'all') states(ch);
  if (scope === 'time' || scope === 'all') time(ch);
  if (scope === 'view' || scope === 'all') view(ch);
  const out = { scope, changed: [...new Set(ch)] }; app.emit('reset', out); return out;
}
