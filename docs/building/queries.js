// SPARQL: the ready examples (each also computed straight from the index, so the
// buttons work without the engine), the lazy wasm engine over the house graph, and
// turning an engine answer into a result the page can show and highlight.
import * as THREE from 'three';
import { app } from './app.js';
import { pretty } from './classes.js';
import { PREFIXES } from './shared.js';

export const RETE = 'https://data.graphplaza.com/cad/fzk-haus-2016-12-21.rete';
// With the services layer the engine reads the combined graph (house + services + thermal inputs) and
// falls back to the house-only file when that one is not there.
export const PRE_SERVICES = `
PREFIX brick: <https://brickschema.org/schema/Brick#>
PREFIX sys: <https://w3id.org/rete/cad/sys#>
PREFIX s: <https://w3id.org/rete/cad/fzk-haus/sys/>`;
export const IRI_BASE = 'https://w3id.org/rete/cad/fzk-haus/';
export const QNS = `PREFIX bot: <https://w3id.org/bot#>
PREFIX cad: <https://w3id.org/rete/cad#>
PREFIX geo: <http://www.opengis.net/ont/geosparql#>
PREFIX geo3: <https://w3id.org/rete/geo3#>
PREFIX geof3: <https://w3id.org/rete/geo3/function/>
PREFIX rdfs: <http://www.w3.org/2000/01/rdf-schema#>`;

const nodes = () => app.graph.nodes;
const floorOf = n => (n.st >= 0 && app.graph.storeys[n.st]) ? app.graph.storeys[n.st].l : '—';
export { floorOf };
const roomRows = idxs => idxs.map(i => { const n = nodes()[i]; return { i, room: n.l, floor: floorOf(n) }; });
const elRows = idxs => idxs.map(i => { const n = nodes()[i]; return { i, el: n.l, type: pretty(n.c), floor: floorOf(n) }; });
const d3 = (a, b) => { let s = 0; for (let k = 0; k < 3; k++) { const d = a[k] - b[k]; s += d * d; } return Math.sqrt(s); };
const findSpace = l => nodes().find(n => n.k === 'Space' && n.l === l);
const center = n => { const m = app.meshByGuid.get(n.n); if (!m) return n.cc; const b = new THREE.Box3().setFromObject(m); const c = b.getCenter(new THREE.Vector3()); return [c.x, c.y, c.z]; };

export function makeExamples() {
  const boundedBy = app.boundedBy;
  return [
    { label: 'Rooms on each floor', desc: "The building's spatial breakdown: every storey and the rooms it aggregates (BOT bot:hasSpace).",
      q: `SELECT ?floor ?room WHERE {\n  ?s a bot:Storey ; rdfs:label ?floor ; bot:hasSpace ?r .\n  ?r rdfs:label ?room .\n  FILTER(LANG(?floor) = "de" && LANG(?room) = "de")\n} ORDER BY ?floor ?room`,
      cols: [{ k: 'floor', h: 'Floor' }, { k: 'room', h: 'Room' }],
      run: ns => { const r = ns.filter(n => n.k === 'Space');
        const rows = r.map(n => ({ i: n.i, floor: floorOf(n), room: n.l })).sort((a, b) => a.floor.localeCompare(b.floor) || a.room.localeCompare(b.room));
        return { idxs: r.map(n => n.i), rows }; } },
    { label: 'Building element inventory', desc: 'How many of each IFC element type the model contains — the bill of parts.',
      q: `SELECT ?type (COUNT(?e) AS ?count) WHERE {\n  ?e a bot:Element ; cad:ifcClass ?type .\n} GROUP BY ?type ORDER BY DESC(?count)`,
      cols: [{ k: 'type', h: 'Element type' }, { k: 'count', h: 'Count' }],
      run: ns => { const e = ns.filter(n => n.k === 'Element'); const by = {}; e.forEach(n => by[n.c] = (by[n.c] || 0) + 1);
        const rows = Object.entries(by).map(([c, n]) => ({ type: pretty(c), count: n })).sort((a, b) => b.count - a.count);
        return { idxs: e.map(n => n.i), rows }; } },
    { label: 'Rooms next to the living room', desc: 'Spatial topology: rooms sharing a boundary with the living room (Wohnen), via cad:adjacentSpace derived from IFC space boundaries.',
      q: `SELECT ?neighbour WHERE {\n  ?w rdfs:label "Wohnen"@de ; cad:adjacentSpace ?n .\n  ?n rdfs:label ?neighbour . FILTER(LANG(?neighbour) = "de")\n}`,
      cols: [{ k: 'room', h: 'Adjacent room' }, { k: 'floor', h: 'Floor' }],
      run: ns => { const w = findSpace('Wohnen'); const idxs = w ? w.adj.slice() : []; return { idxs, rows: roomRows(idxs) }; } },
    { label: 'Walls that enclose the kitchen', desc: 'Which walls physically bound the kitchen (Küche) — IfcRelSpaceBoundary as cad:boundsSpace.',
      q: `SELECT ?wall WHERE {\n  ?k rdfs:label "Küche"@de .\n  ?e cad:boundsSpace ?k ;\n     cad:ifcClass "IfcWallStandardCase" ; rdfs:label ?wall . FILTER(LANG(?wall) = "de" || LANG(?wall) = "")\n}`,
      cols: [{ k: 'el', h: 'Wall' }, { k: 'floor', h: 'Floor' }],
      run: ns => { const k = findSpace('Küche'); if (!k) return { idxs: [], rows: [] };
        const idxs = (boundedBy[k.i] || []).filter(i => ns[i].c === 'IfcWallStandardCase');
        return { idxs, rows: elRows(idxs) }; } },
    { label: '10 elements nearest the staircase (geo3)', desc: '3D nearest-neighbours: the ten elements whose centre is closest to the staircase, ranked by the geo3 distance3D function — GeoSPARQL extended to 3D. Same engine as the anatomy explorer, now on a building.',
      q: `SELECT ?element ?type (ROUND(geof3:distance3D(?sw,?w)*100)/100 AS ?m) WHERE {\n  ?stair cad:ifcClass "IfcStair" ; geo:hasGeometry/geo3:asWKT3D ?sw .\n  ?e a bot:Element ; rdfs:label ?element ; cad:ifcClass ?type ;\n     geo:hasGeometry/geo3:asWKT3D ?w .\n  FILTER(?e != ?stair && ?type != "IfcOpeningElement" && (LANG(?element) = "de" || LANG(?element) = ""))\n} ORDER BY ?m LIMIT 10`,
      cols: [{ k: 'el', h: 'Element' }, { k: 'type', h: 'Type' }, { k: 'm', h: 'Distance (m)' }],
      run: ns => { const s = ns.find(n => n.c === 'IfcStair'); if (!s) return { idxs: [], rows: [] };
        const sc = center(s);
        const m = ns.filter(n => n !== s && n.k === 'Element' && n.c !== 'IfcOpeningElement').map(n => ({ n, d: d3(center(n), sc) })).sort((a, b) => a.d - b.d).slice(0, 10);
        return { idxs: m.map(x => x.n.i), rows: m.map(x => ({ i: x.n.i, el: x.n.l, type: pretty(x.n.c), m: x.d.toFixed(2) })) }; } },
    { label: 'How many walls enclose each room', desc: 'A per-room wall count — the enclosure degree of every space, from the boundary graph.',
      q: `SELECT ?room (COUNT(?e) AS ?walls) WHERE {\n  ?e cad:boundsSpace ?r ; cad:ifcClass "IfcWallStandardCase" .\n  ?r rdfs:label ?room . FILTER(LANG(?room) = "de")\n} GROUP BY ?room ORDER BY DESC(?walls)`,
      cols: [{ k: 'room', h: 'Room' }, { k: 'walls', h: 'Enclosing walls' }],
      run: ns => { const rooms = ns.filter(n => n.k === 'Space');
        const rows = rooms.map(n => ({ i: n.i, room: n.l, walls: (boundedBy[n.i] || []).filter(i => ns[i].c === 'IfcWallStandardCase').length })).sort((a, b) => b.walls - a.walls);
        return { idxs: rooms.map(n => n.i), rows }; } },
  ];
}

// ---------- the engine: whole-file, loaded lazily ----------
let engine = null, engineLoading = null;
export function getEngine(onMsg) {
  if (engine) return Promise.resolve(engine);
  if (engineLoading) return engineLoading;
  engineLoading = (async () => {
    if (onMsg) onMsg('loading the query engine + graph (~9 MB, once)…');
    const m = await import('../engine/rete_wasm.js'); await m.default();
    let bytes = null;
    for (const url of (app.systems && app.SYSTEMS_RETE ? [app.SYSTEMS_RETE, RETE] : [RETE])) {
      try { const r = await fetch(url); if (!r.ok) continue; bytes = new Uint8Array(await r.arrayBuffer()); app.graphSource = url; break; } catch { /* try the next */ }
    }
    if (!bytes) throw new Error('could not load the graph file.');
    const t0 = performance.now();
    engine = new m.Graph(bytes); if (onMsg) onMsg('');
    let triples = null; try { triples = JSON.parse(engine.info()).quads; } catch { /* older engine */ }
    let card = null; try { card = JSON.parse(engine.card()); } catch { /* no card */ }
    app.graphInfo = { url: app.graphSource, combined: /systems/.test(app.graphSource || ''), triples, bytes: bytes.length, card };
    if (app.log) app.log('info', 'data', `Graph opened: ${String(app.graphSource).split('/').pop()} · ${bytes.length} bytes · ${triples != null ? triples + ' triples · ' : ''}${Math.round(performance.now() - t0)} ms`);
    app.emit('graph', app.graphInfo);
    return engine;
  })();
  engineLoading.catch(() => { engineLoading = null; });
  return engineLoading;
}
export const cleanTerm = t => {
  if (t == null) return ''; if (String(t)[0] === '<') return String(t).slice(1, -1);
  const m = String(t).match(/^"([\s\S]*)"(?:@[\w-]+|\^\^.*)?$/); return m ? m[1] : String(t);
};
// a thing of the services layer: <base + id>
export const pickOf = t => { if (!t || String(t)[0] !== '<') return null; const b = app.systems && app.systems.base; const iri = String(t).slice(1, -1); return b && iri.startsWith(b) ? iri.slice(b.length) : null; };
export const resolveNode = t => {
  if (!t || String(t)[0] !== '<') return null; const iri = String(t).slice(1, -1);
  return iri.startsWith(IRI_BASE) ? app.nodeByGuid.get(iri.slice(IRI_BASE.length)) : null;
};
// run a query; returns the engine's JSON ({kind, vars, rows} or {kind:'ask', boolean})
export async function runSparql(q, onMsg, opts = {}) {
  const g = await getEngine(onMsg);
  const t0 = performance.now(); let j;
  const brief = String(q).replace(/PREFIX[^\n]*\n?/g, '').replace(/\s+/g, ' ').trim().slice(0, 160);
  try { j = JSON.parse(g.query(q, 'json')); }
  catch (e) { if (app.log) app.log('error', 'query', 'Query failed: ' + String(e.message || e).slice(0, 200), { query: brief }); throw e; }
  j.ms = Math.round(performance.now() - t0);
  if (app.log && !opts.quiet) app.log('info', 'query', `${j.kind || 'query'}: ${j.rows ? j.rows.length + ' rows' : j.boolean != null ? String(j.boolean) : ''} · ${j.ms} ms`, { query: brief });
  return j;
}
// the facts of one IRI, as a DESCRIBE would give them: what it says (first 200) and what points at it (first 100)
export async function describeIri(iri) {
  const o = await runSparql(`SELECT ?p ?o WHERE { <${iri}> ?p ?o } LIMIT 201`), i = await runSparql(`SELECT ?s ?p WHERE { ?s ?p <${iri}> } LIMIT 100`);
  return { iri, out: o.rows.slice(0, 200).map(r => ({ p: r.p, o: r.o })), in: i.rows.map(r => ({ s: r.s, p: r.p })), truncated: o.rows.length > 200, source: String(app.graphSource || '').split('/').pop() };
}
function prefixParts(i) { for (const [ns, p] of Object.entries(PREFIXES)) if (i.startsWith(ns)) { const rest = i.slice(ns.length); if (/^[A-Za-z0-9_-]+$/.test(rest)) return [p, ns, rest]; } return null; }
export function toTurtle(iri, d) {
  const used = new Map(); const term = t => { t = String(t); if (t[0] === '<') { const i = t.slice(1, -1), pf = prefixParts(i); if (pf) { used.set(pf[0], pf[1]); return pf[0] + ':' + pf[2]; } return t; } return t; };
  const lines = d.out.map(x => `  ${term(x.p)} ${term(x.o)}`);
  const inc = d.in.map(x => `${term(x.s)} ${term(x.p)} <${iri}> .`);
  const head = [...used].map(([p, ns]) => `@prefix ${p}: <${ns}> .`).join('\n');
  return (head ? head + '\n\n' : '') + (lines.length ? `<${iri}>\n` + lines.join(' ;\n') + ' .\n' : '') + (inc.length ? '\n' + inc.join('\n') + '\n' : '');
}
// engine answer -> the shape the result table and the 3D highlight use
export function toResult(label, j) {
  const cols = j.vars.map(v => ({ k: v, h: v }));
  const idxs = [], seen = new Set(), picks = [], seenP = new Set();
  const rows = j.rows.map(row => {
    const r = {}; let node = null, pick = null; const refs = {};
    for (const v of j.vars) {
      r[v] = cleanTerm(row[v]); const nn = resolveNode(row[v]); if (nn) { refs[v] = { i: nn.i }; if (!node) node = nn; }
      const pid = pickOf(row[v]); if (pid && app.svc && app.svc.has(pid)) { r[v] = 's:' + pid; refs[v] = { p: pid }; if (!pick) pick = pid; }
    }
    if (Object.keys(refs).length) r.__ref = refs;
    if (node) { r.__i = node.i; if (!seen.has(node.i)) { seen.add(node.i); idxs.push(node.i); } }
    else if (pick) { r.__p = pick; if (!seenP.has(pick)) { seenP.add(pick); picks.push(pick); } }
    return r;
  });
  return { label, cols, rows, idxs, picks, ms: j.ms };
}
