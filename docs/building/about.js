// The About dialog. Short, plain text. Numbers, titles and licences are read from the dataset card in the .rete (through
// the engine), the shortcuts from the command table, the tool list from the registered WebMCP tools.
import { app } from './app.js';
import { esc, $, ui } from './shared.js';
import { icon } from './icons.js';
import { shortcutTable } from './commands.js';
import { getEngine } from './queries.js';

const SECTIONS = [['page', 'This page'], ['house', 'The house'], ['data', 'Data files'], ['model', 'Model notes'], ['shortcuts', 'Shortcuts'], ['webmcp', 'WebMCP'], ['links', 'Links']];
const HOUSE_JSON = 'https://data.graphplaza.com/cad/fzk-viewer.json';
const HOUSE_RETE = 'https://data.graphplaza.com/cad/fzk-haus-2016-12-21.rete';
const fmtBytes = n => (n == null ? 'size unknown' : n < 1024 ? n + ' B' : n < 1048576 ? (n / 1024).toFixed(0) + ' KB' : (n / 1048576).toFixed(1) + ' MB');

async function fileSize(url) {
  try { const e = performance.getEntriesByType('resource').find(r => r.name.split('?')[0] === url); if (e && (e.encodedBodySize || e.decodedBodySize)) return e.decodedBodySize || e.encodedBodySize; } catch { /* none */ }
  try { const r = await fetch(url, { method: 'HEAD' }); const n = +r.headers.get('content-length'); if (n) return n; } catch { /* offline */ }
  return null;
}
const li = a => `<ul>${a.map(x => `<li>${x}</li>`).join('')}</ul>`;

export async function aboutData() {
  const card = (app.graphInfo && app.graphInfo.card) || null; const info = app.graphInfo || null;
  const files = [
    { role: 'House geometry and index', url: HOUSE_JSON, name: 'fzk-viewer.json' },
    ...(app.systems ? [{ role: 'Services, structure and thermal inputs (JSON index)', url: app.SYSTEMS_JSON, name: String(app.SYSTEMS_JSON).split('/').pop() }] : []),
    { role: app.systems ? 'The graph the queries run on: house, services, structure, state variables' : 'The graph the queries run on: the house', url: info ? info.url : (app.systems ? app.SYSTEMS_RETE : HOUSE_RETE), name: String(info ? info.url : (app.systems ? app.SYSTEMS_RETE : HOUSE_RETE)).split('/').pop() },
  ];
  for (const f of files) f.size = f.url === (info && info.url) && info.bytes ? info.bytes : await fileSize(f.url);
  return { card, triples: info ? info.triples : null, files, graph: info ? (info.combined ? 'house and services' : 'house only') : 'not loaded yet' };
}
const cardLine = (c, k) => (c && c[k] != null && c[k] !== '' ? String(c[k]) : null);

async function render(key, box) {
  if (key === 'page') {
    box.innerHTML = `<h3>This page</h3><p>A 3D viewer for one house with a knowledge graph behind it. Everything you click in the 3D view is a thing in the graph, and the graph answers SPARQL queries that run in your browser, on a <code>.rete</code> file.</p>
      <p>The menus and panels around the 3D view change what is drawn, move the sun, switch devices on and off, run what-if scenarios and a day of thermal simulation. The console runs your own queries; the rows come back as a table and light up in 3D. The Validate tab checks the graph against SHACL shapes, with examples from the data. The graph tab shows the same things as a clustered network. Every action is also a tool for a browser agent, see WebMCP.</p>
      <p>Everything is computed in the page. Nothing is sent to a server except the data files, which are fetched once.</p>`;
  } else if (key === 'house') {
    box.innerHTML = `<h3>The house</h3><p><b>FZK-Haus</b> is a small two-storey single-family house, an IFC4 sample from the Karlsruhe Institute of Technology (KIT), distributed with the IBPSA Project 1 test files.</p>
      <p><b>Real, from the IFC:</b></p>${li(['walls, slabs, doors, windows, the stair, the roof and the seven rooms, with their geometry', 'material names, wall and slab thicknesses, the U-values of windows and doors', 'the site: latitude, longitude, elevation and the direction of true north (the house is turned 50 degrees)'])}
      <p><b>A designed example, not from the IFC:</b></p>${li(['all building services: electrical, lighting, network, water, drainage, heating, ventilation, cooling and safety, with their cables, pipes and ducts', 'the structure additions: foundations, ring beams, lintels, bracing, collar ties and the load paths', 'the thermal assumptions: layer build-ups, g-values, set points, internal gains, typical outdoor temperatures', 'the time zone, the hinge side of doors, timber strength class C24'])}
      <p>Cables are drawn thicker than they are, so they can be seen.</p>`;
  } else if (key === 'data') {
    box.innerHTML = `<h3>Data files</h3><p class="muted">Reading the card from the graph…</p>`;
    let d; try { if (!app.graphInfo) await getEngine(); d = await aboutData(); } catch (e) { box.innerHTML = `<h3>Data files</h3><p>The graph could not be read: ${esc(e.message || e)}</p>`; return; }
    const c = d.card;
    box.innerHTML = `<h3>Data files</h3>
      <table><tr><th>File</th><th>What it holds</th><th>Size</th></tr>${d.files.map(f => `<tr><td><code>${esc(f.name)}</code></td><td>${esc(f.role)}</td><td>${esc(fmtBytes(f.size))}</td></tr>`).join('')}</table>
      <h3>The graph</h3><table>
        <tr><td>Title</td><td>${esc(cardLine(c, 'title') || '—')}</td></tr><tr><td>Version</td><td>${esc(cardLine(c, 'version') || '—')}</td></tr>
        <tr><td>Triples</td><td>${d.triples != null ? Number(d.triples).toLocaleString('en') : '—'}</td></tr><tr><td>Licence</td><td>${esc(cardLine(c, 'license') || cardLine(c, 'licence') || '—')}</td></tr>
        <tr><td>Source</td><td>${esc(cardLine(c, 'source') || '—')}</td></tr><tr><td>In use</td><td>${esc(d.graph)}</td></tr></table>
      ${cardLine(c, 'description') ? `<p class="muted">${esc(cardLine(c, 'description'))}</p>` : ''}
      <p class="note">The values above are read from the dataset card inside the file, through the engine.</p>`;
  } else if (key === 'model') {
    box.innerHTML = `<h3>Model notes</h3><p><b>Thermal day.</b> One temperature per room (seven nodes), 10 minute steps, the day repeated until it closes. Clear-sky sun scaled by a clearness slider, shading by ray casting, outdoor temperature a cosine between a typical minimum and maximum for the month, typed in by hand for Karlsruhe. Insulation and g-values are assumed. It is for comparing orientations and what-ifs, not a certified calculation. It has no weather data, no people, no moisture.</p>
      <p><b>Supply scenarios.</b> A thing is live when it is enabled and everything that feeds it is live. Switch a breaker off and everything downstream goes dark; the same question is a SPARQL property path (<code>brick:feeds+</code>) and the page compares both answers.</p>
      <p><b>Structural scenarios.</b> An element is supported if at least one complete <code>bearsOn</code> path leads from it to the ground through intact elements. This is a path rule, not a structural analysis: no loads are checked, no deflection, no connections. The graph query returns everything that bears on the removed element (an approximation); the exact set comes from the data and is a subset of it.</p>
      <p><b>State variables</b> describe what a kind of thing can be (a range, a unit, named levels such as comfortable or hot). The values in the data are initial values, not results.</p>`;
  } else if (key === 'shortcuts') {
    const t = shortcutTable(); const groups = {}; t.forEach(x => { (groups[x.group] = groups[x.group] || []).push(x); });
    box.innerHTML = `<h3>Keyboard shortcuts</h3><p class="note">Single keys do not work while you type in a field or walk. A pair such as <kbd>G</kbd> then <kbd>C</kbd> means two presses, one after the other. The table is the one the shortcuts are bound from.</p>${Object.keys(groups).map(g => `<h3 style="font-size:var(--fs-md);margin-top:14px">${esc(g)}</h3><table>${groups[g].map(x => `<tr><td>${esc(x.title)}</td><td>${esc(x.keys).split(' then ').map(k => k.split('+').map(p => `<kbd>${p}</kbd>`).join('+')).join(' then ')}</td></tr>`).join('')}</table>`).join('')}`;
  } else if (key === 'webmcp') {
    const w = window.reteWebMCP || { supported: false, tools: [] }; const list = window.reteBuildingTools ? window.reteBuildingTools.list() : [];
    box.innerHTML = `<h3>WebMCP</h3><p>WebMCP lets a web page offer tools to an AI agent that runs in the browser. This page offers <b>${list.length}</b> tools: reading the state, selecting and changing things, running SPARQL, running simulations, and driving this interface. An agent works on the same objects as you do, and its calls are in the Logs tab (source <i>agent</i>).</p>
      <p>This browser: <b>${w.supported ? 'WebMCP is available, the tools are registered' : 'WebMCP is not available, the tools can still be called from the page as <code>window.reteBuildingTools</code>'}</b>.</p>
      <table>${list.map(t => `<tr><td><code>${esc(t.name)}</code></td><td>${esc(String(t.description || '').split(/(?<=\.)\s/)[0].slice(0, 200))}${t.readOnly ? ' <span class="muted">(read only)</span>' : ''}</td></tr>`).join('')}</table>`;
  } else if (key === 'links') {
    const play = 'playground.html#url=' + encodeURIComponent(app.systems && app.SYSTEMS_RETE ? app.SYSTEMS_RETE : HOUSE_RETE);
    box.innerHTML = `<h3>Links</h3>${li([`<a href="building-guide.html" target="_blank" rel="noopener">The guide</a>: how the house and the services were made`, `<a href="${esc(play)}" target="_blank" rel="noopener">The playground, opened on this graph</a>: run any query on the same file`, `<a href="https://github.com/caviri/rete" target="_blank" rel="noopener">The rete project</a>`, `<a href="https://github.com/ibpsa/project1-wp-2-2-bim" target="_blank" rel="noopener">The IFC sample (IBPSA Project 1)</a>`, `<a href="https://www.kit.edu" target="_blank" rel="noopener">KIT</a>`])}`;
  }
}

export function aboutContent() {   // the content as data, for the get_about tool
  const w = window.reteWebMCP || {}; const t = window.reteBuildingTools ? window.reteBuildingTools.list() : [];
  return { sections: SECTIONS.map(s => s[0]) };
}
export async function aboutAsData() {
  const d = await (async () => { try { if (!app.graphInfo) await getEngine(); return await aboutData(); } catch (e) { return { error: String(e.message || e) }; } })();
  const out = {};
  for (const [k, title] of SECTIONS) { const box = document.createElement('div'); if (k === 'data') { out[k] = d; continue; } await render(k, box); out[k] = { title, text: box.textContent.replace(/\s+/g, ' ').trim() }; }
  // the card carries the whole profile and the starter queries: an agent gets its short fields (a tool result stays small)
  const card = d.card ? Object.fromEntries(Object.entries(d.card).filter(([, v]) => v == null || typeof v === 'number' || typeof v === 'boolean' || (typeof v === 'string' && v.length <= 700)).concat(Object.entries(d.card).filter(([, v]) => typeof v === 'string' && v.length > 700).map(([k, v]) => [k, v.slice(0, 700) + '…']))) : null;
  out.data = { title: 'Data files', ...d, card };
  return out;
}
let dlg = null;
export async function openAbout(section = 'page') {
  if (dlg) dlg.remove();
  const d = document.createElement('div'); d.className = 'dlg'; d.setAttribute('role', 'dialog'); d.setAttribute('aria-modal', 'true'); d.setAttribute('aria-labelledby', 'aboutT'); dlg = d;
  d.innerHTML = `<div class="dlg-card"><div class="dlg-head"><h2 id="aboutT">About FZK-Haus 3D Explorer</h2><button class="ibtn" type="button" data-x aria-label="Close">${icon('close')}</button></div>
    <div class="dlg-body"><nav class="dlg-nav" aria-label="About sections">${SECTIONS.map(([k, t]) => `<a role="button" tabindex="0" data-s="${k}">${t}</a>`).join('')}</nav><div class="dlg-main" id="aboutMain" tabindex="-1"></div></div></div>`;
  document.body.append(d);
  const back = document.activeElement;
  const show = async k => { d.querySelectorAll('[data-s]').forEach(a => a.setAttribute('aria-current', String(a.dataset.s === k))); const m = $('aboutMain'); m.scrollTop = 0; await render(k, m); };
  const close = () => { d.remove(); dlg = null; if (back && back.focus) back.focus({ preventScroll: true }); };
  d.addEventListener('click', e => { if (e.target === d || e.target.closest('[data-x]')) close(); const a = e.target.closest('[data-s]'); if (a) show(a.dataset.s); });
  d.addEventListener('keydown', e => { if (e.key === 'Escape') { e.stopPropagation(); close(); } else if ((e.key === 'Enter' || e.key === ' ') && e.target.matches('[data-s]')) { e.preventDefault(); show(e.target.dataset.s); } else if (e.key === 'Tab') { const f = [...d.querySelectorAll('button,[data-s],a[href]')]; if (!f.length) return; const i = f.indexOf(document.activeElement); if (e.shiftKey && i <= 0) { e.preventDefault(); f[f.length - 1].focus(); } else if (!e.shiftKey && i === f.length - 1) { e.preventDefault(); f[0].focus(); } } });
  await show(section); d.querySelector('[data-x]').focus();
}
export { ui };
