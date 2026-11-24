import Graph from "graphology";
import louvain from "graphology-communities-louvain";
import { localName, namespaceOf, mulberry32, nsName, decodeEntities } from "./util.js";

// The data model is kept apart from cytoscape: it holds the full IRIs / literal forms
// (needed for N-Triples) and the degree numbers.
export class Model {
  constructor() { this.clear(); }
  clear() { this.nodes = new Map(); this.edges = new Map(); this._ek = new Set(); this._comm = null; this._comp = null; }

  add(data) {
    const addedN = [], addedE = [];
    for (const n of (data && data.nodes) || []) {
      if (!n || n.id == null) continue;
      const id = String(n.id);
      const ex = this.nodes.get(id);
      if (ex) { this._merge(ex, n); continue; }
      const node = this._norm(id, n);
      this.nodes.set(id, node);
      addedN.push(node);
    }
    for (const e of (data && data.edges) || []) {
      if (!e || e.source == null || e.target == null) continue;
      const s = String(e.source), t = String(e.target), p = e.predicate != null ? String(e.predicate) : "";
      const key = s + "\u0001" + p + "\u0001" + t;
      if (this._ek.has(key)) continue;
      for (const id of [s, t]) if (!this.nodes.has(id)) { const st = this._norm(id, {}); st.stub = true; this.nodes.set(id, st); addedN.push(st); }
      let eid = e.id != null ? String(e.id) : key;
      if (this.edges.has(eid)) eid = key;
      const edge = { id: eid, source: s, target: t, predicate: p, label: e.label != null ? String(e.label) : (p ? localName(p) : "") };
      this._ek.add(key);
      this.edges.set(eid, edge);
      addedE.push(edge);
    }
    this._comm = this._comp = null;
    this.degrees();
    return { nodes: addedN, edges: addedE };
  }

  _norm(id, n) {
    const kind = n.kind === "literal" || n.kind === "bnode" || n.kind === "iri" ? n.kind : (id.startsWith("_:") ? "bnode" : "iri");
    const node = {
      id, kind,
      label: n.label != null && n.label !== "" ? String(n.label) : "",
      types: Array.isArray(n.types) ? n.types.map(String) : [],
      props: n.props && typeof n.props === "object" ? Object.assign({}, n.props) : {},
      iri: n.iri != null ? String(n.iri) : id,
      datatype: n.datatype ? String(n.datatype) : "", lang: n.lang ? String(n.lang) : "",
      deg: 0, indeg: 0, outdeg: 0,
    };
    if (kind === "literal" && n.value != null) node.value = String(n.value);
    return node;
  }
  _merge(ex, n) {
    if (n.label && (!ex.label || ex.stub)) ex.label = String(n.label);
    if (Array.isArray(n.types)) for (const t of n.types) if (!ex.types.includes(String(t))) ex.types.push(String(t));
    if (n.props) Object.assign(ex.props, n.props);
    if (n.kind && ex.stub) ex.kind = n.kind;
    ex.stub = false;
  }
  degrees() {
    for (const n of this.nodes.values()) n.deg = n.indeg = n.outdeg = 0;
    for (const e of this.edges.values()) {
      const s = this.nodes.get(e.source), t = this.nodes.get(e.target);
      s.outdeg++; t.indeg++; s.deg++; if (e.source !== e.target) t.deg++;
    }
  }
  displayLabel(n, labelFor) {
    let l = "";
    if (labelFor) { try { l = labelFor(n.id) || ""; } catch { l = ""; } }
    if (!l) l = n.label;
    if (!l) l = n.kind === "bnode" ? "_:" + n.id.replace(/^_:/, "").slice(0, 8) : localName(n.id);
    return decodeEntities(l);
  }

  communities() {
    if (this._comm) return this._comm;
    const g = new Graph({ type: "undirected", multi: false, allowSelfLoops: false });
    for (const id of this.nodes.keys()) g.addNode(id);
    for (const e of this.edges.values()) if (e.source !== e.target && !g.hasEdge(e.source, e.target)) g.addEdge(e.source, e.target);
    let map = {};
    if (g.size > 0) { try { map = louvain(g, { rng: mulberry32(42), resolution: 1 }); } catch { map = {}; } }
    this._comm = this._order(map, "Community");
    return this._comm;
  }
  components() {
    if (this._comp) return this._comp;
    const adj = new Map();
    for (const id of this.nodes.keys()) adj.set(id, []);
    for (const e of this.edges.values()) { adj.get(e.source).push(e.target); adj.get(e.target).push(e.source); }
    const map = {}; let c = 0;
    for (const id of this.nodes.keys()) {
      if (map[id] != null) continue;
      const st = [id]; map[id] = c;
      while (st.length) { const u = st.pop(); for (const v of adj.get(u)) if (map[v] == null) { map[v] = c; st.push(v); } }
      c++;
    }
    this._comp = this._order(map, "Component");
    return this._comp;
  }
  // Renumber so that the largest community is 1.
  _order(map, word) {
    const cnt = new Map();
    for (const id of this.nodes.keys()) { const c = map[id] != null ? map[id] : -1; cnt.set(c, (cnt.get(c) || 0) + 1); }
    const order = [...cnt.entries()].sort((a, b) => b[1] - a[1]).map((x) => x[0]);
    const rank = new Map(order.map((c, i) => [c, i + 1]));
    const out = new Map();
    for (const id of this.nodes.keys()) out.set(id, word + " " + rank.get(map[id] != null ? map[id] : -1));
    return out;
  }

  // Group key + display name of a node for a "group by" mode.
  groupOf(n, mode) {
    switch (mode) {
      case "type": {
        if (n.types.length) { const tn = this.nodes.get(n.types[0]); return { key: n.types[0], name: tn && tn.label ? tn.label : localName(n.types[0]) }; }
        if (n.kind === "literal") return { key: "#literal", name: "Literal" };
        if (n.kind === "bnode") return { key: "#bnode", name: "Blank node" };
        return { key: "#untyped", name: "Untyped" };
      }
      case "namespace": {
        if (n.kind === "literal") return { key: "#literal", name: "Literal" };
        if (n.kind === "bnode") return { key: "#bnode", name: "Blank node" };
        const ns = namespaceOf(n.iri);
        return { key: ns, name: nsName(ns) };
      }
      case "community": { const c = this.communities().get(n.id); return { key: c, name: c }; }
      case "component": { const c = this.components().get(n.id); return { key: c, name: c }; }
      default: return { key: "#all", name: "All nodes" };
    }
  }
}

const BS = String.fromCharCode(92);
export function ntEscape(s) {
  let o = "";
  for (const ch of String(s)) {
    const c = ch.codePointAt(0);
    if (c === 92) o += BS + BS; else if (c === 34) o += BS + '"'; else if (c === 10) o += BS + "n"; else if (c === 13) o += BS + "r"; else if (c === 9) o += BS + "t";
    else if (c < 32 || c === 127) o += BS + "u" + c.toString(16).padStart(4, "0"); else o += ch;
  }
  return o;
}
const BAD = new Set([...'<>"{}|^`'].map((x) => x.charCodeAt(0)).concat([92]));
export function ntIri(s) {
  let o = "";
  for (const ch of String(s)) { const c = ch.codePointAt(0); o += c <= 32 || BAD.has(c) ? "%" + c.toString(16).toUpperCase().padStart(2, "0") : ch; }
  return "<" + o + ">";
}
export function ntTerm(n) {
  if (n.kind === "literal") {
    const v = n.value != null ? n.value : n.label;
    let o = '"' + ntEscape(v) + '"';
    if (n.lang) o += "@" + n.lang.replace(/[^A-Za-z0-9-]/g, "");
    else if (n.datatype) o += "^^" + ntIri(n.datatype);
    return o;
  }
  if (n.kind === "bnode") return "_:" + n.id.replace(/^_:/, "").replace(/[^A-Za-z0-9_]/g, "_");
  return ntIri(n.iri || n.id);
}
