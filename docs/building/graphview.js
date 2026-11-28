// Clustered, drill-down graph view of a knowledge graph. Plain ES module, no libraries, no build step.
// Layout: ./graphview-layout.js. Everything is read through options.query, so it works on any graph.
//
// import { mountGraphView } from './graphview.js';
// const gv = mountGraphView(container, options);        // container: an element with a height (100% or fixed)
//
// options
//   query     async (sparql) => ({vars, rows})  REQUIRED. rows are objects keyed by var, values are N-Triples
//                                               terms ('<iri>', '"lit"@en', '"1"^^<xsd:integer>', '_:b0').
//   onSelect  (iri|null) => void                a node was selected in the view (click, keyboard, search, card)
//   onHover   (iri|null) => void                the pointer is over a node
//   labelOf   (iri) => string|undefined         label override (else rdfs:label, skos:prefLabel, schema:name ... , else the IRI tail)
//   colorOf   (iri) => string|undefined         CSS colour override for a node, a class or a group IRI
//   stateOf   (iri) => {label, color}|undefined tints a node (live/dead, warm/cold); call updateStates() after the state changed
//   groupings [{id, label, requires?: [predicateIri], levels: [Level]}]   host groupings, added to the built-in ones
//             (same id replaces). A Level is one of
//               {sparql: 'SELECT ?n ?g ?gl WHERE {...}'}  ?n a node, ?g its group (IRI or literal), ?gl optional label
//               {predicate: iri, inverse?: bool}          the group is the object of n-predicate-g (inverse: g-predicate-n)
//               {type: 'class'}                           most specific class (the one with the fewest instances)
//               {type: 'classRoot'}                       top of the rdfs:subClassOf chain of that class
//               {type: 'namespace'}                       IRI up to the last # or /
//             each level may carry a `label`. Level i+1 splits the clusters of level i. A level that does not split
//             a cluster is skipped. `requires` hides the grouping when a predicate is absent from the graph.
//   theme     'dark' (default) | 'light'
//   small     number of members under which an expanded cluster shows its nodes (default 60)
//
// controller (every method that changes the view returns a promise that resolves when the scene is rebuilt)
//   ready                      promise, resolves when the first scene is on screen (rejects if the graph cannot be read)
//   refresh()                  re-read the graph, keep grouping/filters/expanded clusters where they still exist
//   focus(iri)                 reveal the node (expands the clusters that contain it), select it, centre on it; resolves true
//                              when the node is in the view, false for an IRI that is not a typed resource (or is filtered out)
//   select(iri|null)           alias of focus; null clears the selection
//   highlight(iris[])          ring these nodes (a cluster shows how many of its members are highlighted); [] clears
//   setGrouping(id)            'class' | 'namespace' | 'system' | 'storey' | 'room' | 'circuit' | a host id
//   setFilter({predicates?, classes?, text?})
//        predicates: array of predicate IRIs to draw (null = the defaults); classes: array of class IRIs, only nodes of
//        these classes are in the view; text: only nodes whose label or IRI contains it. Omitted keys keep their value;
//        classes/text rebuild the clusters (all collapsed).
//   setFlow(predicateIri|null) order the layout left to right along this predicate (sources left, sinks right)
//   expand(clusterId) / collapse(clusterId)   by the ids in getState().clusters (shift-click on a cluster = expandAll)
//   expandAll(clusterId, maxNodes=600)   expand it and everything below it down to nodes, while at most maxNodes nodes show
//   up()                       one step up: leave ego view, else collapse the cluster expanded last (Backspace)
//   fit()                      fit the whole scene in the view
//   whenIdle()                 promise: queued changes applied and the layout settled (for tests and agents)
//   collapseAll()              back to the top level (also clears ego view and pulled-out nodes)
//   expandNeighbours(iri?)     pull the neighbours of the selected node (or iri) out of their clusters around it
//   ego(iri, hops=1) / exitEgo()   ego view: the node and its 1 or 2 hop neighbourhood (at most 300 nodes), with a way back
//   search(text)               [{iri, label, type}] from the cached label index (the UI box uses the same)
//   clusterMembers(clusterId)  IRIs of all members of a cluster
//   setList(bool)              the plain table of the current level instead of the canvas
//   updateStates()             repaint after options.stateOf changed
//   resize()                   after the container changed size
//   getState()                 {grouping, groupings, universe, expanded, path, clusters[{id,label,count,internal,highlight,...}],
//                               nodes[{iri,label,cluster,...}], edges[{from,to,predicate,count,kind,sel}], counts, selected,
//                               predicates, enabledPredicates, ego, flow, list, layout{alpha,active}, rendering, frames, ...}
//   on(event, fn) -> off       events 'select' (iri|null), 'expand' ({id,label,count}), 'collapse' ({id,label})
//   destroy()
//
// Data model. Nodes are the typed resources (subjects with an rdf:type). Edges are triples between two nodes whose
// predicate is not rdf:type; literals are attributes (shown in the side card), not edges. Clusters partition the nodes:
// the member counts of the top-level clusters add up to the number of nodes. Edges between clusters are aggregated:
// one per direction and predicate family (colour), width by the number of links, a tooltip lists the predicates.
// Limits: the whole type, label and edge tables are fetched once (three queries) and cached; very large graphs
// (a few hundred thousand nodes) would need a server-side aggregate instead.
//
// Flow layout draws only the flow predicate (plus the links of the selected node); things not on any flow path sit dimmed in
// a free group at the right. In flow mode the cluster outlines are not drawn (the layout is flat); the colours still show them.
// At most 32 clusters are shown side by side; the smallest are merged into 'Other (n)', which expands into them.
//
// Keyboard (canvas focused): arrows move to a neighbour, Enter expands (cluster) or pulls neighbours (node),
// Backspace goes up (leaves ego view or collapses the last expanded cluster), / focuses the search, Esc clears.

import { Layout, flowRanks, hash32, rng } from './graphview-layout.js';

const RDF_TYPE = 'http://www.w3.org/1999/02/22-rdf-syntax-ns#type';
const SYS = 'https://w3id.org/rete/cad/sys#', BOT = 'https://w3id.org/bot#', CAD = 'https://w3id.org/rete/cad#';
const PFX = {
  'http://www.w3.org/1999/02/22-rdf-syntax-ns#': 'rdf', 'http://www.w3.org/2000/01/rdf-schema#': 'rdfs', 'http://www.w3.org/2002/07/owl#': 'owl',
  'http://www.w3.org/2001/XMLSchema#': 'xsd', [BOT]: 'bot', [CAD]: 'cad', [SYS]: 'sys', 'https://brickschema.org/schema/Brick#': 'brick',
  'https://w3id.org/rete/geo3#': 'geo3', 'https://w3id.org/rete/cad/fzk-haus/sys/': 's', 'https://w3id.org/rete/cad/fzk-haus/': 'fzk',
  'http://www.w3.org/ns/sosa/': 'sosa', 'http://www.w3.org/2004/02/skos/core#': 'skos', 'http://qudt.org/schema/qudt/': 'qudt',
  'http://schema.org/': 'schema', 'https://schema.org/': 'schema', 'http://purl.org/dc/terms/': 'dct', 'http://xmlns.com/foaf/0.1/': 'foaf',
};
const GENERIC_CLASS = new Set(['http://www.w3.org/2002/07/owl#NamedIndividual', 'http://www.w3.org/2000/01/rdf-schema#Resource', 'http://www.w3.org/2002/07/owl#Thing']);
const MAX_TOP = 32; // most clusters shown side by side; the smallest are merged into "Other (n)"
const NONE = { k: '∅', l: '(none)' };

// ---------- small helpers ----------
const termIri = (t) => (t == null ? null : t[0] === '<' ? t.slice(1, -1) : t.startsWith('_:') ? t : null);
const termLit = (t) => { if (t == null) return null; const m = /^"([\s\S]*)"(?:@[\w-]+|\^\^.*)?$/.exec(t); return m ? m[1] : t; };
const termLang = (t) => { const m = /"@([\w-]+)$/.exec(t || ''); return m ? m[1] : ''; };
const localName = (iri) => { const m = /[^#/]*$/.exec(iri); return m && m[0] ? m[0] : iri; };
const nsOf = (iri) => { const i = Math.max(iri.lastIndexOf('#'), iri.lastIndexOf('/')); return i > 0 ? iri.slice(0, i + 1) : iri; };
const short = (iri) => { const ns = nsOf(iri); const p = PFX[ns]; return p ? p + ':' + iri.slice(ns.length) : localName(iri); };
const esc = (s) => String(s).replace(/[&<>"]/g, (c) => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;' }[c]));
const clamp = (v, a, b) => Math.max(a, Math.min(b, v));
const trunc = (s, n) => (s.length > n ? s.slice(0, n - 1) + '…' : s);
const FAM = [
  ['flow', /^(feeds|from|to|drainsTo|supplies|connectsTo|next|flowsTo)$/i, '#5db0ff', 'supply / flow'],
  ['control', /^(controls|thermostat|breaker|rcd|regulates|switches)$/i, '#ff9f43', 'control'],
  ['group', /^(hasMember|member\w*|inCircuit|hasPart|partOf|belongsTo|system)$/i, '#4ade80', 'membership'],
  ['spatial', /(contains|inStorey|hasSpace|hasStorey|adjacent|bounds|room|location|within|bearsOn|mountedOn|fillsWall|penetrates|hasElement|aggregates)/i, '#c084fc', 'spatial / structure'],
];
const FAM_OTHER = ['other', null, '#8b97ad', 'other'];
const famOf = (local) => FAM.find((f) => f[1].test(local)) || FAM_OTHER;
const DEFAULT_ON = /^(feeds|controls|drainsTo|hasMember|containsElement|hasSpace|hasStorey|hasElement|bearsOn|mountedOn|contains|hasPart|supplies)$/i;

const THEMES = {
  dark: { bg: '#0b0e14', panel: '#121826', panel2: '#0e1420', line: '#1e2636', ink: '#e6ecf5', dim: '#8b97ad', accent: '#5db0ff', sel: '#ffd54a', hl: '#33d6ff', halo: 'rgba(11,14,20,.92)' },
  light: { bg: '#f4f6fa', panel: '#ffffff', panel2: '#eef1f6', line: '#d5dbe6', ink: '#1a2233', dim: '#5b6678', accent: '#1f6fd0', sel: '#c98a00', hl: '#0a8fb5', halo: 'rgba(244,246,250,.92)' },
};

const CSS = `
.gv{--bg:#0b0e14;--panel:#121826;--panel2:#0e1420;--line:#1e2636;--ink:#e6ecf5;--dim:#8b97ad;--accent:#5db0ff;--sel:#ffd54a;--hl:#33d6ff;
 position:relative;display:flex;flex-direction:column;width:100%;height:100%;min-height:0;background:var(--bg);color:var(--ink);font:12px/1.35 ui-sans-serif,system-ui,-apple-system,Segoe UI,Roboto,sans-serif;overflow:hidden}
.gv[data-theme=light]{--bg:#f4f6fa;--panel:#fff;--panel2:#eef1f6;--line:#d5dbe6;--ink:#1a2233;--dim:#5b6678;--accent:#1f6fd0;--sel:#c98a00;--hl:#0a8fb5}
.gv *{box-sizing:border-box}
.gv button,.gv select,.gv input{font:inherit;color:var(--ink);background:var(--panel2);border:1px solid var(--line);border-radius:5px;padding:2px 7px;height:24px}
.gv button{cursor:pointer;white-space:nowrap}.gv button:hover{border-color:var(--accent)}
.gv button[aria-pressed=true],.gv button.on{border-color:var(--accent);color:var(--accent)}
.gv :focus-visible{outline:2px solid var(--accent);outline-offset:1px}
.gv-bar{display:flex;gap:5px;align-items:center;padding:4px 6px;background:var(--panel);border-bottom:1px solid var(--line);flex:none;overflow-x:auto;scrollbar-width:thin}
.gv-bar>*{flex:none}
.gv-bar label{color:var(--dim);display:flex;align-items:center;gap:4px}
.gv-search{position:relative}.gv-search input{width:150px}
.gv-hits{position:absolute;left:0;top:26px;z-index:6;width:280px;max-height:220px;overflow:auto;background:var(--panel);border:1px solid var(--line);border-radius:6px;box-shadow:0 6px 20px rgba(0,0,0,.35)}
.gv-hits button{display:flex;width:100%;justify-content:space-between;gap:8px;border:0;border-radius:0;height:auto;padding:4px 8px;background:transparent;text-align:left;white-space:normal}
.gv-hits button:hover,.gv-hits button.cur{background:var(--panel2)}.gv-hits small{color:var(--dim);flex:none}
.gv-crumbs{display:flex;gap:3px;align-items:center;padding:2px 8px;border-bottom:1px solid var(--line);color:var(--dim);flex:none;overflow-x:auto;white-space:nowrap;min-height:22px;scrollbar-width:none}
.gv-crumbs button{border:0;background:transparent;padding:0 3px;height:18px;color:var(--accent)}.gv-crumbs b{color:var(--ink);font-weight:600}
.gv-crumbs .sp{flex:1}.gv-crumbs .st{color:var(--dim);font-variant-numeric:tabular-nums}
.gv-main{position:relative;flex:1;min-height:0}
.gv-main canvas{position:absolute;inset:0;width:100%;height:100%;touch-action:none;cursor:grab}
.gv-main canvas.over{cursor:pointer}.gv-main canvas.drag{cursor:grabbing}
.gv-pop,.gv-legend,.gv-card,.gv-tip,.gv-msg{position:absolute;background:var(--panel);border:1px solid var(--line);border-radius:7px;box-shadow:0 6px 20px rgba(0,0,0,.35)}
.gv-pop{left:6px;top:4px;z-index:5;padding:6px 8px;max-height:calc(100% - 8px);overflow:auto;min-width:230px}
.gv-pop h4,.gv-card h4,.gv-legend h4{margin:6px 0 3px;font-size:10px;text-transform:uppercase;letter-spacing:.07em;color:var(--dim);font-weight:600}
.gv-pop label{display:flex;gap:6px;align-items:center;padding:1px 0}.gv-pop label span.n{margin-left:auto;color:var(--dim);font-variant-numeric:tabular-nums}
.gv-pop input[type=checkbox]{height:auto;width:auto}.gv-pop .sw{width:10px;height:3px;border-radius:2px;flex:none}
.gv-legend{left:6px;bottom:6px;padding:5px 8px;max-width:260px;max-height:70%;overflow:auto;z-index:3}
.gv-legend div{display:flex;align-items:center;gap:6px;white-space:nowrap}.gv-legend i{width:9px;height:9px;border-radius:50%;flex:none}.gv-legend i.ln{width:14px;height:3px;border-radius:2px}
.gv-card{right:6px;top:6px;bottom:6px;width:250px;z-index:4;overflow:auto;padding:8px 10px}
.gv-card h3{margin:0 0 2px;font-size:13px;word-break:break-word}.gv-card .iri{color:var(--dim);font-size:10px;word-break:break-all}
.gv-card .x{position:absolute;right:4px;top:4px;border:0;background:transparent;height:20px}
.gv-card .chips{display:flex;flex-wrap:wrap;gap:3px;margin:4px 0}.gv-card .chip{border:1px solid var(--line);border-radius:9px;padding:0 6px;color:var(--dim)}
.gv-card .acts{display:flex;gap:4px;flex-wrap:wrap;margin:6px 0}
.gv-card ul{list-style:none;margin:0;padding:0}.gv-card li button{border:0;background:transparent;color:var(--accent);height:auto;padding:1px 0;text-align:left;white-space:normal}
.gv-card .pr{display:flex;justify-content:space-between;gap:6px;color:var(--ink);cursor:pointer;padding:1px 0}.gv-card .pr span:last-child{color:var(--dim)}
.gv-card dl{margin:0;display:grid;grid-template-columns:auto 1fr;gap:1px 8px}.gv-card dt{color:var(--dim)}.gv-card dd{margin:0;word-break:break-word}
.gv[data-narrow] .gv-card{left:6px;right:6px;top:auto;width:auto;height:auto;max-height:46%;bottom:6px}
.gv-tip{z-index:7;pointer-events:none;padding:3px 7px;max-width:260px;white-space:pre-line}
.gv-msg{left:50%;top:50%;transform:translate(-50%,-50%);padding:8px 14px;color:var(--dim);z-index:8}
.gv[data-card]:not([data-narrow]) .gv-list{right:258px}
.gv-list{position:absolute;inset:0;overflow:auto;background:var(--bg);z-index:3}
.gv-list table{border-collapse:collapse;width:100%}.gv-list th{position:sticky;top:0;background:var(--panel);text-align:left;color:var(--dim);font-weight:600;padding:3px 8px;border-bottom:1px solid var(--line)}
.gv-list td{padding:2px 8px;border-bottom:1px solid var(--line)}.gv-list tr[tabindex]{cursor:pointer}.gv-list tr:hover td,.gv-list tr:focus-visible td{background:var(--panel2)}.gv-list td.num{text-align:right;font-variant-numeric:tabular-nums}
.gv-sr{position:absolute;width:1px;height:1px;overflow:hidden;clip:rect(0 0 0 0)}
`;
let cssDone = false;
function injectCss() {
  if (cssDone || typeof document === 'undefined') return; cssDone = true;
  const s = document.createElement('style'); s.setAttribute('data-graphview', ''); s.textContent = CSS; document.head.appendChild(s);
}
function h(tag, attrs, ...kids) {
  const e = document.createElement(tag);
  if (attrs) for (const [k, v] of Object.entries(attrs)) {
    if (v == null || v === false) continue;
    if (k === 'class') e.className = v; else if (k === 'text') e.textContent = v; else if (k.startsWith('on')) e.addEventListener(k.slice(2), v); else e.setAttribute(k, v === true ? '' : v);
  }
  for (const c of kids.flat()) if (c != null && c !== false) e.append(c.nodeType ? c : document.createTextNode(String(c)));
  return e;
}

// ---------- groupings ----------
const Q_NS = `PREFIX bot: <${BOT}> PREFIX cad: <${CAD}> PREFIX sys: <${SYS}>`;
export const BUILTIN_GROUPINGS = [
  { id: 'class', label: 'Class', levels: [{ type: 'classRoot', label: 'Class' }, { type: 'class', label: 'Class' }] },
  { id: 'namespace', label: 'Namespace', levels: [{ type: 'namespace', label: 'Namespace' }, { type: 'class', label: 'Class' }] },
  { id: 'system', label: 'System', requires: [SYS + 'system'], levels: [{ predicate: SYS + 'system', label: 'System' }, { type: 'class', label: 'Device type' }] },
  { id: 'storey', label: 'Storey', requires: [CAD + 'inStorey'], levels: [
    { label: 'Storey', sparql: `${Q_NS} SELECT ?n ?g WHERE { { ?n cad:inStorey ?g } UNION { ?g bot:hasSpace ?n } UNION { ?g a bot:Storey . BIND(?g AS ?n) } }` },
    { label: 'Room', sparql: `${Q_NS} SELECT ?n ?g WHERE { { ?g bot:containsElement ?n } UNION { ?n a bot:Space . BIND(?n AS ?g) } }` },
    { type: 'class', label: 'Class' }] },
  { id: 'room', label: 'Room', requires: [BOT + 'containsElement'], levels: [
    { label: 'Room', sparql: `${Q_NS} SELECT ?n ?g WHERE { { ?g bot:containsElement ?n } UNION { ?n a bot:Space . BIND(?n AS ?g) } }` },
    { type: 'class', label: 'Class' }] },
  { id: 'circuit', label: 'Circuit / group', requires: [SYS + 'hasMember'], levels: [
    { label: 'Circuit', sparql: `${Q_NS} SELECT ?n ?g WHERE { { ?n sys:inCircuit ?g } UNION { ?g sys:hasMember ?n } }` },
    { type: 'class', label: 'Class' }] },
];

// ---------- the graph, as read through options.query ----------
class Store {
  constructor(query) { this.query = query; this.qc = new Map(); }
  async q(sparql) {
    if (this.qc.has(sparql)) return this.qc.get(sparql);
    const r = await this.query(sparql); const rows = (r && r.rows) || []; this.qc.set(sparql, rows); return rows;
  }
  async load(labelOf) {
    this.qc.clear();
    const [ty, lb, ed] = await Promise.all([
      this.q('SELECT ?s ?t WHERE { ?s a ?t }'),
      this.q(`PREFIX rdfs: <http://www.w3.org/2000/01/rdf-schema#> PREFIX skos: <http://www.w3.org/2004/02/skos/core#> PREFIX schema: <http://schema.org/> PREFIX dct: <http://purl.org/dc/terms/> PREFIX foaf: <http://xmlns.com/foaf/0.1/>
        SELECT ?s ?l WHERE { { ?s rdfs:label ?l } UNION { ?s skos:prefLabel ?l } UNION { ?s schema:name ?l } UNION { ?s dct:title ?l } UNION { ?s foaf:name ?l } }`),
      this.q(`SELECT ?s ?p ?o WHERE { ?s ?p ?o . FILTER(!isLiteral(?o)) FILTER(?p != <${RDF_TYPE}>) }`),
    ]);
    const ids = new Map(); this.iri = []; this.types = [];
    for (const r of ty) {
      const s = termIri(r.s), t = termIri(r.t); if (!s || !t) continue;
      let i = ids.get(s); if (i === undefined) { i = this.iri.length; ids.set(s, i); this.iri.push(s); this.types.push([]); }
      this.types[i].push(t);
    }
    this.ids = ids; const n = this.iri.length; this.n = n;
    const cnt = new Map(); for (const ts of this.types) for (const t of ts) cnt.set(t, (cnt.get(t) || 0) + 1);
    this.classCount = cnt;
    this.primary = this.types.map((ts) => {
      let best = null, bc = Infinity;
      for (const t of ts) { if (GENERIC_CLASS.has(t)) continue; const c = cnt.get(t); if (c < bc || (c === bc && t < best)) { best = t; bc = c; } }
      return best || ts[0];
    });
    // labels: prefer no language or English
    this.label = new Array(n).fill(null); const lrank = new Array(n).fill(9);
    for (const r of lb) {
      const i = ids.get(termIri(r.s)); if (i === undefined) continue; const lang = termLang(r.l);
      const rk = lang === '' ? 1 : lang === 'en' ? 0 : 2; if (rk < lrank[i]) { lrank[i] = rk; this.label[i] = termLit(r.l); }
    }
    this.display = this.iri.map((u, i) => (labelOf && labelOf(u)) || this.label[i] || localName(u));
    // edges between nodes
    const preds = new Map(), seen = new Set(); this.preds = []; this.edges = [];
    this.out = Array.from({ length: n }, () => []); this.inn = Array.from({ length: n }, () => []);
    for (const r of ed) {
      const s = ids.get(termIri(r.s)), o = ids.get(termIri(r.o)); if (s === undefined || o === undefined) continue;
      const pi = termIri(r.p); const key = s + ' ' + pi + ' ' + o; if (seen.has(key)) continue; seen.add(key);
      let p = preds.get(pi);
      if (p === undefined) { const ln = localName(pi); const f = famOf(ln); p = this.preds.length; preds.set(pi, p); this.preds.push({ iri: pi, local: ln, short: short(pi), count: 0, fam: f[0], color: f[2], on: DEFAULT_ON.test(ln) }); }
      this.preds[p].count++;
      const e = { s, p, o }; const ei = this.edges.length; this.edges.push(e); this.out[s].push(ei); this.inn[o].push(ei);
    }
    if (!this.preds.some((p) => p.on)) [...this.preds].sort((a, b) => b.count - a.count).slice(0, 6).forEach((p) => { p.on = true; });
    this.predIdx = preds;
    this.degree = new Int32Array(n); for (const e of this.edges) { this.degree[e.s]++; this.degree[e.o]++; }
    // search index
    this.sidx = this.iri.map((u, i) => (this.display[i] + ' ' + (this.label[i] || '') + ' ' + localName(u)).toLowerCase());
  }
  nodeId(iri) { const i = this.ids.get(iri); return i === undefined ? -1 : i; }
}

// ---------- the view ----------
class GraphView {
  constructor(container, opts) {
    injectCss();
    this.c = container; this.o = opts || {}; this.store = new Store(opts.query);
    this.theme = THEMES[this.o.theme] ? this.o.theme : 'dark';
    this.small = this.o.small || 60;
    this.groupings = BUILTIN_GROUPINGS.map((g) => ({ ...g }));
    for (const g of this.o.groupings || []) { const i = this.groupings.findIndex((x) => x.id === g.id); if (i >= 0) this.groupings[i] = g; else this.groupings.push(g); }
    this.avail = new Map(); this.grp = null;
    this.layout = new Layout(); this.items = []; this.edges = []; this.owner = null;
    this.posCache = new Map(); this.view = { x: 0, y: 0, k: 1 }; this.W = 0; this.H = 0; this.dpr = 1;
    this.byId = new Map(); this.roots = []; this.assignCache = new Map();
    this.pulled = new Map(); this.hl = new Set(); this.enabled = new Set(); this.filter = { classes: null, text: '' };
    this.sel = null; this.hover = null; this.ego = null; this.flow = -1; this.listMode = false; this.universe = null;
    this.sub = new Map(); this.q = Promise.resolve(); this.raf = 0; this.frames = 0; this.anim = null; this.pendingFit = null;
    this.colorIdx = new Map(); this.attrCache = new Map(); this.chips = []; this.dead = false; this.expandOrder = [];
    this.stepCount = 0; this.edgeGeom = [];
    this._build();
    this.ready = this._init();
    this.ready.catch(() => {});
  }

  // ----- DOM -----
  _build() {
    const T = THEMES[this.theme];
    this.root = h('section', { class: 'gv', 'data-theme': this.theme, role: 'region', 'aria-label': 'Graph view: clustered knowledge graph' });
    this.selGroup = h('select', { 'aria-label': 'Grouping', title: 'Group the graph by' });
    this.selGroup.addEventListener('change', () => this.setGrouping(this.selGroup.value));
    this.selFlow = h('select', { 'aria-label': 'Flow layout', title: 'Order the layout along a predicate' });
    this.selFlow.addEventListener('change', () => this.setFlow(this.selFlow.value || null));
    this.btnEdges = h('button', { type: 'button', 'aria-haspopup': 'true', 'aria-expanded': 'false', onclick: () => this._togglePop() }, 'Edges ▾');
    this.inSearch = h('input', { type: 'search', placeholder: 'Search (/)', 'aria-label': 'Search nodes by label or IRI', autocomplete: 'off' });
    this.hits = h('div', { class: 'gv-hits', role: 'listbox', hidden: true });
    this.inSearch.addEventListener('input', () => this._onSearch());
    this.inSearch.addEventListener('keydown', (e) => this._searchKey(e));
    this.inSearch.addEventListener('blur', () => setTimeout(() => { this.hits.hidden = true; }, 160));
    this.btnList = h('button', { type: 'button', 'aria-pressed': 'false', title: 'Show the current level as a table', onclick: () => this.setList(!this.listMode) }, 'List');
    this.btnLegend = h('button', { type: 'button', 'aria-pressed': 'false', onclick: () => this._toggleLegend() }, 'Legend');
    this.btnFit = h('button', { type: 'button', title: 'Fit to view', onclick: () => this.fit(true) }, 'Fit');
    this.btnAll = h('button', { type: 'button', title: 'Collapse everything', onclick: () => this.collapseAll() }, 'Collapse');
    this.bar = h('div', { class: 'gv-bar', role: 'toolbar', 'aria-label': 'Graph view controls' },
      this.selGroup, this.selFlow, this.btnEdges,
      h('div', { class: 'gv-search' }, this.inSearch, this.hits), this.btnList, this.btnLegend, this.btnFit, this.btnAll);
    this.crumbs = h('nav', { class: 'gv-crumbs', 'aria-label': 'Path' });
    this.canvas = h('canvas', { tabindex: '0', role: 'img', 'aria-label': 'Clustered graph' });
    this.ctx = this.canvas.getContext('2d');
    this.main = h('div', { class: 'gv-main' }, this.canvas);
    this.tip = h('div', { class: 'gv-tip', hidden: true, role: 'tooltip' });
    this.msg = h('div', { class: 'gv-msg', role: 'status' }, 'Loading the graph…');
    this.live = h('div', { class: 'gv-sr', 'aria-live': 'polite' });
    this.main.append(this.tip, this.msg, this.live);
    this.root.append(this.bar, this.crumbs, this.main);
    this.c.replaceChildren(this.root);
    const cv = this.canvas;
    cv.addEventListener('pointerdown', (e) => this._down(e)); cv.addEventListener('pointermove', (e) => this._move(e));
    cv.addEventListener('pointerup', (e) => this._up(e)); cv.addEventListener('pointercancel', (e) => this._up(e, true));
    cv.addEventListener('pointerleave', () => { if (!this.ptr.size) this._setHover(null); this.tip.hidden = true; });
    cv.addEventListener('wheel', (e) => this._wheel(e), { passive: false });
    cv.addEventListener('keydown', (e) => this._key(e));
    this.ptr = new Map(); this.drag = null;
    this.ro = typeof ResizeObserver !== 'undefined' ? new ResizeObserver(() => this.resize()) : null;
    if (this.ro) this.ro.observe(this.main);
    this._T = T;
    this.resize();
  }

  // ----- init -----
  async _init() {
    try {
      await this.store.load(this.o.labelOf);
      const S = this.store;
      this.enabled = new Set(S.preds.map((p, i) => (p.on ? i : -1)).filter((i) => i >= 0));
      this.universe = new Uint8Array(S.n).fill(1);
      await Promise.all(this.groupings.map(async (g) => {
        let ok = true;
        for (const p of g.requires || []) { const rows = await S.q(`SELECT ?x WHERE { ?x <${p}> ?y } LIMIT 1`); if (!rows.length) { ok = false; break; } }
        this.avail.set(g.id, ok);
      }));
      const first = this.groupings.find((g) => this.avail.get(g.id));
      this.grp = first || this.groupings[0];
      this._fillSelects();
      if (!S.n) { this.msg.textContent = 'No typed resources in this graph.'; this.msg.hidden = false; return this; }
      await this._buildRoots();
      this._scene({ initial: true });
      this.msg.hidden = true; this.fit(false);
      return this;
    } catch (e) { this.msg.textContent = 'Could not read the graph: ' + (e && e.message || e); this.msg.hidden = false; throw e; }
  }

  _fillSelects() {
    this.selGroup.replaceChildren(...this.groupings.filter((g) => this.avail.get(g.id) !== false).map((g) => h('option', { value: g.id, selected: g === this.grp }, 'by ' + g.label)));
    this.selGroup.value = this.grp.id;
    this._fillFlow();
  }
  _fillFlow() {
    const S = this.store; const on = [...this.enabled].sort((a, b) => S.preds[b].count - S.preds[a].count);
    this.selFlow.replaceChildren(h('option', { value: '' }, 'flow: off'), ...on.map((p) => h('option', { value: S.preds[p].iri }, 'flow: ' + S.preds[p].short)));
    this.selFlow.value = this.flow >= 0 && this.enabled.has(this.flow) ? S.preds[this.flow].iri : '';
  }

  enqueue(fn) { const r = this.q.then(() => (this.dead ? undefined : fn())); this.q = r.catch(() => {}); return r; }

  // ----- clusters -----
  async _assign(g, L) {
    const key = g.id + ':' + L; if (this.assignCache.has(key)) return this.assignCache.get(key);
    const S = this.store, lv = g.levels[L], out = new Map();
    const setG = (n, k, l) => { const cur = out.get(n); if (!cur || k < cur.k) out.set(n, { k, l }); };
    if (lv.type === 'class') for (let i = 0; i < S.n; i++) setG(i, S.primary[i], short(S.primary[i]).replace(/^[a-z0-9]+:/, ''));
    else if (lv.type === 'classRoot') {
      const rows = await S.q('SELECT ?c ?d WHERE { ?c <http://www.w3.org/2000/01/rdf-schema#subClassOf> ?d . FILTER(!isLiteral(?d)) }');
      const up = new Map(); for (const r of rows) { const c = termIri(r.c), d = termIri(r.d); if (c && d && c !== d && !up.has(c)) up.set(c, d); }
      const root = (c) => { let x = c, n = 0; while (up.has(x) && n++ < 30) x = up.get(x); return x; };
      for (let i = 0; i < S.n; i++) {
        const p = S.primary[i]; let top = p;
        if (!up.has(p)) { let bc = S.classCount.get(p); for (const t of S.types[i]) { if (GENERIC_CLASS.has(t)) continue; const c = S.classCount.get(t); if (c > bc) { bc = c; top = t; } } }
        const r = root(top); setG(i, r, short(r).replace(/^[a-z0-9]+:/, ''));
      }
    } else if (lv.type === 'namespace') for (let i = 0; i < S.n; i++) { const ns = nsOf(S.iri[i]); setG(i, ns, PFX[ns] ? PFX[ns] + ':' : trunc(ns.replace(/^https?:\/\//, ''), 34)); }
    else {
      const sparql = lv.sparql || (lv.inverse
        ? `SELECT ?n ?g WHERE { ?g <${lv.predicate}> ?n }` : `SELECT ?n ?g WHERE { ?n <${lv.predicate}> ?g }`);
      const rows = await S.q(sparql);
      for (const r of rows) {
        const n = S.nodeId(termIri(r.n)); if (n < 0 || r.g == null) continue;
        const gi = termIri(r.g); const k = gi != null ? gi : 'lit:' + termLit(r.g);
        const gn = gi != null ? S.nodeId(gi) : -1;
        const l = (r.gl != null && termLit(r.gl)) || (gn >= 0 ? S.display[gn] : gi != null ? localName(gi) : termLit(r.g));
        setG(n, k, l);
      }
    }
    this.assignCache.set(key, out); return out;
  }

  _mkCluster(key, label, members, L, parent, usedClass) {
    const c = { id: (parent ? parent.id : '') + '/' + L + ':' + key, key, label, members, level: L, parent, children: null, leaf: false, expanded: false, usedClass, mset: null, internal: 0 };
    this.byId.set(c.id, c); return c;
  }
  async _partition(members, from, parent) {
    const g = this.grp;
    for (let L = from; L < g.levels.length; L++) {
      const a = await this._assign(g, L); const by = new Map();
      for (const id of members) { const x = a.get(id) || NONE; let c = by.get(x.k); if (!c) { c = { k: x.k, l: x.l, m: [] }; by.set(x.k, c); } c.m.push(id); }
      const last = L === g.levels.length - 1;
      if (by.size > 1 || (!parent && last)) {
        const used = (parent ? parent.usedClass : false) || g.levels[L].type === 'class';
        const list = [...by.values()].sort((x, y) => y.m.length - x.m.length || (x.l < y.l ? -1 : 1)).map((x) => this._mkCluster(x.k, x.l, x.m, L, parent, used));
        if (list.length <= MAX_TOP) return list;
        const rest = list.splice(MAX_TOP - 1); const all = [].concat(...rest.map((x) => x.members));
        const other = this._mkCluster('…other', 'Other (' + rest.length + ')', all, L, parent, used); other.rest = rest; rest.forEach((x) => { x.parent = other; });
        list.push(other); return list;
      }
    }
    return null;
  }
  async _buildRoots() {
    this.byId.clear(); this.expandOrder = []; this.topMap = null;
    const members = []; for (let i = 0; i < this.store.n; i++) if (this.universe[i]) members.push(i);
    this.roots = members.length ? (await this._partition(members, 0, null)) || [] : [];
    if (this.roots.length === 1 && this.roots[0].members.length > this.small && this.grp.levels.length > 1) { /* one cluster: leave it, the user expands it */ }
    this.colorIdx = new Map(); this.roots.forEach((c, i) => this.colorIdx.set(c.key, i));
  }
  async _children(c) {
    if (c.children || c.leaf) return;
    if (c.rest) { c.children = c.rest; return; }
    const m = c.members;
    if (m.length <= this.small) { c.leaf = true; return; }
    const p = await this._partition(m, c.level + 1, c);
    if (p) { c.children = p; return; }
    if (m.length <= 150) { c.leaf = true; return; }
    if (!c.usedClass) {
      const by = new Map(); for (const id of m) { const k = this.store.primary[id]; let x = by.get(k); if (!x) by.set(k, x = []); x.push(id); }
      if (by.size > 1) { c.children = [...by.entries()].sort((a, b) => b[1].length - a[1].length).map(([k, ms]) => this._mkCluster(k, short(k).replace(/^[a-z0-9]+:/, ''), ms, c.level + 1, c, true)); return; }
    }
    const sorted = [...m].sort((a, b) => (this.store.display[a] < this.store.display[b] ? -1 : 1)); const ch = [];
    for (let i = 0; i < sorted.length; i += 50) { const part = sorted.slice(i, i + 50); ch.push(this._mkCluster('chunk' + i, trunc(this.store.display[part[0]], 14) + ' … ' + trunc(this.store.display[part[part.length - 1]], 14), part, c.level + 1, c, true)); }
    c.children = ch;
  }
  _mset(c) { if (!c.mset) c.mset = new Set(c.members); return c.mset; }
  _ancestors(c) { const a = []; for (let x = c; x; x = x.parent) a.unshift(x); return a; }
  _topOf(c) { let x = c; while (x.parent) x = x.parent; return x; }

  // ----- scene: visible items and edges -----
  _savePos() { for (const it of this.layout.items) this.posCache.set(it.key, { x: it.x, y: it.y, r: it.r }); }
  _colorKey(key, iri) {
    const T = this._T; const ov = iri && this.o.colorOf && this.o.colorOf(iri); if (ov) return ov;
    let i = this.colorIdx.get(key); if (i === undefined) { i = 100 + (hash32(key) % 200); this.colorIdx.set(key, i); }
    return `hsl(${(i * 137.508 + 20) % 360},${this.theme === 'dark' ? 58 : 62}%,${this.theme === 'dark' ? 60 : 46}%)`;
  }
  _clusterR(count) { return clamp(15 + 4.3 * Math.sqrt(count), 18, 74); }
  _nodeR(id) { return 4.6 + Math.min(5.4, Math.sqrt(this.store.degree[id]) * 0.75); }

  _scene(o = {}) {
    const S = this.store; if (!S.n) return;
    const items = [], owner = new Int32Array(S.n).fill(-1); this.owner = owner;
    const posOf = (key, c) => { const p = this.posCache.get(key); if (p) return { x: p.x, y: p.y }; for (let a = c; a; a = a.parent) { const q = this.posCache.get('c:' + a.id); if (q) return { x: q.x, y: q.y, seed: q }; } return {}; };
    const pulled = this.pulled;
    const addNode = (id, path, c) => {
      const key = 'n:' + id; const p = posOf(key, c); const idx = items.length;
      items.push({ key, kind: 'node', id, r: this._nodeR(id) + (this.ego && this.ego.id === id ? 4 : 0), path, x: p.x, y: p.y, seed: p.seed, top: this._topKeyOf(id), pin: false });
      owner[id] = idx;
    };
    if (this.ego) {
      const ids = this._egoNodes(); const cc = { };
      for (const id of ids) addNode(id, [], null);
    } else {
      const emit = (c, path) => {
        if (!c.expanded) {
          const key = 'c:' + c.id; const p = posOf(key, c.parent); const idx = items.length; let n = 0;
          for (const m of c.members) { if (pulled.has(m)) continue; owner[m] = idx; n++; }
          items.push({ key, kind: 'cluster', cluster: c, id: c.id, count: n, r: this._clusterR(n), path, x: p.x, y: p.y, seed: p.seed, top: this._topOf(c).key, pin: false });
          return;
        }
        const gp = path.concat('g:' + c.id);
        if (c.leaf) { for (const m of c.members) if (!pulled.has(m)) addNode(m, gp, c); } else for (const ch of c.children) emit(ch, gp);
      };
      for (const c of this.roots) emit(c, []);
      const anchorPath = (a) => { const i = owner[a]; return i >= 0 && items[i].kind === 'node' ? items[i].path : []; };
      for (const [m, a] of pulled) if (this.universe[m]) addNode(m, anchorPath(a), null), items[owner[m]].seed = items[owner[m]].seed || (owner[a] >= 0 ? { x: items[owner[a]].x, y: items[owner[a]].y, r: 40 } : undefined);
    }
    for (const it of items) { it.vx = 0; it.vy = 0; if (!this.layout.items.length && it.x == null) { /* initial */ } }
    for (const it of items) { it.cpath = it.path; if (this.flow >= 0) it.path = []; }
    this.items = items; this.hoverEdge = null; this._edges();
    const links = []; const seen = new Map();
    for (const e of this.edges) { const k = e.a < e.b ? e.a + ',' + e.b : e.b + ',' + e.a; if (seen.has(k)) seen.get(k).w += 1; else { const l = { a: e.a, b: e.b, w: 1 }; seen.set(k, l); links.push(l); } }
    this.links = links;
    this._ranks();
    this.layout.aspect = clamp(this.W / Math.max(1, this.H), 0.6, 4);
    this.layout.set(items, links, this.ranks);
    this.layout.wake(o.initial ? 1 : o.alpha || 0.7);
    if (o.initial || o.prerun) this.layout.run(o.initial ? 90 : 60);
    this._renderUi(); this.requestDraw();
  }

  _ranks() {
    this.ranks = null; if (this.flow < 0) return;
    const S = this.store, links = [], seen = new Set();
    for (const e of S.edges) { if (e.p !== this.flow) continue; const a = this.owner[e.s], b = this.owner[e.o]; if (a < 0 || b < 0 || a === b) continue; const k = a + ',' + b; if (!seen.has(k)) { seen.add(k); links.push({ a, b }); } }
    if (!links.length) return;
    const r = flowRanks(this.items.length, links); let mx = 0; for (const x of r) if (x != null && x > mx) mx = x;
    this.items.forEach((it, i) => { it.noflow = r[i] == null; if (r[i] == null) r[i] = mx + 1; });
    this.ranks = r;
  }

  _edges() {
    const S = this.store, items = this.items, owner = this.owner, agg = new Map(); this.edges = [];
    for (const it of items) it.internal = 0;
    const selId = this.sel && this.sel.type === 'node' ? this.sel.id : -1;
    for (const e of S.edges) {
      if (!this.enabled.has(e.p) || e.s === selId || e.o === selId || (this.flow >= 0 && e.p !== this.flow)) continue;
      const a = owner[e.s], b = owner[e.o]; if (a < 0 || b < 0) continue;
      if (a === b) { items[a].internal++; continue; }
      const both = items[a].kind === 'node' && items[b].kind === 'node'; const P = S.preds[e.p];
      const key = a + '>' + b + (both ? '#' + e.p : '~' + P.fam);
      let x = agg.get(key); if (!x) { x = { a, b, n: 0, preds: new Map(), fam: P.fam, color: P.color, both, sel: false, p: both ? e.p : -1 }; agg.set(key, x); }
      x.n++; x.preds.set(e.p, (x.preds.get(e.p) || 0) + 1);
    }
    if (selId >= 0) {
      for (const list of [S.out[selId], S.inn[selId]]) for (const ei of list) {
        const e = S.edges[ei]; const a = owner[e.s], b = owner[e.o]; if (a < 0 || b < 0) continue;
        if (a === b) { if (items[a].kind === 'cluster') items[a].internal++; continue; }
        const P = S.preds[e.p]; const key = 's' + a + '>' + b + '#' + e.p;
        let x = agg.get(key); if (!x) { x = { a, b, n: 0, preds: new Map(), fam: P.fam, color: P.color, both: false, sel: true, p: e.p }; agg.set(key, x); }
        x.n++; x.preds.set(e.p, (x.preds.get(e.p) || 0) + 1);
      }
    }
    this.edges = [...agg.values()];
    const pair = new Map();
    for (const e of this.edges) { const k = e.a < e.b ? e.a + ',' + e.b : e.b + ',' + e.a; let l = pair.get(k); if (!l) pair.set(k, l = []); l.push(e); }
    for (const l of pair.values()) l.forEach((e, i) => { e.off = (i - (l.length - 1) / 2) * 0.22; });
  }

  _egoNodes() {
    const S = this.store, ego = this.ego, seen = new Set([ego.id]); let front = [ego.id]; const order = [ego.id];
    for (let hop = 0; hop < ego.hops; hop++) {
      const next = [];
      for (const u of front) for (const list of [S.out[u], S.inn[u]]) for (const ei of list) {
        const e = S.edges[ei]; if (!this.enabled.has(e.p) && hop > 0) continue; const v = e.s === u ? e.o : e.s;
        if (seen.has(v) || !this.universe[v] || order.length >= 300) continue; seen.add(v); order.push(v); next.push(v);
      }
      front = next;
    }
    return order;
  }

  // ----- tree operations (all async, queued) -----
  async _expandCluster(c, o = {}) {
    if (c.expanded) return; await this._children(c);
    this._savePos(); c.expanded = true; this.expandOrder.push(c.id);
    this._emit('expand', { id: c.id, label: c.label, count: c.members.length });
    if (!o.quiet) { this.pendingFit = c.id; this._scene({ alpha: 0.75 }); }
  }
  _collapseCluster(c, o = {}) {
    if (!c.expanded) return;
    this._savePos();
    const g = this.layout.group('g:' + c.id); if (g) this.posCache.set('c:' + c.id, { x: g.x, y: g.y, r: this._clusterR(c.members.length) });
    const kill = (x) => { if (x.expanded) { x.expanded = false; this.expandOrder = this.expandOrder.filter((i) => i !== x.id); } if (x.children) x.children.forEach(kill); };
    kill(c);
    this._emit('collapse', { id: c.id, label: c.label });
    if (!o.quiet) this._scene({ alpha: 0.6 });
  }
  expand(id) { return this.enqueue(async () => { const c = this.byId.get(id); if (c) await this._expandCluster(c); }); }
  // expand a cluster and everything below it down to nodes, as long as at most maxNodes nodes would show
  expandAll(id, maxNodes = 600) {
    return this.enqueue(async () => {
      const c = this.byId.get(id); if (!c) return; let nodes = this.items.filter((i) => i.kind === 'node').length; const q = [c];
      while (q.length) {
        const x = q.shift(); if (!x.expanded) { await this._children(x); if (nodes + x.members.length > maxNodes && x !== c && !x.children) continue; await this._expandCluster(x, { quiet: true }); }
        if (x.children) { for (const ch of x.children) q.push(ch); } else nodes += x.members.length;
      }
      this.pendingFit = c.id; this._scene({ alpha: 0.9 });
    });
  }
  collapse(id) { return this.enqueue(() => { const c = this.byId.get(id); if (c) this._collapseCluster(c); }); }
  collapseAll() {
    return this.enqueue(() => {
      this._savePos(); this.ego = null; this.pulled.clear();
      for (const c of this.roots) { const g = this.layout.group('g:' + c.id); if (g) this.posCache.set('c:' + c.id, { x: g.x, y: g.y, r: this._clusterR(c.members.length) }); }
      const kill = (x) => { x.expanded = false; if (x.children) x.children.forEach(kill); };
      this.roots.forEach(kill); this.expandOrder = [];
      this._scene({ alpha: 0.8 }); this.fit(true);
    });
  }
  // the path of cluster ids from the top to the last expanded cluster
  _path() {
    const last = this.expandOrder.length ? this.byId.get(this.expandOrder[this.expandOrder.length - 1]) : null;
    return last ? this._ancestors(last).filter((c) => c.expanded) : [];
  }
  _crumbTo(c) { // collapse everything under c, keep c expanded; null = all
    return this.enqueue(() => {
      if (!c) { this.roots.forEach((r) => this._collapseCluster(r, { quiet: true })); this._scene({ alpha: 0.7 }); this.fit(true); return; }
      if (c.children) c.children.forEach((x) => this._collapseCluster(x, { quiet: true }));
      this.expandOrder = this.expandOrder.filter((i) => !this.byId.get(i) || this._ancestors(this.byId.get(i)).some((a) => a === c) === false || i === c.id);
      this.pendingFit = c.id; this._scene({ alpha: 0.6 });
    });
  }

  // reveal a node: expand the cluster items that contain it until it is an item itself
  async _reveal(id) {
    if (this.ego) { if (this.owner[id] < 0 || this.items[this.owner[id]].kind !== 'node') { this.ego = null; this._scene({ alpha: 0.7 }); } }
    for (let guard = 0; guard < 12; guard++) {
      const i = this.owner[id]; if (i >= 0 && this.items[i].kind === 'node') return true;
      if (i < 0) return false;
      const c = this.items[i].cluster; await this._expandCluster(c);
    }
    return false;
  }
  focus(iri) {
    if (iri == null) return this.select(null);
    return this.enqueue(async () => {
      const id = this.store.nodeId(iri); if (id < 0 || !this.universe[id]) return false;
      const ok = await this._reveal(id); this._setSel({ type: 'node', id }, true);
      const it = this.items[this.owner[id]];
      if (ok && it) { this._centerOn(it.key, Math.max(this.view.k, 1.5)); }
      return ok;
    });
  }
  select(iri) { if (iri == null) { this._setSel(null, false); return Promise.resolve(true); } return this.focus(iri); }
  expandNeighbours(iri) {
    return this.enqueue(async () => {
      const id = iri ? this.store.nodeId(iri) : this.sel && this.sel.type === 'node' ? this.sel.id : -1; if (id < 0) return;
      if (this.owner[id] < 0 || this.items[this.owner[id]].kind !== 'node') await this._reveal(id);
      const S = this.store, nb = new Set();
      for (const list of [S.out[id], S.inn[id]]) for (const ei of list) { const e = S.edges[ei]; const v = e.s === id ? e.o : e.s; if (v !== id && this.universe[v]) nb.add(v); }
      this._savePos();
      let n = 0; for (const v of [...nb].sort((a, b) => a - b)) { if (n >= 60) break; const it = this.items[this.owner[v]]; if (it && it.kind === 'node') continue; this.pulled.set(v, id); n++; }
      this._scene({ alpha: 0.8 });
    });
  }
  egoView(iri, hops) {
    return this.enqueue(() => {
      const id = this.store.nodeId(iri); if (id < 0) return; this._savePos(); this.ego = { id, hops: hops === 2 ? 2 : 1 };
      this._setSel({ type: 'node', id }, true); this._scene({ alpha: 1, prerun: true }); this.fit(true);
    });
  }
  exitEgo() { return this.enqueue(() => { if (!this.ego) return; this._savePos(); this.ego = null; this._scene({ alpha: 0.7 }); this.fit(true); }); }

  setGrouping(id) {
    return this.enqueue(async () => {
      const g = this.groupings.find((x) => x.id === id); if (!g || this.avail.get(id) === false) return false;
      this.grp = g; this.selGroup.value = id; this.ego = null; this.pulled.clear(); this.posCache.clear(); this.layout = new Layout();
      await this._buildRoots(); this._scene({ initial: true }); this.fit(false); return true;
    });
  }
  setFilter(f = {}) {
    return this.enqueue(async () => {
      const S = this.store; let rebuild = false;
      if ('predicates' in f) {
        this.enabled = new Set(f.predicates == null ? S.preds.map((p, i) => (p.on ? i : -1)).filter((i) => i >= 0) : f.predicates.map((p) => S.predIdx.get(p)).filter((i) => i !== undefined));
        if (this.flow >= 0 && !this.enabled.has(this.flow)) this.flow = -1; this._fillFlow(); this._fillPop();
      }
      if ('classes' in f) { this.filter.classes = f.classes && f.classes.length ? new Set(f.classes) : null; rebuild = true; }
      if ('text' in f) { this.filter.text = String(f.text || '').trim().toLowerCase(); rebuild = true; }
      if (rebuild) {
        const { classes, text } = this.filter;
        for (let i = 0; i < S.n; i++) this.universe[i] = (!classes || S.types[i].some((t) => classes.has(t))) && (!text || S.sidx[i].includes(text) || S.iri[i].toLowerCase().includes(text)) ? 1 : 0;
        this.pulled.clear(); this.posCache.clear(); this.layout = new Layout(); this.ego = null;
        await this._buildRoots(); this._scene({ initial: true }); this.fit(false);
      } else { this._savePos(); this._scene({ alpha: 0.5 }); }
    });
  }
  setFlow(iri) {
    return this.enqueue(() => {
      const p = iri ? this.store.predIdx.get(iri) : undefined; this.flow = p === undefined ? -1 : p;
      if (this.flow >= 0) this.enabled.add(this.flow);
      this.selFlow.value = this.flow >= 0 ? iri : ''; this._savePos(); this._scene({ alpha: 1, prerun: true }); this.fit(true);
    });
  }
  highlight(iris) { this.hl = new Set((iris || []).map((i) => this.store.nodeId(i)).filter((i) => i >= 0)); this._renderUi(); this.requestDraw(); return Promise.resolve(); }
  setList(on) { this.listMode = !!on; this.btnList.setAttribute('aria-pressed', String(this.listMode)); this._renderUi(); this.requestDraw(); return Promise.resolve(); }
  updateStates() { this.requestDraw(); }
  refresh() {
    return this.enqueue(async () => {
      const keep = [...this.expandOrder], sel = this.sel && this.sel.type === 'node' ? this.store.iri[this.sel.id] : null;
      await this.store.load(this.o.labelOf); this.assignCache.clear(); this.attrCache.clear();
      const S = this.store; this.enabled = new Set([...this.enabled].map((p) => p).filter((p) => p < S.preds.length));
      this.enabled = new Set(S.preds.map((p, i) => (p.on ? i : -1)).filter((i) => i >= 0));
      this.universe = new Uint8Array(S.n).fill(1); this.pulled.clear(); this.posCache.clear(); this.layout = new Layout();
      await this._buildRoots(); this._fillFlow(); this._fillPop();
      for (const id of keep) { const c = this.byId.get(id); if (c) await this._expandCluster(c, { quiet: true }); }
      this.sel = null; this._scene({ initial: true }); this.fit(false);
      if (sel) { const id = S.nodeId(sel); if (id >= 0) this._setSel({ type: 'node', id }, true); }
    });
  }
  clusterMembers(id) { const c = this.byId.get(id); return c ? c.members.map((m) => this.store.iri[m]) : []; }
  search(text) {
    const S = this.store; const t = String(text || '').trim().toLowerCase(); if (!t || !S.sidx) return [];
    const toks = t.split(/\s+/), hits = [];
    for (let i = 0; i < S.n; i++) {
      if (!this.universe[i]) continue; const s = S.sidx[i]; let sc = 0, ok = true;
      for (const k of toks) { const j = s.indexOf(k); if (j < 0) { ok = false; break; } sc += j === 0 ? 3 : s[j - 1] === ' ' || s[j - 1] === '-' || s[j - 1] === '_' ? 2 : 1; }
      if (ok) hits.push([sc, i]);
    }
    hits.sort((a, b) => b[0] - a[0] || S.display[a[1]].length - S.display[b[1]].length || a[1] - b[1]);
    return hits.slice(0, 40).map(([, i]) => ({ iri: S.iri[i], label: S.display[i], type: short(S.primary[i]) }));
  }

  // ----- events -----
  on(ev, fn) { let l = this.sub.get(ev); if (!l) this.sub.set(ev, l = new Set()); l.add(fn); return () => l.delete(fn); }
  _emit(ev, p) { const l = this.sub.get(ev); if (l) for (const f of [...l]) { try { f(p); } catch (e) { console.error(e); } } }
  _setSel(sel, fromApi) {
    const same = (!sel && !this.sel) || (sel && this.sel && sel.type === this.sel.type && sel.id === this.sel.id);
    this.sel = sel; const iri = sel && sel.type === 'node' ? this.store.iri[sel.id] : null;
    if (!same) { this._savePos(); this._scene({ alpha: 0.35 }); }
    this._card();
    if (!same && (sel === null || sel.type === 'node')) { this._emit('select', iri); if (this.o.onSelect) { try { this.o.onSelect(iri); } catch (e) { console.error(e); } } }
    this.requestDraw();
  }
  _setHover(it) {
    const k = it ? it.key : null; if (k === this.hover) return; this.hover = k;
    this.canvas.classList.toggle('over', !!it);
    if (this.o.onHover) { try { this.o.onHover(it && it.kind === 'node' ? this.store.iri[it.id] : null); } catch (e) { console.error(e); } }
    this.requestDraw();
  }

  // ----- rendering loop -----
  requestDraw() { if (!this.raf && !this.dead) this.raf = requestAnimationFrame((t) => this._frame(t)); }
  _frame(ts) {
    this.raf = 0; if (this.dead) return;
    let busy = false;
    if (this.anim) {
      const a = this.anim; const t = clamp((ts - a.t0) / a.dur, 0, 1), e = t < 0.5 ? 2 * t * t : 1 - Math.pow(-2 * t + 2, 2) / 2;
      let to = a.to; if (a.follow) { const it = this.layout._pos.get(a.follow); if (it) to = this._viewFor(it.x, it.y, a.to.k); }
      this.view = { x: a.from.x + (to.x - a.from.x) * e, y: a.from.y + (to.y - a.from.y) * e, k: a.from.k + (to.k - a.from.k) * e };
      if (t >= 1) this.anim = null; else busy = true;
    }
    if (this.layout.alpha > 0) {
      for (let i = 0; i < 2; i++) { this.stepCount++; if (!this.layout.step()) break; }
      if (this.layout.active) busy = true; else this._settled();
    }
    this._draw(); this.frames++;
    if (busy) this.requestDraw();
  }
  _settled() {
    if (this.pendingFit) {
      const id = this.pendingFit; this.pendingFit = null; const g = this.layout.group('g:' + id);
      if (g) { const b = { x0: g.x - g.r, y0: g.y - g.r, x1: g.x + g.r, y1: g.y + g.r }; const v = this._viewForBounds(b, 0.9, 3); const sw = (g.r * 2 * this.view.k) / Math.min(this.W, this.H);
        const out = this._outside(b); if (out || sw < 0.4) this._animate(v, 450); }
    }
    else if (this.refit) { this.refit = false; const b = this.layout.bounds(); if (b) this._animate(this._viewForBounds(b), 400); }
    this.requestDraw();
  }
  // the part of the canvas not covered by the side card
  _area() {
    const c = this.cardEl; let w = this.W, h = this.H;
    if (c) { if (this.root.hasAttribute('data-narrow')) h = Math.max(60, h - c.offsetHeight - 8); else w = Math.max(120, w - 262); }
    return { w, h };
  }
  _outside(b) { const v = this.view, m = 8, a = this._area(); return b.x0 * v.k + v.x < m || b.y0 * v.k + v.y < m || b.x1 * v.k + v.x > a.w - m || b.y1 * v.k + v.y > a.h - m; }
  _viewFor(x, y, k) { const a = this._area(); return { x: a.w / 2 - x * k, y: a.h / 2 - y * k, k }; }
  _viewForBounds(b, fill = 0.88, maxK = 2.2) {
    const a = this._area(); const w = Math.max(b.x1 - b.x0, 40), hh = Math.max(b.y1 - b.y0, 40);
    const k = clamp(Math.min((a.w * fill - 24) / w, (a.h * fill - 34) / hh), 0.05, maxK);
    return { x: a.w / 2 - ((b.x0 + b.x1) / 2) * k, y: a.h / 2 - ((b.y0 + b.y1) / 2) * k - 6, k };
  }
  _animate(to, dur, follow) { if (!dur) { this.view = to; this.anim = null; this.requestDraw(); return; } this.anim = { t0: performance.now(), dur, from: { ...this.view }, to, follow }; this.requestDraw(); }
  _centerOn(key, k) { const it = this.layout._pos.get(key); if (!it) return; this._animate(this._viewFor(it.x, it.y, k), 500, key); }
  fit(animate) {
    if (!this.W) return; const b = this.layout.bounds(); if (!b) return;
    const to = this._viewForBounds(b); this.anim = null; this.refit = true; if (animate) this._animate(to, 400); else { this.view = to; this.requestDraw(); }
  }
  resize() {
    const r = this.main.getBoundingClientRect(); const W = Math.max(1, Math.round(r.width)), H = Math.max(1, Math.round(r.height));
    const dpr = Math.min(2.5, (typeof devicePixelRatio === 'number' && devicePixelRatio) || 1);
    this.root.toggleAttribute('data-narrow', W < 560);
    if (W === this.W && H === this.H && dpr === this.dpr) return;
    const first = !this.W || this.W < 60 || this.H < 60; const dW = W - this.W, dH = H - this.H;
    if (!first && this.W) { this.view = { ...this.view, x: this.view.x + dW / 2, y: this.view.y + dH / 2 }; if (this.anim) this.anim = null; }
    this.W = W; this.H = H; this.dpr = dpr;
    this.canvas.width = Math.round(W * dpr); this.canvas.height = Math.round(H * dpr);
    if (first && this.items.length) this.fit(false);
    this.requestDraw();
  }
}

Object.assign(GraphView.prototype, {
  _topKeyOf(id) {
    if (!this.topMap) { this.topMap = new Map(); for (const c of this.roots) for (const m of c.members) this.topMap.set(m, c.key); }
    return this.topMap.get(id) || this.store.primary[id];
  },
  _itemColor(it) {
    const S = this.store;
    if (it.kind === 'node') { const ov = this.o.colorOf && this.o.colorOf(S.iri[it.id]); if (ov) return ov; }
    else { const ov = this.o.colorOf && this.o.colorOf(it.cluster.key); if (ov) return ov; }
    return this._colorKey(it.top, null);
  },
  _stateOf(it) { if (it.kind !== 'node' || !this.o.stateOf) return null; try { return this.o.stateOf(this.store.iri[it.id]) || null; } catch { return null; } },
  _selIdx() {
    if (!this.sel) return -1;
    if (this.sel.type === 'node') return this.owner ? this.owner[this.sel.id] : -1;
    return this.items.findIndex((i) => i.key === 'c:' + this.sel.id);
  },
  _neighbourSet(idx) { const s = new Set([idx]); for (const e of this.edges) { if (e.a === idx) s.add(e.b); else if (e.b === idx) s.add(e.a); } return s; },

  // ----- drawing -----
  _draw() {
    const ctx = this.ctx, T = this._T, v = this.view, W = this.W, H = this.H, dpr = this.dpr, items = this.items, S = this.store;
    ctx.setTransform(dpr, 0, 0, dpr, 0, 0); ctx.clearRect(0, 0, W, H);
    if (!items.length) return;
    const k = v.k, X = (x) => x * k + v.x, Y = (y) => y * k + v.y;
    const hoverIdx = this.hover ? items.findIndex((i) => i.key === this.hover) : -1, selIdx = this._selIdx();
    const focusIdx = hoverIdx >= 0 ? hoverIdx : selIdx; const focusSet = focusIdx >= 0 ? this._neighbourSet(focusIdx) : null;
    const hlc = new Map(); if (this.hl.size) for (const id of this.hl) { const i = this.owner[id]; if (i >= 0) hlc.set(i, (hlc.get(i) || 0) + 1); }
    ctx.lineJoin = 'round';
    // expanded clusters
    this.chips = [];
    const gs = [...this.layout.groups.values()].filter((g) => g.key).sort((a, b) => a.depth - b.depth);
    ctx.font = '600 11px ui-sans-serif,system-ui,sans-serif'; ctx.textBaseline = 'middle';
    for (const g of gs) {
      const c = this.byId.get(g.key.slice(2)); if (!c) continue; const col = this._colorKey(this._topOf(c).key, null);
      const cx = X(g.x), cy = Y(g.y), r = g.r * k; if (cx + r < 0 || cy + r < 0 || cx - r > W || cy - r > H) continue;
      ctx.beginPath(); ctx.arc(cx, cy, r, 0, 6.2832); ctx.globalAlpha = 0.07; ctx.fillStyle = col; ctx.fill();
      ctx.globalAlpha = 0.5; ctx.strokeStyle = col; ctx.lineWidth = 1.2; ctx.setLineDash([5, 4]); ctx.stroke(); ctx.setLineDash([]); ctx.globalAlpha = 1;
      const txt = '▾ ' + trunc(c.label, 26) + ' · ' + c.members.length; const w = ctx.measureText(txt).width + 12, hh = 17;
      let bx = cx - w / 2, by = cy - r - hh / 2 - 1; if (by < 2) by = clamp(cy - r + 3, 2, H - hh - 2);
      bx = clamp(bx, 2, Math.max(2, W - w - 2));
      ctx.fillStyle = T.panel; ctx.globalAlpha = 0.92; ctx.beginPath(); ctx.roundRect ? ctx.roundRect(bx, by, w, hh, 8) : ctx.rect(bx, by, w, hh); ctx.fill(); ctx.globalAlpha = 1;
      ctx.strokeStyle = col; ctx.lineWidth = 1; ctx.stroke();
      ctx.fillStyle = T.ink; ctx.textAlign = 'left'; ctx.fillText(txt, bx + 6, by + hh / 2 + 0.5);
      this.chips.push({ x: bx, y: by, w, h: hh, id: c.id });
    }
    // edges
    const geo = (e) => {
      const A = items[e.a], B = items[e.b]; const ax = X(A.x), ay = Y(A.y), bx = X(B.x), by = Y(B.y);
      let dx = bx - ax, dy = by - ay; const d = Math.hypot(dx, dy) || 1; dx /= d; dy /= d;
      const sgn = e.a < e.b ? 1 : -1; const nx = -dy * sgn, ny = dx * sgn;
      const cx = (ax + bx) / 2 + nx * e.off * d, cy = (ay + by) / 2 + ny * e.off * d;
      const ra = Math.max(A.r * k, A.kind === 'node' ? 2.2 : 6), rb = Math.max(B.r * k, B.kind === 'node' ? 2.2 : 6);
      let ux = cx - ax, uy = cy - ay, ul = Math.hypot(ux, uy) || 1; const x0 = ax + ux / ul * ra, y0 = ay + uy / ul * ra;
      ux = cx - bx; uy = cy - by; ul = Math.hypot(ux, uy) || 1; const x1 = bx + ux / ul * (rb + 1), y1 = by + uy / ul * (rb + 1);
      e._g = { x0, y0, cx, cy, x1, y1, ex: -ux / ul, ey: -uy / ul, len: d };
      return e._g;
    };
    const drawEdges = (pass) => {
      for (const e of this.edges) {
        if (!!e.sel !== pass) continue; const G = geo(e);
        const inFocus = !focusSet || e.sel || (focusSet.has(e.a) && focusSet.has(e.b) && (e.a === focusIdx || e.b === focusIdx));
        const cl = !e.both && !e.sel; const hov = e === this.hoverEdge;
        let wd = e.sel ? 1.6 + Math.min(2.2, Math.log2(e.n + 1) * 0.7) : cl ? 1.2 + Math.min(6, Math.log2(e.n + 1) * 1.05) : 0.9 + Math.min(1.6, Math.log2(e.n) * 0.5);
        if (hov) wd += 1.5;
        ctx.globalAlpha = hov || e.sel ? 0.95 : inFocus ? (cl ? 0.55 : 0.5) : 0.07; ctx.strokeStyle = e.color; ctx.lineWidth = wd;
        ctx.beginPath(); ctx.moveTo(G.x0, G.y0); ctx.quadraticCurveTo(G.cx, G.cy, G.x1, G.y1); ctx.stroke();
        if (G.len > 26 && (inFocus || hov)) {
          const al = 4 + wd * 1.1; ctx.fillStyle = e.color; ctx.beginPath(); ctx.moveTo(G.x1, G.y1);
          ctx.lineTo(G.x1 - G.ex * al - G.ey * al * 0.5, G.y1 - G.ey * al + G.ex * al * 0.5); ctx.lineTo(G.x1 - G.ex * al + G.ey * al * 0.5, G.y1 - G.ey * al - G.ex * al * 0.5); ctx.fill();
        }
      }
      ctx.globalAlpha = 1;
    };
    drawEdges(false); drawEdges(true);
    // clusters then nodes
    const labels = [];
    const order = [...items.keys()].sort((a, b) => (items[a].kind === 'node') - (items[b].kind === 'node'));
    for (const i of order) {
      const it = items[i]; const sx = X(it.x), sy = Y(it.y); const rs = Math.max(it.r * k, it.kind === 'node' ? 2.2 : 6);
      it._sx = sx; it._sy = sy; it._rs = rs;
      if (sx + rs < -20 || sy + rs < -20 || sx - rs > W + 20 || sy - rs > H + 20) { it._off = true; continue; } it._off = false;
      const dim = (focusSet && !focusSet.has(i)) || (it.noflow && this.flow >= 0 && !hlc.has(i) && i !== selIdx); const col = this._itemColor(it); const st = this._stateOf(it);
      ctx.globalAlpha = dim ? 0.22 : 1;
      if (it.kind === 'cluster') {
        ctx.beginPath(); ctx.arc(sx, sy, rs, 0, 6.2832); ctx.fillStyle = col; ctx.globalAlpha = dim ? 0.06 : 0.24; ctx.fill(); ctx.globalAlpha = dim ? 0.3 : 1; ctx.strokeStyle = col; ctx.lineWidth = 1.8; ctx.stroke();
        const h = hlc.get(i); if (h) { ctx.strokeStyle = T.hl; ctx.lineWidth = 2.4; ctx.setLineDash([4, 3]); ctx.beginPath(); ctx.arc(sx, sy, rs + 3, 0, 6.2832); ctx.stroke(); ctx.setLineDash([]); }
      } else {
        ctx.beginPath(); ctx.arc(sx, sy, rs, 0, 6.2832); ctx.fillStyle = st ? st.color : col; ctx.fill();
        ctx.lineWidth = st ? 2 : 1; ctx.strokeStyle = st ? col : T.bg; ctx.stroke();
        if (hlc.has(i)) { ctx.strokeStyle = T.hl; ctx.lineWidth = 2.4; ctx.beginPath(); ctx.arc(sx, sy, rs + 3, 0, 6.2832); ctx.stroke(); }
      }
      if (i === selIdx) { ctx.globalAlpha = 1; ctx.strokeStyle = T.sel; ctx.lineWidth = 2.6; ctx.beginPath(); ctx.arc(sx, sy, rs + 4, 0, 6.2832); ctx.stroke(); }
      else if (i === hoverIdx) { ctx.globalAlpha = 1; ctx.strokeStyle = T.ink; ctx.lineWidth = 1.6; ctx.beginPath(); ctx.arc(sx, sy, rs + 3, 0, 6.2832); ctx.stroke(); }
      ctx.globalAlpha = 1;
      const pr = i === selIdx ? 9 : i === hoverIdx ? 8 : hlc.has(i) ? 6 : it.kind === 'cluster' ? 5 : (focusSet && focusSet.has(i)) ? 4 : 2;
      const baseName = it.kind === 'cluster' ? it.cluster.label : S.display[it.id];
      if (it.kind === 'cluster') { const h = hlc.get(i); labels.push({ i, it, pr, name: baseName, count: it.count, hl: h || 0, dim }); }
      else if (pr > 2 || (rs >= 6.5 && !dim && k >= 1.1)) labels.push({ i, it, pr, name: baseName, dim });
    }
    // labels with a halo, placed greedily by priority
    labels.sort((a, b) => b.pr - a.pr || (b.it.count || 0) - (a.it.count || 0));
    const placed = []; const hit = (r) => { for (const p of placed) if (r.x < p.x + p.w && r.x + r.w > p.x && r.y < p.y + p.h && r.y + r.h > p.y) return true; return false; };
    ctx.textAlign = 'center'; ctx.textBaseline = 'middle'; ctx.lineWidth = 3.2; ctx.strokeStyle = T.halo;
    for (const L of labels) {
      const it = L.it, sx = it._sx, sy = it._sy, rs = it._rs; if (it._off) continue;
      ctx.globalAlpha = L.dim ? 0.3 : 1;
      if (it.kind === 'cluster') {
        const inside = rs >= 24; ctx.font = `600 ${inside ? clamp(rs / 3.2, 10, 13) : 11}px ui-sans-serif,system-ui,sans-serif`;
        let name = trunc(L.name, inside ? Math.max(6, Math.floor(rs / 3.6)) : 26);
        if (inside) {
          ctx.fillStyle = T.ink; ctx.fillText(name, sx, sy - 6); ctx.font = '11px ui-sans-serif,system-ui,sans-serif'; ctx.fillStyle = T.dim; ctx.fillText(String(L.count) + (L.hl ? ' · ' + L.hl + '★' : ''), sx, sy + 8);
        } else {
          if (rs >= 9) { ctx.font = '10px ui-sans-serif,system-ui,sans-serif'; ctx.fillStyle = T.ink; ctx.fillText(String(L.count), sx, sy); ctx.font = '600 11px ui-sans-serif,system-ui,sans-serif'; }
          const txt = rs >= 9 ? name : name + ' · ' + L.count; const w = ctx.measureText(txt).width + 4, r = { x: sx - w / 2, y: sy + rs + 2, w, h: 13 };
          if (L.pr > 5 || !hit(r)) { placed.push(r); ctx.strokeText(txt, sx, r.y + 7); ctx.fillStyle = T.ink; ctx.fillText(txt, sx, r.y + 7); }
        }
      } else {
        ctx.font = (L.pr >= 8 ? '600 ' : '') + '11px ui-sans-serif,system-ui,sans-serif'; const txt = trunc(L.name, 30); const w = ctx.measureText(txt).width + 4;
        const r = { x: sx - w / 2, y: sy + rs + 3, w, h: 13 };
        if (L.pr >= 8 || !hit(r)) { placed.push(r); ctx.strokeText(txt, sx, r.y + 7); ctx.fillStyle = T.ink; ctx.fillText(txt, sx, r.y + 7); }
      }
    }
    ctx.globalAlpha = 1;
  },

  // ----- hit testing, pointer, keys -----
  _local(e) { const r = this.canvas.getBoundingClientRect(); return { x: e.clientX - r.left, y: e.clientY - r.top }; },
  _hitItem(p) {
    let best = null, bd = Infinity;
    for (let i = this.items.length - 1; i >= 0; i--) {
      const it = this.items[i]; if (it._off || it._sx == null) continue; const d = Math.hypot(it._sx - p.x, it._sy - p.y); const R = it._rs + (it.kind === 'node' ? 4 : 2);
      if (d <= R && (it.kind === 'node' ? d - 100 : d) < bd) { bd = it.kind === 'node' ? d - 100 : d; best = it; }
    }
    return best;
  },
  _hitChip(p) { for (let i = this.chips.length - 1; i >= 0; i--) { const c = this.chips[i]; if (p.x >= c.x && p.x <= c.x + c.w && p.y >= c.y && p.y <= c.y + c.h) return c; } return null; },
  _hitEdge(p) {
    let best = null, bd = 6;
    for (const e of this.edges) {
      const G = e._g; if (!G) continue;
      for (let t = 0.1; t < 1; t += 0.1) {
        const u = 1 - t, x = u * u * G.x0 + 2 * u * t * G.cx + t * t * G.x1, y = u * u * G.y0 + 2 * u * t * G.cy + t * t * G.y1; const d = Math.hypot(x - p.x, y - p.y);
        if (d < bd) { bd = d; best = e; }
      }
    }
    return best;
  },
  _world(p) { return { x: (p.x - this.view.x) / this.view.k, y: (p.y - this.view.y) / this.view.k }; },
  _down(e) {
    this.canvas.focus({ preventScroll: true }); this._shift = e.shiftKey;
    try { this.canvas.setPointerCapture(e.pointerId); } catch { /* synthetic events */ }
    const p = this._local(e); this.ptr.set(e.pointerId, p); this.tip.hidden = true; this.anim = null; this.refit = false;
    if (this.ptr.size === 2) { const [a, b] = [...this.ptr.values()]; this.pinch = { d: Math.hypot(a.x - b.x, a.y - b.y), k: this.view.k, mx: (a.x + b.x) / 2, my: (a.y + b.y) / 2, vx: this.view.x, vy: this.view.y }; this.drag = null; return; }
    const chip = this._hitChip(p); if (chip) { this.drag = { mode: 'chip', chip, p0: p, moved: false }; return; }
    const it = this._hitItem(p);
    if (it) { this.drag = { mode: 'item', it, p0: p, moved: false, off: { x: it.x - this._world(p).x, y: it.y - this._world(p).y } }; }
    else this.drag = { mode: 'pan', p0: p, v0: { ...this.view }, moved: false };
    this.canvas.classList.add('drag');
  },
  _move(e) {
    const p = this._local(e);
    if (this.ptr.has(e.pointerId)) this.ptr.set(e.pointerId, p);
    if (this.pinch && this.ptr.size >= 2) {
      const [a, b] = [...this.ptr.values()]; const d = Math.hypot(a.x - b.x, a.y - b.y) || 1; const k = clamp(this.pinch.k * d / this.pinch.d, 0.05, 6);
      const mx = (a.x + b.x) / 2, my = (a.y + b.y) / 2; const wx = (this.pinch.mx - this.pinch.vx) / this.pinch.k, wy = (this.pinch.my - this.pinch.vy) / this.pinch.k;
      this.view = { k, x: mx - wx * k, y: my - wy * k }; this.requestDraw(); return;
    }
    const dr = this.drag;
    if (dr) {
      if (!dr.moved && Math.hypot(p.x - dr.p0.x, p.y - dr.p0.y) > 4) dr.moved = true;
      if (dr.moved) {
        if (dr.mode === 'pan') { this.view = { k: dr.v0.k, x: dr.v0.x + p.x - dr.p0.x, y: dr.v0.y + p.y - dr.p0.y }; this.requestDraw(); }
        else if (dr.mode === 'item') { const w = this._world(p); dr.it.pin = true; dr.it.x = w.x + dr.off.x; dr.it.y = w.y + dr.off.y; this.layout.wake(0.35); this.requestDraw(); }
      }
      return;
    }
    // hover
    const it = this._hitItem(p); this._setHover(it);
    let edge = null; if (!it) edge = this._hitEdge(p);
    if (edge !== this.hoverEdge) { this.hoverEdge = edge; this.requestDraw(); }
    if (it || edge) { this._showTip(it, edge, p); } else this.tip.hidden = true;
  },
  _up(e, cancel) {
    const p = this._local(e); this.ptr.delete(e.pointerId); if (this.ptr.size < 2) this.pinch = null;
    try { this.canvas.releasePointerCapture(e.pointerId); } catch { /* ok */ }
    this.canvas.classList.remove('drag');
    const dr = this.drag; this.drag = null; if (!dr || cancel) return;
    if (dr.mode === 'item') { dr.it.pin = false; }
    if (dr.moved) return;
    if (dr.mode === 'chip') { this.collapse(dr.chip.id); return; }
    if (dr.mode === 'item') {
      const now = performance.now(), it = dr.it;
      if (it.kind === 'node' && this.lastClick && this.lastClick.key === it.key && now - this.lastClick.t < 380) { this.lastClick = null; this.egoView(this.store.iri[it.id], 1); return; }
      this.lastClick = { key: it.key, t: now }; this._activate(it); return;
    }
    if (this.sel) this._setSel(null, false);
  },
  _activate(it) {
    if (it.kind === 'cluster') { if (!it.cluster.expanded) { if (this._shift) this.expandAll(it.cluster.id); else this.expand(it.cluster.id); } }
    else this.focusNode(it.id);
  },
  focusNode(id) { // select a visible node (no reveal needed)
    this._setSel({ type: 'node', id }, false);
  },
  _wheel(e) {
    e.preventDefault(); this.refit = false; const p = this._local(e); const f = Math.exp(-e.deltaY * (e.ctrlKey ? 0.01 : 0.0016)); const k = clamp(this.view.k * f, 0.05, 6);
    const wx = (p.x - this.view.x) / this.view.k, wy = (p.y - this.view.y) / this.view.k; this.anim = null;
    this.view = { k, x: p.x - wx * k, y: p.y - wy * k }; this.requestDraw();
  },
  _showTip(it, edge, p) {
    const S = this.store; let t = '';
    if (it) {
      if (it.kind === 'node') t = S.display[it.id] + '\n' + short(S.primary[it.id]) + ' · ' + S.degree[it.id] + ' links';
      else t = it.cluster.label + ' · ' + it.count + ' ' + (it.count === 1 ? 'member' : 'members') + '\n' + ((this.grp.levels[it.cluster.level] || {}).label || 'cluster') + (it.internal ? ' · ' + it.internal + ' links inside' : '') + '\nclick to expand';
    } else {
      const A = this.items[edge.a], B = this.items[edge.b], nm = (x) => (x.kind === 'node' ? S.display[x.id] : x.cluster.label);
      t = nm(A) + ' → ' + nm(B) + '\n' + [...edge.preds.entries()].sort((a, b) => b[1] - a[1]).slice(0, 8).map(([pi, n]) => S.preds[pi].short + ' ×' + n).join('\n');
    }
    this.tip.textContent = t; this.tip.hidden = false;
    const tw = this.tip.offsetWidth, th = this.tip.offsetHeight;
    this.tip.style.left = clamp(p.x + 12, 2, Math.max(2, this.W - tw - 2)) + 'px'; this.tip.style.top = clamp(p.y + 14, 2, Math.max(2, this.H - th - 2)) + 'px';
  },
  _key(e) {
    if (e.key === '/' || (e.key === 'f' && (e.ctrlKey || e.metaKey))) { e.preventDefault(); this.inSearch.focus(); this.inSearch.select(); return; }
    if (e.key === 'Escape') { this._setSel(null, false); this.hl = new Set(); this.inSearch.value = ''; this.hits.hidden = true; this._closePop(); this.requestDraw(); return; }
    if (e.key === 'Backspace') { e.preventDefault(); this.up(); return; }
    if (e.key === 'Enter') {
      e.preventDefault(); const i = this._selIdx(); const it = i >= 0 ? this.items[i] : null;
      if (it && it.kind === 'cluster') this.expand(it.cluster.id); else if (this.sel && this.sel.type === 'node') this.expandNeighbours();
      return;
    }
    if (e.key === '+' || e.key === '=') { this._zoomBy(1.25); return; } if (e.key === '-') { this._zoomBy(0.8); return; } if (e.key === '0') { this.fit(true); return; }
    const dir = { ArrowLeft: [-1, 0], ArrowRight: [1, 0], ArrowUp: [0, -1], ArrowDown: [0, 1] }[e.key];
    if (dir) { e.preventDefault(); this._nav(dir); }
  },
  _zoomBy(f) { const k = clamp(this.view.k * f, 0.05, 6), cx = this.W / 2, cy = this.H / 2, wx = (cx - this.view.x) / this.view.k, wy = (cy - this.view.y) / this.view.k; this.view = { k, x: cx - wx * k, y: cy - wy * k }; this.requestDraw(); },
  up() {
    return this.enqueue(() => {
      if (this.ego) { this._savePos(); this.ego = null; this._scene({ alpha: 0.7 }); this.fit(true); return; }
      const id = this.expandOrder[this.expandOrder.length - 1]; const c = id && this.byId.get(id); if (c) this._collapseCluster(c);
    });
  },
  _nav(dir) {
    const items = this.items; if (!items.length) return; let cur = this._selIdx();
    if (cur < 0) { // nearest to the centre
      const cx = (this.W / 2 - this.view.x) / this.view.k, cy = (this.H / 2 - this.view.y) / this.view.k; let bd = Infinity;
      items.forEach((it, i) => { const d = Math.hypot(it.x - cx, it.y - cy); if (d < bd) { bd = d; cur = i; } });
      this._pickIdx(cur); return;
    }
    const nb = this._neighbourSet(cur); nb.delete(cur); const pool = nb.size ? [...nb] : items.map((_, i) => i).filter((i) => i !== cur);
    const A = items[cur]; let best = -1, bs = Infinity;
    const run = (list, maxAng) => {
      for (const i of list) {
        const dx = items[i].x - A.x, dy = items[i].y - A.y, d = Math.hypot(dx, dy) || 1; const cos = (dx * dir[0] + dy * dir[1]) / d; if (cos < maxAng) continue;
        const s = d * (2 - cos); if (s < bs) { bs = s; best = i; }
      }
    };
    run(pool, 0.4); if (best < 0 && nb.size) run(items.map((_, i) => i).filter((i) => i !== cur), 0.5);
    if (best >= 0) this._pickIdx(best);
  },
  _pickIdx(i) {
    const it = this.items[i]; if (!it) return;
    if (it.kind === 'node') this._setSel({ type: 'node', id: it.id }, false); else this._setSel({ type: 'cluster', id: it.cluster.id }, false);
    const sx = it.x * this.view.k + this.view.x, sy = it.y * this.view.k + this.view.y;
    if (sx < 20 || sy < 20 || sx > this.W - 20 || sy > this.H - 20) this._centerOn(it.key, this.view.k);
  },

  // ----- toolbar, crumbs, list, card, legend -----
  _renderUi() {
    if (!this.items) return; const S = this.store;
    // breadcrumb
    const kids = [];
    if (this.ego) {
      const id = this.ego.id;
      kids.push(h('button', { type: 'button', onclick: () => this.exitEgo() }, '← Back'), h('span', null, 'Ego view of '), h('b', null, S.display[id]),
        h('button', { type: 'button', class: this.ego.hops === 1 ? 'on' : '', onclick: () => this.egoView(S.iri[id], 1) }, '1 hop'),
        h('button', { type: 'button', class: this.ego.hops === 2 ? 'on' : '', onclick: () => this.egoView(S.iri[id], 2) }, '2 hops'));
    } else {
      const path = this._path();
      kids.push(path.length ? h('button', { type: 'button', onclick: () => this._crumbTo(null) }, 'All') : h('b', null, 'All'));
      path.forEach((c, i) => { kids.push(h('span', null, '›'), i === path.length - 1 ? h('b', null, c.label) : h('button', { type: 'button', onclick: () => this._crumbTo(c) }, c.label)); });
    }
    const nc = this.items.filter((i) => i.kind === 'cluster').length, nn = this.items.length - nc;
    kids.push(h('span', { class: 'sp' }), h('span', { class: 'st' }, `${nc} clusters · ${nn} nodes · ${this.edges.length} edges`));
    this.crumbs.replaceChildren(...kids);
    this.canvas.setAttribute('aria-label', `Clustered graph, grouped by ${this.grp.label}: ${nc} clusters, ${nn} nodes, ${this.edges.length} edges shown. Use the List button for a table.`);
    this._renderList(); if (this.legend) this._renderLegend(); this._card();
  },
  _renderList() {
    if (this.listEl) { this.listEl.remove(); this.listEl = null; }
    if (!this.listMode) return; const S = this.store;
    const links = new Map(); for (const e of this.edges) { links.set(e.a, (links.get(e.a) || 0) + e.n); links.set(e.b, (links.get(e.b) || 0) + e.n); }
    const rows = this.items.map((it, i) => {
      const cl = it.kind === 'cluster'; const tr = h('tr', { tabindex: '0', role: 'button', 'data-key': it.key, 'data-kind': it.kind, onclick: () => this._activate(it), onkeydown: (e) => { if (e.key === 'Enter' || e.key === ' ') { e.preventDefault(); this._activate(it); } } },
        h('td', null, cl ? it.cluster.label : S.display[it.id]), h('td', null, it.kind),
        h('td', null, cl ? ((this.grp.levels[it.cluster.level] || {}).label || 'cluster') : short(S.primary[it.id])),
        h('td', { class: 'num' }, cl ? String(it.count) : ''), h('td', { class: 'num' }, String((links.get(i) || 0) + (it.internal || 0))));
      return tr;
    });
    this.listEl = h('div', { class: 'gv-list' }, h('table', { 'aria-label': 'Current level of the graph' },
      h('thead', null, h('tr', null, ...['Name', 'Kind', 'Type', 'Members', 'Links'].map((t) => h('th', { scope: 'col' }, t)))), h('tbody', null, ...rows)));
    this.main.append(this.listEl);
  },
  _toggleLegend() { if (this.legend) { this.legend.remove(); this.legend = null; this.btnLegend.setAttribute('aria-pressed', 'false'); } else { this.legend = h('div', { class: 'gv-legend' }); this.main.append(this.legend); this.btnLegend.setAttribute('aria-pressed', 'true'); this._renderLegend(); } },
  _renderLegend() {
    const S = this.store; const used = new Map();
    for (const it of this.items) if (!used.has(it.top)) used.set(it.top, it);
    const lv = this.grp.levels[0] || {}; const kids = [h('h4', null, lv.label || 'Groups')];
    let n = 0; for (const c of this.roots) { if (!used.has(c.key)) continue; if (n++ >= 14) { kids.push(h('div', null, '…')); break; } kids.push(h('div', null, h('i', { style: `background:${this._colorKey(c.key, null)}` }), trunc(c.label, 24) + ' ' + c.members.length)); }
    kids.push(h('h4', null, 'Links'));
    const fams = new Map(); for (const i of this.enabled) { const P = S.preds[i]; fams.set(P.fam, P.color); }
    for (const f of [...FAM, FAM_OTHER]) if (fams.has(f[0])) kids.push(h('div', null, h('i', { class: 'ln', style: `background:${f[2]}` }), f[3]));
    this.legend.replaceChildren(...kids);
  },
  _togglePop() { if (this.pop) this._closePop(); else { this.pop = h('div', { class: 'gv-pop', role: 'dialog', 'aria-label': 'Edges' }); this.main.append(this.pop); this.btnEdges.setAttribute('aria-expanded', 'true'); this._fillPop(); } },
  _closePop() { if (this.pop) { this.pop.remove(); this.pop = null; this.btnEdges.setAttribute('aria-expanded', 'false'); } },
  _fillPop() {
    if (!this.pop) return; const S = this.store; const st = this.pop.scrollTop;
    const rows = S.preds.map((p, i) => ({ p, i })).sort((a, b) => b.p.count - a.p.count);
    const mk = ({ p, i }) => h('label', null, h('input', { type: 'checkbox', checked: this.enabled.has(i), 'data-pred': p.iri, onchange: (e) => { const s = new Set(this.enabled); if (e.target.checked) s.add(i); else s.delete(i); this.setFilter({ predicates: [...s].map((j) => S.preds[j].iri) }); } }),
      h('span', { class: 'sw', style: `background:${p.color}` }), h('span', { title: p.iri }, p.short), h('span', { class: 'n' }, String(p.count)));
    const text = h('input', { type: 'search', placeholder: 'only nodes containing…', value: this.filter.text, 'aria-label': 'Filter nodes by text' });
    let tm = 0; text.addEventListener('input', () => { clearTimeout(tm); tm = setTimeout(() => this.setFilter({ text: text.value }), 350); });
    this.pop.replaceChildren(h('h4', null, 'Edges drawn (links between nodes)'), ...rows.map(mk),
      h('div', { class: 'acts', style: 'display:flex;gap:4px;margin-top:6px' }, h('button', { type: 'button', onclick: () => this.setFilter({ predicates: null }) }, 'Defaults'),
        h('button', { type: 'button', onclick: () => this.setFilter({ predicates: S.preds.map((p) => p.iri) }) }, 'All'), h('button', { type: 'button', onclick: () => this.setFilter({ predicates: [] }) }, 'None'),
        h('button', { type: 'button', onclick: () => this._closePop() }, 'Close')), h('h4', null, 'Nodes'), text,
      h('div', { style: 'color:var(--dim);margin-top:4px' }, 'Literal properties are attributes, not edges. rdf:type is never an edge.'));
    this.pop.scrollTop = st;
  },
  _onSearch() {
    const hits = this.search(this.inSearch.value); this.hitList = hits; this.hitCur = -1;
    this.hits.replaceChildren(...hits.map((x, i) => h('button', { type: 'button', role: 'option', 'data-iri': x.iri, onmousedown: (e) => { e.preventDefault(); this._pickHit(x); } }, h('span', null, x.label), h('small', null, x.type))));
    this.hits.hidden = !hits.length;
  },
  _searchKey(e) {
    const btns = [...this.hits.children];
    if (e.key === 'ArrowDown' || e.key === 'ArrowUp') { e.preventDefault(); if (!btns.length) return; this.hitCur = clamp((this.hitCur ?? -1) + (e.key === 'ArrowDown' ? 1 : -1), 0, btns.length - 1); btns.forEach((b, i) => b.classList.toggle('cur', i === this.hitCur)); btns[this.hitCur].scrollIntoView({ block: 'nearest' }); }
    else if (e.key === 'Enter') { e.preventDefault(); const x = (this.hitList || [])[Math.max(0, this.hitCur)]; if (x) this._pickHit(x); }
    else if (e.key === 'Escape') { this.inSearch.value = ''; this.hits.hidden = true; this.canvas.focus(); }
  },
  _pickHit(x) { this.hits.hidden = true; this.inSearch.value = x.label; this.focus(x.iri).then(() => this.canvas.focus({ preventScroll: true })); },

  _card() {
    if (this.cardEl) { this.cardEl.remove(); this.cardEl = null; }
    this.root.removeAttribute('data-card');
    const S = this.store, sel = this.sel; if (!sel) return;
    queueMicrotask(() => { if (this.cardEl) this.root.setAttribute('data-card', ''); });
    const close = h('button', { type: 'button', class: 'x', 'aria-label': 'Close', onclick: () => this._setSel(null, false) }, '✕');
    if (sel.type === 'cluster') {
      const c = this.byId.get(sel.id); if (!c) return; const by = new Map(); for (const m of c.members) { const k = short(S.primary[m]); by.set(k, (by.get(k) || 0) + 1); }
      this.cardEl = h('aside', { class: 'gv-card', 'aria-label': 'Cluster details' }, close, h('h3', null, c.label), h('div', { class: 'iri' }, `${c.members.length} members · ${((this.grp.levels[c.level] || {}).label) || 'cluster'}`),
        h('div', { class: 'acts' }, h('button', { type: 'button', onclick: () => this.expand(c.id) }, 'Expand'), h('button', { type: 'button', onclick: () => this.expandAll(c.id) }, 'Expand all')),
        h('h4', null, 'Types'), h('dl', null, ...[...by.entries()].sort((a, b) => b[1] - a[1]).slice(0, 10).flatMap(([k, n]) => [h('dt', null, k), h('dd', null, String(n))])));
      this.main.append(this.cardEl); return;
    }
    const id = sel.id, iri = S.iri[id];
    const groups = new Map();
    for (const [list, dir] of [[S.out[id], '→'], [S.inn[id], '←']]) for (const ei of list) {
      const e = S.edges[ei]; const other = dir === '→' ? e.o : e.s; const key = dir + ' ' + S.preds[e.p].short; let g = groups.get(key); if (!g) groups.set(key, g = { key, n: [], color: S.preds[e.p].color }); g.n.push(other);
    }
    const gl = [...groups.values()].sort((a, b) => b.n.length - a.n.length);
    const open = gl.length <= 4;
    const links = gl.map((g) => h('details', { open: open && g.n.length <= 8 }, h('summary', { class: 'pr' }, h('span', { style: `border-left:3px solid ${g.color};padding-left:5px` }, g.key), h('span', null, String(g.n.length))),
      h('ul', null, ...g.n.slice(0, 40).map((o) => h('li', null, h('button', { type: 'button', 'data-iri': S.iri[o], onclick: () => this.focus(S.iri[o]) }, S.display[o]))), g.n.length > 40 ? h('li', { style: 'color:var(--dim)' }, `+${g.n.length - 40} more`) : null)));
    const attrs = h('dl', { 'data-attrs': '' });
    this.cardEl = h('aside', { class: 'gv-card', 'aria-label': 'Node details' }, close, h('h3', null, S.display[id]), h('div', { class: 'iri' }, short(iri)),
      h('div', { class: 'chips' }, ...S.types[id].filter((t) => !GENERIC_CLASS.has(t)).map((t) => h('span', { class: 'chip' }, short(t)))),
      h('div', { class: 'acts' }, h('button', { type: 'button', onclick: () => this.expandNeighbours() }, 'Expand neighbours'), h('button', { type: 'button', onclick: () => this.egoView(iri, 1) }, 'Ego 1 hop'), h('button', { type: 'button', onclick: () => this.egoView(iri, 2) }, 'Ego 2 hops')),
      h('h4', null, `Links (${S.out[id].length + S.inn[id].length})`), ...(links.length ? links : [h('div', { style: 'color:var(--dim)' }, 'none')]), h('h4', null, 'Attributes'), attrs);
    this.main.append(this.cardEl);
    const fill = (rows) => { if (this.cardEl && attrs.isConnected) attrs.replaceChildren(...rows.flatMap((r) => [h('dt', null, short(termIri(r.p))), h('dd', null, trunc(String(termLit(r.o)), 120))])); };
    if (this.attrCache.has(iri)) fill(this.attrCache.get(iri));
    else S.q(`SELECT ?p ?o WHERE { <${iri}> ?p ?o . FILTER(isLiteral(?o)) } LIMIT 40`).then((rows) => { this.attrCache.set(iri, rows); fill(rows); }).catch(() => {});
  },

  // ----- state, teardown -----
  getState() {
    const S = this.store, items = this.items || []; const clusters = [], nodes = [];
    const nm = (it) => (it.kind === 'cluster' ? 'c:' + it.cluster.id : S.iri[it.id]);
    items.forEach((it) => {
      if (it.kind === 'cluster') clusters.push({ id: it.cluster.id, label: it.cluster.label, count: it.count, members: it.cluster.members.length, internal: it.internal || 0, level: it.cluster.level, highlight: [...this.hl].filter((m) => this.owner[m] === items.indexOf(it)).length, x: it.x, y: it.y });
      else nodes.push({ iri: S.iri[it.id], label: S.display[it.id], type: S.primary[it.id], cluster: it.cpath && it.cpath.length ? it.cpath[it.cpath.length - 1].slice(2) : null, x: it.x, y: it.y });
    });
    const edges = this.edges.map((e) => ({ from: nm(items[e.a]), to: nm(items[e.b]), predicate: e.both || e.sel ? S.preds[e.p].iri : e.fam, predicates: [...e.preds.entries()].map(([p, n]) => ({ iri: S.preds[p].iri, count: n })), count: e.n, kind: e.sel ? 'selected' : e.both ? 'node' : 'cluster', sel: e.sel }));
    const inner = items.reduce((a, it) => a + (it.internal || 0), 0);
    const total = items.reduce((a, it) => a + (it.kind === 'cluster' ? it.count : 1), 0);
    return {
      grouping: this.grp && this.grp.id, groupings: this.groupings.map((g) => ({ id: g.id, label: g.label, available: this.avail.get(g.id) !== false, levels: g.levels.length })),
      universe: S.n ? this.universe.reduce((a, b) => a + b, 0) : 0, nodeCount: S.n,
      expanded: [...this.expandOrder], path: this._path().map((c) => ({ id: c.id, label: c.label })), ego: this.ego ? { iri: S.iri[this.ego.id], hops: this.ego.hops } : null,
      clusters, nodes, edges,
      counts: { clusters: clusters.length, nodes: nodes.length, edges: edges.length, links: edges.reduce((a, e) => a + e.count, 0), innerLinks: inner, members: total, pulled: this.pulled.size },
      selected: this.sel ? (this.sel.type === 'node' ? S.iri[this.sel.id] : 'c:' + this.sel.id) : null, highlight: [...this.hl].map((i) => S.iri[i]),
      predicates: S.preds.map((p, i) => ({ iri: p.iri, label: p.short, count: p.count, family: p.fam, on: this.enabled.has(i) })), enabledPredicates: [...this.enabled].map((i) => S.preds[i].iri),
      flow: this.flow >= 0 ? S.preds[this.flow].iri : null, list: this.listMode, view: { ...this.view }, size: { w: this.W, h: this.H },
      layout: { alpha: this.layout.alpha, active: this.layout.active, steps: this.stepCount }, rendering: !!this.raf, frames: this.frames,
      listRows: this.listEl ? this.listEl.querySelectorAll('tbody tr').length : null, filter: { text: this.filter.text, classes: this.filter.classes ? [...this.filter.classes] : null },
    };
  },
  destroy() {
    this.dead = true; if (this.raf) cancelAnimationFrame(this.raf); this.raf = 0; if (this.ro) this.ro.disconnect();
    this.root.remove(); this.sub.clear();
  },
});

function makeController(v) {
  const c = {
    ready: v.ready.then(() => c),
    refresh: () => v.refresh(), focus: (i) => v.focus(i), select: (i) => v.select(i), highlight: (a) => v.highlight(a), setGrouping: (i) => v.setGrouping(i),
    setFilter: (f) => v.setFilter(f), setFlow: (p) => v.setFlow(p), expand: (i) => v.expand(i), expandAll: (i, n) => v.expandAll(i, n), collapse: (i) => v.collapse(i), collapseAll: () => v.collapseAll(),
    expandNeighbours: (i) => v.expandNeighbours(i), ego: (i, n) => v.egoView(i, n), exitEgo: () => v.exitEgo(), search: (t) => v.search(t), clusterMembers: (i) => v.clusterMembers(i),
    setList: (b) => v.setList(b), updateStates: () => v.updateStates(), resize: () => v.resize(), getState: () => v.getState(), on: (e, f) => v.on(e, f), destroy: () => v.destroy(),
    up: () => v.up(), fit: () => v.fit(true), whenIdle: () => v.q.then(() => new Promise((r) => { const t = () => (v.layout.active || v.anim ? setTimeout(t, 40) : r()); t(); })),
  };
  c.ready.catch(() => {});
  return c;
}

export function mountGraphView(container, options) { return makeController(new GraphView(container, options || {})); }
