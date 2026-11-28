// The status bar and app.notify(message, {ms}) (transient notices, in place of toasts).
import { app } from './app.js';
import { $, esc, has, ui } from './shared.js';
import { icon } from './icons.js';

const MON = ['Jan', 'Feb', 'Mar', 'Apr', 'May', 'Jun', 'Jul', 'Aug', 'Sep', 'Oct', 'Nov', 'Dec'];
let msgT = 0, pendingMsg = null;
app.notify = (msg, opts = {}) => {
  const el = $('stMsg'); if (!el) { pendingMsg = msg; return; }
  el.textContent = msg || ''; clearTimeout(msgT);
  if (opts.action) { const b = document.createElement('button'); b.type = 'button'; b.className = 'lnk'; b.style.marginLeft = '10px'; b.textContent = opts.action.label; b.onclick = () => { opts.action.run(); }; el.append(b); }
  if (msg) msgT = setTimeout(() => { el.textContent = ''; }, opts.ms || 4500);
  if (msg && app.log && !opts.quiet && msg.length > 3 && !/…\s+\(/.test(msg)) app.log('debug', 'app', msg);
};

export function initStatusbar() {
  const bar = $('statusbar');
  bar.innerHTML = `<span class="st" id="stHover" title="Hovered element, or the room while walking"></span>
    <button class="st sepl" type="button" id="stSel" title="Selection: click to open the inspector"></button>
    <span class="st grow sepl" id="stMsg" aria-live="polite"></span>
    <button class="st sepl opt-sm" type="button" id="stMode" title="Camera mode and layout"></button>
    <button class="st sepl" type="button" id="stSun" title="Date, time and sun elevation: click to open the sun controls"></button>
    <span id="stInd" style="display:contents"></span>
    <button class="st sepl opt-sm" type="button" id="stQual" title="Quality, last frame time and draw calls: click to change the quality"></button>
    <button class="st sepl opt-sm" type="button" id="stGraph" title="The graph the queries run on: click for the console"></button>
    <button class="st sepl opt-sm" type="button" id="stMcp" title="WebMCP: click for the agent calls in the log"></button>`;
  if (pendingMsg) app.notify(pendingMsg);
  const sh = () => app.shell;
  $('stSel').onclick = () => sh().openInspector(); $('stSun').onclick = () => sh().openSide('sun');
  $('stMode').onclick = e => { import('./actions.js').then(a => a.cameraMenu(e.currentTarget)); };
  $('stQual').onclick = () => $('qChip') && $('qChip').click();
  $('stGraph').onclick = () => sh().openTab('console');
  $('stMcp').onclick = () => { sh().openTab('logs'); app.logsUi && app.logsUi.setFilter({ source: 'agent' }); };

  const sel = () => {
    const s = app.selectedInfo && app.selectedInfo();
    $('stSel').innerHTML = s ? `${icon('eye')}Selected: <b>${esc(s.label)}</b>` : 'Nothing selected'; $('stSel').style.opacity = s ? 1 : .75;
  };
  const mode = () => {
    const lay = has('viewport.getLayout') ? app.viewport.getLayout() : (app.viewport && app.viewport.layout) || 'single';
    const m = has('camera.getMode') ? app.camera.getMode() : app.mode;
    $('stMode').textContent = `${m === 'walk' ? 'First person' : m === 'fly' ? 'Fly' : 'Orbit'} · ${lay === 'quad' ? 'four views' : 'single view'}`;
  };
  const sun = () => { const g = app.sun && app.sun.get(); if (!g) return; $('stSun').innerHTML = `${icon('sun')}${g.timeStr} · ${+g.date.slice(8)} ${MON[+g.date.slice(5, 7) - 1]} · sun ${g.elevation >= 0 ? '+' : '−'}${Math.abs(g.elevation).toFixed(0)}°`; };
  const ind = () => {
    const out = [];
    const sec = has('section.get') ? app.section.get() : null;
    if (sec) { const on = ['x', 'y', 'z'].filter(a => sec[a] && sec[a].enabled); if (on.length) out.push(['section', `Section ${on.join('').toUpperCase()}`, () => { on.forEach(a => app.section.set(a, { enabled: false })); }]); }
    const cut = has('ground.getCutaway') ? app.ground.getCutaway() : null;
    if (cut && cut !== 'none') out.push(['cut', 'Ground: ' + cut, () => app.ground.setCutaway('none')]);
    if (app.svc && app.svc.isXray()) out.push(['xray', 'X-ray', () => app.svc.setXray(false)]);
    if (app.svc && app.svc.isolated) out.push(['iso', 'Isolate: ' + app.svc.isolated, () => app.svc.isolate(null)]);
    const box = $('stInd'); box.innerHTML = '';
    out.forEach(([k, t, off]) => { const b = document.createElement('button'); b.className = 'st act sepl'; b.type = 'button'; b.title = 'Click to switch off'; b.textContent = t; b.onclick = () => { off(); ind(); }; box.append(b); });
  };
  const qual = () => { const i = app.renderer && app.renderer.info; if (!i) return; const q = app.getQuality(); $('stQual').textContent = `${q[0].toUpperCase() + q.slice(1)} · ${app.perf.renderMs.toFixed(1)} ms · ${i.render.calls} calls`; };
  const graph = () => { const g = app.graphInfo; $('stGraph').textContent = g ? `Graph: ${g.combined ? 'house + services' : 'house only'} · ${g.triples != null ? Number(g.triples).toLocaleString('en') + ' triples' : '…'}` : 'Graph: not loaded yet'; };
  const mcp = () => { const w = window.reteWebMCP || { supported: false, tools: [] }; const n = (window.reteBuildingTools ? window.reteBuildingTools.list().length : 0) || w.tools.length; const last = app.lastAgentCall; $('stMcp').textContent = `WebMCP: ${w.supported ? 'on' : 'not in this browser'} · ${n} tools${last ? ' · ' + last : ''}`; };
  sel(); mode(); sun(); graph(); mcp(); qual(); ind();
  app.on('select', sel); app.on('mode', mode); app.on('layout', () => { mode(); }); app.on('viewport', mode); app.on('camera-mode', mode);
  app.on('time', sun); app.on('site', sun); app.on('graph', graph);
  for (const ev of ['xray', 'isolate', 'section', 'cutaway', 'viewport', 'reset', 'layout']) app.on(ev, ind);
  app.on('hover', h => { if (app.mode === 'walk' || app.mode === 'fly') return; $('stHover').textContent = h && h.label ? 'Hover: ' + h.label + (h.pane && h.pane !== '3d' ? ' (' + h.pane + ')' : '') : ''; });
  app.on('quality', qual); app.on('agent', mcp);
  document.addEventListener('rete:webmcp', mcp);
  // frame time and draw calls, once a second; the room while walking, a few times a second
  let tq = 0, tr = 0;
  app.on('render', () => {
    const now = performance.now();
    if (now - tq > 1000) { tq = now; qual(); app.shell && app.shell.syncTop(); }
    if ((app.mode === 'walk' || app.mode === 'fly') && now - tr > 250) { tr = now; const w = app.walk && app.walk.info && app.walk.info(); if (w && w.active) $('stHover').textContent = 'Room: ' + (w.room || 'outside'); }
  });
  // the hovered element: one ray cast per frame at most, never while a button is down
  const cv = app.renderer.domElement; let hx = 0, hy = 0, hraf = 0, last = null;
  cv.addEventListener('pointermove', e => {
    if (e.buttons || app.mode !== 'orbit' || (app.viewport && app.viewport.getLayout)) return; hx = e.clientX; hy = e.clientY;   // with the viewport module the 'hover' event says it
    if (hraf) return; hraf = setTimeout(() => {
      hraf = 0; const hit = app.pickAt && app.pickAt(hx, hy); const id = hit ? hit.id : null; if (id === last) return; last = id;
      const t = id ? (app.nodeByGuid.get(id)?.l || (app.svc && app.svc.thingFor(id)?.d.label) || id) : '';
      $('stHover').textContent = t ? 'Hover: ' + t : '';
    }, 90);
  });
  cv.addEventListener('pointerleave', () => { if (app.mode === 'orbit') { $('stHover').textContent = ''; last = null; } });
}
