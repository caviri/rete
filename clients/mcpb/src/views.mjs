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
import { readFile } from "node:fs/promises";
import { join } from "node:path";

import { RESOURCE_MIME_TYPE, registerAppResource, registerAppTool } from "@modelcontextprotocol/ext-apps/server";
import { z } from "zod";
import { Term } from "rete-graph";

const VIEW_URI = "ui://rete/graph-view.html";
const MAX_NODES = 1500; // per view; more is unreadable on a canvas anyway
const MAX_EDGES = 4000;
const MAX_ROWS = 5000; // table rows a view keeps
const MAX_VIEWS = 20;
const MAX_LOG = 300; // patches kept for views that are behind
const WAIT_MS = 35_000; // a view call answers before the client's 60 s limit
const NEIGHBOURS = 150; // statements per direction when a node is expanded
const LABEL_BATCH = 200;

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
    labels: z.boolean().optional().describe("show node labels (default true)"),
    edge_labels: z.boolean().optional().describe("show predicate names on edges (default false)"),
    group_by: z
      .string()
      .optional()
      .describe('colour and layer nodes by "type" (rdf:type, default), "namespace", "none", or a predicate IRI'),
    group_boxes: z.boolean().optional().describe("draw each group as a box around its nodes (default false)"),
    layout: z.enum(["cose", "concentric", "breadthfirst", "circle", "grid"]).optional(),
    view: z.enum(["graph", "table"]).optional().describe("which tab is shown"),
    literals_as_nodes: z.boolean().optional().describe("draw literal values as nodes instead of node properties"),
  })
  .optional();

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
    this.version = 0;
    this.log = []; // [{ version, ...patch }]
    this.ui = null; // what the view last reported: selection, options, …
    this.seenAt = null; // when the view last synced
    this.status = "";
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
export function registerGraphViews({ server, store, jobs, here, guard, json, UsageError, dataset }) {
  const views = new Map();
  let seq = 0;
  let html = null;

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
    const job = await run(view.dataset, text);
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
      run(view.dataset, `CONSTRUCT { ${node} ?p ?o } WHERE { ${node} ?p ?o } LIMIT ${NEIGHBOURS}`),
      run(view.dataset, `CONSTRUCT { ?s ?p ${node} } WHERE { ?s ?p ${node} } LIMIT ${NEIGHBOURS}`),
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
    const job = await run(view.dataset, `SELECT ?s ?p ?o WHERE { ?s ?p ?o FILTER(?p IN (${preds})) }`, {
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

  const summary = (view) => ({
    view_id: view.id,
    dataset: view.dataset,
    nodes: view.nodes.size,
    edges: view.edges.size,
    table_rows: view.table?.rows.length ?? 0,
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
        "read what the user selected with graph_view_state.",
      inputSchema: {
        dataset,
        query: z.string().optional().describe("SPARQL CONSTRUCT or SELECT whose results to show"),
        iri: z.string().optional().describe("or: an entity whose statements (both directions) to show"),
        title: z.string().optional().describe("a short title shown above the graph"),
        options: optionsSchema,
      },
      annotations: { readOnlyHint: true, openWorldHint: true },
      _meta: { ui: { resourceUri: VIEW_URI } },
    },
    guard(async ({ dataset: name, query: text, iri, title, options = {} }) => {
      const source = await store.resolve(name);
      const view = openView(source.key, title ?? (iri ? localName(String(iri).replace(/[<>]/g, "")) : "graph"), options);
      let note = "";
      if (text) {
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
        "neighbourhood, highlight or focus nodes, remove nodes, or change its look — labels, " +
        "edge_labels, group_by (type / namespace / none / a predicate IRI), group_boxes, layout, " +
        "view (graph / table), literals_as_nodes. The open view picks the change up within a " +
        "second or two. Answers with the new counts and what the user has selected.",
      inputSchema: {
        view_id: z.string().describe("the view, from graph_view"),
        query: z.string().optional().describe("SPARQL CONSTRUCT or SELECT whose results to add"),
        mode: z.enum(["add", "replace"]).optional().describe("for query: add to the view (default) or replace it"),
        expand: z.string().optional().describe("an entity IRI whose statements to add"),
        highlight: z.array(z.string()).optional().describe("entity IRIs to highlight (an empty list clears)"),
        focus: z.string().optional().describe("an entity IRI to centre on"),
        remove: z.array(z.string()).optional().describe("entity IRIs to take out of the view"),
        title: z.string().optional(),
        options: optionsSchema,
      },
      annotations: { readOnlyHint: false, destructiveHint: false, openWorldHint: true },
    },
    guard(async ({ view_id, query: text, mode = "add", expand: iri, highlight, focus, remove, title, options }) => {
      const view = viewOf(view_id);
      const done = [];
      if (options || title) {
        Object.assign(view.options, options ?? {});
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
      if (remove?.length) {
        const ids = remove.map(iriToken).filter((id) => view.nodes.has(id));
        for (const id of ids) view.nodes.delete(id);
        for (const [eid, e] of view.edges) if (ids.includes(e.source) || ids.includes(e.target)) view.edges.delete(eid);
        view.patch({ op: "remove", ids });
        done.push(`removed ${ids.length} nodes`);
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
        "the search text, hidden layers, the look (labels, grouping, layout, tab) and whether the " +
        "view is still open. Use it when the user refers to \"this\", \"these\" or \"the selected\" " +
        "nodes.",
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
      const { graph } = await store.graph(view.dataset);
      const nodes = [];
      for (const { label: l, subject } of graph.prefixSearch(text, 20)) {
        const iri = String(subject).replace(/^<|>$/g, "");
        nodes.push({ id: `<${iri}>`, iri, kind: "iri", label: l, types: [], props: [] });
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
}
