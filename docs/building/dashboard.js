// The application shell: the rail, the side panel, the inspector, the bottom panel and their sizes, the top-right
// controls, theme and density, resizing the renderer. The markup is in building.html; this module wires it.
// app.shell = { openSide(id), closeSide(), toggleSide(), openInspector(), closeInspector(), openTab(id), closeBottom(),
//               toggleBottom(), maximiseBottom(on), state(), onTab(id, fn), setTheme(t), setDensity(d), setUnits(u) }
import { app } from './app.js';
import { icon } from './icons.js';
import { $, ui, saveUi, isNarrow, isMedium, has, esc } from './shared.js';
import { showMenu } from './menus.js';

const RAIL = [['explore', 'Explore', 'explore'], ['services', 'Services', 'services'], ['sun', 'Sun and look', 'sun'], ['thermal', 'Thermal', 'thermal'], ['states', 'States', 'states'], ['query', 'Queries', 'query']];
const TABS = [['console', 'Console'], ['results', 'Results'], ['validate', 'Validate'], ['graph', 'Graph'], ['thermal', 'Thermal'], ['scenarios', 'Scenarios'], ['logs', 'Logs']];
const hooks = {}, sideHooks = {};   // id -> [fn]
const root = document.documentElement;
let tabEls = {}, railEls = {};

// ---------- sizes and the renderer ----------
let rzT = 0;
export function notifyResize() {
  clearTimeout(rzT);
  rzT = setTimeout(() => {
    dispatchEvent(new Event('resize'));
    if (app.viewport && app.viewport.resize) { try { app.viewport.resize(); } catch { /* the other module */ } }
    if (app.graphView && app.graphView.resize) app.graphView.resize();
    app.requestRender && app.requestRender();
    app.emit('shellresize', { w: $('stage').clientWidth, h: $('stage').clientHeight });
  }, 60);
}
const maxSide = () => Math.max(220, innerWidth - 48 - (ui.insp && !isMedium() ? ui.inspW : 0) - 320);
const maxInsp = () => Math.max(240, innerWidth - 48 - (ui.side ? ui.sideW : 0) - 320);
const maxBot = () => Math.max(140, innerHeight - 40 - 24 - 200);
function applySizes() {
  const narrow = isNarrow();
  if (narrow) { if (ui.botOpen) { ui.side = null; ui.insp = false; } else if (ui.insp) ui.side = null; for (const [k, b] of Object.entries(railEls)) b.setAttribute('aria-selected', String(k === ui.side)); }
  ui.sideW = Math.min(Math.max(ui.sideW, 220), maxSide()); ui.inspW = Math.min(Math.max(ui.inspW, 240), maxInsp()); ui.botH = Math.min(Math.max(ui.botH, 120), maxBot());
  root.style.setProperty('--side-w', ui.side && !narrow ? ui.sideW + 'px' : '0px');
  root.style.setProperty('--insp-w', ui.insp && !isMedium() ? ui.inspW + 'px' : '0px');
  root.style.setProperty('--bot-h', ui.botH + 'px');
  $('side').hidden = !ui.side; $('inspector').hidden = !ui.insp;
  $('rzSide').hidden = !ui.side || narrow; $('rzInsp').hidden = !ui.insp || isMedium();
  const b = $('bottom'); b.classList.toggle('closed', !ui.botOpen); b.classList.toggle('max', !!(ui.botMax && ui.botOpen));
  $('rzBot').hidden = !ui.botOpen || ui.botMax || narrow;
}
function layoutChanged() { applySizes(); saveUi(); notifyResize(); app.emit('shell', state()); }

// ---------- side panel ----------
export function openSide(id) {
  if (!RAIL.some(r => r[0] === id)) throw new Error('No side panel "' + id + '". Panels: ' + RAIL.map(r => r[0]).join(', ') + '.');
  if (isNarrow()) { ui.insp = false; ui.botOpen = false; } else if (isMedium()) ui.insp = false;
  ui.side = id; document.querySelectorAll('#side .pane').forEach(p => { p.hidden = p.dataset.pane !== id; });
  for (const [k, b] of Object.entries(railEls)) { b.setAttribute('aria-selected', String(k === id)); b.tabIndex = k === id ? 0 : -1; }
  layoutChanged(); (sideHooks[id] || []).forEach(f => { try { f(); } catch (e) { console.error('[building] panel hook failed', e); } });
  app.emit('panel', { side: id });
}
export function closeSide() { ui.side = null; for (const b of Object.values(railEls)) b.setAttribute('aria-selected', 'false'); layoutChanged(); app.emit('panel', { side: null }); }
export const toggleSide = () => (ui.side ? closeSide() : openSide(ui.lastSide || 'explore'));
// ---------- inspector ----------
export function openInspector(opts = {}) {
  if (isNarrow()) { ui.side = ui.side && opts.keepSide ? ui.side : null; ui.botOpen = false; if (!ui.side) for (const b of Object.values(railEls)) b.setAttribute('aria-selected', 'false'); }
  if (isMedium() && !isNarrow() && ui.side) { ui.lastSide = ui.side; ui.side = null; for (const b of Object.values(railEls)) b.setAttribute('aria-selected', 'false'); }
  ui.insp = true; if (!opts.auto) ui.inspClosed = false; layoutChanged(); app.emit('panel', { inspector: true });
}
export function closeInspector(auto) { ui.insp = false; if (!auto) ui.inspClosed = true; layoutChanged(); app.emit('panel', { inspector: false }); }
export const toggleInspector = () => (ui.insp ? closeInspector() : openInspector());

// ---------- bottom panel ----------
export function openTab(id, opts = {}) {
  if (!TABS.some(t => t[0] === id)) throw new Error('No tab "' + id + '". Tabs: ' + TABS.map(t => t[0]).join(', ') + '.');
  if (isNarrow()) { ui.side = null; ui.insp = false; for (const b of Object.values(railEls)) b.setAttribute('aria-selected', 'false'); }
  ui.botTab = id; ui.botOpen = true;
  document.querySelectorAll('#btBody .tp').forEach(p => p.classList.toggle('on', p.dataset.tab === id));
  for (const [k, b] of Object.entries(tabEls)) { b.setAttribute('aria-selected', String(k === id)); b.tabIndex = k === id ? 0 : -1; }
  layoutChanged();
  (hooks[id] || []).forEach(f => { try { f(opts); } catch (e) { console.error('[building] tab hook failed', e); } });
  setBadge(id, null);
  app.emit('tab', { tab: id });
}
export function closeBottom() { ui.botOpen = false; ui.botMax = false; syncBotButtons(); layoutChanged(); app.emit('tab', { tab: null }); }
export const toggleBottom = () => (ui.botOpen ? closeBottom() : openTab(ui.botTab || 'console'));
export function maximiseBottom(on) { ui.botMax = on == null ? !ui.botMax : !!on; if (ui.botMax && !ui.botOpen) ui.botOpen = true; syncBotButtons(); layoutChanged(); }
function syncBotButtons() {
  const mx = $('btMax'), cl = $('btClose'); if (!mx) return;
  mx.setAttribute('aria-pressed', String(!!ui.botMax)); mx.innerHTML = icon(ui.botMax ? 'minimise' : 'maximise'); mx.title = ui.botMax ? 'Restore the panel size' : 'Maximise the panel'; mx.setAttribute('aria-label', mx.title);
  cl.innerHTML = icon(ui.botOpen ? 'chevronDown' : 'chevronUp'); cl.title = ui.botOpen ? 'Collapse the panel' : 'Open the panel'; cl.setAttribute('aria-label', cl.title);
}
const badges = {};
export function setBadge(tab, text, hot) {
  const b = tabEls[tab]; if (!b) return; let s = b.querySelector('.badge');
  if (text == null || text === '') { if (s) s.remove(); return; }
  if (!s) { s = document.createElement('span'); s.className = 'badge'; b.append(s); }
  s.textContent = text; s.classList.toggle('hot', !!hot); badges[tab] = text;
}
export const onTab = (id, fn) => { (hooks[id] = hooks[id] || []).push(fn); };
export const onSide = (id, fn) => { (sideHooks[id] = sideHooks[id] || []).push(fn); };

// ---------- resizers ----------
function dragger(el, axis, get, set) {
  el.addEventListener('pointerdown', e => {
    e.preventDefault(); el.setPointerCapture(e.pointerId); el.classList.add('drag');
    const x0 = e.clientX, y0 = e.clientY, v0 = get();
    const mv = ev => { set(v0 + (axis === 'x' ? ev.clientX - x0 : y0 - ev.clientY) * (el.dataset.dir === '-' ? -1 : 1)); applySizes(); };
    const up = () => { el.classList.remove('drag'); el.removeEventListener('pointermove', mv); el.removeEventListener('pointerup', up); el.removeEventListener('pointercancel', up); saveUi(); notifyResize(); };
    el.addEventListener('pointermove', mv); el.addEventListener('pointerup', up); el.addEventListener('pointercancel', up);
  });
  el.addEventListener('keydown', e => {   // the separator is a keyboard control too
    const step = e.shiftKey ? 40 : 12; let d = 0;
    if (axis === 'x') d = e.key === 'ArrowRight' ? step : e.key === 'ArrowLeft' ? -step : 0; else d = e.key === 'ArrowUp' ? step : e.key === 'ArrowDown' ? -step : 0;
    if (!d) return; e.preventDefault(); set(get() + d * (el.dataset.dir === '-' ? -1 : 1)); layoutChanged();
  });
}

// ---------- the top-right controls ----------
const CAMERA = [['orbit', 'Orbit', 'orbit'], ['walk', 'First person', 'walk'], ['fly', 'Fly', 'fly']];
const LAYOUT = [['single', 'Single view', 'single'], ['quad', 'Four views', 'quad']];
function setCameraMode(m) {
  if (has('camera.setMode')) return app.camera.setMode(m);
  if (m === 'walk') { return app.walk.active ? app.walk.leave() : app.walk.enter({}); }
  if (m === 'orbit' && app.mode === 'walk') return app.walk.leave();
  if (m === 'orbit') return;
  app.notify('Fly mode is not available yet.');
}
function buildTopRight() {
  const box = $('topright');
  box.innerHTML = `<div class="seg" id="camSeg" role="group" aria-label="Camera mode">${CAMERA.map(([id, t, ic]) => `<button class="segb" type="button" data-cam="${id}" ${id === 'walk' ? 'id="walkBtn"' : ''} title="${t}${id === 'walk' ? ' (F)' : id === 'orbit' ? ' (O)' : ' (Shift+F)'}" aria-label="${t}" aria-pressed="false">${icon(ic)}<span class="hide-md">${t}</span></button>`).join('')}</div>
    <div class="seg hide-sm" id="layoutSeg" role="group" aria-label="Layout">${LAYOUT.map(([id, t, ic]) => `<button class="segb" type="button" data-lay="${id}" title="${t}" aria-label="${t}" aria-pressed="false">${icon(ic)}</button>`).join('')}</div>
    <button class="qchip" type="button" id="qChip" aria-haspopup="menu" aria-label="Quality"><span id="qChipT">…</span></button>
    <span class="sepv hide-sm"></span>
    <button class="btn sm" type="button" id="resetBtn" aria-haspopup="menu" title="Reset">${icon('reset')}<span class="hide-md">Reset</span></button>
    <button class="ibtn hide-sm" type="button" id="settingsBtn" title="Settings" aria-label="Settings">${icon('settings')}</button>
    <button class="ibtn hide-sm" type="button" id="fullBtn" title="Full screen" aria-label="Full screen">${icon('fullscreen')}</button>`;
  box.querySelectorAll('[data-cam]').forEach(b => b.addEventListener('click', () => setCameraMode(b.dataset.cam)));
  box.querySelectorAll('[data-lay]').forEach(b => b.addEventListener('click', () => { if (has('viewport.setLayout')) app.viewport.setLayout(b.dataset.lay); }));
  $('fullBtn').onclick = () => { if (document.fullscreenElement) document.exitFullscreen(); else document.documentElement.requestFullscreen && document.documentElement.requestFullscreen().catch(() => app.notify('Full screen is not allowed here.')); };
  syncTop();
}
export function syncTop() {
  const mode = app.mode || 'orbit', cm = has('camera.getMode') ? app.camera.getMode() : mode;
  document.querySelectorAll('[data-cam]').forEach(b => {
    const id = b.dataset.cam, off = id === 'fly' && !has('camera.setMode');
    b.setAttribute('aria-pressed', String(id === cm)); b.classList.toggle('on', id === cm);
    if (off) { b.setAttribute('aria-disabled', 'true'); b.title = 'Fly mode: not available yet'; } else b.removeAttribute('aria-disabled');
  });
  const lay = has('viewport.getLayout') ? app.viewport.getLayout() : (app.viewport && app.viewport.layout) || 'single';
  document.querySelectorAll('[data-lay]').forEach(b => {
    const off = !has('viewport.setLayout'); b.setAttribute('aria-pressed', String(b.dataset.lay === lay)); b.classList.toggle('on', b.dataset.lay === lay);
    if (off) { b.setAttribute('aria-disabled', 'true'); b.title = (b.dataset.lay === 'quad' ? 'Four views' : 'Single view') + ': not available yet'; } else b.removeAttribute('aria-disabled');
  });
  const q = $('qChipT'); if (q) { const fm = app.perf && app.perf.renderMs; q.textContent = (app.getQuality ? app.getQuality() : '') .replace(/^./, c => c.toUpperCase()) + (fm ? ' · ' + fm.toFixed(0) + ' ms' : ''); }
}

// ---------- theme, density, units ----------
export function setTheme(t) { ui.theme = t === 'light' ? 'light' : 'dark'; root.dataset.theme = ui.theme; saveUi(); app.emit('theme', { theme: ui.theme }); app.requestRender(); }
export function setDensity(d) { ui.density = d === 'comfortable' ? 'comfortable' : 'compact'; root.dataset.density = ui.density; saveUi(); layoutChanged(); }
export function setUnits(u) { ui.units = u === 'imperial' ? 'imperial' : 'metric'; saveUi(); app.emit('units', { units: ui.units }); app.refreshInfo && app.refreshInfo(); }
export function openSettings() {
  const d = document.createElement('div'); d.className = 'dlg'; d.setAttribute('role', 'dialog'); d.setAttribute('aria-modal', 'true'); d.setAttribute('aria-label', 'Settings');
  const row = (label, key, opts) => `<div class="row"><span>${label}</span><div class="seg" role="group" aria-label="${label}">${opts.map(([v, t]) => `<button class="segb" type="button" data-k="${key}" data-v="${v}" aria-pressed="${ui[key] === v}">${t}</button>`).join('')}</div></div>`;
  d.innerHTML = `<div class="dlg-card" style="width:min(380px,94vw)"><div class="dlg-head"><h2>Settings</h2><button class="ibtn" type="button" data-x aria-label="Close">${icon('close')}</button></div><div class="settings">${row('Theme', 'theme', [['dark', 'Dark'], ['light', 'Light']])}${row('Density', 'density', [['compact', 'Compact'], ['comfortable', 'Comfortable']])}${row('Units', 'units', [['metric', 'Metric'], ['imperial', 'Imperial']])}<p class="note">Units change how the inspector shows lengths and areas. The data stays metric.</p></div></div>`;
  document.body.append(d);
  const close = () => { d.remove(); $('settingsBtn') && $('settingsBtn').focus(); };
  d.addEventListener('click', e => { if (e.target === d || e.target.closest('[data-x]')) close(); const b = e.target.closest('[data-k]'); if (!b) return; ({ theme: setTheme, density: setDensity, units: setUnits })[b.dataset.k](b.dataset.v); d.querySelectorAll(`[data-k="${b.dataset.k}"]`).forEach(x => x.setAttribute('aria-pressed', String(x === b))); });
  d.addEventListener('keydown', e => { if (e.key === 'Escape') { e.stopPropagation(); close(); } });
  const f = d.querySelector('button[aria-pressed="true"]'); f && f.focus();
}

// ---------- the keyboard side of the rail and the tabs ----------
function tabKeys(container, sel, vertical) {
  container.addEventListener('keydown', e => {
    const bs = [...container.querySelectorAll(sel)], i = bs.indexOf(document.activeElement); if (i < 0) return;
    const nx = vertical ? 'ArrowDown' : 'ArrowRight', pv = vertical ? 'ArrowUp' : 'ArrowLeft';
    let j = -1; if (e.key === nx) j = (i + 1) % bs.length; else if (e.key === pv) j = (i - 1 + bs.length) % bs.length; else if (e.key === 'Home') j = 0; else if (e.key === 'End') j = bs.length - 1;
    if (j < 0) return; e.preventDefault(); bs[j].focus(); bs[j].click();
  });
}

export function initDashboard() {
  root.dataset.theme = ui.theme; root.dataset.density = ui.density;
  // rail
  const rail = $('rail');
  rail.innerHTML = RAIL.map(([id, t, ic]) => `<button class="rb" type="button" role="tab" id="rail-${id}" data-pane="${id}" aria-selected="false" aria-controls="pane-${id}" title="${t}" aria-label="${t}" tabindex="-1">${icon(ic)}<span>${t.split(' ')[0]}</span></button>`).join('') + '<span class="grow"></span>';
  rail.querySelectorAll('.rb').forEach(b => { railEls[b.dataset.pane] = b; b.addEventListener('click', () => { if (ui.side === b.dataset.pane) { ui.lastSide = ui.side; closeSide(); } else openSide(b.dataset.pane); }); });
  tabKeys(rail, '.rb', true);
  const mqN = matchMedia('(max-width:640px)'); const setOrient = () => rail.setAttribute('aria-orientation', mqN.matches ? 'horizontal' : 'vertical'); setOrient(); mqN.addEventListener('change', () => { setOrient(); applySizes(); });
  matchMedia('(max-width:900px)').addEventListener('change', () => { applySizes(); notifyResize(); });
  document.querySelectorAll('[data-close-side]').forEach(b => { b.innerHTML = icon('chevronLeft'); b.onclick = () => { ui.lastSide = ui.side; closeSide(); }; });
  document.querySelectorAll('[data-close-insp]').forEach(b => { b.innerHTML = icon('chevronRight'); b.onclick = () => closeInspector(); });
  $('insClear').innerHTML = icon('close'); $('insClear').onclick = () => { app.clearSelection && app.clearSelection(); };
  $('insClear').title = 'Clear the selection';
  // bottom tabs
  const tb = $('btTabs');
  tb.innerHTML = TABS.map(([id, t]) => `<button class="tab" type="button" role="tab" id="tab-${id}" data-tab="${id}" aria-selected="false" aria-controls="tp-${id}" tabindex="-1">${t}</button>`).join('');
  tb.querySelectorAll('.tab').forEach(b => { tabEls[b.dataset.tab] = b; b.addEventListener('click', () => { if (ui.botOpen && ui.botTab === b.dataset.tab && b.dataset.clicked) closeBottom(); else openTab(b.dataset.tab); }); });
  tb.addEventListener('pointerdown', e => { const b = e.target.closest('.tab'); tb.querySelectorAll('.tab').forEach(x => delete x.dataset.clicked); if (b && ui.botOpen && ui.botTab === b.dataset.tab) b.dataset.clicked = '1'; });
  tabKeys(tb, '.tab', false);
  $('btActions').innerHTML = `<button class="ibtn" type="button" id="btMax" aria-pressed="false"></button><button class="ibtn" type="button" id="btClose"></button>`;
  $('btMax').onclick = () => maximiseBottom(); $('btClose').onclick = () => (ui.botOpen ? closeBottom() : openTab(ui.botTab));
  syncBotButtons();
  // resizers
  $('rzSide').dataset.dir = '+'; $('rzInsp').dataset.dir = '-';
  dragger($('rzSide'), 'x', () => ui.sideW, v => { ui.sideW = v; });
  dragger($('rzInsp'), 'x', () => ui.inspW, v => { ui.inspW = v; });
  dragger($('rzBot'), 'y', () => ui.botH, v => { ui.botH = v; });
  buildTopRight();
  // initial state
  if (isNarrow() || (matchMedia('(max-width:900px)').matches && !ui.side)) { ui.side = null; ui.insp = false; ui.botOpen = false; }
  const startSide = ui.side && RAIL.some(r => r[0] === ui.side) ? ui.side : (isNarrow() ? null : 'explore');
  if (startSide) { ui.side = null; openSide(startSide); } else { ui.side = null; applySizes(); }
  if (!ui.insp || isNarrow()) ui.insp = false;
  if (ui.botOpen && !isNarrow()) openTab(TABS.some(t => t[0] === ui.botTab) ? ui.botTab : 'console'); else { ui.botOpen = false; const t = TABS.some(t => t[0] === ui.botTab) ? ui.botTab : 'console'; ui.botTab = t; document.querySelectorAll('#btBody .tp').forEach(p => p.classList.toggle('on', p.dataset.tab === t)); for (const [k, b] of Object.entries(tabEls)) { b.setAttribute('aria-selected', String(k === t)); b.tabIndex = k === t ? 0 : -1; } }
  applySizes();
  addEventListener('resize', () => { applySizes(); });
  // events
  app.on('mode', syncTop); app.on('quality', syncTop); app.on('layout', syncTop); app.on('viewport', syncTop); app.on('camera-mode', syncTop);
  app.on('select', e => { if (e && e.id && !isNarrow() && !ui.inspClosed && !ui.insp) openInspector({ auto: true }); else if (e && e.id && isNarrow()) openInspector({ auto: true }); });
  addEventListener('keydown', e => { if (e.key === 'Escape' && !e.defaultPrevented) { if (ui.botMax) { maximiseBottom(false); } } });
  Object.assign(app, { shell: { openSide, closeSide, toggleSide, openInspector, closeInspector, toggleInspector, openTab, closeBottom, toggleBottom, maximiseBottom, state, onTab, onSide, setBadge, setTheme, setDensity, setUnits, openSettings, notifyResize, RAIL, TABS, syncTop } });
  setTimeout(notifyResize, 0);
  return app.shell;
}
export function state() {
  return { side: ui.side, inspector: !!ui.insp, bottom: ui.botOpen ? ui.botTab : null, bottomMaximised: !!ui.botMax, theme: ui.theme, density: ui.density, units: ui.units,
    sideWidth: ui.sideW, inspectorWidth: ui.inspW, bottomHeight: ui.botH, stage: { w: $('stage').clientWidth, h: $('stage').clientHeight } };
}
