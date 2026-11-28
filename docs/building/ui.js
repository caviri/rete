// The controls of the house: classes, storeys, layers, look, states, search, ready queries; selection, picking and
// what each of them does to the scene. The shell around them is in dashboard.js; the inspector in inspector.js; the
// result table and the console in console.js.
import * as THREE from 'three';
import { app } from './app.js';
import { CLASS_STYLE, CLASS_ORDER, pretty, colOf } from './classes.js';
import { QNS, makeExamples, runSparql, toResult, floorOf } from './queries.js';
import { renderInspector, bindInspector } from './inspector.js';

const SELECT_COLOR = 0xffd54a, ROOM_COLOR = 0x4ade80, ELEM_COLOR = 0xff9f43, RESULT_COLOR = 0x33d6ff, CHECK_COLOR = 0xffb454;
const $ = id => document.getElementById(id);
const esc = s => String(s ?? '').replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/"/g, '&quot;');

export const view = { selected: null, result: null, resultVisible: true, checked: new Set(), dim: 0.08 };
const classOn = {}, storeyOn = [], classCb = {}, storeyCb = [];
let roofOn = true, labelsOn = true;
const isRoof = new Map();      // guid -> bool
const infoProviders = [];
const searchProviders = [];   // fn(query) -> [{label, tag, run}]: other things the search list can find
const layers = new Map();
const pickables = new Map();   // Object3D -> {id, meta}
let EXAMPLES = [];
const GROUPS = [];             // [{title, from, to}]
let lastQuery = null;

const nodeAt = i => app.graph.nodes[i];
const objOf = n => app.meshByGuid.get(n.n);

// ---------- public registrations ----------
export function registerInfo(fn) { infoProviders.push(fn); return () => { const i = infoProviders.indexOf(fn); if (i >= 0) infoProviders.splice(i, 1); }; }
export function registerSearch(fn) { searchProviders.push(fn); return () => { const i = searchProviders.indexOf(fn); if (i >= 0) searchProviders.splice(i, 1); }; }
export function registerPickable(obj, id, meta = {}) {
  pickables.set(obj, { id, meta });
  obj.userData.pickId = id;
  return () => { pickables.delete(obj); delete obj.userData.pickId; };
}
export function registerLayer({ id, label, color = '#8895aa', group, count = null, visible = true } = {}) {
  if (!id || !label) throw new Error('registerLayer needs id and label');
  if (layers.has(id)) layers.get(id).el.remove();
  if (group && !group.parent) app.house.add(group);
  if (group) group.visible = visible;
  const el = document.createElement('label'); el.className = 'sys';
  el.innerHTML = `<input type="checkbox" ${visible ? 'checked' : ''}><span class="swatch" style="background:${esc(color)}"></span>${esc(label)}<span class="m"></span>`;
  const m = el.querySelector('.m'), cb = el.querySelector('input');
  const setCount = c => { m.textContent = c == null ? '' : c; };
  setCount(count);
  const h = {
    id, el, group, setCount,
    setVisible(v) { cb.checked = !!v; if (group) group.visible = !!v; app.requestRender(); app.emit('layer', { id, visible: !!v }); },
    isVisible: () => cb.checked,
    remove() { el.remove(); layers.delete(id); },
  };
  cb.onchange = () => h.setVisible(cb.checked);
  $('layers').appendChild(el); layers.set(id, h);
  return h;
}

// ---------- visibility ----------
function visibleNode(n) {
  const cOn = classOn[n.c] !== false;
  const sOn = n.st < 0 ? true : storeyOn[n.st] !== false;
  return cOn && sOn && (roofOn || !isRoof.get(n.n));
}
function decor() { app.updateRoomDecor({ slabsOn: classOn.IfcSlab !== false, storeyOn, labelsOn }); }
export function applyVisibility() {
  app.meshByGuid.forEach((o, guid) => { const n = app.nodeByGuid.get(guid); o.visible = n ? visibleNode(n) : true; });
  decor(); app.requestRender();
  app.emit('visibility', visibility());
}
export function visibility() {
  return { classes: CLASS_ORDER.filter(c => classOn[c] !== false), storeys: app.graph.storeys.map((s, i) => i).filter(i => storeyOn[i] !== false), roof: roofOn, roomNames: labelsOn };
}
export function setVisibility({ classes, storeys, roof, roomNames } = {}) {
  if (Array.isArray(classes)) {
    const want = new Set(classes.map(c => normClass(c)));
    CLASS_ORDER.forEach(c => { classOn[c] = want.has(c); if (classCb[c]) classCb[c].checked = classOn[c]; });
  }
  if (Array.isArray(storeys)) { const w = new Set(storeys.map(Number)); app.graph.storeys.forEach((s, i) => { storeyOn[i] = w.has(i); if (storeyCb[i]) storeyCb[i].checked = storeyOn[i]; }); }
  if (typeof roof === 'boolean') { roofOn = roof; $('roofCb').checked = roof; }
  if (typeof roomNames === 'boolean') { labelsOn = roomNames; $('labelsCb').checked = roomNames; }
  applyVisibility();
  return visibility();
}
export function normClass(c) {
  const s = String(c).toLowerCase().replace(/^ifc/, '');
  const hit = CLASS_ORDER.find(k => k.toLowerCase().replace(/^ifc/, '') === s || (CLASS_STYLE[k].n || '').toLowerCase() === s || k.toLowerCase().replace(/^ifc/, '').replace(/standardcase$/, '') === s || (CLASS_STYLE[k].n || '').toLowerCase().startsWith(s) && s.length > 2);
  if (!hit) throw new Error(`Unknown class "${c}". Classes: ${CLASS_ORDER.map(k => k.replace(/^Ifc/, '')).join(', ')}.`);
  return hit;
}
function ensureVisible(idxs) {
  let changed = false;
  for (const i of idxs) {
    const n = nodeAt(i);
    if (classOn[n.c] === false) { classOn[n.c] = true; if (classCb[n.c]) classCb[n.c].checked = true; changed = true; }
    if (n.st >= 0 && storeyOn[n.st] === false) { storeyOn[n.st] = true; if (storeyCb[n.st]) storeyCb[n.st].checked = true; changed = true; }
    if (!roofOn && isRoof.get(n.n)) { roofOn = true; $('roofCb').checked = true; changed = true; }
  }
  if (changed) applyVisibility();
}
// all / none for a group of toggles (the Layers menu)
export function setAllClasses(on) { CLASS_ORDER.forEach(c => { if (classCount(c)) { classOn[c] = on; if (classCb[c]) classCb[c].checked = on; } }); applyVisibility(); }
export function setAllStoreys(on) { app.graph.storeys.forEach((s, i) => { storeyOn[i] = on; if (storeyCb[i]) storeyCb[i].checked = on; }); applyVisibility(); }
export function setClassVisible(c, on) { classOn[c] = on; if (classCb[c]) classCb[c].checked = on; applyVisibility(); }
export function setStoreyVisible(i, on) { storeyOn[i] = on; if (storeyCb[i]) storeyCb[i].checked = on; applyVisibility(); }
export const classCountOf = c => classCount(c);

// ---------- panels ----------
const classCount = c => app.graph.nodes.filter(n => n.c === c).length;
function buildClassToggles() {
  const box = $('classes'); box.innerHTML = '';
  CLASS_ORDER.forEach(c => {
    const cnt = classCount(c); if (!cnt) return;
    const el = document.createElement('label'); el.className = 'sys';
    el.innerHTML = `<input type="checkbox" ${classOn[c] ? 'checked' : ''}><span class="swatch" style="background:${colOf(c)}"></span>${pretty(c)}<span class="m">${cnt}</span>`;
    el.querySelector('input').onchange = e => { classOn[c] = e.target.checked; applyVisibility(); };
    classCb[c] = el.querySelector('input'); box.appendChild(el);
  });
}
function buildStoreyToggles() {
  const box = $('storeys'); box.innerHTML = '';
  app.graph.storeys.forEach((s, i) => {
    const el = document.createElement('label'); el.className = 'sys';
    el.innerHTML = `<input type="checkbox" checked><span class="swatch" style="background:#46577a"></span>${esc(s.l)}<span class="m">${(+s.el).toFixed(2)} m</span>`;
    el.querySelector('input').onchange = e => { storeyOn[i] = e.target.checked; applyVisibility(); };
    storeyCb[i] = el.querySelector('input'); box.appendChild(el);
  });
}
function buildLook() {
  const seg = (id, items, cur, fn) => {
    const box = $(id); box.innerHTML = '';
    items.forEach(([v, l]) => { const b = document.createElement('button'); b.type = 'button'; b.className = 'segb' + (v === cur ? ' on' : ''); b.setAttribute('aria-pressed', String(v === cur)); b.textContent = l; b.dataset.v = v; b.onclick = () => fn(v); box.appendChild(b); });
  };
  const mark = (id, v) => $(id).querySelectorAll('.segb').forEach(b => { b.classList.toggle('on', b.dataset.v === v); b.setAttribute('aria-pressed', String(b.dataset.v === v)); });
  seg('colourSeg', [['realistic', 'Realistic'], ['class', 'By class']], app.getColourMode(), v => app.setColourMode(v));
  app.on('colourmode', e => mark('colourSeg', e.mode));
  seg('qualitySeg', [['high', 'High'], ['medium', 'Medium'], ['low', 'Low']], app.getQuality(), v => app.setQuality(v));
  app.on('quality', e => mark('qualitySeg', e.quality));
  const sun = app.sun, t = $('tod');
  const yr = () => app.sun.get().date.slice(0, 4);
  const showSun = () => {
    const g = sun.get();
    $('todv').textContent = g.timeStr; t.value = g.time; $('sunDate').value = g.date;
    $('sunRead').innerHTML = `Sunrise <b>${g.sunriseStr}</b> · Sunset <b>${g.sunsetStr}</b><br>Azimuth <b>${g.azimuth.toFixed(1)}°</b> · Elevation <b>${g.elevation.toFixed(1)}°</b>${g.isDay ? '' : ' · night'}`;
  };
  t.oninput = () => { sun.playDay(false); sun.set({ time: +t.value }); };
  $('sunDate').onchange = e => { if (e.target.value) sun.set({ date: e.target.value }); };
  document.querySelectorAll('#seasonPresets button').forEach(b => b.onclick = () => sun.set({ date: yr() + '-' + b.dataset.d }));
  document.querySelectorAll('#todPresets button[data-k]').forEach(b => b.onclick = () => {
    sun.playDay(false); const g = sun.get(), k = b.dataset.k;
    sun.set({ time: k === 'sunrise' ? (g.sunrise ?? 6) + 0.1 : k === 'noon' ? (g.solarNoon ?? 12) : k === 'sunset' ? (g.sunset ?? 18) - 0.25 : 23 });
  });
  $('playBtn').onclick = () => sun.playDay(!sun.isPlaying());
  app.on('play', e => { $('playBtn').textContent = e.playing ? 'Stop' : 'Play day'; });
  $('pathCb').onchange = e => sun.setPathVisible(e.target.checked);
  app.on('sunpath', e => { $('pathCb').checked = e.visible; });
  const siteIn = () => { const s = sun.site; $('siteLat').value = s.latitude; $('siteLon').value = s.longitude; $('siteTz').value = s.timezone; $('siteNorth').value = +s.northAzimuthDeg.toFixed(2); $('siteName').textContent = s.name || ''; };
  const siteSet = () => { try { sun.setSite({ latitude: $('siteLat').value, longitude: $('siteLon').value, timezone: $('siteTz').value, northAzimuthDeg: $('siteNorth').value }); $('siteMsg').textContent = ''; } catch (e) { $('siteMsg').textContent = e.message; } };
  ['siteLat', 'siteLon', 'siteTz', 'siteNorth'].forEach(id => { $(id).onchange = siteSet; });
  app.on('site', () => { siteIn(); showSun(); });
  app.on('time', showSun); siteIn(); showSun();
  const lc = $('lightsCb'); lc.checked = app.getInteriorLights(); lc.onchange = () => app.setInteriorLights(lc.checked);
  app.on('lights', e => { lc.checked = e.on; });
  $('roofCb').onchange = e => { roofOn = e.target.checked; applyVisibility(); };
  $('labelsCb').onchange = e => { labelsOn = e.target.checked; decor(); };
  app.on('mode', () => decor());
}

// ---------- search list and states ----------
const stateBadge = n => (n.c === 'IfcDoor' || n.c === 'IfcWindow') && app.getState(n.n) ? `<span class="stb" data-id="${n.n}">${esc(app.getState(n.n))}</span>` : '';
function buildList(q) {
  const ul = $('list'); ul.innerHTML = '';
  const pool = view.result ? view.result.idxs.map(nodeAt) : app.graph.nodes;
  let shown = 0;
  for (const n of pool) {
    if (q && !n.l.toLowerCase().includes(q)) continue;
    if (++shown > 400) { const li = document.createElement('li'); li.className = 'muted'; li.textContent = '… refine the search to see more'; ul.appendChild(li); break; }
    const li = document.createElement('li'); li.tabIndex = -1; li.setAttribute('role', 'option');
    li.innerHTML = `<span>${esc(n.l)}${stateBadge(n)}</span><span class="m">${pretty(n.c)}</span>`;
    li.onclick = () => select(n);
    ul.appendChild(li);
  }
  let extra = 0;
  if (q && !view.result) for (const fn of searchProviders) { try { for (const it of fn(q).slice(0, 80)) {
    const li = document.createElement('li'); li.tabIndex = -1; li.setAttribute('role', 'option'); li.innerHTML = `<span>${esc(it.label)}</span><span class="m">${esc(it.tag || '')}</span>`; li.onclick = it.run; ul.appendChild(li); extra++; } } catch (e) { console.error('[building] search provider failed', e); } }
  const first = ul.querySelector('li[role=option]'); if (first) first.tabIndex = 0;
  $('count').textContent = view.result ? `(result: ${view.result.idxs.length})` : `(${app.graph.nodes.length}${extra ? ' + ' + extra : ''})`;
}
function listKeys() {
  const ul = $('list');
  ul.addEventListener('keydown', e => {
    const items = [...ul.querySelectorAll('li[role=option]')], i = items.indexOf(document.activeElement); if (i < 0) return;
    let j = -1; if (e.key === 'ArrowDown') j = Math.min(items.length - 1, i + 1); else if (e.key === 'ArrowUp') j = Math.max(0, i - 1); else if (e.key === 'Home') j = 0; else if (e.key === 'End') j = items.length - 1;
    if (j >= 0) { e.preventDefault(); items[i].tabIndex = -1; items[j].tabIndex = 0; items[j].focus(); } else if (e.key === 'Enter') { e.preventDefault(); items[i].click(); }
  });
  $('search').addEventListener('keydown', e => { if (e.key === 'ArrowDown') { const f = ul.querySelector('li[role=option]'); if (f) { e.preventDefault(); f.focus(); } } });
}
function buildStates() {
  const box = $('statesList'); box.innerHTML = '';
  const items = app.stateList().filter(s => s.kind === 'door' || s.kind === 'window');
  items.sort((a, b) => a.kind.localeCompare(b.kind) || a.label.localeCompare(b.label));
  items.forEach(s => {
    const row = document.createElement('div'); row.className = 'strow'; row.dataset.id = s.id;
    row.innerHTML = `<button class="lnk stl" type="button">${esc(s.label)}</button><button class="btn sm stb" type="button" ${s.values.length > 1 ? 'title="Change the state"' : 'disabled title="Fixed: it cannot be opened"'}>${esc(s.state)}</button>`;
    row.querySelector('.stl').onclick = () => select(s.id);
    row.querySelector('.stb').onclick = () => app.toggleState(s.id);
    box.appendChild(row);
  });
  $('statesCount').textContent = `(${items.length})`;
  const all = st => () => app.stateList().filter(s => s.kind === 'door' || s.kind === 'window').forEach(s => { if (s.values.includes(st)) app.setState(s.id, st); });
  $('openAll').onclick = all('open'); $('closeAll').onclick = all('closed');
}
function onState(e) {
  document.querySelectorAll(`.stb[data-id="${e.id}"]`).forEach(b => { b.textContent = e.state; });
  const row = document.querySelector(`.strow[data-id="${e.id}"] .stb`); if (row) row.textContent = e.state;
  if (view.selected && (view.selected.n === e.id || (view.selected.pick && view.selected.id === e.id))) renderInfo(view.selected);
}

// ---------- selection and painting ----------
const isSpace = i => nodeAt(i).k === 'Space';
const paintIdx = (i, color, opacity, emis) => { const o = objOf(nodeAt(i)); if (o) app.paint(o, { color, opacity, emissive: emis }); };
function relatedOf(n) {
  if (n.k === 'Space') return { rooms: n.adj || [], elems: app.boundedBy[n.i] || [] };
  return { rooms: n.bd || [], elems: [] };
}
export function restyle() {
  const sel = view.selected && view.selected.n ? view.selected : null;
  const active = !!(view.result || sel || view.checked.size);
  app.meshByGuid.forEach((o, guid) => {
    const n = app.nodeByGuid.get(guid); const space = n && n.k === 'Space';
    app.paint(o, active ? { opacity: space ? Math.min(view.dim, 0.1) : view.dim } : null);
  });
  if (view.result && view.resultVisible) for (const i of view.result.idxs) paintIdx(i, RESULT_COLOR, isSpace(i) ? 0.4 : 0.95, RESULT_COLOR);
  for (const i of view.checked) paintIdx(i, CHECK_COLOR, isSpace(i) ? 0.45 : 1, CHECK_COLOR);
  if (sel) {
    if (!view.result) { const rel = relatedOf(sel); rel.rooms.forEach(i => paintIdx(i, ROOM_COLOR, 0.42, ROOM_COLOR)); rel.elems.forEach(i => paintIdx(i, ELEM_COLOR, 0.95, ELEM_COLOR)); }
    paintIdx(sel.i, SELECT_COLOR, isSpace(sel.i) ? 0.5 : 1, SELECT_COLOR);
  }
  app.requestRender();
}
function resolve(x) {
  if (x == null) return null;
  if (typeof x === 'object' && x.n && x.i != null) return x;
  const s = String(x);
  if (app.nodeByGuid.has(s)) return app.nodeByGuid.get(s);
  let by = app.graph.nodes.filter(n => n.l.toLowerCase() === s.toLowerCase());
  if (by.length > 1) by = by.filter(n => n.c !== 'IfcOpeningElement');   // a door and its opening share a label
  return by.length === 1 ? by[0] : null;
}
// select(guidOrId | node | null): an element or room by GUID (or unique label), or a registered pickable by id
export function select(x, opts = {}) {
  if (x == null) { clearSelection(); return null; }
  const n = resolve(x);
  if (n) {
    view.selected = n; renderInfo(n);
    const rel = relatedOf(n); ensureVisible([n.i, ...rel.rooms, ...rel.elems]);
    restyle(); if (opts.focus !== false && app.mode === 'orbit') app.focus(objOf(n));
    app.emit('select', { id: n.n, kind: n.k === 'Space' ? 'room' : 'element', node: n, label: n.l });
    return { id: n.n, kind: n.k === 'Space' ? 'room' : 'element', label: n.l, class: n.c };
  }
  for (const [obj, p] of pickables) if (p.id === String(x)) {
    view.selected = { pick: true, id: p.id, obj, meta: p.meta, l: p.meta.label || p.id };
    renderInfo(view.selected); restyle(); if (opts.focus !== false && app.mode === 'orbit') app.focus(obj);
    app.emit('select', { id: p.id, kind: 'pickable', object: obj, meta: p.meta, label: view.selected.l });
    return { id: p.id, kind: 'pickable', label: view.selected.l };
  }
  throw new Error('Nothing with id "' + x + '".');
}
export function clearSelection() {
  const had = !!view.selected; view.selected = null; restyle(); renderInfo(null);
  if (had) app.emit('select', { id: null, kind: null });
}
export function selectedInfo() {
  const s = view.selected; if (!s) return null;
  if (s.pick) return { id: s.id, kind: 'pickable', label: s.l };
  return { id: s.n, kind: s.k === 'Space' ? 'room' : 'element', label: s.l, class: s.c, storey: s.st >= 0 ? app.graph.storeys[s.st].l : null, state: app.getState(s.n) || null };
}

// ---------- the inspector ----------
const deps = { providers: infoProviders, relatedOf, nodeAt, floorOf, pretty, select: (x, o) => select(x, o) };
app._inspDeps = deps;
export function renderInfo(n) { renderInspector($('info'), n, deps); }

// ---------- queries ----------
function exampleText(ex) { return QNS + (ex.pre || '') + '\n' + ex.q; }
function addExampleButtons(from, title) {
  const box = $('queries');
  if (title) { const h = document.createElement('div'); h.className = 'svc-h'; h.textContent = title; box.appendChild(h); }
  for (let k = from; k < EXAMPLES.length; k++) {
    const ex = EXAMPLES[k];
    const b = document.createElement('button'); b.type = 'button'; b.className = 'scen-btn qbtn'; b.innerHTML = `${esc(ex.label)}<small>${esc(ex.desc || '')}</small>`; b.title = ex.desc || ''; b.onclick = () => runExample(k); box.appendChild(b);
  }
  GROUPS.push({ title: title || 'The house', from, to: EXAMPLES.length });
  app.exampleGroups = GROUPS; app.emit('rendered-examples', {});
}
// more ready queries after the page is built (the services group): [{label, desc, q, pre, cols, run?}]
export function addExamples(title, items) { const from = EXAMPLES.length; EXAMPLES.push(...items); addExampleButtons(from, title); return from; }
function buildQueries() {
  EXAMPLES = makeExamples(); app.examples = EXAMPLES;
  addExampleButtons(0);
  const op = $('opacity'); op.oninput = () => { view.dim = +op.value / 100; restyle(); };
  $('openConsole').onclick = () => app.shell.openTab('console');
}
async function runExample(k) {
  const ex = EXAMPLES[k];
  [...document.querySelectorAll('.qbtn')].forEach((b, j) => b.classList.toggle('on', j === k));
  if (app.console) app.console.setText(exampleText(ex));
  if (!ex.run) {   // a query that only the engine can answer (the services group)
    const msg = $('qmeta'); msg.hidden = false; msg.textContent = 'running on the graph…';
    try { const j = await runSparql(exampleText(ex), m => { msg.textContent = m; }); const r = toResult(ex.label, j); r.query = exampleText(ex); r.source = 'ready'; showResult(r, { open: true }); }
    catch (e) { msg.textContent = 'Query error: ' + (e.message || e); }
    return;
  }
  const t0 = performance.now(); const { idxs, rows } = ex.run(app.graph.nodes);
  if (app.log) app.log('info', 'query', `${ex.label}: ${rows.length} rows (from the index) · ${Math.round(performance.now() - t0)} ms`);
  showResult({ label: ex.label, cols: ex.cols, rows, idxs: [...new Set(idxs)], query: exampleText(ex), source: 'ready' }, { open: true });
}
export async function runCustom(q, { quiet } = {}) {
  if (!q.trim()) return null;
  const j = await runSparql(q);
  if (j.kind !== 'select') return j;
  showResult(Object.assign(toResult('Custom query', j), { query: q, source: 'custom' }), quiet ? {} : { open: true });
  return j;
}
export function showResult(r, opts = {}) {
  view.result = r; view.resultVisible = opts.highlight !== false; view.checked.clear(); view.selected = null;
  lastQuery = { label: r.label, rows: r.rows.length, matched: r.idxs.length };
  ensureVisible(r.idxs); restyle();
  if (app.results) app.results.show(r, opts); else app.pendingResult = [r, opts];
  buildList($('search').value.trim().toLowerCase()); renderResultLayer(); renderInfo(null);
  if (app.mode === 'orbit' && opts.frame !== false) frameSet(r.idxs);
  const vis = r.idxs.filter(i => objOf(nodeAt(i))).length, np = (r.picks || []).length;
  const meta = $('qmeta'); meta.hidden = false; meta.textContent = `${r.idxs.length + np} matched${vis < r.idxs.length ? ` · ${vis} in 3D` : ''}${np ? ` · ${np} services` : ''}`;
  app.resultPicks = r.picks || [];
  app.emit('result', { ...lastQuery, picks: app.resultPicks });
}
export const lastQueryInfo = () => lastQuery;
export function clearResult() {
  view.result = null; view.resultVisible = true; view.checked.clear(); view.selected = null; lastQuery = null; restyle();
  [...document.querySelectorAll('.qbtn')].forEach(b => b.classList.remove('on'));
  $('qmeta').hidden = true; buildList($('search').value.trim().toLowerCase()); renderResultLayer(); renderInfo(null);
  if (app.results) app.results.clear();
  app.resultPicks = []; app.emit('resultclear');
}
function renderResultLayer() {
  const el = $('resultLayer');
  if (!view.result) { el.innerHTML = ''; return; }
  el.innerHTML = `<label class="sys" style="border-top:1px solid var(--line);margin-top:8px;padding-top:6px"><input type="checkbox" id="resultToggle" ${view.resultVisible ? 'checked' : ''}>
    <span class="swatch" style="background:var(--c-res)"></span>Query result <span class="muted">(${view.result.idxs.length + (view.result.picks || []).length})</span>
    <span style="flex:1"></span><button class="btn ghost sm" type="button" id="resultClear" title="Clear the query result">Clear</button></label>`;
  $('resultToggle').onchange = e => { view.resultVisible = e.target.checked; restyle(); };
  $('resultClear').onclick = clearResult;
}
function frameSet(idxs) {
  const objs = idxs.map(i => objOf(nodeAt(i))).filter(Boolean);
  if (objs.length) app.focus(objs, { distance: undefined });
}

// ---------- picking in the scene ----------
const ray = new THREE.Raycaster(), mouse = new THREE.Vector2();
function deepVisible(o) { for (; o; o = o.parent) if (!o.visible) return false; return true; }
function pickAt(clientX, clientY) {
  const r = app.renderer.domElement.getBoundingClientRect();
  mouse.x = ((clientX - r.left) / r.width) * 2 - 1; mouse.y = -((clientY - r.top) / r.height) * 2 + 1;
  ray.setFromCamera(mouse, app.camera);
  let best = null;
  for (const h of ray.intersectObject(app.house, true)) {
    const o = h.object;
    if (o.isSprite || o.userData.storey != null || !deepVisible(o)) continue;
    if ((o.userData.alpha ?? 1) <= 0.2) continue;   // dimmed context is not picked (its look is in the batches; the proxy keeps the alpha)
    let p = o, guid = null, pid = null;
    while (p) { if (p.userData.pickId && !pid) pid = p.userData.pickId; if (p.userData.guid) { guid = p.userData.guid; break; } p = p.parent; }
    if (pid) { best = { id: pid, dist: h.distance, pick: true }; break; }
    if (guid && app.nodeByGuid.has(guid)) { best = { id: guid, dist: h.distance }; break; }
  }
  if (pickables.size) {
    const hs = ray.intersectObjects([...pickables.keys()], true).filter(h => deepVisible(h.object));
    if (hs.length && (!best || hs[0].distance < best.dist)) { let p = hs[0].object; while (p && !p.userData.pickId) p = p.parent; if (p) best = { id: p.userData.pickId, dist: hs[0].distance, pick: true }; }
  }
  return best;
}
function setupPicking() {
  const cv = app.renderer.domElement; let down = null;
  cv.addEventListener('pointerdown', e => {
    if (app.mode !== 'orbit' || e.button !== 0 || e.shiftKey || (app.measure && app.measure.active)) return;
    down = [e.clientX, e.clientY];
    const up = ev => {
      removeEventListener('pointerup', up);
      if (!down || Math.hypot(ev.clientX - down[0], ev.clientY - down[1]) > 5) return;
      const hit = pickAt(ev.clientX, ev.clientY);
      if (hit) select(hit.id);
    };
    addEventListener('pointerup', up);
  });
  cv.addEventListener('dblclick', e => {
    if (app.mode !== 'orbit') return;
    const hit = pickAt(e.clientX, e.clientY);
    if (hit && app.getState(hit.id) && ['door', 'window'].includes(app.stateList().find(s => s.id === hit.id)?.kind)) app.toggleState(hit.id);
  });
}

// ---------- boot ----------
export function initUI() {
  const g = app.graph;
  app.boundedBy = {};
  g.nodes.forEach(n => { (n.bd || []).forEach(r => { (app.boundedBy[r] = app.boundedBy[r] || []).push(n.i); }); });
  CLASS_ORDER.forEach(c => { classOn[c] = CLASS_STYLE[c].on; });
  g.storeys.forEach((s, i) => { storeyOn[i] = true; });
  g.nodes.forEach(n => {
    const o = objOf(n); if (!o) return;
    const b = new THREE.Box3().setFromObject(o);
    isRoof.set(n.n, (n.c === 'IfcSlab' && /^Dach/.test(n.l)) || ((n.c === 'IfcMember' || n.c === 'IfcBeam') && b.min.z > 2.85));
  });
  $('roofCount').textContent = [...isRoof.values()].filter(Boolean).length;
  buildClassToggles(); buildStoreyToggles(); buildLook(); buildStates(); buildList(''); buildQueries(); setupPicking(); listKeys(); bindInspector($('info'));
  $('count').textContent = `(${g.nodes.length})`;
  $('search').addEventListener('input', e => buildList(e.target.value.trim().toLowerCase()));
  $('resetView').onclick = () => { clearResult(); clearSelection(); app.frameAll(false); };
  app.on('state', onState);
  applyVisibility(); restyle();
  Object.assign(app, { select, clearSelection, registerInfo, registerPickable, registerLayer, registerSearch, addExamples, pickables, setVisibility, getVisibility: visibility,
    showResult, clearResult, selectedInfo, applyVisibility, lastQueryInfo, normClass, restyle, refreshInfo: () => renderInfo(view.selected), rebuildList: () => buildList($('search').value.trim().toLowerCase()),
    pickAt, setAllClasses, setAllStoreys, setClassVisible, setStoreyVisible, classCountOf: classCount, runCustom, uiView: view });
}
