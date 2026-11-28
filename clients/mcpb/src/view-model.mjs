// The graph view's model: what a view holds and how query results become its
// nodes and edges. Pure JavaScript, no Node and no engine, because two hosts
// run it: the extension's server (views.mjs), for the view Claude Desktop
// shows, and the rete file explorer, which shows the same view page and
// answers its tool calls itself (experiments/rete-file-explorer/js/graph-host.js).
//
// Term tokens (N-Triples-shaped strings, as the engine emits them) are read
// with a parser the host passes in: `parse(token)` → { kind, value, lang }.
//
// rete-server has a Python port of it (clients/server/rete_server/views.py),
// for the same page: change them together.

import "../../chrome/lib/hints.js"; // Retelens query hints (# view: map, # column ?v: Label — note): globalThis.RetelensHints

/** The tab a query's hints name (# view: table | cards | graph | map), unless the options name one. */
export const withHintedView = (options, text) => {
  if (options?.view || !text) return options;
  const view = globalThis.RetelensHints.parse(text).view;
  return view ? { ...options, view } : options;
};

export const MAX_NODES = 1500; // per view; more is unreadable on a canvas anyway
export const MAX_EDGES = 4000;
export const MAX_ROWS = 5000; // table rows a view keeps
export const MAX_LOG = 300; // patches kept for views that are behind
export const NEIGHBOURS = 150; // statements per direction when a node is expanded
export const LABEL_BATCH = 200;
export const SCHEMA_CLASSES = 60; // classes a schema view draws: the most frequent
// What a file opened in a view shows first, unless something else was asked for.
// A SELECT, not a CONSTRUCT: the engine does not apply LIMIT to a CONSTRUCT
// (it builds the whole answer), and s/p/o columns read as triples all the same.
export const SAMPLE = "SELECT ?s ?p ?o WHERE { ?s ?p ?o } LIMIT 300";

export const RDF_TYPE = "http://www.w3.org/1999/02/22-rdf-syntax-ns#type";
export const LABELS = [
  "http://www.w3.org/2000/01/rdf-schema#label",
  "http://www.w3.org/2004/02/skos/core#prefLabel",
  "http://schema.org/name",
  "https://schema.org/name",
  "http://xmlns.com/foaf/0.1/name",
  "http://purl.org/dc/terms/title",
  "http://purl.org/dc/elements/1.1/title",
];

/**
 * A diagram's own look, unless the call sets it: boxes and classes read best
 * laid out in layers with square connectors, dots in a force layout.
 */
export function lookOf(options, current = {}) {
  const look = { ...options };
  if (look.diagram && look.diagram !== (current.diagram ?? "graph")) {
    const boxes = look.diagram !== "graph";
    look.layout ??= boxes ? "layered" : "cose";
    look.edge_style ??= boxes ? "square" : "curved";
  }
  return look;
}

/** A view's graph, kept by the host so a view that reloads, or falls behind, gets it whole. */
export class View {
  constructor(id, dataset, title, options) {
    this.id = id;
    this.dataset = dataset;
    this.title = title;
    this.options = { ...options };
    this.nodes = new Map(); // token -> node
    this.edges = new Map(); // id -> edge
    this.table = null; // { vars, rows } of the last SELECT, for the table tab
    this.aggregates = new Map(); // id -> { id, label, members }: nodes drawn as one
    this.aggregated = 0; // aggregates made, for their ids
    this.version = 0;
    this.log = []; // [{ version, ...patch }]
    this.ui = null; // what the view last reported: selection, options, …
    this.seenAt = null; // when the view last synced
    this.status = "";
    this.awaiting = null; // { file, key }: a chat attachment the view asks the user for
    this.then = null; // what to show once that file is open: { query, iri }
    this.touchedAt = Date.now();
  }

  /** Record a change: it goes into the graph here and, numbered, to the view. */
  patch(change) {
    this.version += 1;
    this.log.push({ version: this.version, ...change });
    if (this.log.length > MAX_LOG) this.log.splice(0, this.log.length - MAX_LOG);
    this.touchedAt = Date.now();
    return this.version;
  }

  /** What a view at `since` lacks: the patches after it, or everything when it is too far behind. */
  since(since) {
    if (!since || !this.log.length || since < this.log[0].version - 1) return { version: this.version, snapshot: this.snapshot() };
    return { version: this.version, patches: this.log.filter((p) => p.version > since) };
  }

  snapshot() {
    return {
      view_id: this.id,
      dataset: this.dataset,
      title: this.title,
      options: this.options,
      nodes: [...this.nodes.values()],
      edges: [...this.edges.values()],
      table: this.table,
      status: this.status,
      awaiting: this.awaiting,
      aggregates: [...this.aggregates.values()],
    };
  }

  /** Merge elements; the ones actually new go in the patch. */
  add(graph) {
    const nodes = [];
    const edges = [];
    for (const n of graph.nodes) {
      const had = this.nodes.get(n.id);
      if (had) {
        // A node seen again may bring its label, types or more properties.
        let changed = false;
        if (!had.label && n.label) (had.label = n.label), (changed = true);
        for (const t of n.types) if (!had.types.includes(t)) had.types.push(t), (changed = true);
        for (const p of n.props) {
          if (!had.props.some(([a, b]) => a === p[0] && b === p[1])) had.props.push(p), (changed = true);
        }
        if (changed) nodes.push(had);
      } else if (this.nodes.size < MAX_NODES) {
        this.nodes.set(n.id, n);
        nodes.push(n);
      }
    }
    for (const e of graph.edges) {
      if (this.edges.has(e.id) || !this.nodes.has(e.source) || !this.nodes.has(e.target)) continue;
      if (this.edges.size >= MAX_EDGES) break;
      this.edges.set(e.id, e);
      edges.push(e);
    }
    return { nodes, edges };
  }

  clear() {
    this.nodes.clear();
    this.edges.clear();
    this.table = null;
    this.aggregates.clear();
  }

  /** Take nodes out, with their edges; a node leaves its aggregate, and an emptied one goes. */
  remove(ids) {
    const gone = new Set(ids.filter((id) => this.nodes.has(id)));
    for (const id of gone) this.nodes.delete(id);
    for (const [eid, e] of this.edges) if (gone.has(e.source) || gone.has(e.target)) this.edges.delete(eid);
    for (const a of this.aggregates.values()) {
      a.members = a.members.filter((m) => !gone.has(m));
      if (!a.members.length) this.aggregates.delete(a.id);
    }
    this.patch({ op: "remove", ids: [...gone] });
    return gone.size;
  }

  /**
   * Draw nodes as one. An aggregate among them is merged into the new one:
   * a node belongs to one aggregate at a time.
   */
  aggregate(ids, label) {
    const members = new Set();
    const merged = [];
    for (const id of ids) {
      const a = this.aggregates.get(id);
      if (a) {
        for (const m of a.members) members.add(m);
        merged.push(a.id);
      } else if (this.nodes.has(id)) members.add(id);
    }
    if (members.size < 2) throw new Error("aggregate at least two nodes of the view");
    const shrunk = [];
    for (const a of this.aggregates.values()) {
      if (merged.includes(a.id) || !a.members.some((m) => members.has(m))) continue;
      a.members = a.members.filter((m) => !members.has(m));
      if (a.members.length) shrunk.push(a);
      else merged.push(a.id);
    }
    for (const id of merged) this.aggregates.delete(id);
    if (merged.length) this.patch({ op: "ungroup", ids: merged });
    // Copies: a patch says what was, and the aggregate changes on.
    for (const a of shrunk) this.patch({ op: "aggregate", aggregate: { ...a, members: [...a.members] } });
    const aggregate = { id: `agg:${++this.aggregated}`, label: label || `${members.size} nodes`, members: [...members] };
    this.aggregates.set(aggregate.id, aggregate);
    this.patch({ op: "aggregate", aggregate: { ...aggregate, members: [...aggregate.members] } });
    return aggregate;
  }

  /** Split aggregates back into their nodes: one, or "all". */
  ungroup(id) {
    const ids = id === "all" ? [...this.aggregates.keys()] : this.aggregates.has(id) ? [id] : [];
    for (const a of ids) this.aggregates.delete(a);
    if (ids.length) this.patch({ op: "ungroup", ids });
    return ids.length;
  }
}

// --- RDF → graph elements ----------------------------------------------------

export const localName = (iri) => {
  const s = String(iri);
  const cut = Math.max(s.lastIndexOf("#"), s.lastIndexOf("/"));
  return cut >= 0 && cut < s.length - 1 ? s.slice(cut + 1) : s;
};

const literalText = (term) => term.value + (term.lang ? `@${term.lang}` : "");

/** `<iri>` for an IRI given bare or bracketed; anything that cannot be one is refused. */
export const iriToken = (iri) => {
  const bare = String(iri).trim().replace(/^<|>$/g, "");
  if (!bare || /[\s<>"{}|\\^`]/.test(bare)) throw new Error(`not a usable IRI: ${iri}`);
  return `<${bare}>`;
};

/**
 * A schema as graph elements: each class a node of kind "class" with its
 * instance count, the relations to literals its properties, the relations to
 * other classes edges. The most frequent classes only, and what links them.
 */
export function graphOfSchema({ classes, relations }) {
  const kept = [...classes].sort((a, b) => b[1] - a[1]).slice(0, SCHEMA_CLASSES);
  const nodes = new Map();
  const node = (c, count = null) => {
    const iri = c === "(untyped)" ? null : c;
    const id = iri ? `<${iri}>` : "_:untyped";
    if (!nodes.has(id)) nodes.set(id, { id, iri, kind: "class", label: iri ? localName(iri) : "(untyped)", types: [], props: [], count });
    return nodes.get(id);
  };
  for (const [c, n] of kept) node(c, n);
  const known = (c) => nodes.has(c === "(untyped)" ? "_:untyped" : `<${c}>`);
  const edges = [];
  const rows = [];
  for (const [s, p, o, n] of [...relations].sort((a, b) => b[3] - a[3])) {
    if (p === RDF_TYPE || !known(s)) continue;
    if (o === "(literal)") {
      node(s).props.push([p, n.toLocaleString("en-US")]);
    } else if (known(o)) {
      const from = node(s).id;
      const to = node(o).id;
      edges.push({ id: `${from} <${p}> ${to}`, source: from, target: to, p, label: `${localName(p)} (${n.toLocaleString("en-US")})` });
    } else continue;
    rows.push({ class: localName(s), property: localName(p), object: o === "(literal)" ? "literal" : localName(o), count: String(n) });
  }
  return { nodes: [...nodes.values()], edges, table: { vars: ["class", "property", "object", "count"], rows }, classes: classes.length };
}

/**
 * The parts that read term tokens, bound to the host's parser:
 * `parse(token)` → { kind: "iri" | "literal" | "bnode" | "triple", value, lang }.
 */
export function graphReaders(parse) {
  /**
   * Graph elements from triples of term tokens. Literal objects become
   * properties of their subject (the view can draw them as nodes); rdf:type
   * becomes a node's types (what "group by type" groups on), not an edge.
   */
  function graphOf(triples) {
    const nodes = new Map();
    const edges = [];
    const node = (token, term) => {
      let n = nodes.get(token);
      if (!n) {
        n = {
          id: token,
          iri: term.kind === "iri" ? term.value : null,
          kind: term.kind,
          label: term.kind === "iri" ? null : term.value,
          types: [],
          props: [],
        };
        nodes.set(token, n);
      }
      return n;
    };
    for (const [s, p, o] of triples) {
      const st = parse(s);
      const ot = parse(o);
      const pIri = p ? parse(p).value : null;
      const subject = node(s, st);
      if (ot.kind === "literal") {
        subject.props.push([pIri ?? "value", literalText(ot)]);
        if (!subject.label && LABELS.includes(pIri)) subject.label = ot.value;
      } else if (pIri === RDF_TYPE && ot.kind === "iri") {
        if (!subject.types.includes(ot.value)) subject.types.push(ot.value);
      } else {
        node(o, ot);
        edges.push({ id: `${s} ${p ?? "related"} ${o}`, source: s, target: o, p: pIri, label: pIri ? localName(pIri) : "related" });
      }
    }
    return { nodes: [...nodes.values()], edges };
  }

  /**
   * A SELECT's rows as a graph. Columns shaped like subject / predicate /
   * object are read as triples; otherwise the first column is the thing each
   * row is about, an IRI in another column an edge named after the column, and
   * a literal a property.
   */
  function graphOfRows(vars, rows) {
    const names = vars.map((v) => v.toLowerCase());
    const tripleShape =
      ["s", "p", "o"].every((v) => names.includes(v)) || ["subject", "predicate", "object"].every((v) => names.includes(v));
    if (tripleShape) {
      const [s, p, o] = names.includes("s") ? ["s", "p", "o"] : ["subject", "predicate", "object"];
      const col = (n) => vars[names.indexOf(n)];
      return graphOf(rows.filter((r) => r[col(s)] && r[col(o)]).map((r) => [r[col(s)], r[col(p)], r[col(o)]]));
    }
    const [first, ...rest] = vars;
    const triples = [];
    const subjects = new Set();
    const columnIri = (v) => `<urn:rete:column:${v}>`;
    for (const row of rows) {
      if (!row[first]) continue;
      subjects.add(row[first]);
      for (const v of rest) if (row[v]) triples.push([row[first], columnIri(v), row[v]]);
    }
    const graph = graphOf(triples);
    // Rows whose only value is the first column still get their node.
    const have = new Set(graph.nodes.map((n) => n.id));
    for (const s of subjects) {
      if (!have.has(s)) {
        const t = parse(s);
        graph.nodes.push({ id: s, iri: t.kind === "iri" ? t.value : null, kind: t.kind, label: t.kind === "iri" ? null : t.value, types: [], props: [] });
      }
    }
    for (const n of graph.nodes) {
      n.props = n.props.map(([p, v]) => [p.replace(/^urn:rete:column:/, "?"), v]);
      // A ?title / ?label / ?name column names the row's entity.
      n.label ??= n.props.find(([p]) => /^\?(label|title|name|prefLabel)$/i.test(p))?.[1].replace(/@[a-z-]+$/i, "") ?? null;
    }
    for (const e of graph.edges) {
      if (e.p?.startsWith("urn:rete:column:")) {
        e.label = `?${e.p.slice("urn:rete:column:".length)}`;
        e.p = null;
      }
    }
    return graph;
  }

  const display = (token) => {
    const t = parse(token);
    return t.kind === "literal" ? literalText(t) : t.value;
  };

  /** A cell as the Retelens cells take it: { kind, value, datatype?, lang? } (a quoted triple is shown as text). */
  const termOf = (token) => {
    const t = parse(token);
    const o = { kind: t.kind === "triple" ? "literal" : t.kind, value: t.value };
    if (t.datatype) o.datatype = t.datatype;
    if (t.lang) o.lang = t.lang;
    return o;
  };

  /**
   * Table rows: `rows` as plain display strings (filter, sort, CSV), `terms` the
   * same cells typed (the table's rich cells and column statistics, the cards, the map).
   */
  function tableOf(env) {
    if (env.kind === "construct") {
      const spo = ["subject", "predicate", "object"];
      const triples = env.triples.slice(0, MAX_ROWS);
      return {
        vars: spo,
        rows: triples.map((t) => Object.fromEntries(spo.map((k, i) => [k, display(t[i])]))),
        terms: triples.map((t) => Object.fromEntries(spo.map((k, i) => [k, termOf(t[i])]))),
      };
    }
    const rows = env.rows.slice(0, MAX_ROWS);
    return {
      vars: env.vars,
      rows: rows.map((r) => Object.fromEntries(env.vars.map((v) => [v, r[v] === undefined ? "" : display(r[v])]))),
      terms: rows.map((r) => Object.fromEntries(env.vars.filter((v) => r[v] !== undefined).map((v) => [v, termOf(r[v])]))),
    };
  }

  /** Elements and table rows of a finished query's envelope. */
  function elementsOf(env) {
    if (env.kind === "construct") return { graph: graphOf(env.triples), table: tableOf(env) };
    if (env.kind === "select") return { graph: graphOfRows(env.vars, env.rows), table: tableOf(env) };
    throw new Error("an ASK answer is one boolean — there is no graph to show; use SELECT or CONSTRUCT");
  }

  return { graphOf, graphOfRows, tableOf, elementsOf, display };
}

/**
 * Put a finished query's results into a view: added, or replacing what it
 * holds. `elementsOf` is a graphReaders() one. `query`, the SPARQL text, rides
 * with the table: its comments can name the columns (Retelens query hints,
 * clients/chrome/lib/hints.js).
 */
export function loadInto(view, elementsOf, env, mode, query) {
  const { graph, table } = elementsOf(env);
  if (table && query) table.query = String(query).slice(0, 20000);
  if (mode === "replace") {
    view.clear();
    const added = view.add(graph);
    view.table = table;
    view.patch({ op: "replace", nodes: added.nodes, edges: added.edges, table });
    return { ...added, rows: table.rows.length, truncated: graph.nodes.length > MAX_NODES || graph.edges.length > MAX_EDGES };
  }
  const added = view.add(graph);
  if (table) {
    view.table =
      view.table && view.table.vars.join() === table.vars.join()
        ? {
            vars: table.vars,
            rows: [...view.table.rows, ...table.rows].slice(0, MAX_ROWS),
            terms: [...(view.table.terms ?? []), ...(table.terms ?? [])].slice(0, MAX_ROWS),
            ...(view.table.query ? { query: view.table.query } : {}),
          }
        : table;
  }
  view.patch({ op: "add", nodes: added.nodes, edges: added.edges, table: view.table });
  return { ...added, rows: table.rows.length, truncated: view.nodes.size >= MAX_NODES || view.edges.size >= MAX_EDGES };
}

/** A schema's elements into a view, replacing what it holds. */
export function schemaInto(view, schema) {
  const { nodes, edges, table, classes } = graphOfSchema(schema);
  view.clear();
  const added = view.add({ nodes, edges });
  view.table = table;
  view.patch({ op: "replace", nodes: added.nodes, edges: added.edges, table });
  return { classes, shown: nodes.length, relations: edges.length };
}

/**
 * The two queries that bring a node's statements, both directions, each with
 * how its rows become triples. SELECTs, because the engine does not apply
 * LIMIT to a CONSTRUCT: a hub's expansion would read every link it has.
 */
export const neighbourQueries = (node) => [
  { query: `SELECT ?p ?o WHERE { ${node} ?p ?o } LIMIT ${NEIGHBOURS}`, triple: (r) => [node, r.p, r.o] },
  { query: `SELECT ?s ?p WHERE { ?s ?p ${node} } LIMIT ${NEIGHBOURS}`, triple: (r) => [r.s, r.p, node] },
];

/** The triples of neighbourQueries' answers (envelopes), in its order. */
export const neighbourTriples = (node, envs) =>
  neighbourQueries(node).flatMap(({ triple }, i) => (envs[i]?.kind === "select" ? envs[i].rows.filter((r) => r.p && (r.o || r.s)).map(triple) : []));

/** The IRI nodes of a view, among `ids`, that still have no label. */
export const unlabelled = (view, ids) =>
  ids.filter((id) => {
    const n = view.nodes.get(id);
    return n && n.kind === "iri" && !n.label;
  });

/** Labels and types read for some nodes, merged into the view and patched to it. */
export function applyLabels(view, found) {
  const labels = {};
  for (const n of found.nodes) {
    const had = view.nodes.get(n.id);
    if (!had) continue;
    if (!had.label && n.label) had.label = n.label;
    for (const t of n.types) if (!had.types.includes(t)) had.types.push(t);
    labels[n.id] = { label: had.label, types: had.types };
  }
  if (Object.keys(labels).length) view.patch({ op: "labels", labels });
  return Object.keys(labels).length;
}
