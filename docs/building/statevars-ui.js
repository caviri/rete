// State variables in the panels: what a room, a surface or a device can be (from the definitions in the data),
// the current value and its named level, and controls generated from the definition: an enumerated variable gets
// buttons, a numeric one a slider over its range with the level bands drawn on the track.
// Values come from app.statevars (getValue/setValue) when the rendering side provides it, else from the data file's
// initial values. Without app.statevars the sliders are shown but disabled.
import { app } from './app.js';
import { esc, fmt, has } from './shared.js';

// the definitions: app.statevars.variables() (same shape: kind is variableKind there) or the data file
export const defsAll = () => {
  const api = app.statevars; if (api && typeof api.variables === 'function') { try { return api.variables().map(v => ({ ...v, variableKind: v.variableKind || v.kind })); } catch { /* fall back */ } }
  return (app.systems && app.systems.stateVariables) || [];
};
const svApi = () => (app.statevars && typeof app.statevars.setValue === 'function' ? app.statevars : null);

// which definitions apply to a thing: kind is 'room' | 'surface' | 'device' | 'opening' | 'structure'
export function applicable(kind, thing = {}) {
  return defsAll().filter(d => {
    const a = d.appliesTo || {}; if (a.kind !== kind) return false;
    if (kind === 'surface') return !a.surfaceKinds || a.surfaceKinds.includes(thing.kind);
    if (kind === 'device') return !a.nodeTypes || a.nodeTypes.includes(thing.type);
    if (kind === 'opening') return a.openingKind === thing.kind && (!a.operation || a.operation === thing.operation) && !(d.id === 'state-window' && thing.operation === 'fixed');
    return true;
  });
}
export function valueOf(thingId, d, ctx = {}) {
  const api = app.statevars;
  if (api && typeof api.getValue === 'function') { try { const v = api.getValue(d.id, thingId); if (v !== undefined && v !== null) return v; } catch { /* the variable does not apply to this thing */ } }
  if (d.variableKind === 'enumerated') { const s = ctx.state != null ? ctx.state : app.getState && app.getState(thingId); return s != null ? s : d.default; }
  const sv = app.systems && app.systems.stateValues && app.systems.stateValues[thingId];
  if (sv && sv[d.id] !== undefined) return sv[d.id];
  return typeof d.default === 'number' ? d.default : null;
}
// the absolute [lo, hi) of every level of a numeric variable (offsets from a property of the same thing are resolved)
export function bands(d, thingId) {
  if (!d.levels || !d.levels.length) return [];
  let base = 0;
  if (d.relativeTo) { base = valueOf(thingId, defsAll().find(x => x.shortcut === 'setpointC' || x.id === 'room-setpoint') || { id: 'room-setpoint' }); if (typeof base !== 'number') base = 20; }
  return d.levels.map(l => ({ ...l, lo: Math.max(d.min, d.relativeTo ? base + l.min : l.min), hi: Math.min(d.max, d.relativeTo ? base + l.max : l.max) }));
}
export function levelOf(d, v, thingId) {
  if (typeof v !== 'number') return null;
  const api = app.statevars; if (api && typeof api.levelOf === 'function') { try { const l = api.levelOf(d.id, thingId, v); if (l) return l; } catch { /* use the bands */ } }
  const bs = bands(d, thingId);
  return bs.find((b, i) => v >= b.lo && (v < b.hi || (i === bs.length - 1 && v <= b.hi))) || null;
}
const readable = c => { const h = c.replace('#', ''); const n = parseInt(h.length === 3 ? h.split('').map(x => x + x).join('') : h, 16); const l = (0.299 * (n >> 16) + 0.587 * ((n >> 8) & 255) + 0.114 * (n & 255)); return l > 150 ? '#0b1017' : '#fff'; };
export const levelChip = lv => lv ? `<span class="lvl" style="background:${esc(lv.color)};color:${readable(lv.color)}">${esc(lv.label)}</span>` : '';
const fmtVal = (d, v) => (typeof v === 'number' ? (d.unit === 'DEG_C' ? fmt.temp(v) : `${Number.isInteger(v) ? v : v.toFixed(d.step < 1 ? 1 : 0)} ${d.unitLabel || ''}`.trim()) : String(v));

// build the controls into `box`. items: [{d, id, ctx}]; id is the thing's id; ctx: {state, set(value), kind}
export function renderVars(box, items) {
  box.innerHTML = '';
  for (const { d, id, ctx = {} } of items) {
    const row = document.createElement('div'); row.className = 'vrow'; row.dataset.var = d.id;
    const v = valueOf(id, d, ctx);
    if (d.variableKind === 'enumerated') {
      row.innerHTML = `<div class="vh"><span>${esc(d.label)}</span><b>${esc(v)}</b></div><div class="stbtns" role="group" aria-label="${esc(d.label)}"></div>`;
      const g = row.querySelector('.stbtns');
      for (const o of d.values) { const b = document.createElement('button'); b.type = 'button'; b.className = 'btn sm' + (o.value === v ? ' on' : ''); b.textContent = o.label; b.setAttribute('aria-pressed', String(o.value === v)); b.onclick = () => ctx.set && ctx.set(o.value); if (!ctx.set) b.disabled = true; g.append(b); }
    } else {
      const lv = levelOf(d, v, id), bs = bands(d, id), span = d.max - d.min || 1, can = !!svApi();
      row.innerHTML = `<div class="vh"><span>${esc(d.label)}</span><span><b data-v>${v == null ? '—' : esc(fmtVal(d, v))}</b> <span data-l>${levelChip(lv)}</span></span></div>
        <div class="vtrack"><div class="bands">${bs.map(b => `<i style="width:${100 * (b.hi - b.lo) / span}%;background:${esc(b.color)}" title="${esc(b.label)}: ${b.lo.toFixed(d.step < 1 ? 1 : 0)} to ${b.hi.toFixed(d.step < 1 ? 1 : 0)} ${esc(d.unitLabel || '')}"></i>`).join('') || `<i style="width:100%;background:var(--line2)"></i>`}</div>
        <input type="range" min="${d.min}" max="${d.max}" step="${d.step || 1}" value="${v == null ? d.min : v}" aria-label="${esc(d.label)}" ${can ? '' : 'disabled'} ${can ? '' : 'title="Setting values: not available yet"'}></div>
        <div class="vlim"><span>${d.min}</span><span>${esc(d.unitLabel || '')}${d.setBy ? ' · set by ' + esc(d.setBy) : ''}</span><span>${d.max}</span></div>`;
      const inp = row.querySelector('input');
      if (can) inp.oninput = () => { const nv = +inp.value; try { app.statevars.setValue(d.id, id, nv); } catch (e) { app.notify(String(e.message || e)); return; } row.querySelector('[data-v]').textContent = fmtVal(d, nv); row.querySelector('[data-l]').innerHTML = levelChip(levelOf(d, nv, id)); app.emit('statevar', { id, variable: d.id, value: nv }); };
    }
    box.append(row);
  }
}
// all definitions, grouped by what they apply to: the overview in the States panel
export function renderOverview(box) {
  const defs = defsAll(); if (!defs.length) { box.innerHTML = '<p class="note">No state variables in the data.</p>'; return; }
  const groups = {}; defs.forEach(d => { (groups[d.appliesTo.kind] = groups[d.appliesTo.kind] || []).push(d); });
  const names = { room: 'Rooms', surface: 'Surfaces', device: 'Devices', opening: 'Doors and windows', structure: 'Structure' };
  box.innerHTML = Object.keys(groups).map(k => `<details class="sec" style="border:0"><summary style="padding-left:0">${names[k] || k} <span class="badge">${groups[k].length}</span></summary><div class="sec-b" style="padding-left:0;padding-right:0">${groups[k].map(d => `<div class="vrow"><div class="vh"><span>${esc(d.label)}</span><span class="muted">${d.variableKind === 'numeric' ? `${d.min} to ${d.max} ${esc(d.unitLabel || '')}` : d.values.map(o => o.label).join(' · ')}</span></div>${d.levels && d.levels.length ? `<div class="ins-chips">${d.levels.map(l => levelChip(l)).join('')}</div>` : ''}</div>`).join('')}</div></details>`).join('');
}
export const canSetValues = () => !!svApi();
export { has };

// ---------- the explorer in the States panel: pick a variable, see every thing it applies to with its value and level ----------
function thingsOf(d) {
  const k = d.appliesTo.kind, a = d.appliesTo, out = [];
  if (k === 'room') app.roomByGuid.forEach(r => out.push({ id: r.guid, label: r.label, kind: 'room' }));
  else if (k === 'surface') ((app.systems && app.systems.thermal && app.systems.thermal.surfaces) || []).filter(s => !a.surfaceKinds || a.surfaceKinds.includes(s.kind)).forEach(s => out.push({ id: s.id, label: `${s.id} · ${s.kind}`, kind: 'surface', surf: s }));
  else if (k === 'device') (app.svc ? app.svc.nodes : []).filter(t => !a.nodeTypes || a.nodeTypes.includes(t.d.type)).forEach(t => out.push({ id: t.id, label: t.d.label.split(',')[0], kind: 'device', type: t.d.type }));
  else if (k === 'opening') app.stateList().filter(s => s.kind === a.openingKind).forEach(s => out.push({ id: s.id, label: s.label, kind: 'opening' }));
  else if (k === 'structure') ((app.systems && app.systems.structure && app.systems.structure.elements) || []).forEach(e => out.push({ id: e.guid || e.id, label: e.name || e.id, kind: 'structure' }));
  return out;
}
let lastVar = null, lastThing = null;   // the variable and the thing whose control is shown: a redraw (a value changed) puts it back
export function renderExplorer(box, keep) {
  const defs = defsAll(); if (!defs.length) { box.innerHTML = '<p class="note">No state variables in the data.</p>'; return; }
  const cur = defs.find(d => d.id === ((keep && keep.v) || lastVar)) || defs[0]; lastVar = cur.id;
  const api = app.statevars && typeof app.statevars.setValue === 'function';
  box.innerHTML = `<select class="sel" id="varSel" style="width:100%" aria-label="State variable">${defs.map(d => `<option value="${esc(d.id)}" ${d.id === cur.id ? 'selected' : ''}>${esc(d.label)}${d.variableKind === 'enumerated' ? ' (states)' : d.unitLabel ? ' · ' + esc(d.unitLabel) : ''}</option>`).join('')}</select>
    <div class="vlim" style="margin-top:6px"><span>${cur.variableKind === 'numeric' ? `${cur.min} to ${cur.max} ${esc(cur.unitLabel || '')}` : cur.values.map(v => v.label).join(' · ')}</span><span>set by ${esc(cur.setBy || 'data')}</span></div>
    ${cur.levels && cur.levels.length ? `<div class="ins-chips" style="margin:6px 0">${cur.levels.map(l => levelChip(l)).join('')}</div>` : ''}
    ${cur.variableKind === 'numeric' ? `<div class="bar" style="margin-top:6px"><button class="btn sm" type="button" id="varColour" ${app.statevars && app.statevars.colourBy && cur.levels && cur.levels.length ? '' : 'aria-disabled="true" disabled title="Colouring by a variable: not available yet"'}>Colour the 3D view by this</button></div>` : ''}
    <ul class="svc-list" id="varThings" style="max-height:200px" aria-label="Things this variable applies to"></ul><div id="varCtl"></div>${api ? '' : '<p class="note">Setting numeric values: not available yet.</p>'}`;
  const list = box.querySelector('#varThings'), things = thingsOf(cur);
  things.slice(0, 80).forEach(t => {
    const v = valueOf(t.id, cur, { state: cur.variableKind === 'enumerated' ? (app.getState(t.id) || (app.svc && app.svc.thingFor(t.id) && app.svc.stateOf(app.svc.thingFor(t.id)))) : undefined });
    const lv = levelOf(cur, v, t.id), li = document.createElement('li'); li.tabIndex = 0; li.setAttribute('role', 'button'); li.dataset.id = t.id;
    li.innerHTML = `<span>${esc(t.label)}</span><span class="m">${v == null ? '—' : esc(typeof v === 'number' ? fmtVal(cur, v) : v)} ${lv ? levelChip(lv) : ''}</span>`;
    li.onclick = () => {
      if (t.kind === 'surface') import('./inspector.js').then(m => m.inspectSurface(t.id)); else { try { app.select(t.id); } catch { /* not in 3D */ } }
      lastThing = { v: cur.id, id: t.id }; li._ctl();
    };
    li._ctl = () => {
      const c = box.querySelector('#varCtl'), ctx = {};
      if (cur.variableKind === 'enumerated') { ctx.set = s => { if (t.kind === 'structure') { s === 'removed' ? app.structure.remove(t.id) : app.structure.restore(t.id); } else app.setState(t.id, s); renderExplorer(box, { v: cur.id }); }; ctx.state = app.getState(t.id); }
      renderVars(c, [{ d: cur, id: t.id, ctx }]);
    };
    list.append(li);
  });
  if (lastThing && lastThing.v === cur.id) { const li = list.querySelector('li[data-id="' + lastThing.id + '"]'); if (li && li._ctl) li._ctl(); }
  if (things.length > 80) list.insertAdjacentHTML('beforeend', `<li class="muted">+ ${things.length - 80} more</li>`);
  if (!things.length) list.innerHTML = '<li class="muted">Nothing in the model has this variable.</li>';
  box.querySelector('#varSel').onchange = e => renderExplorer(box, { v: e.target.value });
  const cb = box.querySelector('#varColour'); if (cb && !cb.disabled) cb.onclick = () => { try { const on = app.statevars.legend && app.statevars.legend() && app.statevars.legend().variable === cur.id; app.statevars.colourBy(on ? null : cur.id); cb.textContent = on ? 'Colour the 3D view by this' : 'Stop colouring'; } catch (e) { app.notify(String(e.message || e)); } };
}
