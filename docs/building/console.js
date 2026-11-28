// The SPARQL console and the results table.
//   app.results = { show(r, {open}), clear(), get(), csv(), json(), sortBy(col, dir), filter(text), rows() }
//   app.console = { run(q?), setText(q), getText(), history(), runExample(k) }
// Results of any origin (a ready query, a scenario, the console, an agent's run_sparql) come through app.showResult(r),
// which ends here. A result is { label, cols:[{k,h}], rows:[{...}], idxs, picks, query?, ms?, source? }; a row may carry
// __i (house element index), __p (service thing id) and __ref {var: {i}|{p}} for the cells that are things of the model.
import { app } from './app.js';
import { $, esc, shortTerm, prefixLines, PREFIXES } from './shared.js';
import { icon } from './icons.js';
import { QNS, PRE_SERVICES, runSparql, toResult } from './queries.js';

const KW = 'SELECT|CONSTRUCT|DESCRIBE|ASK|WHERE|FROM|NAMED|GRAPH|OPTIONAL|UNION|MINUS|FILTER|BIND|VALUES|SERVICE|GROUP|BY|ORDER|HAVING|LIMIT|OFFSET|DISTINCT|REDUCED|AS|ASC|DESC|PREFIX|BASE|IN|NOT|EXISTS|COUNT|SUM|MIN|MAX|AVG|SAMPLE|GROUP_CONCAT|SEPARATOR|STR|LANG|LANGMATCHES|DATATYPE|BOUND|IF|COALESCE|REGEX|REPLACE|SUBSTR|STRLEN|UCASE|LCASE|CONTAINS|STRSTARTS|STRENDS|ROUND|CEIL|FLOOR|ABS|isIRI|isURI|isBLANK|isLITERAL|sameTerm|CONCAT|a';
const TOK = new RegExp(`(#[^\\n]*)|(<[^<>"{}|^\`\\\\\\s]*>)|("(?:[^"\\\\\\n]|\\\\.)*"|'(?:[^'\\\\\\n]|\\\\.)*')|([?$][A-Za-z_][\\w]*)|(\\b\\d+(?:\\.\\d+)?\\b)|(\\b(?:${KW})\\b)`, 'gi');
export function highlight(text) {
  let out = '', last = 0;
  text.replace(TOK, (m, cmt, iri, str, v, num, kw, off) => {
    out += esc(text.slice(last, off)); last = off + m.length;
    const c = cmt ? 'cmt' : iri ? 'iri' : str ? 'str' : v ? 'var' : num ? 'num' : 'kw';
    out += `<span class="tk-${c}">${esc(m)}</span>`; return m;
  });
  return out + esc(text.slice(last)) + '\n';
}

// ---------- the table, shared by Results and the console preview ----------
function cellText(row, k) { const v = row[k]; return v == null ? '' : String(v); }
const shortCell = s => (/^https?:\/\//.test(s) ? shortTerm('<' + s + '>') : s);
const isNum = s => s !== '' && !isNaN(+s) && /^-?[\d.]+(e[+-]?\d+)?$/i.test(s);
function refOf(row, k) { const r = row.__ref && row.__ref[k]; if (r) return r; return null; }
export function selectRef(ref) {
  if (!ref) return false;
  try { if (ref.i != null) app.select(app.graph.nodes[ref.i]); else if (ref.p) app.select(ref.p); return true; } catch (e) { app.notify && app.notify(String(e.message || e)); return false; }
}
function tableHtml(cols, rows, { sort, sortable = !!sort, cap = 500, numbers = true } = {}) {   // sortable: the headers are buttons (aria-sort), also while nothing is sorted yet
  const head = `<thead><tr>${numbers ? '<th class="rn" aria-label="Row">#</th>' : ''}${cols.map(c => `<th data-k="${esc(c.k)}" ${sortable ? `aria-sort="${sort && sort.k === c.k ? (sort.dir > 0 ? 'ascending' : 'descending') : 'none'}" tabindex="0" role="columnheader"` : ''}>${esc(c.h)}</th>`).join('')}</tr></thead>`;
  const body = rows.slice(0, cap).map((r, n) => {
    const rowRef = r.__i != null ? { i: r.__i } : r.__p ? { p: r.__p } : null;
    return `<tr data-n="${r.__n}" ${rowRef ? 'data-row-ref="1"' : ''}>${numbers ? `<td class="rn">${n + 1}</td>` : ''}${cols.map(c => {
      const raw = cellText(r, c.k), ref = refOf(r, c.k), txt = shortCell(raw);
      return `<td class="${isNum(raw) ? 'num' : ''}${ref ? ' ref' : ''}" ${ref ? `data-ref='${esc(JSON.stringify(ref))}'` : ''} title="${esc(raw)}">${esc(txt)}</td>`;
    }).join('')}</tr>`;
  }).join('');
  return head + `<tbody>${body}</tbody>`;
}

// ---------- results ----------
let res = null, view = [], sortSt = null, filterTxt = '';
const CAP = 500;
function recompute() {
  if (!res) { view = []; return; }
  let rows = res.rows;
  if (filterTxt) { const t = filterTxt.toLowerCase(); rows = rows.filter(r => res.cols.some(c => cellText(r, c.k).toLowerCase().includes(t))); }
  if (sortSt) { const k = sortSt.k, d = sortSt.dir; rows = rows.slice().sort((a, b) => { const x = cellText(a, k), y = cellText(b, k); const nx = isNum(x) && isNum(y); return d * (nx ? +x - +y : x.localeCompare(y, 'en', { numeric: true })); }); }
  view = rows;
}
function drawResults() {
  const t = $('results'), empty = $('resEmpty');
  if (!res) { t.hidden = true; empty.hidden = false; $('footer-title').textContent = 'Results'; $('footer-count').textContent = ''; return; }
  recompute(); empty.hidden = true; t.hidden = false;
  t.innerHTML = tableHtml(res.cols, view, { sort: sortSt, sortable: true, cap: CAP });
  $('footer-title').textContent = res.label || 'Result';
  $('footer-count').textContent = `${filterTxt ? view.length + ' of ' : ''}${res.rows.length} row${res.rows.length === 1 ? '' : 's'}${view.length > CAP ? ` · showing ${CAP}` : ''}${res.ms != null ? ` · ${res.ms} ms` : ''}`;
}
function bindResultsTable() {
  const t = $('results');
  const sortBy = th => { if (!th || !th.dataset.k) return; const k = th.dataset.k; sortSt = sortSt && sortSt.k === k ? (sortSt.dir > 0 ? { k, dir: -1 } : null) : { k, dir: 1 }; drawResults(); };
  t.addEventListener('click', e => {
    const th = e.target.closest('th'); if (th) { sortBy(th); return; }
    const td = e.target.closest('td'); if (!td) return;
    if (td.dataset.ref) { selectRef(JSON.parse(td.dataset.ref)); markRow(td.parentElement); return; }
    const tr = td.parentElement; if (tr.dataset.rowRef) { const r = res.rows[+tr.dataset.n]; selectRef(r.__i != null ? { i: r.__i } : { p: r.__p }); markRow(tr); }
  });
  t.addEventListener('keydown', e => { if ((e.key === 'Enter' || e.key === ' ') && e.target.matches('th')) { e.preventDefault(); sortBy(e.target); } });
  $('resFilter').addEventListener('input', e => { filterTxt = e.target.value.trim(); drawResults(); });
  $('resClear').onclick = () => app.clearResult();
  $('resCsv').onclick = () => copy(toCsv(), 'CSV copied');
  $('resJson').onclick = () => copy(toJson(), 'JSON copied');
}
const markRow = tr => { $('results').querySelectorAll('tr.sel').forEach(x => x.classList.remove('sel')); tr.classList.add('sel'); };
async function copy(text, ok) { try { await navigator.clipboard.writeText(text); app.notify(ok); } catch { app.notify('Could not copy to the clipboard'); } }
export function toCsv() { if (!res) return ''; recompute(); const q = v => { v = String(v ?? ''); return /[",\n]/.test(v) ? '"' + v.replace(/"/g, '""') + '"' : v; }; return [res.cols.map(c => q(c.h)).join(','), ...view.map(r => res.cols.map(c => q(cellText(r, c.k))).join(','))].join('\n'); }
export function toJson() { if (!res) return '[]'; recompute(); return JSON.stringify(view.map(r => Object.fromEntries(res.cols.map(c => [c.h, cellText(r, c.k)]))), null, 1); }
function showResultTable(r, opts = {}) {
  res = { ...r, rows: r.rows.map((x, n) => { x.__n = n; return x; }) }; sortSt = null; filterTxt = ''; $('resFilter').value = '';
  drawResults();
  const n = res.rows.length; app.shell && app.shell.setBadge('results', n > 999 ? '999+' : String(n), true);
  if (opts.open || (app.shell && !app.shell.state().bottom)) app.shell.openTab('results');
  else if (app.shell && app.shell.state().bottom === 'results') app.shell.setBadge('results', null);
  consolePreview();
}

// ---------- the console ----------
const HIST = []; let ed, gut, hl, ta, busy = false, conEsc = false;
const defaultQuery = `${QNS}\nSELECT ?room ?neighbour WHERE {\n  ?r a bot:Space ; rdfs:label ?room ; cad:adjacentSpace ?n .\n  ?n rdfs:label ?neighbour .\n  FILTER(LANG(?room) = "de" && LANG(?neighbour) = "de")\n} ORDER BY ?room`;
function paintEditor() { hl.innerHTML = highlight(ta.value); const n = ta.value.split('\n').length; gut.textContent = Array.from({ length: n }, (_, i) => i + 1).join('\n'); }
function syncScroll() { hl.scrollTop = ta.scrollTop; hl.scrollLeft = ta.scrollLeft; gut.scrollTop = ta.scrollTop; }
function conMsg(html, cls = '') { const m = $('conMsg'); m.className = 'con-msg ' + cls; m.innerHTML = html; }
function consolePreview() {
  const box = $('conPrev'); if (!box) return;
  if (!res || res.source !== 'console') { box.hidden = true; return; }
  box.hidden = false; const t = document.createElement('table'); t.className = 'tbl'; t.innerHTML = tableHtml(res.cols, res.rows.slice(0, 8), { cap: 8 });
  box.innerHTML = ''; box.append(t);
  t.addEventListener('click', e => { const td = e.target.closest('td'); if (td && td.dataset.ref) selectRef(JSON.parse(td.dataset.ref)); });
}
function histDraw() {
  const ul = $('conHist'); ul.innerHTML = '';
  HIST.slice().reverse().forEach(h => {
    const li = document.createElement('li'); li.className = h.ok ? '' : 'bad';
    li.innerHTML = `<span class="q" title="${esc(h.q)}">${esc(h.q.replace(/PREFIX[^\n]*\n/g, '').replace(/\s+/g, ' ').trim().slice(0, 80))}</span><span class="meta">${h.ok ? h.rows + ' · ' + h.ms + ' ms' : 'error'}</span><button class="ibtn" type="button" title="Run again" aria-label="Run again">${icon('play')}</button><button class="ibtn" type="button" title="Copy the query" aria-label="Copy the query">${icon('copy')}</button>`;
    const [b1, b2] = li.querySelectorAll('button');
    b1.onclick = () => { setText(h.q); run(); }; b2.onclick = () => copy(h.q, 'Query copied');
    li.querySelector('.q').onclick = () => { setText(h.q); ta.focus(); };
    ul.append(li);
  });
  if (!HIST.length) ul.innerHTML = '<li><span class="q muted">Nothing run yet.</span></li>';
}
const setText = q => { ta.value = q; paintEditor(); syncScroll(); };
function graphLabel() {
  const g = app.graphInfo; const el = $('conGraph'); if (!el) return;
  el.innerHTML = g ? `${icon('graph')}<span><b>${g.combined ? 'House + services' : 'House only'}</b> · ${g.triples != null ? Number(g.triples).toLocaleString('en') + ' triples' : '…'} · ${esc(String(g.url || '').split('/').pop())}</span>` : `${icon('graph')}<span>Graph loads on the first run</span>`;
}
async function run(q) {
  if (busy) return; if (q != null) setText(q);
  const text = ta.value; if (!text.trim()) { conMsg('Write a SPARQL query first.'); return; }
  busy = true; $('conRun').disabled = true; conMsg('Running…');
  const t0 = performance.now();
  try {
    const j = await runSparql(text, m => { if (m) conMsg(esc(m)); });
    const ms = Math.round(performance.now() - t0);
    if (j.kind === 'ask') { conMsg(`ASK → <b>${j.boolean}</b> · ${ms} ms`); HIST.push({ q: text, ok: true, rows: 1, ms }); }
    else if (j.kind === 'select') {
      const r = toResult('Console query', j); r.query = text; r.ms = ms; r.source = 'console';
      app.showResult(r, { highlight: $('conHl').checked });
      conMsg(`<b>${j.rows.length}</b> row${j.rows.length === 1 ? '' : 's'} · ${ms} ms · <span class="muted">${(r.idxs.length + (r.picks || []).length) ? (r.idxs.length + (r.picks || []).length) + ' things of the model' + ($('conHl').checked ? ' highlighted in 3D' : '') : 'no things of the model in these rows'}</span> <button class="lnk" type="button" id="conToRes">All rows in Results</button>`);
      const b = $('conToRes'); if (b) b.onclick = () => app.shell.openTab('results');
      HIST.push({ q: text, ok: true, rows: j.rows.length, ms });
    } else {
      const rows = (j.triples || j.rows || []).slice(0, 2000); const cols = j.vars ? j.vars.map(v => ({ k: v, h: v })) : [{ k: 's', h: 's' }, { k: 'p', h: 'p' }, { k: 'o', h: 'o' }];
      const rr = rows.map(x => Array.isArray(x) ? { s: shortTerm(x[0]), p: shortTerm(x[1]), o: shortTerm(x[2]) } : x);
      const r = { label: 'Console query (' + j.kind + ')', cols, rows: rr, idxs: [], picks: [], query: text, ms, source: 'console' };
      app.showResult(r, { highlight: false }); conMsg(`<b>${rr.length}</b> triples · ${ms} ms`); HIST.push({ q: text, ok: true, rows: rr.length, ms });
    }
  } catch (e) {
    const ms = Math.round(performance.now() - t0);
    conMsg(esc(e.message || String(e)), 'err'); HIST.push({ q: text, ok: false, rows: 0, ms });
  } finally { busy = false; $('conRun').disabled = false; if (HIST.length > 50) HIST.shift(); histDraw(); graphLabel(); }
}
function pickerFill() {
  const sel = $('conPick'); if (!sel) return;
  const ex = app.examples || [], groups = app.exampleGroups || [];
  let h = '<option value="">Ready queries…</option>';
  if (groups.length) groups.forEach(g => { h += `<optgroup label="${esc(g.title || 'The house')}">` + ex.slice(g.from, g.to).map((e, i) => `<option value="${g.from + i}">${esc(e.label)}</option>`).join('') + '</optgroup>'; });
  sel.innerHTML = h;
}
function exampleText(ex) { return QNS + (ex.pre || '') + '\n' + ex.q; }

export function initConsole() {
  const root = $('tp-console');
  root.innerHTML = `<div class="tp-bar">
      <button class="btn primary" type="button" id="conRun" title="Run (Ctrl+Enter)">${icon('play')}Run</button>
      <select class="sel" id="conPick" aria-label="Ready queries" style="max-width:260px"></select>
      <button class="btn" type="button" id="conPfx" title="Add the PREFIX lines the query uses">Prefixes</button>
      <button class="btn" type="button" id="conClear">Clear</button>
      <label class="opt" style="min-height:0"><input type="checkbox" id="conHl" checked>Highlight rows in 3D</label>
      <span class="grow"></span><span class="gsrc" id="conGraph"></span></div>
    <div class="con"><div class="con-main">
      <div class="editor"><div class="ed-gut" id="edGut" aria-hidden="true">1</div><div class="ed-wrap"><pre class="ed-hl" id="edHl" aria-hidden="true"></pre><textarea class="ed-ta" id="edTa" spellcheck="false" autocapitalize="off" autocomplete="off" wrap="off" aria-label="SPARQL query" aria-describedby="conHint"></textarea></div></div>
      <div class="con-msg" id="conMsg" role="status"></div>
      <div class="con-prev" id="conPrev" hidden></div></div>
      <div class="con-side"><h3>History</h3><ul class="hist" id="conHist" aria-label="Query history"></ul><span class="sr" id="conHint">Ctrl or Command plus Enter runs the query. Escape then Tab leaves the editor.</span></div></div>`;
  gut = $('edGut'); hl = $('edHl'); ta = $('edTa');
  ta.value = defaultQuery; paintEditor(); histDraw(); graphLabel(); conMsg('Ctrl+Enter runs the query. The rows go to Results and light up in 3D.');
  ta.addEventListener('input', paintEditor); ta.addEventListener('scroll', syncScroll);
  ta.addEventListener('keydown', e => {
    if (e.key === 'Enter' && (e.ctrlKey || e.metaKey)) { e.preventDefault(); run(); return; }
    if (e.key === 'Escape') { conEsc = true; return; }
    if (e.key === 'Tab' && !conEsc) {
      e.preventDefault(); const s = ta.selectionStart, en = ta.selectionEnd;
      if (e.shiftKey) { const ls = ta.value.lastIndexOf('\n', s - 1) + 1; if (ta.value.slice(ls, ls + 2) === '  ') { ta.setRangeText('', ls, ls + 2, 'preserve'); ta.selectionStart = Math.max(ls, s - 2); ta.selectionEnd = Math.max(ls, en - 2); } }
      else ta.setRangeText('  ', s, en, 'end');
      paintEditor(); return;
    }
    conEsc = false;
  });
  ta.addEventListener('blur', () => { conEsc = false; });
  $('conRun').onclick = () => run();
  $('conClear').onclick = () => { setText(''); ta.focus(); };
  $('conPfx').onclick = () => { const add = prefixLines(ta.value); if (!add.length) { app.notify('Every prefix the query uses is declared.'); return; } setText(add.join('\n') + '\n' + ta.value); };
  $('conPick').onchange = e => { const k = +e.target.value; if (e.target.value === '') return; const ex = app.examples[k]; setText(exampleText(ex) + (ex.run && !ex.q ? '' : '')); e.target.value = ''; ta.focus(); };
  app.on('graph', graphLabel); app.on('rendered-examples', pickerFill); pickerFill();
  bindResultsTable();
  app.results = { show: showResultTable, clear() { res = null; drawResults(); consolePreview(); app.shell.setBadge('results', null); }, get: () => res, csv: toCsv, json: toJson, rows: () => view,
    sortBy(k, dir = 1) { sortSt = k ? { k, dir } : null; drawResults(); }, filter(t) { filterTxt = t || ''; $('resFilter').value = filterTxt; drawResults(); } };
  app.console = { run, setText, getText: () => ta.value, history: () => HIST.map(h => ({ ...h })), runExample(k) { const ex = app.examples[k]; setText(exampleText(ex)); return run(); }, focus: () => ta.focus(), refreshPicker: pickerFill };
  app.shell.onTab('console', o => { setTimeout(() => { if (!o || o.focus !== false) ta.focus({ preventScroll: true }); }, 0); graphLabel(); });
  app.shell.onTab('results', () => { app.shell.setBadge('results', null); });
  drawResults();
}
export { PRE_SERVICES, PREFIXES };

// A small editor in the same style as the console's (line numbers, highlighted overlay, Tab inserts spaces). Returns {ta, set, get}.
export function makeEditor(root, { value = '', label = 'Editor', onRun } = {}) {
  root.classList.add('editor');
  root.innerHTML = `<div class="ed-gut" aria-hidden="true">1</div><div class="ed-wrap"><pre class="ed-hl" aria-hidden="true"></pre><textarea class="ed-ta" spellcheck="false" autocapitalize="off" autocomplete="off" wrap="off" aria-label="${esc(label)}"></textarea></div>`;
  const gut = root.querySelector('.ed-gut'), hlp = root.querySelector('.ed-hl'), t = root.querySelector('.ed-ta'); let esc2 = false;
  const paint = () => { hlp.innerHTML = highlight(t.value); gut.textContent = Array.from({ length: t.value.split('\n').length }, (_, i) => i + 1).join('\n'); };
  const sync = () => { hlp.scrollTop = t.scrollTop; hlp.scrollLeft = t.scrollLeft; gut.scrollTop = t.scrollTop; };
  t.addEventListener('input', paint); t.addEventListener('scroll', sync); t.addEventListener('blur', () => { esc2 = false; });
  t.addEventListener('keydown', e => {
    if (e.key === 'Enter' && (e.ctrlKey || e.metaKey) && onRun) { e.preventDefault(); onRun(); return; }
    if (e.key === 'Escape') { esc2 = true; return; }
    if (e.key === 'Tab' && !esc2 && !e.shiftKey) { e.preventDefault(); t.setRangeText('  ', t.selectionStart, t.selectionEnd, 'end'); paint(); return; }
    esc2 = false;
  });
  t.value = value; paint();
  return { ta: t, set(v) { t.value = v; paint(); sync(); }, get: () => t.value };
}
