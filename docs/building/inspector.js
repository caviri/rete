// The inspector: what is selected, as collapsible sections. A house element, a room (zone data, surfaces, state
// variables), a device or run (the services module adds its rows through registerInfo), a structural element, a surface.
// Links between things are clickable; "Show in graph" focuses the Graph tab; "Facts" lists the triples of the thing.
import * as THREE from 'three';
import { app } from './app.js';
import { esc, fmt, shortIri, shortTerm, has } from './shared.js';
import { icon } from './icons.js';
import { applicable, renderVars, levelChip, levelOf, valueOf, defsAll } from './statevars-ui.js';

const open = new Map();   // section title -> open (kept between renders)
const IRI_HOUSE = 'https://w3id.org/rete/cad/fzk-haus/';
export const iriOf = sel => {
  if (!sel) return null;
  if (sel.surface) return (app.systems && app.systems.base || IRI_HOUSE + 'sys/') + sel.surface.id;
  if (sel.pick) return (app.systems && app.systems.base || IRI_HOUSE + 'sys/') + sel.id;
  if (sel.n) return IRI_HOUSE + sel.n;
  return null;
};
const sec = (title, body, { count, def = true, cls = '' } = {}) => body ? `<details class="sec ${cls}" data-t="${esc(title)}" ${(open.has(title) ? open.get(title) : def) ? 'open' : ''}><summary>${esc(title)}${count != null ? ` <span class="badge">${count}</span>` : ''}</summary><div class="sec-b">${body}</div></details>` : '';
const kv = (k, v) => (v == null || v === '' ? '' : `<tr><td>${esc(k)}</td><td>${v}</td></tr>`);
const tab = rows => { const r = rows.filter(Boolean).join(''); return r ? `<table class="proptab">${r}</table>` : ''; };
const lnk = (id, label, kind = 'g') => `<button type="button" class="lnk" data-go="${esc(id)}" data-kind="${kind}">${esc(label)}</button>`;
const structEls = () => (app.systems && app.systems.structure && app.systems.structure.elements) || [];
const labelOfKey = key => { const n = app.nodeByGuid.get(key); if (n) return n.l; const t = app.svc && app.svc.thingFor(key); if (t) return t.d.label; const e = structEls().find(x => x.id === key); return e ? (e.name || e.id) : key; };

function structureBlock(key) {
  const el = structEls().find(e => (e.guid || e.id) === key); if (!el) return '';
  const carries = structEls().filter(e => (e.bearsOn || []).includes(key)).map(e => e.guid || e.id);
  const p = el.props || {};
  const api = has('structure.remove'), removed = has('structure.stateOf') ? (() => { try { return app.structure.stateOf(key) === 'removed'; } catch { return false; } })() : false;
  const unsup = has('structure.state') ? (() => { try { return app.structure.state().unsupported.find(u => u.id === key || u.guid === key); } catch { return null; } })() : null;
  const list = (ids, cap = 12) => ids.slice(0, cap).map(i => `<li data-go="${esc(i)}" tabindex="0" role="button"><span>${esc(labelOfKey(i))}</span></li>`).join('') + (ids.length > cap ? `<li class="muted">+ ${ids.length - cap} more</li>` : '');
  return sec('Structure', `${tab([kv('Role', esc(el.role)), kv('Material', esc(p.material)), kv('Thickness', p.thicknessM != null ? fmt.len(p.thicknessM) : ''), kv('Volume', p.volumeM3 != null ? p.volumeM3.toFixed(2) + ' m³' : ''), kv('Self weight', p.selfWeightKN != null ? p.selfWeightKN.toFixed(1) + ' kN' : ''), kv('Load bearing', p.loadBearing != null ? (p.loadBearing ? 'yes' : 'no') : ''), kv('Origin', el.fromIfc ? 'IFC element' : 'designed addition')])}
    ${(el.bearsOn || []).length ? `<div class="svc-h">Bears on <span class="badge">${el.bearsOn.length}</span></div><ul class="svc-list">${list(el.bearsOn)}</ul>` : ''}
    ${carries.length ? `<div class="svc-h">Carries <span class="badge">${carries.length}</span></div><ul class="svc-list">${list(carries)}</ul>` : ''}
    ${unsup ? `<div class="live-bad" style="margin-top:6px">Lost its support: ${esc(unsup.reason || '')}</div>` : ''}
    <div class="ins-act" style="padding:8px 0 0"><button class="btn sm" type="button" data-trace="${esc(key)}" data-dir="down" ${has('structure.tracePath') ? '' : 'disabled title="Load path: not available yet"'}>Trace load path</button>
      <button class="btn sm" type="button" data-trace="${esc(key)}" data-dir="up" ${has('structure.tracePath') ? '' : 'disabled title="Load path: not available yet"'}>What bears on it</button>
      <button class="btn sm" type="button" data-rm="${esc(key)}" ${api ? '' : 'disabled title="Remove: not available yet"'}>${removed ? 'Restore' : 'Remove'}</button></div>`);
}

function roomBlocks(n) {
  const sys = app.systems; if (!sys) return '';
  const z = sys.thermal && sys.thermal.zones.find(x => x.room === n.n), r = sys.rooms && sys.rooms.find(x => x.guid === n.n);
  const out = [];
  if (z || r) out.push(sec('Zone', tab([kv('Floor area', fmt.area(z ? z.floorAreaM2 : r.area)), kv('Volume', (z ? z.volumeM3 : r.volumeM3) + ' m³'), kv('Set point', z ? fmt.temp(z.setpointC) : ''), kv('Design heat load', z ? z.heatLoadW + ' W' : r && r.heatLoadW + ' W'), kv('Air changes', z ? z.airChangesPerHour + ' per h (' + z.effectiveAirChangesPerHour + ' with heat recovery)' : ''), kv('Internal gains', z ? z.internalGainsW + ' W' : '')])));
  const defs = applicable('room'), items = defs.map(d => ({ d, id: n.n, ctx: {} }));
  if (items.length) out.push(sec('State variables', '<div data-vars="room"></div>', { count: items.length }));
  const surfs = sys.thermal ? sys.thermal.surfaces.filter(s => s.room === n.n) : [];
  if (surfs.length) out.push(sec('Surfaces', `<ul class="svc-list" style="max-height:200px">${surfs.map(s => `<li data-surf="${esc(s.id)}" tabindex="0" role="button"><span>${esc(s.kind)} · ${esc(s.boundary)}${s.otherRoom ? ' → ' + esc(labelOfKey(s.otherRoom)) : ''}</span><span class="m">${s.areaM2.toFixed(1)} m² · U ${s.uValue != null ? s.uValue : '—'}</span></li>`).join('')}</ul>`, { count: surfs.length, def: false }));
  if (app.svc) { const things = app.svc.find({ room: n.l }).filter(t => t.kind === 'node'); if (things.length) out.push(sec('Devices in the room', `<ul class="svc-list">${things.slice(0, 40).map(t => `<li data-go="${esc(t.id)}" tabindex="0" role="button"><span>${esc(t.d.label)}</span><span class="m">${esc(t.d.type)}</span></li>`).join('')}${things.length > 40 ? `<li class="muted">+ ${things.length - 40} more</li>` : ''}</ul>`, { count: things.length, def: false })); }
  return { html: out.join(''), items };
}

export function renderInspector(el, sel, deps) {
  const { providers, relatedOf, nodeAt, floorOf, pretty, select } = deps;
  const wrap = document.createElement('div');
  if (!sel) { el.innerHTML = '<div class="pad" style="padding:12px"><div class="empty">Nothing selected. Click something in the 3D view, or search with Ctrl+K.</div></div>'; return; }
  const pick = !!sel.pick, surface = sel.surface, thing = pick ? { id: sel.id, kind: 'pickable', object: sel.obj, meta: sel.meta, label: sel.l } : null;
  let html = '', roomItems = null, varItems = [], title, chips = [];
  const iri = iriOf(sel);
  if (surface) {
    const s = surface; title = `Surface ${s.id}`; chips = [`<span class="chip">${esc(s.kind)}</span>`, `<span class="chip">${esc(s.boundary)}</span>`];
    html += sec('Surface', tab([kv('Room', lnk(s.room, labelOfKey(s.room))), kv('Element', s.element ? lnk(s.element, labelOfKey(s.element)) : ''), kv('Other side', s.otherRoom ? lnk(s.otherRoom, labelOfKey(s.otherRoom)) : s.boundary), kv('Area', fmt.area(s.areaM2)), kv('Azimuth', s.azimuthDeg != null ? Math.round(s.azimuthDeg) + '° from true north' : ''), kv('Tilt', s.tiltDeg != null ? s.tiltDeg + '°' : ''), kv('Construction', esc(s.construction)), kv('U-value', s.uValue != null ? s.uValue + ' W/m²K' : ''), kv('g-value', s.gValue)]));
    varItems = applicable('surface', s).map(d => ({ d, id: s.id, ctx: {} }));
    if (varItems.length) html += sec('State variables', '<div data-vars="surface"></div>', { count: varItems.length });
  } else if (pick) {
    title = sel.l; const t = app.svc && app.svc.thingFor(sel.id);
    chips = [`<span class="chip">${esc(sel.meta.kind || 'item')}</span>`]; if (t) chips.unshift(`<span class="chip"><i style="background:${esc(app.svc.sys.get(t.system).color)}"></i>${esc(app.svc.sys.get(t.system).label)}</span>`);
    html += '<div class="x"></div>';
    if (t && t.kind === 'node') {
      const ty = t.d.type; varItems = applicable('device', { type: ty }).filter(d => d.variableKind === 'numeric').map(d => ({ d, id: t.id, ctx: {} }));
      if (varItems.length) html += sec('State variables', '<div data-vars="device"></div>', { count: varItems.length });
    }
    html += t && t.system === 'structure' ? structureBlock(t.id) : '';
  } else {
    const n = sel, isRoom = n.k === 'Space', rel = relatedOf(n), o = app.meshByGuid.get(n.n);
    title = n.l; chips = [`<span class="chip">${esc(pretty(n.c))}</span>`, `<span class="chip">${isRoom ? 'room' : 'element'}</span>`];
    let centre = '', size = ''; if (o) { const b = new THREE.Box3().setFromObject(o), c = b.getCenter(new THREE.Vector3()), s = b.getSize(new THREE.Vector3()); centre = [c.x, c.y, c.z].map(v => v.toFixed(2)).join(', ') + (fmt.len(0).includes('ft') ? ' ft' : ' m'); size = [s.x, s.y, s.z].map(v => v.toFixed(2)).join(' × '); }
    html += sec('Properties', tab([kv('Floor', esc(n.st >= 0 ? floorOf(n) : '—')), kv('Material', esc(n.mt)), kv('Floor area', n.ar ? fmt.area(n.ar) : ''), kv('Centre', centre), kv('Size', size), kv('IFC GUID', `<code>${esc(n.n)}</code>`)]));
    const st = app.getState(n.n);
    if (st) { html += sec('State', `<div class="kv"><b>State:</b> <span class="stnow">${esc(st)}</span></div>${app.states(n.n).length > 1 ? `<div class="stbtns">${app.states(n.n).map(v => `<button class="btn sm${v === st ? ' on' : ''}" type="button" data-st="${v}" aria-pressed="${v === st}">${v === 'closed' ? 'Close' : v === 'open' ? 'Open' : 'Tilt'}</button>`).join('')}</div>` : '<p class="note">Fixed: it has one state and cannot be opened.</p>'}`); }
    html += '<div class="x"></div>';
    if (isRoom) { const rb = roomBlocks(n); html += rb.html; roomItems = rb.items; varItems = rb.items; }
    else html += structureBlock(n.n);
    const relList = (title, color, idxs, lab) => idxs.length ? sec(title, `<ul class="nlist" role="listbox" aria-label="${esc(title)}">${idxs.map(i => { const nb = nodeAt(i); return `<li data-i="${i}" tabindex="0" role="option"><span>${esc(nb.l)}</span><span class="m">${esc(lab(nb))}</span></li>`; }).join('')}</ul>`, { count: idxs.length, def: isRoom }) : '';
    html += isRoom ? relList('Adjacent rooms', 'var(--c-room)', rel.rooms, nb => floorOf(nb)) + relList('Enclosed by', 'var(--c-elem)', rel.elems, nb => pretty(nb.c)) : relList('Encloses rooms', 'var(--c-room)', rel.rooms, nb => floorOf(nb));
  }
  const gt = true;
  const head = `<div class="ins-head"><div class="t">${esc(title)}</div><div class="ins-chips">${chips.join('')}</div></div>
    <div class="ins-act"><button class="btn sm" type="button" data-act="graph" ${gt ? '' : 'disabled'}>${icon('graph')}Show in graph</button><button class="btn sm" type="button" data-act="copy" title="Copy the IRI">${icon('copy')}IRI</button>${sel.n || pick || surface ? `<button class="btn sm" type="button" data-act="turtle" title="Download the facts of this thing as Turtle">${icon('download')}Turtle</button>` : ''}</div>`;
  const facts = iri ? sec('Facts', '<div id="facts"><p class="note">Opens the triples of this thing from the graph.</p></div>', { def: false, cls: 'facts-sec' }) : '';
  el.innerHTML = head + html + facts;
  // providers of other modules (the services rows of a device)
  if (pick) { const slot = el.querySelector('.x'); const holder = document.createElement('div'); holder.className = 'sec-b'; holder.style.padding = '0 12px 8px'; for (const fn of providers) { try { const out = fn(thing); if (out == null || out === '') continue; if (typeof out === 'string') holder.insertAdjacentHTML('beforeend', out); else if (out instanceof Node) holder.append(out); else if (Array.isArray(out)) out.forEach(x => { if (x instanceof Node) holder.append(x); else if (x) holder.insertAdjacentHTML('beforeend', String(x)); }); } catch (e) { console.error('[building] info provider failed', e); } } if (slot) slot.replaceWith(holder); }
  else if (sel.n) { const slot = el.querySelector('.x'); const holder = document.createElement('div'); holder.style.padding = '0 12px'; const ctx = { id: sel.n, kind: sel.k === 'Space' ? 'room' : 'element', node: sel, object: app.meshByGuid.get(sel.n), label: sel.l }; for (const fn of providers) { try { const out = fn(ctx); if (out == null || out === '') continue; if (typeof out === 'string') holder.insertAdjacentHTML('beforeend', out); else if (out instanceof Node) holder.append(out); else if (Array.isArray(out)) out.forEach(x => { if (x instanceof Node) holder.append(x); else if (x) holder.insertAdjacentHTML('beforeend', String(x)); }); } catch (e) { console.error('[building] info provider failed', e); } } if (slot) slot.replaceWith(holder); }
  // the variable controls
  const vbox = el.querySelector('[data-vars]');
  if (vbox) {
    const id = surface ? surface.id : pick ? sel.id : sel.n;
    renderVars(vbox, varItems.map(x => ({ ...x, id })));
  }
  // state buttons of an element
  el.querySelectorAll('[data-st]').forEach(b => b.onclick = () => app.setState(sel.n, b.dataset.st));
  el.querySelectorAll('details.sec').forEach(d => d.addEventListener('toggle', () => { open.set(d.dataset.t, d.open); if (d.classList.contains('facts-sec') && d.open) loadFacts(el, iri); }));
  el._ins = { iri, title, select, nodeAt };
  el.scrollTop = 0;
  const f = el.querySelector('details.facts-sec[open]'); if (f) loadFacts(el, iri);
}

// one listener for all renders
export function bindInspector(el) {
  el.addEventListener('click', e => {
    const c = el._ins; if (!c) return;
    const a = e.target.closest('[data-go],[data-i],[data-surf],[data-trace],[data-rm],[data-act]'); if (!a || !el.contains(a)) return;
    if (a.dataset.i != null) c.select(c.nodeAt(+a.dataset.i)); else if (a.dataset.surf) inspectSurface(a.dataset.surf);
    else if (a.dataset.go) { try { c.select(a.dataset.go); } catch { app.notify('Not a thing in the 3D model: ' + a.dataset.go); } }
    else if (a.dataset.trace) { try { app.structure.tracePath(a.dataset.trace, a.dataset.dir || 'down'); } catch (err) { app.notify(String(err.message || err)); } }
    else if (a.dataset.rm) { try { const k = a.dataset.rm; app.structure.stateOf(k) === 'removed' ? app.structure.restore(k) : app.structure.remove(k); app.refreshInfo(); } catch (err) { app.notify(String(err.message || err)); } }
    else if (a.dataset.act === 'graph') showInGraph(c.iri);
    else if (a.dataset.act === 'copy') { navigator.clipboard && navigator.clipboard.writeText(c.iri).then(() => app.notify('IRI copied'), () => app.notify(c.iri)); }
    else if (a.dataset.act === 'turtle') exportTurtle(c.iri, c.title);
  });
  el.addEventListener('keydown', e => { if ((e.key === 'Enter' || e.key === ' ') && e.target.matches('li[tabindex]')) { e.preventDefault(); e.target.click(); } });
}
export function showInGraph(iri, grouping) {
  if (!iri) return; app.shell.openTab('graph'); return app.graphTab ? app.graphTab.focus(iri, grouping) : import('./graph-tab.js').then(m => m.focusIri(iri, grouping));
}
async function loadFacts(el, iri) {
  const box = el.querySelector('#facts'); if (!box || box.dataset.iri === iri) return; box.dataset.iri = iri; box.innerHTML = '<p class="note">Asking the graph…</p>';
  try {
    const { describeIri } = await import('./queries.js'); const d = await describeIri(iri);
    const row = (a, b) => `<tr><td>${esc(shortTerm(a))}</td><td>${esc(shortTerm(b))}</td></tr>`;
    box.innerHTML = `<table class="facts">${d.out.map(x => row(x.p, x.o)).join('')}</table>${d.in.length ? `<div class="svc-h">Pointing here <span class="badge">${d.in.length}</span></div><table class="facts">${d.in.map(x => row(x.p, x.s)).join('')}</table>` : ''}<p class="note">${d.out.length} outgoing${d.truncated ? ' (first 200)' : ''}, ${d.in.length} incoming. A DESCRIBE-like query on ${esc(d.source)}.</p>`;
  } catch (e) { box.innerHTML = `<p class="note" style="color:var(--bad)">${esc(e.message || e)}</p>`; }
}
export async function exportTurtle(iri, name = 'thing') {
  try {
    const { describeIri, toTurtle } = await import('./queries.js'); const d = await describeIri(iri);
    const text = toTurtle(iri, d); const a = document.createElement('a'); a.href = URL.createObjectURL(new Blob([text], { type: 'text/turtle' })); a.download = String(name).replace(/[^\w.-]+/g, '_').slice(0, 40) + '.ttl'; a.click(); setTimeout(() => URL.revokeObjectURL(a.href), 2000);
    app.notify(`Saved ${d.out.length + d.in.length} facts as Turtle`);
  } catch (e) { app.notify('Turtle export failed: ' + (e.message || e)); }
}
// a surface is not a pickable of the 3D scene: show it in the inspector, and light up the element it belongs to
export function inspectSurface(id) {
  const s = app.systems && app.systems.thermal && app.systems.thermal.surfaces.find(x => x.id === id); if (!s) throw new Error('No surface "' + id + '".');
  app.inspectSurface = s; const el = document.getElementById('info');
  renderInspector(el, { surface: s }, app._inspDeps); app.shell.openInspector(); app.emit('inspect', { kind: 'surface', id });
  return s;
}
export { levelChip, levelOf, valueOf, defsAll };
