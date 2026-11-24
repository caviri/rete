// Graph view: query result rows -> GraphKit nodes/edges (lib/graphkit/graphkit.js, window.GraphKit).
// Every value comes from an untrusted .rete file: GraphKit renders it through textContent only, links are
// opened only when http(s) (onOpen), and nothing here touches innerHTML.
//
// Term -> node:  IRI {id: value, kind: "iri"} | blank node {id: "_:" + value, kind: "bnode"} |
//                literal {id: "lit:" + value + "^^" + datatype + "@" + lang, kind: "literal", label, value, datatype, lang}
// Edge:          {id: s + "|" + p + "|" + o, source, target, predicate: p, label: localName(p)}
(function () {
  const { h, num, safeUrl } = window.RetelensFmt;
  const RDF_TYPE = "http://www.w3.org/1999/02/22-rdf-syntax-ns#type";
  const LABEL_PREDS = new Set([
    "http://www.w3.org/2000/01/rdf-schema#label", "http://xmlns.com/foaf/0.1/name", "http://www.w3.org/2004/02/skos/core#prefLabel",
    "http://schema.org/name", "https://schema.org/name",
  ]);
  const IRI_OK = /^[^\s<>"{}|\\^`]+$/;
  const SHORT_LIT = 160;
  const MAX_NODES = 1500, MAX_EDGES = 3000;
  const EXPAND_LIMIT = 50;
  const NAMEISH = /(^|[^a-z])(name|label|title|prefLabel|fullname|heading)([^a-z]|$)|(name|label|title)$/i;
  const IMG = /\.(jpe?g|png|gif|webp|svg)(\?.*)?$/i;

  const localName = (iri) => { try { return decodeURIComponent(iri.split(/[#/]/).filter(Boolean).pop() || iri).replace(/_/g, " "); } catch (_) { return iri.split(/[#/]/).filter(Boolean).pop() || iri; } };
  const litId = (t) => "lit:" + t.value + "^^" + (t.datatype || "") + "@" + (t.lang || "");

  function nodeOf(t) {
    if (!t) return null;
    if (t.kind === "iri") return { id: t.value, kind: "iri" };
    if (t.kind === "bnode") return { id: "_:" + t.value, kind: "bnode" };
    if (t.kind === "literal") return { id: litId(t), kind: "literal", label: t.value, value: t.value, datatype: t.datatype || "", lang: t.lang || "" };
    return null; // quoted triples are not drawn
  }

  class Builder {
    constructor(short) { this.short = short; this.nodes = new Map(); this.edges = new Map(); this.capped = false; }
    node(t) {
      const n = nodeOf(t);
      if (!n) return null;
      const have = this.nodes.get(n.id);
      if (have) return have;
      if (this.nodes.size >= MAX_NODES) { this.capped = true; return null; }
      if (n.kind !== "literal") { n.types = []; n.props = {}; }
      this.nodes.set(n.id, n);
      return n;
    }
    edge(a, p, b) {
      if (!a || !b) return;
      const id = a.id + "|" + p + "|" + b.id;
      if (this.edges.has(id)) return;
      if (this.edges.size >= MAX_EDGES) { this.capped = true; return; }
      this.edges.set(id, { id, source: a.id, target: b.id, predicate: p, label: localName(p) });
    }
    prop(n, key, value) {
      const cur = n.props[key];
      if (cur == null) n.props[key] = value;
      else if (Array.isArray(cur)) { if (cur.length < 8 && !cur.includes(value)) cur.push(value); }
      else if (cur !== value) n.props[key] = [cur, value];
    }
    // CONSTRUCT-style triple: rdf:type -> types, label predicates -> label, other short literals -> props,
    // everything else an edge.
    triple(s, p, o, asEdges) {
      if (!s || !p || !o || p.kind !== "iri" || s.kind === "literal") return;
      const a = this.node(s);
      if (!a) return;
      if (!asEdges) {
        if (p.value === RDF_TYPE && o.kind === "iri") { if (!a.types.includes(o.value)) a.types.push(o.value); return; }
        if (o.kind === "literal") {
          if (LABEL_PREDS.has(p.value)) { if (!a.label) a.label = o.value; return; }
          if (o.value.length <= SHORT_LIT) this.prop(a, this.short(p.value), o.value + (o.lang ? "@" + o.lang : ""));
          return;
        }
      }
      this.edge(a, p.value, this.node(o));
    }
    // generic SELECT row: chain the IRI / blank-node cells in column order (edge labelled by the target variable);
    // literal cells describe the first node
    row(row, vars) {
      const nodes = [];
      for (const v of vars) {
        const t = row[v];
        if (!t || (t.kind !== "iri" && t.kind !== "bnode")) continue;
        if (t.kind === "iri" && IMG.test(t.value) && nodes.length) { this.prop(nodes[0].n, v, t.value); continue; }
        const n = this.node(t);
        if (n) nodes.push({ n, v });
      }
      const owner = nodes[0] && nodes[0].n;
      if (owner) {
        for (const v of vars) {
          const t = row[v];
          if (!t || t.kind !== "literal") continue;
          if (NAMEISH.test(v)) { if (!owner.label) owner.label = t.value; }
          else if (t.value.length <= SHORT_LIT) this.prop(owner, v, t.value + (t.lang ? "@" + t.lang : ""));
        }
      }
      for (let i = 1; i < nodes.length; i++) this.edge(nodes[i - 1].n, nodes[i].v, nodes[i].n);
    }
    ingest(rows, vars, form) {
      const construct = form === "CONSTRUCT" || form === "DESCRIBE";
      const spo = !construct && vars.length === 3 && rows.length > 0 &&
        rows.every((r) => r[vars[1]] && r[vars[1]].kind === "iri" && r[vars[0]] && r[vars[0]].kind !== "literal");
      for (const r of rows) {
        if (construct) this.triple(r.s, r.p, r.o, false);
        else if (spo) this.triple(r[vars[0]], r[vars[1]], r[vars[2]], true);
        else this.row(r, vars);
      }
    }
    data() { return { nodes: [...this.nodes.values()], edges: [...this.edges.values()] }; }
  }

  function GraphView(result, rows, vars, ctx) {
    const skin = ctx.skin || "hf";
    const noteEl = h("div", { "data-rl": "graph-note", class: "flex-none border-b px-3 py-1 text-xs text-gray-500 dark:border-gray-800" });
    const stage = h("div", { class: "relative min-h-[380px] flex-1" });
    const holder = h("div", { class: "absolute inset-0" });
    stage.append(holder);
    const box = h("div", { class: "relative flex min-h-[380px] flex-1 flex-col", "data-rl": "graph" }, noteEl, stage);
    const builder = new Builder(ctx.short);
    builder.ingest(rows, vars, result.form);
    const data = builder.data();
    const known = new Map(data.nodes.map((n) => [n.id, n.kind]));
    let kit = null, unwatch = null, ro = null, dead = false, expanded = 0;

    const note = () => {
      const s = kit ? kit.stats() : { nodes: data.nodes.length, edges: data.edges.length };
      let t = `${num(s.nodes)} nodes, ${num(s.edges)} edges`;
      if (result.length != null && rows.length < result.length) t += `. Showing the first ${num(rows.length)} rows of ${num(result.length)}`;
      if (builder.capped) t += ` (capped at ${num(MAX_NODES)} nodes / ${num(MAX_EDGES)} edges)`;
      if (expanded) t += `. Expanded ${expanded} time${expanded === 1 ? "" : "s"}`;
      noteEl.textContent = t;
    };

    const openIri = (iri) => { const u = safeUrl(iri); if (u) window.open(u, "_blank", "noopener,noreferrer"); };

    // out / in neighbours of one IRI through the shared engine, plus one VALUES query for their labels
    async function onExpand(id, dir) {
      if (known.get(id) !== "iri" || !IRI_OK.test(id)) return { nodes: [], edges: [] };
      const b = new Builder(ctx.short);
      const me = { kind: "iri", value: id };
      const jobs = [];
      if (dir === "out" || dir === "both") jobs.push(ctx.collect(`SELECT ?p ?o WHERE { <${id}> ?p ?o } LIMIT ${EXPAND_LIMIT}`).then((rs) => { for (const r of rs) b.triple(me, r.p, r.o, false); }));
      if (dir === "in" || dir === "both") jobs.push(ctx.collect(`SELECT ?s ?p WHERE { ?s ?p <${id}> } LIMIT ${EXPAND_LIMIT}`).then((rs) => { for (const r of rs) b.triple(r.s, r.p, me, false); }));
      await Promise.all(jobs);
      const need = [...b.nodes.values()].filter((n) => n.kind === "iri" && !n.label && IRI_OK.test(n.id)).slice(0, 100);
      if (need.length) {
        const q = `SELECT ?x ?l WHERE { VALUES ?x { ${need.map((n) => "<" + n.id + ">").join(" ")} } VALUES ?lp { ${[...LABEL_PREDS].map((p) => "<" + p + ">").join(" ")} } ?x ?lp ?l }`;
        try { for (const r of await ctx.collect(q)) { const n = r.x && b.nodes.get(r.x.value); if (n && !n.label && r.l && r.l.kind === "literal") n.label = r.l.value; } } catch (_) { /* labels are optional */ }
      }
      const out = b.data();
      for (const n of out.nodes) if (!known.has(n.id)) known.set(n.id, n.kind);
      expanded++;
      setTimeout(note, 0);
      if (ctx.onExpand) ctx.onExpand(out.edges.length);
      return out;
    }

    const applyTheme = () => {
      if (!kit) return;
      const t = window.RetelensTheme.tokens(box, skin);
      kit.setTheme({ tokens: t, dark: t.dark });
    };

    function start() {
      if (dead) return;
      if (!window.GraphKit) { holder.append(h("div", { class: "p-6 text-sm text-red-600" }, "Graph library not loaded.")); return; }
      if (!data.nodes.length) { holder.append(h("div", { class: "p-6 text-sm text-gray-500" }, "Nothing to draw: this result has no IRI or blank-node columns.")); note(); return; }
      const tokens = window.RetelensTheme.tokens(box, skin);
      kit = window.GraphKit.mount(holder, {
        skin, tokens,
        workerUrl: location.protocol === "chrome-extension:" ? chrome.runtime.getURL("lib/graphkit/graphkit-worker.js") : undefined, dark: tokens.dark,
        onExpand, onOpen: openIri,
        labelFor: (id) => { const n = builder.nodes.get(id); return n && n.label ? n.label : ""; },
      });
      view.kit = kit;
      // GraphKit falls back per component to its own themed look where the host lacks the skin classes
      box.setAttribute("data-rl-skin", JSON.stringify({ skin, fellBack: kit.fellBack || null, workerOk: kit.workerOk }));
      window.__retelensKit = kit;
      kit.setData(data);
      kit.on("data", note);
      note();
      unwatch = window.RetelensTheme.watch(applyTheme);
      try { ro = new ResizeObserver(() => kit && kit.resize()); ro.observe(stage); } catch (_) {}
    }
    // the container needs its size before GraphKit measures it
    requestAnimationFrame(() => requestAnimationFrame(start));

    const view = {
      el: box, builder, kit: null, skinCheck: null,
      resize() { kit && kit.resize(); },
      restyle: applyTheme,
      destroy() { dead = true; try { unwatch && unwatch(); ro && ro.disconnect(); kit && kit.destroy(); } catch (_) {} kit = null; },
    };
    return view;
  }

  // graphkit.js is 1.4 MB. As a manifest content script it stops Chrome from loading the extension at all in the test
  // harness (and would be parsed on every HF / GitHub page), so a content script asks the service worker to inject
  // it when a Graph view is first opened; extension pages (viewer.html) load it with a <script>.
  let gkP = null;
  function ensure() {
    if (window.GraphKit) return Promise.resolve();
    gkP = gkP || new Promise((resolve, reject) => {
      chrome.runtime.sendMessage({ retelens: "loadGraphKit" }, (r) => (chrome.runtime.lastError || !r || !r.ok ? reject(new Error("graph library failed to load: " + ((chrome.runtime.lastError && chrome.runtime.lastError.message) || (r && r.error) || "no answer"))) : resolve()));
    });
    return gkP;
  }

  window.RetelensGraph = { GraphView, Builder, nodeOf, ensure };
})();
