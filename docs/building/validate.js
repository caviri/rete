// The Validate tab: SHACL example shapes from the data (systems JSON `shaclExamples`), an editable Turtle editor, a Run button
// and the report. The engine call is the playground's: Graph.shacl(shapesTurtle, null, 'json') -> {conforms, results:[{focusNode,
// resultPath, resultSeverity, sourceConstraintComponent, resultMessage|messages, value}]}.
//   app.validate = { examples(), select(id), run({id, shapes}) -> report, report(), supported() }
import { app } from './app.js';
import { $, esc, shortTerm } from './shared.js';
import { icon } from './icons.js';
import { getEngine, resolveNode, pickOf } from './queries.js';
import { makeEditor } from './console.js';

let ed = null, cur = null, last = null, busy = false;
const lang = o => (o && (o.en || Object.values(o)[0])) || '';
const exs = () => ((app.systems && app.systems.shaclExamples) || []).map(e => ({ id: e.id, label: lang(e.labels), description: lang(e.descriptions), shapes: e.shapes, expect: e.expect, expectedCount: e.expectedCount, expectedFocus: e.expectedFocus || [], focus: e.focus }));
const term = t => { t = String(t == null ? '' : t); return t[0] === '<' || t[0] === '"' || t.startsWith('_:') ? t : '<' + t + '>'; };
function thingOf(t) {
  const x = term(t); const n = resolveNode(x); if (n) return { i: n.i, id: n.n };
  const p = pickOf(x); if (p && app.svc && app.svc.has(p)) return { p, id: p };
  return null;
}
export const supported = async () => { try { const g = await getEngine(); return typeof g.shacl === 'function'; } catch { return false; } };

function drawList() {
  const box = $('valList'); if (!box) return; const list = exs();
  if (!list.length) { box.innerHTML = '<p class="note" style="padding:8px 12px">No SHACL examples in the data yet. You can still write shapes in the editor.</p>'; return; }
  box.innerHTML = list.map(e => `<button type="button" class="scen-btn${cur && cur.id === e.id ? ' on' : ''}" data-id="${esc(e.id)}">${esc(e.label)}<small>${esc(e.description)}</small></button>`).join('');
}
export function select(id) {
  const e = exs().find(x => x.id === id); if (!e) throw new Error('No SHACL example "' + id + '". Examples: ' + exs().map(x => x.id).join(', ') + '.');
  cur = e; ed.set(e.shapes || ''); drawList(); return e;
}
function show(rep, ex, ms) {
  const box = $('valReport'); const rs = rep.results || [];
  const things = rs.map(r => thingOf(r.focusNode)).filter(Boolean);
  const mism = ex && ex.expectedCount != null && rs.length !== ex.expectedCount;
  let h = `<div class="con-msg" style="border-top:0"><b class="${rep.conforms ? 'live-ok' : 'live-bad'}">${rep.conforms ? 'Conforms' : 'Does not conform'}</b><span>${rs.length} violation${rs.length === 1 ? '' : 's'}</span><span class="muted">${ms} ms</span>${ex ? `<span class="muted">expected: ${esc(ex.expect)}${ex.expectedCount != null ? ', ' + ex.expectedCount : ''}</span>` : ''}</div>`;
  if (mism) h += `<div class="note" style="padding:0 12px" id="valMismatch">The count differs from the example's expected count (${ex.expectedCount}).</div>`;
  if (rs.length) h += `<table class="tbl" id="valTable"><thead><tr><th>Focus node</th><th>Path</th><th>Value</th><th>Message</th><th>Severity</th></tr></thead><tbody>${rs.slice(0, 500).map((r, n) => {
    const t = thingOf(r.focusNode), loc = s => String(s || '').split(/[#/]/).pop();
    return `<tr><td class="${t ? 'ref' : ''}" ${t ? `data-n="${n}"` : ''} title="${esc(r.focusNode)}">${esc(shortTerm(term(r.focusNode)))}</td><td title="${esc(r.resultPath || '')}">${esc(r.resultPath ? shortTerm(term(r.resultPath)) : '')}</td><td>${esc(r.value != null ? shortTerm(term(r.value)) : '')}</td><td>${esc((r.resultMessage || (r.messages || []).join(' ') || ''))}</td><td>${esc(loc(r.resultSeverity || r.severity))}</td></tr>`;
  }).join('')}</tbody></table>`;
  box.innerHTML = h;
  box.querySelectorAll('td.ref').forEach(td => td.onclick = () => { const t = thingOf(rs[+td.dataset.n].focusNode); if (t) { try { app.select(t.id); } catch (e) { app.notify(String(e.message || e)); } } });
  if ($('valHl').checked && things.length) {
    const idxs = [...new Set(things.filter(t => t.i != null).map(t => t.i))], picks = [...new Set(things.filter(t => t.p).map(t => t.p))];
    app.showResult({ label: 'Failing things: ' + (ex ? ex.label : 'shapes'), cols: [{ k: 'focus', h: 'Focus node' }, { k: 'msg', h: 'Message' }], rows: rs.map(r => ({ focus: shortTerm(term(r.focusNode)), msg: r.resultMessage || '' })), idxs, picks, source: 'validate' }, { open: false });
  }
}
export async function run({ id, shapes } = {}) {
  if (busy) throw new Error('A validation is running.');
  let ex = null; if (id) ex = select(id); else if (shapes != null) ed.set(shapes); else ex = cur;
  const text = ed.get(); if (!text.trim()) throw new Error('Enter SHACL shapes (Turtle).');
  busy = true; $('valRun').disabled = true; $('valReport').innerHTML = '<p class="note" style="padding:8px 12px">Validating…</p>';
  const t0 = performance.now(); const name = ex ? ex.id : 'custom shapes';
  try {
    const g = await getEngine(); if (typeof g.shacl !== 'function') throw new Error('This engine build has no SHACL validation.');
    const rep = JSON.parse(g.shacl(text, null, 'json')); const ms = Math.round(performance.now() - t0);
    last = { example: ex ? ex.id : null, conforms: !!rep.conforms, count: (rep.results || []).length, results: rep.results || [], ms, expectedCount: ex ? ex.expectedCount : null };
    show(rep, ex && ex.shapes === text ? ex : (ex ? { ...ex, shapes: text } : null), ms);
    if (app.log) app.log('info', 'query', `SHACL ${name}: ${rep.conforms ? 'conforms' : (rep.results || []).length + ' violations'} · ${ms} ms`, { conforms: !!rep.conforms, violations: (rep.results || []).length });
    app.shell.setBadge('validate', rep.conforms ? '✓' : String((rep.results || []).length), !rep.conforms);
    return last;
  } catch (e) {
    $('valReport').innerHTML = `<div class="con-msg err">${esc(e.message || e)}</div>`; last = { error: String(e.message || e) };
    if (app.log) app.log('error', 'query', `SHACL ${name} failed: ${String(e.message || e).slice(0, 200)}`);
    throw e;
  } finally { busy = false; $('valRun').disabled = false; }
}
export function initValidate() {
  const root = $('tp-validate');
  root.innerHTML = `<div class="tp-bar"><button class="btn primary" type="button" id="valRun" title="Run (Ctrl+Enter)">${icon('play')}Run</button><label class="opt" style="min-height:0"><input type="checkbox" id="valHl" checked>Highlight failing things in 3D</label><span class="grow"></span><span class="muted" id="valNote"></span></div>
    <div class="con"><div class="con-main"><div id="valEd" style="flex:1 1 120px;min-height:90px"></div><div id="valReport" class="tp-scroll" style="flex:1 1 120px"></div></div><div class="con-side"><h3>Examples</h3><div class="scen-list" id="valList" style="border:0;overflow:auto;flex:1"></div></div></div>`;
  ed = makeEditor($('valEd'), { value: '', label: 'SHACL shapes (Turtle)', onRun: () => run().catch(() => {}) });
  $('valRun').onclick = () => run().catch(() => {});
  $('valList').addEventListener('click', e => { const b = e.target.closest('[data-id]'); if (b) select(b.dataset.id); });
  const refresh = async () => {
    drawList(); const ok = await supported();
    $('valNote').textContent = ok ? '' : 'This engine build has no SHACL validation.';
    if (!ok) { $('valRun').disabled = true; $('valReport').innerHTML = '<p class="note" style="padding:8px 12px">Validation needs an engine build that exposes <code>Graph.shacl</code>. This one does not, so nothing can be run here.</p>'; }
    else if (!$('valReport').innerHTML) $('valReport').innerHTML = '<p class="note" style="padding:8px 12px">Choose an example or write shapes, then Run.</p>';
    if (!cur && exs().length && !ed.get()) select(exs()[0].id);
  };
  app.shell.onTab('validate', () => { refresh(); app.shell.setBadge('validate', null); });
  app.on('ready', refresh); app.on('interactive', drawList);
  app.validate = { examples: exs, select, run, report: () => last, supported };
  drawList();
}
