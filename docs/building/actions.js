// What the user can do: the commands (one table: menus, palette, shortcuts), the menu contents, the palette sources,
// reset with undo, the shareable link, exports. The viewport API of the rendering side is used when it is there
// (feature-detected at the moment a menu opens or a command runs), else the control shows as not available.
import { app } from './app.js';
import { defineCommands, runCommand, getCommand, listCommands, disabledReason, bindShortcuts, keyLabel } from './commands.js';
import { initMenubar, showMenu, openPalette } from './menus.js';
import { $, esc, ui, has, saveUi } from './shared.js';
import { defsAll } from './statevars-ui.js';
import { icon } from './icons.js';
import { openSectionPop, sectionState } from './viewport-ui.js';
import { CLASS_ORDER, CLASS_STYLE, pretty } from './classes.js';

const NA = 'not available yet';
const sv = () => app.svc;
const needSvc = () => (app.svc ? false : 'needs the services layer');
const needApi = p => () => (has(p) ? false : NA);
const quality = () => (app.getQuality ? app.getQuality() : 'high');

// ---------------------------------------------------------------- the commands
function defineAll() {
  const side = (id, key, title) => ({ id: 'panel.' + id, title: 'Open ' + title, group: 'Panels', keys: key, run: () => app.shell.openSide(id) });
  const tab = (id, key, title) => ({ id: 'tab.' + id, title: 'Open ' + title, group: 'Bottom panel', keys: key, run: () => app.shell.openTab(id) });
  defineCommands([
    { id: 'palette', title: 'Search and commands', group: 'General', keys: 'Mod+K', run: () => openPalette(paletteSources()) },
    { id: 'help.shortcuts', title: 'Keyboard shortcuts', group: 'General', keys: '?', run: () => import('./about.js').then(m => m.openAbout('shortcuts')) },
    { id: 'escape', title: 'Clear the selection', group: 'General', keyNote: 'Esc', run: () => app.clearSelection() },
    // modes and layout
    { id: 'cam.orbit', title: 'Camera: orbit', group: 'View', keys: 'O', inWalk: true, run: () => (app.mode === 'walk' ? app.walk.leave() : has('camera.setMode') ? app.camera.setMode('orbit') : undefined) },
    { id: 'cam.walk', title: 'Camera: first person', group: 'View', keyNote: 'F', run: () => (app.walk.active ? app.walk.leave() : app.walk.enter({})) },
    { id: 'cam.fly', title: 'Camera: fly', group: 'View', keys: 'Shift+F', enabled: needApi('camera.setMode'), run: () => app.camera.setMode('fly') },
    { id: 'layout.single', title: 'Layout: single view', group: 'View', keys: '1', enabled: needApi('viewport.setLayout'), run: () => app.viewport.setLayout('single') },
    { id: 'layout.quad', title: 'Layout: four views', group: 'View', keys: '4', enabled: needApi('viewport.setLayout'), run: () => app.viewport.setLayout('quad') },
    { id: 'view.xray', title: 'Toggle x-ray', group: 'View', keys: 'X', enabled: needSvc, run: () => sv().setXray(!sv().isXray()) },
    { id: 'view.reset', title: 'Reset the view', group: 'View', keys: 'H', run: () => doReset('view') },
    { id: 'view.sunpath', title: 'Toggle the sun path', group: 'View', run: () => app.sun.setPathVisible(!$('pathCb').checked) },
    { id: 'view.labels', title: 'Toggle room names', group: 'View', run: () => app.setVisibility({ roomNames: !app.getVisibility().roomNames }) },
    // panels
    side('explore', 'P E', 'Explore'), side('services', 'P S', 'Services'), side('sun', 'P U', 'Sun and look'), side('thermal', 'P T', 'Thermal'), side('states', 'P A', 'States'), side('query', 'P Q', 'ready queries'),
    { id: 'side.toggle', title: 'Show or hide the side panel', group: 'Panels', keys: 'Mod+B', run: () => app.shell.toggleSide() },
    { id: 'insp.toggle', title: 'Show or hide the inspector', group: 'Panels', keys: 'I', run: () => app.shell.toggleInspector() },
    { id: 'bottom.toggle', title: 'Show or hide the bottom panel', group: 'Panels', keys: 'Mod+J', run: () => app.shell.toggleBottom() },
    { id: 'bottom.max', title: 'Maximise the bottom panel', group: 'Panels', run: () => app.shell.maximiseBottom() },
    tab('console', 'G C', 'the console'), tab('results', 'G R', 'Results'), tab('validate', 'G V', 'the SHACL validation'), tab('graph', 'G G', 'the graph'), tab('thermal', 'G T', 'the thermal results'), tab('scenarios', 'G S', 'the scenarios'), tab('logs', 'G L', 'the log'),
    { id: 'console.run', title: 'Run the console query', group: 'Console', keys: 'Mod+Enter', run: () => { app.shell.openTab('console', { focus: false }); return app.console.run(); } },
    // simulate
    { id: 'sim.play', title: 'Play the day', group: 'Simulate', run: () => app.sun.playDay(!app.sun.isPlaying()) },
    { id: 'sim.thermal', title: 'Run the thermal day', group: 'Simulate', enabled: needSvc, run: async () => { await app.loadThermal(); app.thermalUi.runAndShow(); } },
    { id: 'sim.reset', title: 'Reset the simulation', group: 'Simulate', run: () => doReset('simulation') },
    { id: 'states.reset', title: 'Reset the states', group: 'Simulate', run: () => doReset('states') },
    { id: 'time.reset', title: 'Reset date and time', group: 'Simulate', run: () => doReset('time') },
    { id: 'states.open', title: 'Open all doors and windows', group: 'Layers', run: () => $('openAll').click() },
    { id: 'states.close', title: 'Close all doors and windows', group: 'Layers', run: () => $('closeAll').click() },
    // tools
    { id: 'tool.measure', title: 'Measure a distance', group: 'Tools', keys: 'M', run: () => import('./measure.js').then(m => m.toggleMeasure()) },
    { id: 'tool.link', title: 'Copy a link to this view', group: 'Tools', run: () => copyLink() },
    { id: 'export.thermal', title: 'Export the thermal day as CSV', group: 'Tools', enabled: needSvc, run: () => exportThermalCsv() },
    { id: 'export.states', title: 'Export the current states as JSON', group: 'Tools', run: () => exportStates() },
    { id: 'export.facts', title: 'Export the selection as Turtle', group: 'Tools', enabled: () => (app.selectedInfo() ? false : 'select something first'), run: () => exportSelectionTurtle() },
    { id: 'export.results', title: 'Copy the results as CSV', group: 'Tools', enabled: () => (app.results && app.results.get() ? false : 'no result yet'), run: () => $('resCsv').click() },
    { id: 'reset.all', title: 'Reset everything', group: 'Tools', keys: 'Mod+Shift+Backspace', run: () => doReset('all') },
    { id: 'reset.undo', title: 'Undo the last reset', group: 'Tools', keys: 'Mod+Z', global: false, enabled: () => (lastSnap ? false : 'nothing to undo'), run: () => undoReset() },
    { id: 'about.open', title: 'About this page', group: 'About', run: () => import('./about.js').then(m => m.openAbout('page')) },
    { id: 'about.mcp', title: 'WebMCP tools', group: 'About', run: () => import('./about.js').then(m => m.openAbout('webmcp')) },
    { id: 'settings', title: 'Settings: theme, density, units', group: 'General', run: () => app.shell.openSettings() },
    { id: 'theme.toggle', title: 'Switch between the dark and the light theme', group: 'General', run: () => app.shell.setTheme(ui.theme === 'dark' ? 'light' : 'dark') },
    { id: 'fullscreen', title: 'Full screen', group: 'General', run: () => $('fullBtn').click() },
  ]);
}

// ---------------------------------------------------------------- reset, with undo
let lastSnap = null, firstTime = null;
function snapshot() {
  const c = app.camera, t = app.controls && app.controls.target;
  return {
    t: Date.now(), sun: app.sun.get && { date: app.sun.get().date, time: app.sun.get().time }, states: app.allStates(), vis: app.getVisibility(), colour: app.getColourMode(),
    xray: app.svc ? app.svc.isXray() : false, iso: app.svc ? app.svc.isolated : null, layers: app.svc ? Object.fromEntries(app.svc.layerState().map(l => [l.id, l.visible])) : null,
    cam: c && { p: c.position.toArray(), t: t && t.toArray() }, sel: app.selectedInfo() && app.selectedInfo().id, site: app.sun.site ? { ...app.sun.site } : null,
    rooms: has('rooms.getStyle') ? app.rooms.getStyle() : null, section: sectionState() && JSON.parse(JSON.stringify(sectionState())), thermalView: app.thermalUi ? app.thermalUi.view : null,
  };
}
async function restoreSnap(s) {
  app.setVisibility({ classes: s.vis.classes, storeys: s.vis.storeys, roof: s.vis.roof, roomNames: s.vis.roomNames });
  app.setColourMode(s.colour);
  if (app.svc) { if (s.layers) for (const [id, v] of Object.entries(s.layers)) { try { app.svc.setLayer(id, v); } catch { /* gone */ } } app.svc.setXray(s.xray); app.svc.isolate(s.iso); }
  for (const [id, st] of Object.entries(s.states)) { try { if (app.getState(id) !== st) app.setState(id, st); } catch { /* cannot be set */ } }
  if (s.sun) { app.sun.playDay(false); app.sun.set(s.sun); }
  if (s.cam && s.cam.t) { const V = app.camera.position.constructor; app.animateCam(new V(...s.cam.p), new V(...s.cam.t), 400); }
  if (s.rooms && has('rooms.setStyle')) app.rooms.setStyle(s.rooms);
  if (s.section && has('section.set')) { for (const a of ['x', 'y', 'z']) if (s.section[a]) { try { app.section.set(a, s.section[a]); } catch { /* ignore */ } } }
}
export async function doReset(scope = 'view') {
  const scopes = ['view', 'states', 'time', 'simulation', 'all']; if (!scopes.includes(scope)) throw new Error('scope must be one of ' + scopes.join(', ') + '.');
  const snap = snapshot(); const did = [], notDone = [];
  const ownerHas = has('reset');
  if (ownerHas) { try { const r = await app.reset(scope); did.push('app.reset: ' + ((r && r.changed && r.changed.join(', ')) || 'nothing was different')); } catch (e) { notDone.push('app.reset: ' + (e.message || e)); } }
  else {   // without app.reset the shell does what it can from its side
  const view = scope === 'view' || scope === 'all', states = scope === 'states' || scope === 'all', time = scope === 'time' || scope === 'all', simu = scope === 'simulation' || scope === 'all';
  if (view) {
    app.clearResult(); app.clearSelection(); app.setColourMode('realistic');
    app.setVisibility({ classes: CLASS_ORDER.filter(c => CLASS_STYLE[c].on), storeys: app.graph.storeys.map((s, i) => i), roof: true, roomNames: true });
    if (app.svc) { app.svc.setXray(false); app.svc.isolate(null); app.svc.layers.forEach((l, id) => l.setVisible(id === 'lighting')); }
    if (!ownerHas) { if (has('section.set')) for (const a of ['x', 'y', 'z']) { try { app.section.set(a, { enabled: false }); } catch { /* ignore */ } } if (has('ground.setCutaway')) { try { app.ground.setCutaway('none'); } catch { /* ignore */ } } if (has('rooms.setStyle')) { try { app.rooms.setStyle('outline'); } catch { /* ignore */ } } if (has('viewport.setLayout')) { try { app.viewport.setLayout('single'); } catch { /* ignore */ } } }
    app.frameAll(false); did.push('view');
  }
  if (states) { if (app.svc) app.svc.resetStates({ openings: true }); else app.stateList().forEach(s => { if (s.state !== s.values[0]) { try { app.setState(s.id, s.values[0]); } catch { /* ignore */ } } }); if (has('structure.restoreAll')) { try { app.structure.restoreAll(); } catch { /* ignore */ } } did.push('states'); }
  if (time) { app.sun.playDay(false); if (firstTime) app.sun.set(firstTime); did.push('date and time'); }
  if (simu) { if (app.svc) app.svc.resetStates({ openings: false }); app.sun.playDay(false); app.svc && app.svc.clearTrace(); did.push('simulation'); }
  }
  if ((scope === 'simulation' || scope === 'all') && app.thermalUi) { app.thermalUi.setTempView(false); app.thermalUi.setSunView(false); const t1 = $('thTemps'), t2 = $('thSun'); if (t1) t1.checked = false; if (t2) t2.checked = false; did.push('thermal views'); }
  if ((scope === 'view' || scope === 'all') && has('rooms.setStyle')) { try { app.rooms.setStyle('outline'); } catch { /* ignore */ } }
  if (scope === 'view' || scope === 'all') { if (app.measure && app.measure.active) app.measure.stop(); const sp = $('sectionPop'); if (sp) sp.hidden = true; }
  lastSnap = snap;
  if (!ownerHas) app.emit('reset', { scope });
  app.notify(`Reset: ${scope}`, { ms: 8000, action: { label: 'Undo', run: undoReset } });
  return { scope, did, notDone };
}
export async function undoReset() { if (!lastSnap) return false; const s = lastSnap; lastSnap = null; await restoreSnap(s); app.log('info', 'user', 'Reset undone'); app.notify('Undone'); return true; }

// ---------------------------------------------------------------- the shareable link
const num = (v, d = 3) => +(+v).toFixed(d);
export function encodeLink() {
  const p = new URLSearchParams(); const g = app.sun.get();
  p.set('v', '1');
  const lay = has('viewport.getLayout') ? app.viewport.getLayout() : (app.viewport && app.viewport.layout); if (lay && lay !== 'single') p.set('layout', lay);
  const cm = has('camera.getMode') ? app.camera.getMode() : app.mode; if (cm && cm !== 'orbit') p.set('cam', cm);
  const c = app.camera.position, t = app.controls.target; p.set('cp', [c.x, c.y, c.z].map(v => num(v, 2)).join(',')); p.set('ct', [t.x, t.y, t.z].map(v => num(v, 2)).join(','));
  p.set('d', g.date); p.set('t', num(g.time, 2));
  const vis = app.getVisibility(); p.set('cls', vis.classes.map(c => c.replace(/^Ifc/, '')).join(',')); p.set('sto', vis.storeys.join(',')); if (!vis.roof) p.set('roof', '0'); if (!vis.roomNames) p.set('names', '0');
  if (app.svc) { const on = app.svc.layerState().filter(l => l.visible).map(l => l.id); p.set('sys', on.join(',') || '-'); if (app.svc.isXray()) p.set('xray', '1'); if (app.svc.isolated) p.set('iso', app.svc.isolated); }
  if (app.getColourMode() !== 'realistic') p.set('cm', app.getColourMode());
  const s = app.selectedInfo(); if (s) p.set('sel', s.id);
  const sec = sectionState(); if (sec) { const on = ['x', 'y', 'z'].filter(a => sec[a] && sec[a].enabled).map(a => `${a}:${num(sec[a].position, 2)}:${sec[a].flip ? 1 : 0}`); if (on.length) p.set('sec', on.join(',')); }
  if (has('rooms.getStyle')) { const rs = app.rooms.getStyle(); if (rs && rs !== 'outline') p.set('rooms', rs); }
  if (has('ground.getCutaway')) { const cu = app.ground.getCutaway(); if (cu && cu !== 'none') p.set('cut', cu); }
  if (ui.botOpen) p.set('tab', ui.botTab); if (ui.side) p.set('side', ui.side);
  return p.toString();
}
export const linkUrl = () => location.href.split('#')[0] + '#' + encodeLink();
export async function applyLink(str) {
  const h = String(str || '').includes('#') ? String(str).split('#')[1] : String(str || '').replace(/^[#?]/, ''); const p = new URLSearchParams(h); if (!p.get('v')) return false;
  const applied = [];
  try {
    if (p.get('cls')) { app.setVisibility({ classes: p.get('cls').split(',').filter(Boolean), storeys: (p.get('sto') || '').split(',').filter(x => x !== '').map(Number), roof: p.get('roof') !== '0', roomNames: p.get('names') !== '0' }); applied.push('layers'); }
    if (p.get('cm')) app.setColourMode(p.get('cm'));
    if (app.svc && p.get('sys') != null) { const on = new Set(p.get('sys').split(',')); app.svc.layers.forEach((l, id) => l.setVisible(on.has(id))); if (p.get('xray')) app.svc.setXray(true); if (p.get('iso')) app.svc.isolate(p.get('iso')); }
    if (p.get('d')) { app.sun.playDay(false); app.sun.set({ date: p.get('d'), time: +p.get('t') }); applied.push('time'); }
    if (p.get('layout') && has('viewport.setLayout')) app.viewport.setLayout(p.get('layout'));
    if (p.get('cam') && has('camera.setMode')) app.camera.setMode(p.get('cam'));
    if (p.get('sec') && has('section.set')) { for (const part of p.get('sec').split(',')) { const [a, pos, fl] = part.split(':'); app.section.set(a, { enabled: true, position: +pos, flip: fl === '1' }); } applied.push('sections'); }
    if (has('rooms.setStyle')) app.rooms.setStyle(p.get('rooms') || 'outline');
    if (p.get('cut') && has('ground.setCutaway')) app.ground.setCutaway(p.get('cut'));
    if (p.get('cp') && p.get('ct') && app.mode === 'orbit') { const V = app.camera.position.constructor, a = p.get('cp').split(',').map(Number), b = p.get('ct').split(',').map(Number); app.camera.position.set(...a); app.controls.target.set(...b); app.camera.lookAt(app.controls.target); app.controls.update(); app.requestRender(); applied.push('camera'); void V; }
    if (p.get('sel')) { try { app.select(p.get('sel'), { focus: false }); applied.push('selection'); } catch { /* unknown id */ } }
    if (p.get('side') && ui.side !== p.get('side')) app.shell.openSide(p.get('side'));
    if (p.get('tab')) app.shell.openTab(p.get('tab')); else if (ui.botOpen && !p.get('tab')) app.shell.closeBottom();
  } catch (e) { app.log('warn', 'user', 'The link could not be applied completely: ' + (e.message || e)); }
  app.log('info', 'user', 'Opened a link', { applied }); return applied.length > 0;
}
async function copyLink() { const u = linkUrl(); try { await navigator.clipboard.writeText(u); app.notify('Link copied'); } catch { app.notify(u, { ms: 12000 }); } try { history.replaceState(null, '', '#' + encodeLink()); } catch { /* sandboxed */ } return u; }

// ---------------------------------------------------------------- exports
function download(name, type, text) { const a = document.createElement('a'); a.href = URL.createObjectURL(new Blob([text], { type })); a.download = name; a.click(); setTimeout(() => URL.revokeObjectURL(a.href), 2000); }
async function exportThermalCsv() { await app.loadThermal(); const th = app.thermal; if (!th.last()) th.run(); download(`fzk-haus-thermal-${th.last().date}.csv`, 'text/csv', th.csv()); app.notify('Thermal day exported'); app.log('info', 'user', 'Exported the thermal day as CSV'); }
export function statesJson() {
  const o = { exported: new Date().toISOString(), openings: {}, devices: {}, date: app.sun.get().date, time: app.sun.get().timeStr };
  for (const s of app.stateList()) { if (s.kind === 'door' || s.kind === 'window') o.openings[s.label] = s.state; else o.devices[s.id] = s.state; }
  if (!Object.keys(o.devices).length && app.svc) for (const t of app.svc.nodes) if (t.hasState) o.devices[t.id] = app.svc.stateOf(t);
  return o;
}
function exportStates() { download('fzk-haus-states.json', 'application/json', JSON.stringify(statesJson(), null, 1)); app.notify('States exported'); app.log('info', 'user', 'Exported the current states as JSON'); }
async function exportSelectionTurtle() { const { iriOf, exportTurtle } = await import('./inspector.js'); const v = app.uiView.selected; if (!v) return app.notify('Select something first'); return exportTurtle(iriOf(v), v.l || v.id); }

// ---------------------------------------------------------------- menus
const chk = (label, get, set, extra = {}) => ({ label, check: get, run: set, keepOpen: true, ...extra });
const rad = (label, get, set, extra = {}) => ({ label, radio: get, run: set, keepOpen: true, ...extra });
const cmdItem = (id, label) => { const c = getCommand(id); return { label: label || c.title, keys: c.keys, disabled: () => disabledReason(c), run: () => runCommand(id) }; };
const camMode = () => (has('camera.getMode') ? app.camera.getMode() : app.mode);
const ROOM_STYLES = [['outline', 'Outline'], ['floor', 'Outline and floor'], ['fill', 'Filled volume'], ['off', 'Hidden']];
const STRUCT_MODES = [['material', 'Material'], ['role', 'Role'], ['load-bearing', 'Load bearing'], ['from-ifc', 'From the IFC or designed']];

function viewMenu() {
  const lay = () => (has('viewport.getLayout') ? app.viewport.getLayout() : (app.viewport && app.viewport.layout) || 'single');
  const items = [
    { heading: 'Layout' },
    rad('Single view', () => lay() === 'single', () => runCommand('layout.single'), { disabled: needApi('viewport.setLayout'), keys: '1' }),
    rad('Four views', () => lay() === 'quad', () => runCommand('layout.quad'), { disabled: needApi('viewport.setLayout'), keys: '4' }),
    { heading: 'Camera' },
    rad('Orbit', () => camMode() === 'orbit', () => runCommand('cam.orbit'), { keys: 'O' }),
    rad('First person', () => camMode() === 'walk', () => runCommand('cam.walk'), { meta: 'F' }),
    rad('Fly', () => camMode() === 'fly', () => runCommand('cam.fly'), { disabled: needApi('camera.setMode'), keys: 'Shift+F' }),
    { heading: 'Colours' },
    rad('Realistic', () => app.getColourMode() === 'realistic', () => app.setColourMode('realistic')),
    rad('By class', () => app.getColourMode() === 'class', () => app.setColourMode('class')),
  ];
  items.push({ heading: 'Structure colours' });
  for (const [id, l] of STRUCT_MODES) items.push(rad(l, () => has('structure.getColourMode') && app.structure.getColourMode() === id, () => app.structure.setColourMode(id), { disabled: needApi('structure.setColourMode') }));
  const numeric = defsAll().filter(d => d.variableKind === 'numeric' && d.appliesTo.kind !== 'surface');
  const cby = has('statevars.colourBy');
  items.push({ heading: 'Colour by a state variable' });
  const curVar = () => { try { const l = app.statevars && app.statevars.legend && app.statevars.legend(); return l ? l.variable : null; } catch { return null; } };
  items.push(rad('Off', () => !curVar(), () => app.statevars.colourBy(null), { disabled: () => (cby ? false : NA) }));
  numeric.filter(d => d.setBy !== 'person' || d.levels).slice(0, 12).forEach(d => items.push(rad(d.label, () => curVar() === d.id, () => app.statevars.colourBy(d.id), { disabled: () => (cby ? false : NA) })));
  items.push({ heading: 'Display' },
    chk('X-ray', () => !!(app.svc && app.svc.isXray()), () => runCommand('view.xray'), { disabled: needSvc, keys: 'X' }));
  if (app.svc) { items.push({ heading: 'Isolate a system' }, rad('No isolation', () => !app.svc.isolated, () => app.svc.isolate(null))); app.svc.sysList.forEach(s => items.push(rad(s.label, () => app.svc.isolated === s.id, () => app.svc.isolate(s.id)))); }
  items.push({ heading: 'Section cuts' });
  for (const a of ['x', 'y', 'z']) items.push(chk(`Section ${a.toUpperCase()}`, () => !!(sectionState() && sectionState()[a] && sectionState()[a].enabled), () => app.section.set(a, { enabled: !(sectionState()[a] && sectionState()[a].enabled) }), { disabled: needApi('section.set') }));
  items.push({ label: 'Section settings…', disabled: needApi('section.set'), run: () => openSectionPop() });
  items.push({ heading: 'Ground' });
  for (const [v, l] of [['none', 'Ground as it is'], ['trench', 'Trench around the house'], ['hidden', 'Ground hidden']]) items.push(rad(l, () => has('ground.getCutaway') && app.ground.getCutaway() === v, () => app.ground.setCutaway(v), { disabled: needApi('ground.setCutaway') }));
  items.push({ heading: 'Rooms' });
  for (const [v, l] of ROOM_STYLES) items.push(rad(l, () => has('rooms.getStyle') && app.rooms.getStyle() === v, () => app.rooms.setStyle(v), { disabled: needApi('rooms.setStyle') }));
  items.push({ heading: 'Shadows and quality' });
  for (const [v, l, m] of [['high', 'High', '2048 px shadows'], ['medium', 'Medium', '1024 px shadows'], ['low', 'Low', 'no shadows']]) items.push(rad(l, () => quality() === v, () => app.setQuality(v), { meta: m }));
  items.push({ heading: 'Scenery' });
  const sc = () => (app.scenery ? app.scenery.get() : { on: true, dither: 0.55, wind: 0.4 });
  items.push(chk('Sky, ground and trees', () => sc().on, async () => { const s = await app.loadScenery(); s.set({ on: !s.get().on }); }));
  for (const [l, v] of [['Off', 0], ['Low', 0.3], ['Medium', 0.55], ['High', 1]]) items.push(rad('Dither: ' + l, () => Math.abs(sc().dither - v) < 0.14, async () => { (await app.loadScenery()).set({ dither: v }); }));
  for (const [l, v] of [['Off', 0], ['Light', 0.4], ['Strong', 1]]) items.push(rad('Wind: ' + l, () => Math.abs(sc().wind - v) < 0.22, async () => { (await app.loadScenery()).set({ wind: v }); }));
  items.push({ heading: 'Show' },
    chk('Sun path', () => $('pathCb').checked, () => runCommand('view.sunpath')),
    chk('Room names', () => app.getVisibility().roomNames, () => runCommand('view.labels')),
    chk('Interior lights', () => app.getInteriorLights(), () => app.setInteriorLights(!app.getInteriorLights())),
    { sep: true }, cmdItem('view.reset', 'Reset the view'));
  return items;
}
function layersMenu() {
  const items = [{ heading: 'Element classes' }];
  for (const c of CLASS_ORDER) { const n = app.classCountOf(c); if (!n) continue; items.push(chk(pretty(c), () => app.getVisibility().classes.includes(c), () => app.setClassVisible(c, !app.getVisibility().classes.includes(c)), { meta: String(n) })); }
  items.push({ label: 'All classes', keepOpen: true, run: () => app.setAllClasses(true) }, { label: 'No classes', keepOpen: true, run: () => app.setAllClasses(false) });
  items.push({ heading: 'Storeys' });
  app.graph.storeys.forEach((s, i) => items.push(chk(s.l, () => app.getVisibility().storeys.includes(i), () => app.setStoreyVisible(i, !app.getVisibility().storeys.includes(i)), { meta: (+s.el).toFixed(2) + ' m' })));
  items.push(chk('Roof', () => app.getVisibility().roof, () => app.setVisibility({ roof: !app.getVisibility().roof })));
  items.push({ heading: 'Services and structure' });
  if (app.svc) {
    app.svc.sysList.forEach(s => items.push(chk(s.label, () => app.svc.layerState().find(l => l.id === s.id).visible, () => app.svc.setLayer(s.id, !app.svc.layerState().find(l => l.id === s.id).visible), { meta: String(app.svc.counts(s.id)) })));
    items.push({ label: 'All systems', keepOpen: true, run: () => app.svc.layers.forEach(l => l.setVisible(true)) }, { label: 'No systems', keepOpen: true, run: () => app.svc.layers.forEach(l => l.setVisible(false)) });
  } else items.push({ label: 'The services layer is not loaded', disabled: () => NA, run() {} });
  items.push({ heading: 'States overview' }, cmdItem('states.open'), cmdItem('states.close'), { label: 'Open the States panel', run: () => app.shell.openSide('states') });
  return items;
}
function simulateMenu() {
  const g = () => app.sun.get(), yr = () => g().date.slice(0, 4);
  const items = [{ heading: 'Date and time' },
    { label: 'Date and time controls…', run: () => app.shell.openSide('sun') },
    chk('Play the day', () => app.sun.isPlaying(), () => runCommand('sim.play')),
    ...[['Sunrise', 'sunrise'], ['Noon', 'noon'], ['Sunset', 'sunset'], ['Night', 'night']].map(([l, k]) => ({ label: l, run: () => document.querySelector(`#todPresets [data-k="${k}"]`).click() })),
    ...[['Equinox, March', '03-20'], ['Solstice, June', '06-21'], ['Equinox, September', '09-22'], ['Solstice, December', '12-21']].map(([l, d]) => ({ label: l, run: () => app.sun.set({ date: yr() + '-' + d }) })),
    { label: 'Site and true north…', run: () => { app.shell.openSide('sun'); const d = document.querySelector('#pane-sun details.sec:nth-of-type(2)'); if (d) d.open = true; setTimeout(() => $('siteNorth') && $('siteNorth').focus(), 60); } },
    { heading: 'Thermal day' },
    { label: 'Run the thermal day', disabled: needSvc, run: () => runCommand('sim.thermal') },
    { label: 'Thermal settings…', disabled: needSvc, run: () => app.shell.openSide('thermal') },
    chk('Room temperatures in 3D', () => !!(app.thermalUi && app.thermalUi.view.temps), async () => { await app.loadThermal(); app.thermalUi.setTempView(!app.thermalUi.view.temps); $('thTemps').checked = app.thermalUi.view.temps; }, { disabled: needSvc }),
    chk('Sun on the windows', () => !!(app.thermalUi && app.thermalUi.view.sun), async () => { await app.loadThermal(); app.thermalUi.setSunView(!app.thermalUi.view.sun); $('thSun').checked = app.thermalUi.view.sun; }, { disabled: needSvc }),
  ];
  items.push({ heading: 'Scenarios' });
  if (app.svc) {
    const sc = app.svc.data.scenarios;
    for (const [k, t] of [['supply', 'Supply'], ['structure', 'Structure']]) sc.filter(s => (s.kind || 'supply') === k).forEach(s => items.push({ label: s.label, meta: k === 'structure' ? 'structure' : '', run: () => { app.shell.openTab('scenarios'); app.scenarioApi.apply(s.id); } }));
  } else items.push({ label: 'Needs the services layer', disabled: () => NA, run() {} });
  items.push({ sep: true }, cmdItem('sim.reset'), cmdItem('states.reset'));
  return items;
}
function toolsMenu() {
  return [
    { label: 'Search and commands…', keys: 'Mod+K', run: () => runCommand('palette') },
    chk('Measure a distance', () => !!(app.measure && app.measure.active), () => runCommand('tool.measure'), { keys: 'M', keepOpen: false }),
    { heading: 'Export' },
    { label: 'Thermal day as CSV', disabled: needSvc, run: () => runCommand('export.thermal') },
    { label: 'Current states as JSON', run: () => runCommand('export.states') },
    { label: 'Selection as Turtle', disabled: () => (app.selectedInfo() ? false : 'select something first'), run: () => runCommand('export.facts') },
    { label: 'Results as CSV (copy)', disabled: () => (app.results && app.results.get() ? false : 'no result yet'), run: () => runCommand('export.results') },
    { heading: 'Share' },
    { label: 'Copy a link to this view', run: () => runCommand('tool.link') },
    { heading: 'Reset' },
    cmdItem('reset.undo'), cmdItem('reset.all', 'Reset everything'),
  ];
}
function aboutMenu() {
  return [
    { label: 'About this page', run: () => runCommand('about.open') },
    { label: 'Keyboard shortcuts', keys: '?', run: () => runCommand('help.shortcuts') },
    { label: 'WebMCP tools', run: () => runCommand('about.mcp') },
    { sep: true },
    { label: 'Guide', run: () => window.open('building-guide.html', '_blank', 'noopener') },
    { label: 'Open this graph in the playground', run: () => window.open('playground.html#url=' + encodeURIComponent(app.SYSTEMS_RETE || ''), '_blank', 'noopener') },
    { label: 'The rete project', run: () => window.open('https://github.com/caviri/rete', '_blank', 'noopener') },
  ];
}
export function resetMenu(anchor) {
  showMenu(anchor, [
    { heading: 'Reset' },
    ...[['view', 'View: camera, layers, colours, x-ray'], ['states', 'States: doors, windows, devices'], ['time', 'Date and time'], ['simulation', 'Simulation: thermal views, scenarios'], ['all', 'Everything']].map(([s, l]) => ({ label: l, run: () => doReset(s) })),
    { sep: true }, { label: 'Undo the last reset', keys: 'Mod+Z', disabled: () => (lastSnap ? false : 'nothing to undo'), run: () => undoReset() },
  ], { label: 'Reset' });
}
export function cameraMenu(anchor) {
  showMenu(anchor, viewMenu().slice(0, 9), { label: 'Camera and layout' });
}
export function qualityMenu(anchor) {
  showMenu(anchor, [{ heading: 'Quality' }, ...[['high', 'High', '2048 px shadows'], ['medium', 'Medium', '1024 px shadows'], ['low', 'Low', 'no shadows']].map(([v, l, m]) => rad(l, () => quality() === v, () => app.setQuality(v), { meta: m }))], { label: 'Quality' });
}

// ---------------------------------------------------------------- the palette
function paletteSources() {
  return [
    q => {   // commands
      const all = listCommands().filter(c => !/^(escape)$/.test(c.id));
      const hits = (q ? all.filter(c => (c.title + ' ' + c.group).toLowerCase().includes(q)) : all.slice(0, 14)).slice(0, q ? 8 : 14);
      return hits.map(c => ({ kind: 'Command', label: c.title, tag: c.keys ? keyLabel(c.keys) : c.group, run: () => runCommand(c.id) }));
    },
    q => {   // ready queries
      if (!q) return []; return (app.examples || []).map((e, k) => ({ e, k })).filter(({ e }) => e.label.toLowerCase().includes(q)).slice(0, 4).map(({ e, k }) => ({ kind: 'Query', label: e.label, tag: 'run it', run: () => { app.shell.openSide('query'); document.querySelectorAll('.qbtn')[k].click(); } }));
    },
    q => {   // elements and rooms
      if (!q || q.length < 2) return []; const out = [];
      for (const n of app.graph.nodes) { if (n.l.toLowerCase().includes(q)) { out.push({ kind: n.k === 'Space' ? 'Room' : 'Element', label: n.l, tag: app.graph.storeys[n.st] ? app.graph.storeys[n.st].l : '', run: () => app.select(n) }); if (out.length >= 8) break; } }
      return out;
    },
    q => {   // devices and runs
      if (!q || q.length < 2 || !app.svc) return []; return app.svc.find({ text: q }).slice(0, 8).map(t => ({ kind: 'Device', label: t.d.label, tag: app.svc.sys.get(t.system).label, run: () => app.select(t.id) }));
    },
  ];
}

// ---------------------------------------------------------------- init
export function initActions() {
  defineAll(); bindShortcuts();
  firstTime = { date: app.sun.get().date, time: app.sun.get().time };
  initMenubar($('menubar'), { view: viewMenu, layers: layersMenu, simulate: simulateMenu, tools: toolsMenu, about: aboutMenu }, $('menuAll'));
  $('resetBtn').addEventListener('click', e => resetMenu(e.currentTarget));
  $('qChip').addEventListener('click', e => qualityMenu(e.currentTarget));
  $('settingsBtn').onclick = () => app.shell.openSettings();
  $('openConsole') && ($('openConsole').onclick = () => app.shell.openTab('console'));
  Object.assign(app, { commands: { list: () => listCommands().map(c => ({ id: c.id, title: c.title, group: c.group, keys: c.keys || null, disabled: disabledReason(c) })), run: runCommand } });
  app.resetShell = doReset; app.undoReset = undoReset; app.link = { encode: encodeLink, url: linkUrl, apply: applyLink };
}
export { esc, saveUi };
