// One log of everything that happens. The store starts with the page (it is imported by main.js), the Logs tab
// is built when the shell mounts it. Other modules call app.log(level, source, message, data?).
//   level:  'debug' | 'info' | 'warn' | 'error'
//   source: 'user' | 'query' | 'simulation' | 'scenario' | 'data' | 'perf' | 'agent' | 'app' | 'error' (any string works)
// Events of the app are subscribed here, so the modules that emit them are not touched.
import { app } from './app.js';

const CAP = 1500;
const entries = [];            // oldest first
let seq = 0, paused = false, unread = 0, visible = false;
const subs = new Set();
const SOURCES = ['user', 'query', 'simulation', 'scenario', 'data', 'perf', 'agent', 'app', 'error'];
const LEVELS = ['debug', 'info', 'warn', 'error'];

function clip(v, depth = 0) {   // keep data small and serialisable
  if (v == null || typeof v === 'number' || typeof v === 'boolean') return v;
  if (typeof v === 'string') return v.length > 300 ? v.slice(0, 300) + '…' : v;
  if (depth > 2) return '…';
  if (Array.isArray(v)) return v.slice(0, 12).map(x => clip(x, depth + 1)).concat(v.length > 12 ? ['… ' + (v.length - 12) + ' more'] : []);
  if (typeof v === 'object') { const o = {}; let n = 0; for (const k in v) { if (++n > 14) { o['…'] = 'more'; break; } try { o[k] = clip(v[k], depth + 1); } catch { o[k] = '?'; } } return o; }
  return String(v);
}
export function log(level, source, message, data) {
  const e = { id: ++seq, t: Date.now(), level: LEVELS.includes(level) ? level : 'info', source: String(source || 'app'), message: String(message == null ? '' : message).slice(0, 600) };
  if (data !== undefined) e.data = clip(data);
  entries.push(e); if (entries.length > CAP) entries.splice(0, entries.length - CAP + 100);
  if (!visible && e.level !== 'info' && e.level !== 'debug') unread++;   // the badge counts warnings and errors, not the routine entries
  for (const f of subs) { try { f(e); } catch { /* ignore */ } }
  return e;
}
export const getEntries = () => entries;
export const clearLog = () => { entries.length = 0; unread = 0; for (const f of subs) f(null); };
export const subscribe = f => { subs.add(f); return () => subs.delete(f); };
export const unreadCount = () => unread;
export const setVisible = v => { visible = v; if (v) { unread = 0; for (const f of subs) f(undefined); } };

app.log = log;
app.logs = { entries: () => entries.slice(), clear: clearLog, subscribe, SOURCES, LEVELS, get unread() { return unread; }, query(f) { return filterEntries(f); } };

export function filterEntries({ source, level, text, limit = 100 } = {}) {
  const li = level ? LEVELS.indexOf(level) : 0, tx = text ? String(text).toLowerCase() : '';
  const out = [];
  for (let i = entries.length - 1; i >= 0 && out.length < limit; i--) {
    const e = entries[i];
    if (source && (Array.isArray(source) ? !source.includes(e.source) : e.source !== source)) continue;
    if (LEVELS.indexOf(e.level) < li) continue;
    if (tx && !(e.message.toLowerCase().includes(tx) || e.source.includes(tx) || (e.data !== undefined && JSON.stringify(e.data).toLowerCase().includes(tx)))) continue;   // the text filter also looks into the data (a tool call's arguments)
    out.push(e);
  }
  return out;
}

// ---- errors ----
addEventListener('error', ev => { log('error', 'error', (ev.message || 'Script error') + (ev.filename ? ' (' + String(ev.filename).split('/').pop() + ':' + ev.lineno + ')' : ''), ev.error && ev.error.stack ? { stack: String(ev.error.stack).split('\n').slice(0, 3).join(' | ') } : undefined); });
addEventListener('unhandledrejection', ev => { const r = ev.reason; log('error', 'error', 'Unhandled rejection: ' + (r && r.message || r), r && r.stack ? { stack: String(r.stack).split('\n').slice(0, 3).join(' | ') } : undefined); });
const ce = console.error.bind(console);
console.error = (...a) => { try { log('error', 'app', a.map(x => (x && x.message) || (typeof x === 'string' ? x : JSON.stringify(clip(x)))).join(' ').slice(0, 400)); } catch { /* ignore */ } ce(...a); };

// ---- data loading: the files the page fetches, with sizes and times ----
const SEEN = new Set();
function resource(r) {
  if (SEEN.has(r.name)) return; SEEN.add(r.name);
  const n = r.name.split('?')[0];
  if (!/\.(json|rete|glb|wasm)$/.test(n) || /graphview/.test(n)) return;
  const size = r.encodedBodySize || r.transferSize || 0;
  log('info', 'data', 'Loaded ' + n.split('/').pop() + (size ? ' · ' + (size / 1024 < 1024 ? Math.round(size / 1024) + ' KB' : (size / 1048576).toFixed(1) + ' MB') : '') + ' · ' + Math.round(r.duration) + ' ms', { url: n });
}
try {
  performance.getEntriesByType('resource').forEach(resource);
  new PerformanceObserver(l => l.getEntries().forEach(resource)).observe({ type: 'resource', buffered: true });
} catch { /* not supported */ }

// ---- the app's events ----
let stBuf = [], stT = 0, tmT = 0;
function onState(e) {
  stBuf.push(e); clearTimeout(stT);
  stT = setTimeout(() => {
    const b = stBuf; stBuf = [];
    if (b.length === 1) log('info', app.agentBusy ? 'agent' : 'user', `${b[0].label || b[0].id}: ${b[0].prev || '?'} → ${b[0].state}`, { id: b[0].id, kind: b[0].kind });
    else log('info', app.agentBusy ? 'agent' : 'user', `${b.length} states changed`, b.slice(0, 6).map(x => `${x.label || x.id} → ${x.state}`));
  }, 120);
}
export function subscribeApp() {
  const on = (ev, f) => app.on(ev, f);
  on('state', onState);
  on('select', e => { if (e && e.id) log('debug', 'user', 'Selected ' + (e.label || e.id), { kind: e.kind }); });
  on('mode', e => log('info', 'user', 'Camera mode: ' + e.mode));
  on('quality', e => log('info', 'perf', 'Quality set to ' + e.quality));
  on('time', () => { clearTimeout(tmT); tmT = setTimeout(() => { const g = app.sun && app.sun.get && app.sun.get(); if (g) log('debug', 'user', `Sun: ${g.date} ${g.timeStr}, elevation ${g.elevation.toFixed(0)}°`); }, 500); });
  on('site', () => { const s = app.sun.site; log('info', 'user', `Site: ${s.latitude}, ${s.longitude}, north ${(+s.northAzimuthDeg).toFixed(0)}°`); });
  on('colourmode', e => log('info', 'user', 'Colours: ' + e.mode));
  on('layer', e => log('debug', 'user', `Layer ${e.id}: ${e.visible ? 'shown' : 'hidden'}`));
  on('xray', e => log('info', 'user', 'X-ray ' + (e.on ? 'on' : 'off')));
  on('isolate', e => log('info', 'user', 'Isolate: ' + (e.system || 'none')));
  on('trace', t => log('info', 'user', t ? 'Trace: ' + t.labels.join(' → ') : 'Trace cleared', t ? { runs: t.runs.length } : undefined));
  on('scenario', r => log('info', 'scenario', r ? `Applied "${r.label}": ${r.off.length} off, ${r.affected.length} affected` : 'Scenario cleared', r ? { same: r.same } : undefined));
  on('thermal', r => { if (r && r.rows) { const hi = r.rows.reduce((a, b) => b.maxC > a.maxC ? b : a); log('info', 'simulation', `Thermal day ${r.date}: warmest ${hi.room} ${hi.maxC.toFixed(1)} °C, solar ${r.totals.solarKWh.toFixed(1)} kWh, heating ${r.totals.heatingKWh.toFixed(1)} kWh, ${r.ms} ms`); } });
  on('thermalview', e => log('debug', 'user', `Thermal view: temperatures ${e.temps ? 'on' : 'off'}, window sun ${e.sun ? 'on' : 'off'}`));
  on('play', e => log('debug', 'user', 'Play day ' + (e.playing ? 'started' : 'stopped')));
  on('lights', e => log('debug', 'user', 'Interior lights ' + (e.on ? 'on' : 'off')));
  on('visibility', () => log('debug', 'user', 'Visibility changed'));
  on('interactive', () => log('info', 'data', 'The house is on screen', { ms: Math.round(performance.now()) }));
  on('ready', e => log('info', 'data', 'Ready' + (e && e.services ? ' (with the services layer)' : ' (house only)'), { ms: Math.round(performance.now()) }));
  on('layout', e => log('info', 'user', 'Layout: ' + (e.layout || '') + (e.maximised ? ', ' + e.maximised + ' maximised' : '')));
  on('section', e => { const on = ['x', 'y', 'z'].filter(a => e && e[a] && e[a].enabled).map(a => a.toUpperCase() + ' ' + (+e[a].position).toFixed(2)); log('info', 'user', 'Section cuts: ' + (on.join(', ') || 'off')); });
  on('cutaway', e => log('info', 'user', 'Ground: ' + (e.mode || e)));
  on('structure', e => log('info', 'user', e && e.removed && e.removed.length ? `Structure: ${e.removed.length} removed, ${(e.unsupported || []).length} lost their support` : 'Structure: all intact'));
  on('structurecolour', e => log('info', 'user', 'Structure colours: ' + e.mode));
  on('colour', e => log('info', 'user', 'Colour by ' + (e && e.variable || 'nothing')));
  on('value', e => { if (e && e.source !== 'simulation') log('info', 'user', `${e.variable} of ${e.thing}: ${e.prev} → ${e.value}${e.pinned ? ' (pinned)' : ''}`); });
  on('reset', e => log('info', 'user', 'Reset ' + (e && e.scope || ''), e));
  on('measure', e => log('debug', 'user', 'Measure ' + (e.active ? 'on' : 'off')));
  // slow frames, sampled
  let lastCheck = 0, warned = 0;
  on('render', () => { const now = performance.now(); if (now - lastCheck < 3000) return; lastCheck = now; const f = app.perf && app.perf.frameMs; if (f > 80 && now - warned > 15000) { warned = now; log('warn', 'perf', `Slow frames: ${Math.round(f)} ms per frame`, { drawCalls: app.renderer.info.render.calls }); } });
}

// ---- the tab ----
const fmtT = t => { const d = new Date(t); return d.toTimeString().slice(0, 8) + '.' + String(d.getMilliseconds()).padStart(3, '0'); };
const esc = s => String(s ?? '').replace(/&/g, '&amp;').replace(/</g, '&lt;');
export function mountLogs(root) {
  root.innerHTML = `<div class="tp-bar">
      <select class="sel" id="lgSrc" aria-label="Source"><option value="">All sources</option>${SOURCES.map(s => `<option>${s}</option>`).join('')}</select>
      <select class="sel" id="lgLvl" aria-label="Level"><option value="">All levels</option><option value="info">Info and up</option><option value="warn">Warnings and errors</option><option value="error">Errors</option></select>
      <input class="inp" id="lgTxt" type="search" placeholder="Filter text" aria-label="Filter text" style="width:170px">
      <span class="grow"></span><span class="muted" id="lgCount"></span>
      <button class="btn sm" type="button" id="lgPause" aria-pressed="false">Pause</button>
      <button class="btn sm" type="button" id="lgCopy">Copy JSON</button><button class="btn sm" type="button" id="lgSave">Save JSON</button><button class="btn sm" type="button" id="lgClear">Clear</button></div>
    <div class="tp-scroll"><ul class="log" id="lgList" aria-label="Log, newest first"></ul></div>`;
  const $ = s => root.querySelector(s); const list = $('#lgList');
  const state = { source: '', level: '', text: '' };
  let raf = 0, frozenAt = null;
  const view = () => { const f = { limit: 400 }; if (state.source) f.source = state.source; if (state.level) f.level = state.level; if (state.text) f.text = state.text; let r = filterEntries(f); if (frozenAt != null) r = r.filter(e => e.id <= frozenAt); return r; };
  const draw = () => {
    raf = 0; const r = view();
    list.innerHTML = r.map(e => `<li class="${e.level}" data-id="${e.id}"><span class="t">${fmtT(e.t)}</span><span class="src">${esc(e.source)}</span><span class="lv">${e.level}</span><span class="msg">${esc(e.message)}${e.data !== undefined ? `<span class="dat">${esc(JSON.stringify(e.data)).slice(0, 260)}</span>` : ''}</span></li>`).join('') || '<li><span class="msg muted">Nothing logged yet.</span></li>';
    $('#lgCount').textContent = `${r.length} shown of ${entries.length}`;
  };
  const sched = () => { if (!raf && visible) raf = requestAnimationFrame(draw); };
  subscribe(e => { if (e === undefined || e === null) { draw(); return; } if (!paused) sched(); });
  $('#lgSrc').onchange = e => { state.source = e.target.value; draw(); };
  $('#lgLvl').onchange = e => { state.level = e.target.value; draw(); };
  $('#lgTxt').oninput = e => { state.text = e.target.value; draw(); };
  $('#lgPause').onclick = e => { paused = !paused; frozenAt = paused ? seq : null; e.target.setAttribute('aria-pressed', String(paused)); e.target.textContent = paused ? 'Resume' : 'Pause'; draw(); };
  $('#lgClear').onclick = () => clearLog();
  const json = () => JSON.stringify(view().slice().reverse(), null, 1);
  $('#lgCopy').onclick = async () => { try { await navigator.clipboard.writeText(json()); app.notify && app.notify('Log copied as JSON'); } catch { app.notify && app.notify('Could not copy'); } };
  $('#lgSave').onclick = () => { const a = document.createElement('a'); a.href = URL.createObjectURL(new Blob([json()], { type: 'application/json' })); a.download = 'fzk-haus-log.json'; a.click(); setTimeout(() => URL.revokeObjectURL(a.href), 2000); };
  // filter from outside (the status bar's WebMCP indicator)
  return { draw, setFilter(f) { Object.assign(state, f); $('#lgSrc').value = state.source || ''; $('#lgLvl').value = state.level || ''; $('#lgTxt').value = state.text || ''; draw(); } };
}
