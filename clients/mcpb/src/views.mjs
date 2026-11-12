// Graph views: an MCP App (SEP-1865) that Claude Desktop renders inline in the
// conversation — the graph of a query's results, which the user explores
// (labels, grouping layers, layouts, zoom, a table, search, expanding nodes)
// and Claude drives with tool calls.
//
// The data never travels through the model: graph_view answers Claude with a
// view id and counts, and the view fetches its elements with app-only tools
// (visibility ["app"]: the host hides them from the model). MCP Apps has no
// server push, so a change Claude makes (graph_view_update) is recorded here
// as a numbered patch, which the view collects on its next sync — about every
// second while it is shown — together with what the user selected, which
// graph_view_state reports back to Claude.
//
// A view also takes a file from the user — chosen or dropped in it, sent here
// in parts (graph_view_upload). That is how a file attached to the chat reaches
// the extension at all: Claude's copy is in its own sandbox, out of reach.
import { readFile } from "node:fs/promises";
import { join } from "node:path";

import { RESOURCE_MIME_TYPE, registerAppResource, registerAppTool } from "@modelcontextprotocol/ext-apps/server";
import { z } from "zod";
import { Term } from "rete-graph";

import { AttachmentError, uploadLimit } from "./graphs.mjs";

const VIEW_URI = "ui://rete/graph-view.html";
const MAX_NODES = 1500; // per view; more is unreadable on a canvas anyway
const MAX_EDGES = 4000;
const MAX_ROWS = 5000; // table rows a view keeps
const MAX_VIEWS = 20;
const MAX_LOG = 300; // patches kept for views that are behind
const WAIT_MS = 35_000; // a view call answers before the client's 60 s limit
const NEIGHBOURS = 150; // statements per direction when a node is expanded
const LABEL_BATCH = 200;
const SCHEMA_CLASSES = 60; // classes a schema view draws: the most frequent
// Files being opened in views are held whole until their last part: an upload
// the view stopped sending is dropped after this long, and all of them
// together may hold this much (one file opened in a view may be 512 MB).
const UPLOAD_IDLE_MS = 2 * 60_000;
const UPLOADS_MAX_BYTES = 512 * 1024 * 1024;
// What a file opened in a view shows first, unless Claude asked for a query.
const SAMPLE = "CONSTRUCT { ?s ?p ?o } WHERE { ?s ?p ?o } LIMIT 300";
const mb = (n) => `${(n / 1024 / 1024).toFixed(n < 10 * 1024 * 1024 ? 1 : 0)} MB`;

const RDF_TYPE = "http://www.w3.org/1999/02/22-rdf-syntax-ns#type";
const LABELS = [
  "http://www.w3.org/2000/01/rdf-schema#label",
  "http://www.w3.org/2004/02/skos/core#prefLabel",
  "http://schema.org/name",
  "https://schema.org/name",
  "http://xmlns.com/foaf/0.1/name",
  "http://purl.org/dc/terms/title",
  "http://purl.org/dc/elements/1.1/title",
];

/** The shape of a graph view's look, shared by graph_view and graph_view_update. */
const optionsSchema = z
  .object({
    diagram: z
      .enum(["graph", "boxes", "classes"])
      .optional()
      .describe(
        "how to draw it: graph (default) — each entity a dot; boxes — each entity a box with its " +
          "name and type, its properties listed below (a UML object diagram); classes — one box per " +
          "rdf:type listing the properties its members carry, linked by the relations between the " +
          "classes (a UML class diagram). boxes and classes come laid out in layers with square lines",
      ),
    labels: z.boolean().optional().describe("show node labels (default true)"),
    properties: z.boolean().optional().describe("list properties inside boxes (default true)"),
    node_shape: z.enum(["circle", "square"]).optional().describe("the dots of the graph diagram (default circle)"),
    edge_style: z
      .enum(["curved", "straight", "square"])
      .optional()
      .describe("lines between nodes: square draws orthogonal connectors, as in UML diagrams"),
    edge_labels: z.boolean().optional().describe("show predicate names on edges (default false)"),
    group_by: z
      .string()
      .optional()
      .describe('colour and layer nodes by "type" (rdf:type, default), "namespace", "none", or a predicate IRI'),
    group_boxes: z.boolean().optional().describe("draw each group as a box around its nodes (default false)"),
    layout: z
      .enum(["cose", "layered", "concentric", "breadthfirst", "circle", "grid"])
      .optional()
      .describe("cose: force-directed (default for graph); layered: top-down layers (default for boxes and classes)"),
    view: z.enum(["graph", "table"]).optional().describe("which tab is shown"),
    theme: z.enum(["auto", "light", "dark"]).optional().describe("the view's colours; auto (default) follows Claude's theme"),
    literals_as_nodes: z.boolean().optional().describe("draw literal values as nodes instead of node properties"),
  })
  .optional();

/**
 * A diagram's own look, unless the call sets it: boxes and classes read best
 * laid out in layers with square connectors, dots in a force layout.
 */
function lookOf(options, current = {}) {
  const look = { ...options };
  if (look.diagram && look.diagram !== (current.diagram ?? "graph")) {
    const boxes = look.diagram !== "graph";
    look.layout ??= boxes ? "layered" : "cose";
    look.edge_style ??= boxes ? "square" : "curved";
  }
  return look;
}

/** A view's graph, kept here so a view that reloads, or falls behind, gets it whole. */
class View {
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

const localName = (iri) => {
  const s = String(iri);
  const cut = Math.max(s.lastIndexOf("#"), s.lastIndexOf("/"));
  return cut >= 0 && cut < s.length - 1 ? s.slice(cut + 1) : s;
};

const literalText = (term) => term.value + (term.lang ? `@${term.lang}` : "");

/**
 * A schema as graph elements: each class a node of kind "class" with its
 * instance count, the relations to literals its properties, the relations to
 * other classes edges. The most frequent classes only, and what links them.
 */
function graphOfSchema({ classes, relations }) {
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
    const st = Term.parse(s);
    const ot = Term.parse(o);
    const pIri = p ? Term.parse(p).value : null;
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
      const t = Term.parse(s);
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

/** Table rows as plain display strings. */
function tableOf(env) {
  if (env.kind === "construct") {
    return { vars: ["subject", "predicate", "object"], rows: env.triples.slice(0, MAX_ROWS).map((t) => Object.fromEntries(["subject", "predicate", "object"].map((k, i) => [k, display(t[i])]))) };
  }
  return { vars: env.vars, rows: env.rows.slice(0, MAX_ROWS).map((r) => Object.fromEntries(env.vars.map((v) => [v, r[v] === undefined ? "" : display(r[v])]))) };
}

const display = (token) => {
  const t = Term.parse(token);
  return t.kind === "literal" ? literalText(t) : t.value;
};

/** Elements and table rows of a finished query's envelope. */
function elementsOf(env) {
  if (env.kind === "construct") return { graph: graphOf(env.triples), table: tableOf(env) };
  if (env.kind === "select") return { graph: graphOfRows(env.vars, env.rows), table: tableOf(env) };
  throw new Error("an ASK answer is one boolean — there is no graph to show; use SELECT or CONSTRUCT");
}

const iriToken = (iri) => {
  const bare = String(iri).trim().replace(/^<|>$/g, "");
  if (!bare || /[\s<>"{}|\\^`]/.test(bare)) throw new Error(`not a usable IRI: ${iri}`);
  return `<${bare}>`;
};

// --- the tools ---------------------------------------------------------------

/**
 * Register the graph view: its UI resource, the three tools Claude uses, and
 * the app-only tools the view uses.
 */
export function registerGraphViews({ server, store, jobs, here, guard, json, UsageError, dataset, capabilities, schemaOf }) {
  const views = new Map();
  const uploads = new Map(); // view id -> the file it is receiving: { name, size, bytes, received, timer }
  let seq = 0;
  let html = null;

  /** Forget a view's unfinished upload, and the memory it holds. */
  const dropUpload = (id) => {
    clearTimeout(uploads.get(id)?.timer);
    uploads.delete(id);
  };
  /** (Re)arm an upload's idle timer: one the view stopped sending is dropped. */
  const touchUpload = (id, up) => {
    clearTimeout(up.timer);
    up.timer = setTimeout(() => uploads.get(id) === up && uploads.delete(id), UPLOAD_IDLE_MS);
    up.timer.unref?.();
  };

  /** A view's dataset — none yet while it waits for the user to open a file. */
  const datasetOf = (view) => {
    if (view.dataset) return view.dataset;
    throw new UsageError(`graph view ${view.id} is still waiting for the user to open ${view.awaiting?.file ?? "a file"} in it`);
  };

  /** Run a query on a worker for a view; settles with the job, done or still running after `waitMs`. */
  async function run(name, query, { variable, keys, waitMs = WAIT_MS } = {}) {
    const source = await store.resolve(name);
    const job = jobs.submit({
      dataset: source.key,
      key: `${source.cacheKey}#${await store.stamp(source)}`,
      target: source.openTarget,
      query,
      reason: false,
      internal: true,
      ...(keys ? { var: variable, keys } : {}),
    });
    let timer;
    await Promise.race([job.settled, new Promise((r) => (timer = setTimeout(r, waitMs)))]);
    clearTimeout(timer);
    if (job.state === "failed") throw new UsageError(job.error);
    return job;
  }

  function viewOf(id) {
    const view = views.get(String(id ?? "").trim());
    if (!view) {
      throw new UsageError(
        `no graph view ${id} — views last as long as the extension runs (the newest ${MAX_VIEWS} are kept); open a new one with graph_view`,
      );
    }
    return view;
  }

  function openView(datasetKey, title, options) {
    const id = `v${++seq}`;
    const view = new View(id, datasetKey, title, options);
    views.set(id, view);
    while (views.size > MAX_VIEWS) {
      const oldest = [...views.values()].sort((a, b) => a.touchedAt - b.touchedAt)[0];
      views.delete(oldest.id);
      dropUpload(oldest.id);
    }
    return view;
  }

  /** Put a finished query's results into a view: added, or replacing what it holds. */
  function load(view, env, mode) {
    const { graph, table } = elementsOf(env);
    if (mode === "replace") {
      view.clear();
      const added = view.add(graph);
      view.table = table;
      view.patch({ op: "replace", nodes: added.nodes, edges: added.edges, table });
      return { ...added, rows: table.rows.length, truncated: graph.nodes.length > MAX_NODES || graph.edges.length > MAX_EDGES };
    }
    const added = view.add(graph);
    if (table) {
      view.table = view.table && view.table.vars.join() === table.vars.join() ? { vars: table.vars, rows: [...view.table.rows, ...table.rows].slice(0, MAX_ROWS) } : table;
    }
    view.patch({ op: "add", nodes: added.nodes, edges: added.edges, table: view.table });
    return { ...added, rows: table.rows.length, truncated: view.nodes.size >= MAX_NODES || view.edges.size >= MAX_EDGES };
  }

  /** Run `query` into `view`; a query still running at the deadline lands when it finishes. */
  async function query(view, text, mode) {
    const job = await run(datasetOf(view), text);
    if (job.state !== "done") {
      view.status = "running a query…";
      view.patch({ op: "status", status: view.status });
      job.settled.then(() => {
        if (job.state === "done") {
          view.status = "";
          load(view, job.env, mode);
        } else {
          view.status = `the query ${job.state}${job.error ? `: ${job.error}` : ""}`;
          view.patch({ op: "status", status: view.status });
        }
      });
      return { pending: true };
    }
    return load(view, job.env, mode);
  }

  /** A node's statements, both directions, into the view. */
  async function expand(view, iri) {
    const node = iriToken(iri);
    const [out, inc] = await Promise.all([
      run(datasetOf(view), `CONSTRUCT { ${node} ?p ?o } WHERE { ${node} ?p ?o } LIMIT ${NEIGHBOURS}`),
      run(datasetOf(view), `CONSTRUCT { ?s ?p ${node} } WHERE { ?s ?p ${node} } LIMIT ${NEIGHBOURS}`),
    ]);
    const triples = [out, inc].flatMap((j) => (j.state === "done" ? j.env.triples : []));
    const added = view.add(graphOf(triples));
    view.patch({ op: "add", nodes: added.nodes, edges: added.edges, focus: node });
    return added;
  }

  /** Labels and types of nodes that lack them, read with one split query on the subjects. */
  async function label(view, ids) {
    const want = ids.filter((id) => {
      const n = view.nodes.get(id);
      return n && n.kind === "iri" && !n.label;
    });
    if (!want.length) return { labelled: 0 };
    const preds = [...LABELS, RDF_TYPE].map((p) => `<${p}>`).join(", ");
    const job = await run(datasetOf(view), `SELECT ?s ?p ?o WHERE { ?s ?p ?o FILTER(?p IN (${preds})) }`, {
      variable: "s",
      keys: want.slice(0, LABEL_BATCH),
    });
    if (job.state !== "done") return { labelled: 0, pending: true };
    const found = graphOf(job.env.rows.map((r) => [r.s, r.p, r.o]));
    const labels = {};
    for (const n of found.nodes) {
      const had = view.nodes.get(n.id);
      if (!had) continue;
      if (!had.label && n.label) had.label = n.label;
      for (const t of n.types) if (!had.types.includes(t)) had.types.push(t);
      labels[n.id] = { label: had.label, types: had.types };
    }
    if (Object.keys(labels).length) view.patch({ op: "labels", labels });
    return { labelled: Object.keys(labels).length };
  }

  /**
   * A view for a chat attachment the extension cannot read: it asks the user
   * to choose or drop the file, which graph_view_upload then brings here.
   */
  function awaitFile(error, { title, options, query: text, iri, schema }) {
    const view = openView(null, title ?? error.file, options);
    view.awaiting = { file: error.file, key: error.key };
    view.then = { query: text, iri, schema };
    return {
      content: [
        {
          type: "text",
          text:
            `Graph view ${view.id} is waiting for ${error.file}: Claude's copy of it is in Claude's own ` +
            "sandbox, out of this extension's reach, so the view asks the user to choose or drop the " +
            "file. Tell them to. Once they do, it opens there" +
            (text || iri || schema ? " with what you asked for" : "") +
            `, it is dataset "${error.key}" for every tool (the path you were given works too), and the ` +
            `view says so in the conversation. graph_view_state({view_id: "${view.id}"}) shows whether it is open yet.`,
        },
      ],
      structuredContent: summary(view),
    };
  }

  /** A file the user opened in a view, whole: it becomes a chat graph, and the view shows it. */
  async function opened(view, name, bytes) {
    const wasAwaiting = view.awaiting;
    // An awaited attachment keeps its own name, so the sandbox path Claude has resolves to it.
    const entry = await store.loadFile(wasAwaiting?.file ?? name, bytes);
    const { graph } = await store.graph(entry.source.key);
    const then = view.then ?? {};
    view.dataset = entry.source.key;
    view.awaiting = null;
    view.then = null;
    if (!wasAwaiting) view.title = name;
    view.clear();
    view.status = "";
    view.patch({ op: "dataset", dataset: view.dataset, title: view.title });
    try {
      if (then.schema) await schemaInto(view);
      else if (then.query) await query(view, then.query, "replace");
      else if (then.iri) await expand(view, then.iri);
      else await query(view, SAMPLE, "replace");
    } catch (error) {
      // The file is open all the same; the view says what went wrong.
      view.status = String(error?.message ?? error).split("\n")[0];
      view.patch({ op: "status", status: view.status });
    }
    return { dataset: view.dataset, statements: graph.info().quads, file: name, awaited: Boolean(wasAwaiting) };
  }

  /** A dataset's schema into a view, as boxes of classes and their properties. */
  async function schemaInto(view) {
    const schema = await schemaOf(datasetOf(view));
    const { nodes, edges, table, classes } = graphOfSchema(schema);
    view.clear();
    const added = view.add({ nodes, edges });
    view.table = table;
    view.patch({ op: "replace", nodes: added.nodes, edges: added.edges, table });
    return { classes, shown: nodes.length, relations: edges.length, from: schema.from };
  }

  const summary = (view) => ({
    view_id: view.id,
    dataset: view.dataset,
    nodes: view.nodes.size,
    edges: view.edges.size,
    table_rows: view.table?.rows.length ?? 0,
    ...(view.aggregates.size
      ? { aggregates: [...view.aggregates.values()].map(({ id, label, members }) => ({ id, label, members: members.length })) }
      : {}),
    ...(view.awaiting ? { awaiting: view.awaiting.file } : {}),
  });

  /** What the user has done in the view, as it last reported. */
  const userState = (view) =>
    view.ui
      ? {
          open: view.seenAt !== null && Date.now() - view.seenAt < 15_000,
          last_seen_s: view.seenAt === null ? null : Math.round((Date.now() - view.seenAt) / 1000),
          ...view.ui,
        }
      : { open: false, note: "the view has not reported yet (it may not be rendered in this client)" };

  // --- the UI resource ---------------------------------------------------------

  registerAppResource(
    server,
    "Graph view",
    VIEW_URI,
    { description: "Interactive graph and table view of query results, with labels, layers, layouts, zoom and search." },
    async () => {
      html ??= await readFile(join(here, "graph-view.html"), "utf8");
      return { contents: [{ uri: VIEW_URI, mimeType: RESOURCE_MIME_TYPE, text: html, _meta: { ui: { prefersBorder: true } } }] };
    },
  );

  // --- tools Claude calls --------------------------------------------------------

  registerAppTool(
    server,
    "graph_view",
    {
      title: "Show a graph view",
      description:
        "Open an interactive graph view in the conversation: the results of a SPARQL query " +
        "(CONSTRUCT, or a SELECT — s/p/o columns are read as triples, otherwise the first column " +
        "is the subject and the others its links and properties), or the neighbourhood of one " +
        "entity (iri). The user can toggle node and edge labels, group nodes into coloured layers " +
        "(by rdf:type, namespace or a predicate) and hide layers, change the layout, zoom, search, " +
        "expand a node by double-clicking it, and switch to a table. Keep queries to a few hundred " +
        "nodes (a LIMIT). Answers with a view_id: change the open view with graph_view_update and " +
        "read what the user selected with graph_view_state. Also the way to open a file attached " +
        "to the chat (a path like /mnt/user-data/uploads/…, which this extension cannot read): pass " +
        "that path as dataset, and the view asks the user to choose or drop the file. " +
        "options.diagram draws it as dots (graph), as boxes listing each entity's properties " +
        "(boxes), or as a class diagram (classes); schema=true draws the dataset's own schema — " +
        "its classes as boxes with their properties, linked by their relations — with no query.",
      inputSchema: {
        dataset,
        query: z.string().optional().describe("SPARQL CONSTRUCT or SELECT whose results to show"),
        iri: z.string().optional().describe("or: an entity whose statements (both directions) to show"),
        schema: z
          .boolean()
          .optional()
          .describe("or: the dataset's schema — classes as boxes listing their properties, linked by their relations"),
        title: z.string().optional().describe("a short title shown above the graph"),
        options: optionsSchema,
      },
      annotations: { readOnlyHint: true, openWorldHint: true },
      _meta: { ui: { resourceUri: VIEW_URI } },
    },
    guard(async ({ dataset: name, query: text, iri, schema, title, options = {} }) => {
      let source;
      try {
        source = await store.resolve(name);
      } catch (error) {
        if (error instanceof AttachmentError) {
          const look = lookOf(schema ? { diagram: "boxes", group_by: "namespace", edge_labels: true, ...options } : options);
          return awaitFile(error, { title, options: look, query: text, iri, schema });
        }
        throw error;
      }
      // A schema reads best as boxes of classes, coloured by vocabulary, its relations named.
      const look = lookOf(schema ? { diagram: "boxes", group_by: "namespace", edge_labels: true, ...options } : options);
      const view = openView(
        source.key,
        title ?? (schema ? `schema of ${source.key}` : iri ? localName(String(iri).replace(/[<>]/g, "")) : "graph"),
        look,
      );
      let note = "";
      if (schema) {
        const r = await schemaInto(view);
        note =
          ` The schema (from its ${r.from === "card" ? "card" : "schema pyramid"}): ` +
          (r.classes > r.shown ? `the ${r.shown} most frequent of ${r.classes} classes` : `${r.shown} classes`) +
          `, ${r.relations} relations between them; each box lists the class's literal properties.`;
      } else if (text) {
        const r = await query(view, text, "replace");
        note = r.pending ? " The query is still running; the view fills in when it finishes." : r.truncated ? ` Capped at ${MAX_NODES} nodes / ${MAX_EDGES} edges: narrow the query.` : "";
      } else if (iri) {
        await expand(view, iri);
      }
      const s = summary(view);
      return {
        content: [
          {
            type: "text",
            text:
              `Graph view ${view.id} on ${view.dataset}: ${s.nodes} nodes, ${s.edges} edges.${note} ` +
              "It is shown to the user, who can change labels, layers, layout and zoom, search, expand " +
              `nodes and switch to the table. Change it with graph_view_update({view_id: "${view.id}", …}); ` +
              `see what the user selected with graph_view_state({view_id: "${view.id}"}).`,
          },
        ],
        structuredContent: s,
      };
    }),
  );

  server.registerTool(
    "graph_view_update",
    {
      title: "Change a graph view",
      description:
        "Change a graph view the user has open (from graph_view): add the results of another " +
        "query to it (mode add, default) or replace them (mode replace), expand an entity's " +
        "neighbourhood, highlight or focus nodes, remove nodes, aggregate several into one node " +
        "(or every node of a type) and ungroup them, or change its look — diagram " +
        "(graph / boxes / classes), labels, edge_labels, properties, node_shape, edge_style " +
        "(curved / straight / square), group_by (type / namespace / none / a predicate IRI), " +
        "group_boxes, layout, view (graph / table), literals_as_nodes, theme (auto / light / dark). " +
        "The open view picks the change up within a second or two. Answers with the new counts " +
        "and what the user has selected.",
      inputSchema: {
        view_id: z.string().describe("the view, from graph_view"),
        query: z.string().optional().describe("SPARQL CONSTRUCT or SELECT whose results to add"),
        mode: z.enum(["add", "replace"]).optional().describe("for query: add to the view (default) or replace it"),
        expand: z.string().optional().describe("an entity IRI whose statements to add"),
        highlight: z.array(z.string()).optional().describe("entity IRIs to highlight (an empty list clears)"),
        focus: z.string().optional().describe("an entity IRI to centre on"),
        remove: z.array(z.string()).optional().describe("entity IRIs to take out of the view"),
        aggregate: z
          .object({
            iris: z.array(z.string()).optional().describe("the entities to draw as one node"),
            type: z.string().optional().describe("or: every node of this rdf:type (an IRI)"),
            label: z.string().optional().describe("the aggregate's name (default: the type, or a count)"),
          })
          .optional()
          .describe("draw several nodes as one aggregate node, their links merged and counted"),
        ungroup: z.string().optional().describe('an aggregate id (from graph_view_state) to split back into its nodes, or "all"'),
        title: z.string().optional(),
        options: optionsSchema,
      },
      annotations: { readOnlyHint: false, destructiveHint: false, openWorldHint: true },
    },
    guard(async ({ view_id, query: text, mode = "add", expand: iri, highlight, focus, remove, aggregate, ungroup, title, options }) => {
      const view = viewOf(view_id);
      const done = [];
      if (options || title) {
        Object.assign(view.options, lookOf(options ?? {}, view.options));
        if (title) view.title = title;
        view.patch({ op: "options", options: view.options, title: view.title });
        done.push("look changed");
      }
      if (text) {
        const r = await query(view, text, mode);
        done.push(r.pending ? "query still running (it lands when done)" : `${mode === "replace" ? "replaced with" : "added"} ${r.nodes.length} nodes and ${r.edges.length} edges`);
      }
      if (iri) {
        const r = await expand(view, iri);
        done.push(`expanded ${iri}: +${r.nodes.length} nodes, +${r.edges.length} edges`);
      }
      if (remove?.length) done.push(`removed ${view.remove(remove.map(iriToken))} nodes`);
      if (ungroup) done.push(`ungrouped ${view.ungroup(ungroup)} aggregate(s)`);
      if (aggregate) {
        const ids = aggregate.type
          ? [...view.nodes.values()].filter((n) => n.types.includes(String(aggregate.type).replace(/^<|>$/g, ""))).map((n) => n.id)
          : (aggregate.iris ?? []).map(iriToken);
        const label = aggregate.label ?? (aggregate.type ? localName(String(aggregate.type).replace(/^<|>$/g, "")) : undefined);
        try {
          const a = view.aggregate(ids, label);
          done.push(`aggregated ${a.members.length} nodes as ${a.id} "${a.label}"`);
        } catch (error) {
          throw new UsageError(`${error.message} (${ids.length} matched)`);
        }
      }
      if (highlight) {
        view.patch({ op: "highlight", ids: highlight.map(iriToken) });
        done.push(highlight.length ? `highlighted ${highlight.length}` : "highlight cleared");
      }
      if (focus) {
        view.patch({ op: "focus", id: iriToken(focus) });
        done.push("focused");
      }
      return json({ ...summary(view), done, user: userState(view) });
    }),
  );

  server.registerTool(
    "graph_view_state",
    {
      title: "What the user did in a graph view",
      description:
        "What the user has selected and set in a graph view: the selected nodes (IRI and label), " +
        "the search text, hidden layers, the look (labels, grouping, layout, tab), the aggregates " +
        "and whether the view is still open. Use it when the user refers to \"this\", \"these\" " +
        "or \"the selected\" nodes.",
      inputSchema: { view_id: z.string().describe("the view, from graph_view") },
      annotations: { readOnlyHint: true },
    },
    guard(async ({ view_id }) => {
      const view = viewOf(view_id);
      return json({ ...summary(view), title: view.title, user: userState(view) });
    }),
  );

  // --- app-only tools: what the view itself calls ----------------------------------

  const appOnly = { ui: { resourceUri: VIEW_URI, visibility: ["app"] } };
  const since = z.number().int().min(0).describe("the version the view has");

  registerAppTool(
    server,
    "graph_view_sync",
    {
      title: "Graph view: sync",
      description: "The view's changes since `since`, and the user's state into the server.",
      inputSchema: { view_id: z.string(), since, state: z.record(z.string(), z.any()).optional() },
      _meta: appOnly,
    },
    guard(async ({ view_id, since: v, state }) => {
      const view = viewOf(view_id);
      view.seenAt = Date.now();
      if (state) view.ui = state;
      return json(view.since(v));
    }),
  );

  registerAppTool(
    server,
    "graph_view_expand",
    {
      title: "Graph view: expand a node",
      description: "Add a node's statements, both directions, to the view.",
      inputSchema: { view_id: z.string(), iri: z.string(), since },
      _meta: appOnly,
    },
    guard(async ({ view_id, iri, since: v }) => {
      const view = viewOf(view_id);
      await expand(view, iri);
      return json(view.since(v));
    }),
  );

  registerAppTool(
    server,
    "graph_view_search",
    {
      title: "Graph view: find entities",
      description: "Find entities by name in the view's graph and add them to the view.",
      inputSchema: { view_id: z.string(), text: z.string(), since },
      _meta: appOnly,
    },
    guard(async ({ view_id, text, since: v }) => {
      const view = viewOf(view_id);
      // The header says which index there is to search, before a giant graph is opened for nothing.
      const caps = await capabilities(await store.resolve(datasetOf(view)));
      if (caps && !caps.name_search && !caps.full_text_search) {
        return json({ ...view.since(v), found: 0, note: `${view.dataset} has no name index to search — ask Claude to find it with a query` });
      }
      const { graph } = await store.graph(view.dataset);
      const nodes = [];
      const hits = !caps || caps.name_search ? graph.prefixSearch(text, 20) : graph.textSearch(text, { limit: 20 }).map((subject) => ({ subject }));
      for (const { label: l, subject } of hits) {
        const iri = String(subject).replace(/^<|>$/g, "");
        nodes.push({ id: `<${iri}>`, iri, kind: "iri", label: l ?? null, types: [], props: [] });
      }
      const added = view.add({ nodes, edges: [] });
      view.patch({ op: "add", nodes: added.nodes, edges: [], highlight: nodes.map((n) => n.id) });
      return json({ ...view.since(v), found: nodes.length });
    }),
  );

  registerAppTool(
    server,
    "graph_view_labels",
    {
      title: "Graph view: labels and types",
      description: "Read the labels and rdf:types of nodes that lack them.",
      inputSchema: { view_id: z.string(), ids: z.array(z.string()), since },
      _meta: appOnly,
    },
    guard(async ({ view_id, ids, since: v }) => {
      const view = viewOf(view_id);
      await label(view, ids);
      return json(view.since(v));
    }),
  );

  registerAppTool(
    server,
    "graph_view_edit",
    {
      title: "Graph view: edit",
      description: "The user's edits from the view's menu: take nodes out of it, aggregate nodes into one, ungroup.",
      inputSchema: {
        view_id: z.string(),
        remove: z.array(z.string()).optional().describe("node ids to take out"),
        aggregate: z.object({ ids: z.array(z.string()), label: z.string().optional() }).optional(),
        ungroup: z.string().optional().describe('an aggregate id, or "all"'),
        since,
      },
      _meta: appOnly,
    },
    guard(async ({ view_id, remove, aggregate, ungroup, since: v }) => {
      const view = viewOf(view_id);
      if (remove?.length) view.remove(remove);
      if (ungroup) view.ungroup(ungroup);
      if (aggregate) {
        try {
          view.aggregate(aggregate.ids, aggregate.label);
        } catch (error) {
          throw new UsageError(error.message);
        }
      }
      return json(view.since(v));
    }),
  );

  registerAppTool(
    server,
    "graph_view_upload",
    {
      title: "Graph view: open a file",
      description:
        "A file the user chose or dropped in the view, in base64 parts sent in order: the last one " +
        "opens it as a chat graph and shows it in the view.",
      inputSchema: {
        view_id: z.string(),
        name: z.string().describe("the file's name"),
        size: z.number().int().min(1).describe("the whole file's bytes"),
        offset: z.number().int().min(0).describe("where this part starts"),
        data: z.string().describe("this part, base64"),
        since,
      },
      _meta: appOnly,
    },
    guard(async ({ view_id, name, size, offset, data, since: v }) => {
      const view = viewOf(view_id);
      let up = uploads.get(view.id);
      if (offset === 0) {
        dropUpload(view.id); // a restart replaces the view's unfinished upload
        const max = uploadLimit(view.awaiting?.file ?? name);
        if (size > max) {
          throw new UsageError(
            `${name} is ${mb(size)}, more than the ${mb(max)} a file opened in a view may be. Put it in a ` +
              "Graph folder instead (the extension's settings): there it is read lazily, at any size.",
          );
        }
        const pending = [...uploads.values()].reduce((sum, u) => sum + u.size, 0);
        if (pending + size > UPLOADS_MAX_BYTES) {
          throw new UsageError(
            `${name} would bring the files being opened in views to ${mb(pending + size)}, more than ` +
              `${mb(UPLOADS_MAX_BYTES)} at once — let the others finish, or put it in a Graph folder`,
          );
        }
        up = { name, size, bytes: new Uint8Array(new SharedArrayBuffer(size)), received: 0, timer: null };
        uploads.set(view.id, up);
      }
      if (!up || up.name !== name || up.size !== size || offset !== up.received) {
        throw new UsageError(`the parts of ${name} came out of order (at ${offset}, expected ${up?.received ?? 0}) — open it again`);
      }
      const part = Buffer.from(data, "base64");
      if (offset + part.length > size) {
        dropUpload(view.id);
        throw new UsageError(`${name} came out longer than its ${size} bytes — open it again`);
      }
      up.bytes.set(part, offset);
      up.received += part.length;
      if (up.received < size) {
        touchUpload(view.id, up);
        return json({ received: up.received, size });
      }
      dropUpload(view.id);
      const done = await opened(view, name, up.bytes);
      return json({ ...view.since(v), ...done });
    }),
  );
}
