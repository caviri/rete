// The Graph tab: graphview.js mounted lazily on first open, wired both ways to the 3D scene.
//   3D selection -> focus in the graph (when the tab is visible, else on the next visit); graph selection -> select in 3D;
//   highlights of results, scenarios and traces; state tints (live or dead, removed, temperature level) through stateOf;
//   groupings that give every kind of thing a meaningful group.
// app.graphTab = { focus(iri, grouping), mounted, controller, state() }
import { app } from './app.js';
import { $, esc, ui } from './shared.js';
import { runSparql } from './queries.js';
import { valueOf, levelOf, defsAll } from './statevars-ui.js';

const HOUSE = 'https://w3id.org/rete/cad/fzk-haus/';
const SYS = 'https://w3id.org/rete/cad/sys#', BOT = 'https://w3id.org/bot#', CAD = 'https://w3id.org/rete/cad#', SOSA = 'http://www.w3.org/ns/sosa/';
const NS = `PREFIX bot: <${BOT}> PREFIX cad: <${CAD}> PREFIX sys: <${SYS}> PREFIX sosa: <${SOSA}> PREFIX rdfs: <http://www.w3.org/2000/01/rdf-schema#>`;
// The built-in "system" grouping leaves everything that has no sys:system in "(none)". This one names the rest: a table of
// [group, label, types]; a thing that has no system and none of these types still lands in "Other", so no "(none)" cluster remains.
const RDF = 'http://www.w3.org/1999/02/22-rdf-syntax-ns#', OWL = 'http://www.w3.org/2002/07/owl#', RDFS = 'http://www.w3.org/2000/01/rdf-schema#', GEO3 = 'https://w3id.org/rete/geo3#';
const ROOT_GROUPS = [
  ['House spaces', 'House: rooms, storeys, site', [BOT + 'Space', BOT + 'Storey', BOT + 'Building', BOT + 'Site'], 'FILTER NOT EXISTS { ?n cad:ifcClass ?c0 }'],
  ['Thermal model', 'Thermal model', [SYS + 'Zone', SYS + 'Surface', SYS + 'Construction', SYS + 'Glazing', SYS + 'Shading', SYS + 'Site']],
  ['State definitions', 'State definitions and rules', [SYS + 'StateVariable', SYS + 'Level', SYS + 'StateValue', SYS + 'Rule', SYS + 'BlockingState']],
  ['Observations', 'Observations (state values)', [SOSA + 'Observation']],
  ['Geometry', 'Geometry (3D shapes)', [GEO3 + 'Geometry3D']],
  ['Vocabulary', 'Vocabulary and dataset description', [SYS + 'System', OWL + 'Class', OWL + 'ObjectProperty', OWL + 'DatatypeProperty', OWL + 'SymmetricProperty', OWL + 'AnnotationProperty', OWL + 'Ontology', RDFS + 'Class', RDF + 'Property', 'http://www.w3.org/ns/dcat#Dataset']],
];
const NO_SYS = 'FILTER NOT EXISTS { ?n sys:system ?x0 }';
const iriList = ts => ts.map(t => '<' + t + '>').join(', ');
const SYSTEM_SPARQL = `${NS} SELECT DISTINCT ?n ?g ?gl WHERE {
  { ?n sys:system ?g . OPTIONAL { ?g rdfs:label ?gl . FILTER(LANG(?gl) = "en" || LANG(?gl) = "") } }
  UNION { ?n cad:ifcClass ?c . ${NO_SYS} BIND("House elements" AS ?g) BIND("House elements (by IFC class)" AS ?gl) }
  ${ROOT_GROUPS.map(([g, l, types, extra]) => `UNION { ?n a ?t . FILTER(?t IN (${iriList(types)})) ${NO_SYS} ${extra || ''} BIND("${g}" AS ?g) BIND("${l}" AS ?gl) }`).join(' ')}
  UNION { ?n a ?t . FILTER(?t NOT IN (${iriList(ROOT_GROUPS.flatMap(x => x[2]))})) ${NO_SYS} FILTER NOT EXISTS { ?n cad:ifcClass ?c1 } BIND("Other" AS ?g) BIND("Other things" AS ?gl) }
}`;
const SYSTEM_GROUPING = { id: 'system', label: 'System', requires: [SYS + 'system'], levels: [{ label: 'System', sparql: SYSTEM_SPARQL }, { type: 'class', label: 'Class' }] };

let gv = null, mountP = null, host = null, pendingIri = null, syncing = false, hlOn = false, tintT = 0;
const visible = () => ui.botOpen && ui.botTab === 'graph';
const iriForSel = e => (!e || !e.id ? null : e.kind === 'pickable' ? (app.systems && app.systems.base || HOUSE + 'sys/') + e.id : HOUSE + e.id);
function thingOfIri(iri) {
  if (!iri) return null; const base = app.systems && app.systems.base;
  if (base && iri.startsWith(base)) return { id: iri.slice(base.length), pick: true };
  if (iri.startsWith(HOUSE)) { const g = iri.slice(HOUSE.length); if (app.nodeByGuid.has(g)) return { id: g }; }
  return null;
}
const labelOf = iri => { const t = thingOfIri(iri); if (!t) return undefined; if (t.pick) { const th = app.svc && app.svc.thingFor(t.id); return th ? th.d.label : undefined; } const n = app.nodeByGuid.get(t.id); return n ? n.l : undefined; };
const colorOf = iri => { const t = thingOfIri(iri); if (t && t.pick && app.svc) { const th = app.svc.thingFor(t.id); if (th) return app.svc.sys.get(th.system).color; } return undefined; };
function structState(id) {   // removed, or lost its support (structure.js)
  if (!app.structure || typeof app.structure.stateOf !== 'function') return null;
  try { if (app.structure.stateOf(id) === 'removed') return { label: 'removed', color: '#ff9f43' }; const u = app.structure.state().unsupported.find(x => x.id === id || x.guid === id); if (u) return { label: 'unsupported: ' + (u.reason || ''), color: '#ff5a5a' }; } catch { /* not a structural thing */ }
  return null;
}
function stateOf(iri) {
  const t = thingOfIri(iri); if (!t) return undefined;
  if (t.pick) {
    const sr = structState(t.id); if (sr) return sr;
    const th = app.svc && app.svc.thingFor(t.id); if (!th) return undefined;
    if (!th.live) return { label: 'no supply' + (th.why ? ': ' + th.why.text : ''), color: '#ff5a5a' };
    if (th.hasState && /^(off|closed)$/.test(app.svc.stateOf(th))) return { label: app.svc.stateOf(th), color: '#9aa4b2' };
    return undefined;
  }
  const n = app.nodeByGuid.get(t.id); if (!n) return undefined;
  const sr2 = structState(t.id); if (sr2) return sr2;
  if (n.k === 'Space') {   // the temperature level of the room: from the last thermal run when there is one, else the initial value
    const d = defsAll().find(x => x.id === 'room-air-temperature'); if (!d) return undefined;
    let v = null; const th = app.thermal;
    if (th && th.last && th.last() && th.zones) { try { const T = th.tempsAt(app.sun.get().time), z = th.zones.find(x => x.guid === n.n); if (z) v = T[z.i]; } catch { /* no run */ } }
    if (v == null) v = valueOf(n.n, d);
    const lv = levelOf(d, v, n.n); return lv ? { label: `${lv.label} (${(+v).toFixed(1)} °C)`, color: lv.color } : undefined;
  }
  const s = app.getState(n.n); if (s === 'open' || s === 'tilted') return { label: s, color: '#5aa9ff' };
  return undefined;
}
export function refreshTints() { if (!gv) return; clearTimeout(tintT); tintT = setTimeout(() => { try { gv.updateStates(); } catch { /* destroyed */ } }, 120); }

function irisOfResult(r) {
  const out = []; for (const i of r.idxs || []) { const n = app.graph.nodes[i]; if (n) out.push(HOUSE + n.n); }
  const base = app.systems && app.systems.base; for (const p of r.picks || []) out.push((base || HOUSE + 'sys/') + p);
  return out;
}
function highlightFrom(ids) { if (!gv) return; const base = app.systems && app.systems.base || HOUSE + 'sys/'; gv.highlight(ids.map(x => (x.startsWith('http') ? x : base + x))); hlOn = ids.length > 0; }

async function mount() {
  if (gv) { gv.resize(); applyPending(); return gv; }
  if (mountP) return mountP;
  const root = $('tp-graph');
  root.innerHTML = '<div id="graphHost"><div class="gmsg" id="gmsg">Loading the graph…</div></div>';
  host = $('graphHost'); const t0 = performance.now();
  mountP = (async () => {
    const [{ mountGraphView }] = await Promise.all([import('./graphview.js')]);
    const query = async q => { const j = await runSparql(q, null, { quiet: true }); return { vars: j.vars || [], rows: j.rows || [] }; };
    await query('ASK { ?s ?p ?o }');   // the engine and the graph are loaded before the view asks
    const div = document.createElement('div'); div.className = 'gvh'; host.append(div);
    gv = mountGraphView(div, {
      query, theme: ui.theme === 'light' ? 'light' : 'dark', labelOf, colorOf, stateOf, groupings: [SYSTEM_GROUPING],
      onSelect: iri => { if (syncing) return; const t = thingOfIri(iri); if (!iri) return; if (t) { syncing = true; try { app.select(t.id); } catch { /* not in 3D */ } finally { syncing = false; } } else if (/\/sys\/surf-/.test(iri)) { import('./inspector.js').then(m => { try { m.inspectSurface(iri.split('/').pop()); } catch { /* unknown surface */ } }); } },
    });
    app.graphView = gv;
    await gv.ready; const g = $('gmsg'); if (g) g.remove();
    const st = gv.getState();
    if (app.log) app.log('info', 'data', `Graph view ready: ${st.universe} nodes in ${st.counts.clusters} groups · ${Math.round(performance.now() - t0)} ms`);
    applyPending(); return gv;
  })();
  mountP.catch(e => { host.innerHTML = `<div class="gmsg">The graph view could not start: ${esc(e.message || e)}</div>`; mountP = null; if (app.log) app.log('error', 'data', 'Graph view failed: ' + (e.message || e)); });
  return mountP;
}
function applyPending() {
  const sel = app.selectedInfo && app.selectedInfo(); const iri = pendingIri || iriForSel(sel); pendingIri = null;
  if (iri && gv) { syncing = true; gv.focus(iri).finally(() => { syncing = false; }); }
}
export async function focusIri(iri, grouping) {
  pendingIri = iri; app.shell.openTab('graph'); const g = await mount();
  if (grouping) { try { await g.setGrouping(grouping); } catch { /* unknown grouping */ } }
  syncing = true; try { return await g.focus(iri); } finally { syncing = false; pendingIri = null; }
}
export function initGraphTab() {
  app.shell.onTab('graph', () => { mount().then(() => gv && gv.resize()); });
  app.on('select', e => { if (!e || !e.id) return; const iri = iriForSel(e); if (!gv || !visible()) { pendingIri = iri; return; } if (syncing) return; syncing = true; gv.focus(iri).finally(() => { syncing = false; }); });
  app.on('result', e => { if (!gv) return; const r = app.uiView && app.uiView.result; if (r) highlightFrom(irisOfResult(r)); });
  app.on('resultclear', () => { if (gv && hlOn) { gv.highlight([]); hlOn = false; } });
  app.on('scenario', r => { if (!gv) return; if (r) highlightFrom([...r.off, ...r.affected, ...(r.affectedRuns || [])]); else if (hlOn) { gv.highlight([]); hlOn = false; } });
  app.on('trace', t => { if (!gv) return; if (t) highlightFrom([...t.path, ...t.runs]); else if (hlOn) { gv.highlight([]); hlOn = false; } });
  for (const ev of ['svclive', 'state', 'thermal', 'thermalview', 'structure', 'statevar', 'reset', 'xray']) app.on(ev, refreshTints);
  let tt = 0; app.on('time', () => { if (gv && app.thermal && app.thermal.last && app.thermal.last()) { clearTimeout(tt); tt = setTimeout(refreshTints, 250); } });
  app.on('theme', async () => { if (!gv) return; const iri = gv.getState().selected; gv.destroy(); gv = null; mountP = null; app.graphView = null; if (visible()) { await mount(); if (iri) gv.focus(iri); } });
  app.graphTab = { focus: focusIri, ready: mount, get mounted() { return !!gv; }, get controller() { return gv; }, state: () => (gv ? gv.getState() : null), refresh: refreshTints };
}
