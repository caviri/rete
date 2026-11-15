// graph-host.js — the graph view, inside the explorer.
//
// The page in the frame is the very graph view the Claude Desktop extension
// shows in a conversation (clients/mcpb/src/view), copied here by
// clients/mcpb/build-explorer.mjs. It is an MCP App: it holds no data, asks its
// host for its view's elements with tool calls, and applies the numbered
// patches it gets back. In Claude the host forwards those calls to the
// extension's server; here the explorer *is* the host, and answers them itself
// from the archive it has open — the same view model (src/view-model.mjs), the
// same queries, run on the explorer's engine (the wasm worker in a browser,
// rete-core in the desktop app).
//
// So nothing about the view is reimplemented: what works in Claude works here,
// and what the view lacks here is only what needs Claude (sending a selection
// to the chat).
import {
  AppBridge,
  LABEL_BATCH,
  LABELS,
  PostMessageTransport,
  RDF_TYPE,
  SAMPLE,
  View,
  applyLabels,
  graphReaders,
  iriToken,
  loadInto,
  localName,
  lookOf,
  neighbourQueries,
  neighbourTriples,
  schemaInto,
  unlabelled,
} from "../graph/host-lib.js";
import { parseTerm, searchGraph } from "./rete-fs.js";

/** A term token as the view model reads it: { kind, value, lang }. */
function parseToken(token) {
  const t = String(token ?? "");
  if (t.startsWith("<<")) return { kind: "triple", value: t };
  if (t.startsWith("<") && t.endsWith(">")) return { kind: "iri", value: t.slice(1, -1) };
  if (t.startsWith("_:")) return { kind: "bnode", value: t };
  const lit = parseTerm(t);
  return { kind: "literal", value: lit.value, lang: lit.lang || null, datatype: lit.datatype || null };
}

const { graphOf, elementsOf } = graphReaders(parseToken);

// A schema reads best as boxes of classes, coloured by vocabulary, its relations named.
const SCHEMA_LOOK = { diagram: "boxes", group_by: "namespace", edge_labels: true };
const GRAPH_LOOK = { diagram: "graph", group_by: "type", edge_labels: false };
const CLASS_SAMPLE = 40; // instances a class seed draws, with their statements
const PART_MAX = 512 * 1024 * 1024; // a file dropped on the view, opened whole

/**
 * The graph view for one open archive.
 *
 *   const host = new GraphHost({ frame, ctx, dataset, onOpenFile, onStatus });
 *   await host.show({ schema: true });   // or { iri }, { cls }, { predicate }, { graph }, { query, title }, { sample: true }
 *
 * `frame` is an empty <iframe>; `ctx` the rete-fs context of the open archive.
 */
export class GraphHost {
  constructor({ frame, ctx, dataset, onOpenFile, onStatus, theme = "dark" }) {
    this.frame = frame;
    this.ctx = ctx;
    this.dataset = dataset;
    this.onOpenFile = onOpenFile;
    this.onStatus = onStatus || (() => {});
    this.theme = theme;
    this.view = null;
    this.seq = 0;
    this.upload = null;
    this.ready = null; // resolves once the page has connected and holds a view
  }

  /** Load the page and connect to it. Resolves when the page has its first view. */
  mount() {
    if (this.ready) return this.ready;
    this.ready = (async () => {
      const html = await (await fetch(new URL("../graph/graph-view.html", import.meta.url))).text();
      const bridge = new AppBridge(
        null,
        { name: "rete file explorer", version: "1" },
        // No `message` and no `updateModelContext`: there is no conversation
        // here, and the view leaves out what would need one.
        { openLinks: {}, serverTools: {}, downloadFile: {} },
        { hostContext: { theme: this.theme, displayMode: "inline", availableDisplayModes: ["inline"], platform: "desktop" } },
      );
      bridge.oncalltool = (params) => this.#call(params);
      bridge.onopenlink = async ({ url }) => {
        if (/^https?:/i.test(url)) window.open(url, "_blank", "noopener");
        return {};
      };
      bridge.ondownloadfile = async ({ contents }) => {
        for (const c of contents ?? []) {
          const r = c.resource ?? {};
          if (typeof r.text !== "string") continue;
          const name = String(r.uri ?? "graph").split("/").pop() || "graph";
          saveFile(name, r.mimeType || "text/plain", r.text);
        }
        return {};
      };
      bridge.onrequestdisplaymode = async () => ({ mode: "inline" });
      const initialized = new Promise((resolve) => (bridge.oninitialized = resolve));
      await bridge.connect(new PostMessageTransport(this.frame.contentWindow, this.frame.contentWindow));
      this.frame.srcdoc = html;
      await initialized;
      this.bridge = bridge;
      // The page learns its view the way it does in Claude: from a tool's input and result.
      this.view = this.#newView("graph", GRAPH_LOOK);
      bridge.sendToolInput({ arguments: { dataset: this.dataset, title: this.view.title, options: this.view.options } });
      bridge.sendToolResult({
        content: [{ type: "text", text: `Graph view ${this.view.id} on ${this.dataset}` }],
        structuredContent: { view_id: this.view.id, dataset: this.dataset, nodes: 0, edges: 0 },
      });
    })();
    return this.ready;
  }

  /** Tear the page down; a new archive gets a new host. */
  destroy() {
    this.bridge?.close?.().catch?.(() => {});
    this.frame.srcdoc = "";
    this.view = null;
    this.upload = null;
  }

  /**
   * Put something in the view. A schema, a sample, a class, a predicate, a
   * graph or a query replace what it shows; an entity is added to it, so
   * exploring one thing after another builds up a picture.
   */
  async show(seed) {
    await this.mount();
    const view = this.view;
    const run = async (sparql) => JSON.parse(await this.ctx.engine.query(sparql, "table"));
    const look = (options, title) => {
      const next = { ...lookOf(options, view.options) };
      const changed = Object.entries(next).some(([k, v]) => view.options[k] !== v) || title !== view.title;
      Object.assign(view.options, next);
      view.title = title;
      if (changed) view.patch({ op: "options", options: view.options, title });
    };
    this.onStatus(`graph: ${describeSeed(seed)}…`);
    if (seed.schema) {
      const schema = bareSchema(this.ctx.meta.schema);
      if (!schema.classes.length) throw new Error("this file carries no schema to draw — try a sample instead");
      look(SCHEMA_LOOK, `schema of ${this.dataset}`);
      const r = schemaInto(view, schema);
      return `${r.shown} of ${r.classes} classes, ${r.relations} relations`;
    }
    if (seed.iri) {
      // An entity starts a picture when there is none, or when the view shows
      // the schema (an entity is not one more box among the classes).
      if (!view.nodes.size || view.title.startsWith("schema of")) {
        if (view.nodes.size) {
          view.clear();
          view.patch({ op: "replace", nodes: [], edges: [], table: null });
        }
        look(GRAPH_LOOK, localName(seed.iri));
      }
      const added = await this.#expand(view, seed.iri);
      return `${added.nodes.length} nodes and ${added.edges.length} links added`;
    }
    const query = seed.query
      ?? (seed.cls && `SELECT ?s ?p ?o WHERE { { SELECT ?s WHERE { ?s <${RDF_TYPE}> ${iriToken(seed.cls)} } LIMIT ${CLASS_SAMPLE} } ?s ?p ?o } LIMIT 1200`)
      ?? (seed.predicate && `SELECT ?s ?p ?o WHERE { ?s ${iriToken(seed.predicate)} ?o BIND(${iriToken(seed.predicate)} AS ?p) } LIMIT 300`)
      ?? (seed.graph && `SELECT ?s ?p ?o WHERE { GRAPH ${iriToken(seed.graph)} { ?s ?p ?o } } LIMIT 400`)
      ?? SAMPLE;
    look(seed.look ?? GRAPH_LOOK, seed.title ?? describeSeed(seed));
    const r = loadInto(view, elementsOf, await run(query), "replace");
    return `${r.nodes.length} nodes, ${r.edges.length} links${r.truncated ? " (capped)" : ""}`;
  }

  #newView(title, options) {
    return new View(`v${++this.seq}`, this.dataset, title, { ...options });
  }

  /** A node's statements, both directions, into the view. */
  async #expand(view, iri) {
    const node = iriToken(iri);
    const envs = [];
    for (const { query } of neighbourQueries(node)) envs.push(JSON.parse(await this.ctx.engine.query(query, "table")));
    const added = view.add(graphOf(neighbourTriples(node, envs)));
    view.patch({ op: "add", nodes: added.nodes, edges: added.edges, focus: node });
    return added;
  }

  /** The page's tool calls: the app-only tools the extension's server registers, answered here. */
  async #call({ name, arguments: args = {} }) {
    try {
      const view = this.view;
      if (!view || args.view_id !== view.id) throw new Error(`no graph view ${args.view_id} — this one belongs to an archive that was closed`);
      return ok(await this.#tool(name, view, args));
    } catch (error) {
      return { isError: true, content: [{ type: "text", text: String(error?.message ?? error) }] };
    }
  }

  async #tool(name, view, args) {
    const v = args.since;
    switch (name) {
      case "graph_view_sync":
        view.seenAt = Date.now();
        if (args.state) view.ui = args.state;
        return view.since(v);
      case "graph_view_expand":
        await this.#expand(view, args.iri);
        return view.since(v);
      case "graph_view_labels": {
        const want = unlabelled(view, args.ids ?? []).slice(0, LABEL_BATCH);
        if (want.length) {
          const preds = [...LABELS, RDF_TYPE].map((p) => `<${p}>`).join(", ");
          const env = JSON.parse(
            await this.ctx.engine.query(`SELECT ?s ?p ?o WHERE { VALUES ?s { ${want.join(" ")} } ?s ?p ?o FILTER(?p IN (${preds})) }`, "table"),
          );
          applyLabels(view, graphOf((env.rows ?? []).map((r) => [r.s, r.p, r.o])));
        }
        return view.since(v);
      }
      case "graph_view_search": {
        const { hits, labels } = await searchGraph(this.ctx, String(args.text ?? ""), 20);
        if (!hits.length) return { ...view.since(v), found: 0, note: "nothing found — a file with a label or text index finds names" };
        const named = new Map((labels ?? []).map((h) => [bare(h.subject), h.label]));
        const nodes = hits.slice(0, 20).map((h) => {
          const iri = bare(h);
          return { id: `<${iri}>`, iri, kind: "iri", label: named.get(iri) ?? null, types: [], props: [] };
        });
        const added = view.add({ nodes, edges: [] });
        view.patch({ op: "add", nodes: added.nodes, edges: [], highlight: nodes.map((n) => n.id) });
        return { ...view.since(v), found: nodes.length };
      }
      case "graph_view_edit":
        if (args.remove?.length) view.remove(args.remove);
        if (args.ungroup) view.ungroup(args.ungroup);
        if (args.aggregate) view.aggregate(args.aggregate.ids, args.aggregate.label);
        return view.since(v);
      case "graph_view_upload":
        return this.#receive(view, args);
      default:
        throw new Error(`${name} is not available in the file explorer`);
    }
  }

  /** A file chosen or dropped in the view, in parts: the explorer opens it, as if dropped on its window. */
  async #receive(view, { name, size, offset, data, since }) {
    if (offset === 0) {
      if (!/\.rete$/i.test(name)) throw new Error(`${name} is not a .rete file`);
      if (size > PART_MAX) throw new Error(`${name} is too big to send through the view — open it from the explorer's home`);
      this.upload = { name, size, bytes: new Uint8Array(size), received: 0 };
    }
    const up = this.upload;
    if (!up || up.name !== name || up.size !== size || offset !== up.received) throw new Error(`the parts of ${name} came out of order — open it again`);
    const part = Uint8Array.from(atob(data), (c) => c.charCodeAt(0));
    if (offset + part.length > size) throw new Error(`${name} came out longer than its ${size} bytes`);
    up.bytes.set(part, offset);
    up.received += part.length;
    if (up.received < size) return { received: up.received, size };
    this.upload = null;
    const opened = await this.onOpenFile?.(new File([up.bytes], name));
    return { ...view.since(since), dataset: name, statements: opened?.statements ?? "?", file: name };
  }
}

const ok = (value) => ({ content: [{ type: "text", text: JSON.stringify(value) }] });
const bare = (s) => String(s ?? "").replace(/^<|>$/g, "");

/** The engine's schema with bare IRIs, as the view model takes it (it writes "<iri>" tokens). */
const bareSchema = (schema) => ({
  classes: (schema?.classes ?? []).map(([c, n]) => [bare(c), n]),
  relations: (schema?.relations ?? []).map(([s, p, o, n]) => [bare(s), bare(p), bare(o), n]),
});

function describeSeed(seed) {
  if (seed.schema) return "the schema";
  if (seed.iri) return localName(seed.iri);
  if (seed.cls) return `${localName(seed.cls)} — a sample of ${CLASS_SAMPLE}`;
  if (seed.predicate) return `${localName(seed.predicate)} — a sample`;
  if (seed.graph) return `graph ${localName(seed.graph)}`;
  if (seed.query) return seed.title || "query results";
  return "a sample of the file";
}

function saveFile(filename, mime, body) {
  const a = document.createElement("a");
  a.href = URL.createObjectURL(new Blob([body], { type: mime }));
  a.download = filename;
  a.click();
  setTimeout(() => URL.revokeObjectURL(a.href), 5000);
}
