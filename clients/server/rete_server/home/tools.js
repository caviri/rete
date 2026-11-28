/* WebMCP for the rete-server home page, in the style of the playground's tools
   (web/playground-src/app.js): registered through document.modelContext, then
   navigator.modelContext; nothing happens in a browser without either.
   Read tools answer from /api and leave the page as it is; open_dataset and
   show_query do what a click would, so the person watching sees the same.
   Each call is logged, shown in a pill while it runs, and listed by get_activity.
   Results are plain JSON under ~18 KB and say when they were cut. */
(() => {
  "use strict";
  const home = window.reteHome;
  if (!home) return;
  const $ = (id) => document.getElementById(id);
  const MAX = 18000;
  const { api, clip, state: S } = home;
  const size = (v) => JSON.stringify(v).length;
  const int = (v, dflt, max) => Math.min(max, Math.max(1, Number.isFinite(+v) && +v > 0 ? Math.floor(+v) : dflt));
  const enc = (key) => String(key).split("/").map(encodeURIComponent).join("/");
  function fit(items, wrap) {
    let n = items.length;
    while (n > 0 && size(wrap(items.slice(0, n))) > MAX) n = Math.max(0, Math.floor(n * 0.8) - (n < 5 ? 1 : 0));
    return { items: items.slice(0, n), cut: n < items.length };
  }
  function shrink(v, items, chars) {
    if (typeof v === "string") return clip(v, chars);
    if (Array.isArray(v)) return v.slice(0, items).map((x) => shrink(x, items, chars));
    if (v && typeof v === "object") { const o = {}; Object.keys(v).forEach((k) => { o[k] = shrink(v[k], items, chars); }); return o; }
    return v;
  }
  const keyOf = (a) => {
    const k = String((a && a.key) || S.selected || "");
    if (!k) throw new Error("No dataset is open. Call open_dataset with a key from list_datasets.");
    return k;
  };
  const obj = (properties, required) => ({ type: "object", properties: properties || {}, required: required || [], additionalProperties: false });
  const ro = { readOnlyHint: true };

  // ---- activity log and pill ---------------------------------------------------
  const log = [];
  let seq = 0, pillTimer = null;
  function note(out) {
    if (!out || typeof out !== "object") return "";
    if (Array.isArray(out.rows)) return out.rows.length + " of " + (out.total != null ? out.total : out.rows.length) + " rows";
    if (out.kind === "ask") return "ASK " + out.boolean;
    if (Array.isArray(out.datasets)) return out.datasets.length + " datasets";
    if (Array.isArray(out.matches)) return out.matches.length + " matches";
    if (Array.isArray(out.examples)) return out.examples.length + " examples";
    if (Array.isArray(out.classes)) return out.classes.length + " classes";
    if (out.key) return String(out.key);
    return home.bytes(size(out));
  }
  function pill(text, kind) {
    const el = $("pill");
    el.textContent = text; el.className = "pill " + (kind || ""); el.hidden = false;
    el.title = "An agent is using this page through WebMCP.";
    clearTimeout(pillTimer);
    if (kind !== "run") pillTimer = setTimeout(() => { el.hidden = true; }, 6000);
  }
  function watched(t) {
    const execute = async (a) => {
      const entry = { id: ++seq, ts: Date.now(), tool: t.name, args: clip(JSON.stringify(a || {}), 240), state: "running", ms: null, note: "" };
      log.unshift(entry);
      if (log.length > 200) log.length = 200;
      pill("Agent · " + t.name + " …", "run");
      const t0 = performance.now();
      try {
        const out = await t.execute(a || {});
        entry.state = "ok"; entry.note = note(out);
        return out;
      } catch (e) {
        entry.state = "error"; entry.note = clip((e && e.message || String(e)).split("\n")[0], 160);
        throw e;
      } finally {
        entry.ms = Math.round(performance.now() - t0);
        pill("Agent · " + t.name + (entry.state === "ok" ? " ✓ " : " ✗ ") + entry.note, entry.state === "ok" ? "ok" : "bad");
      }
    };
    return Object.assign({}, t, { execute });
  }

  // ---- tools -------------------------------------------------------------------
  function summary(d) {
    return { key: d.key, title: d.label, description: clip(d.description, 110), triples: d.triples != null ? d.triples : null,
      size: d.size_bytes || d.size || null, licence: d.license || null, tags: (d.tags || []).slice(0, 5), kind: d.kind || null };
  }
  async function ensure(key) {
    if (S.selected !== key || !S.entry) await home.select(key, true);
    return S.entry;
  }
  function rows(res, limit) {
    if (res.kind === "ask" || (res.boolean != null && !res.head)) return { kind: "ask", boolean: !!res.boolean };
    const vars = (res.head && res.head.vars) || [];
    const all = res.table || [];
    if (!vars.length && Array.isArray(res.triples)) {
      const f = fit(res.triples.slice(0, limit), (x) => ({ triples: x }));
      return { kind: res.kind || "construct", triples: f.items, total: res.triples.length, truncated: f.cut || res.triples.length > limit };
    }
    const f = fit(all.slice(0, limit), (x) => ({ rows: x }));
    return { kind: "select", vars, rows: f.items, returned: f.items.length, total: all.length,
      truncated: f.cut || all.length > limit || !!res.truncated };
  }
  function tools() {
    return [
      {
        name: "get_state", annotations: ro,
        description: "What the home page has open: the server version, the selected dataset, the query in the box, whether uploads are on. Call this first.",
        inputSchema: obj(),
        execute: async () => ({ page: "rete-server home", version: S.health ? S.health.version : null, datasets: S.datasets.length,
          dataset: S.selected, query: clip($("q").value, 4000), uploads: S.uploads ? S.uploads.enabled : null, link: location.origin + location.pathname })
      },
      {
        name: "list_datasets", annotations: ro,
        description: "The datasets this server answers for (key, title, triples, size, licence, tags). Keys go to open_dataset and the other tools.",
        inputSchema: obj({
          query: { type: "string", description: "Words to look for in key, title, description and tags. Empty lists everything.", maxLength: 200 },
          limit: { type: "integer", minimum: 1, maximum: 200, description: "Default 60." },
          offset: { type: "integer", minimum: 0, description: "Skip this many matches; pass the previous result's next." },
        }),
        execute: async (a) => {
          const w = String((a && a.query) || "").toLowerCase().split(/\s+/).filter(Boolean);
          const all = S.datasets.filter((d) => { const t = home.words(d); return w.every((x) => t.includes(x)); }).map(summary);
          const off = Math.max(0, Math.floor(+(a && a.offset)) || 0);
          const f = fit(all.slice(off, off + int(a && a.limit, 60, 200)), (x) => ({ datasets: x }));
          const end = off + f.items.length;
          return { datasets: f.items, returned: f.items.length, matched: all.length, next: end < all.length ? end : null };
        },
      },
      {
        name: "describe_dataset", annotations: ro,
        description: "One answer about a dataset before you query it: title, description, size, triples, licence, its largest classes and its example queries by title. Without a key it describes the open dataset. For the whole card use get_dataset_card.",
        inputSchema: obj({ key: { type: "string", description: "A key from list_datasets. Default: the open dataset." } }),
        execute: async (a) => {
          const key = keyOf(a);
          const known = S.datasets.find((d) => d.key === key);
          if (!known) throw new Error('No dataset with key "' + clip(key, 80) + '". Call list_datasets.');
          const [entry, schema, examples] = await Promise.allSettled([
            api("/api/datasets/" + enc(key)), api("/api/datasets/" + enc(key) + "/schema"), api("/api/datasets/" + enc(key) + "/examples")]);
          const e = entry.status === "fulfilled" ? entry.value : known, card = e.card || {};
          const cls = schema.status === "fulfilled" ? (schema.value.schema.classes || []) : [];
          const out = Object.assign(summary(e), { open: key === S.selected, title: card.title || e.label, description: clip(card.description || e.description, 1500),
            totalClasses: cls.length, topClasses: cls.slice(0, 15),
            examples: (examples.status === "fulfilled" ? examples.value : []).map((x, i) => ({ index: i, title: x.title || x.question })).slice(0, 40) });
          return size(out) <= MAX ? out : shrink(out, 20, 400);
        },
      },
      {
        name: "get_dataset_card", annotations: ro,
        description: "The Dataset Card embedded in the file: title, description, licence, source, counts and starter queries. Costs a few KB even on a very large remote file. Null when the file carries none.",
        inputSchema: obj({ key: { type: "string", description: "Default: the open dataset." } }),
        execute: async (a) => {
          const key = keyOf(a);
          let card = null;
          try { card = await api("/api/datasets/" + enc(key) + "/card"); }
          catch (e) { if (e.status !== 404) throw e; return { key, card: null, note: "This file carries no Dataset Card." }; }
          const out = { key, card };
          if (size(out) <= MAX) return out;
          for (const [items, chars] of [[12, 600], [6, 300], [3, 160]]) {
            const small = { key, card: shrink(card, items, chars), truncated: true };
            if (size(small) <= MAX) return small;
          }
          return { key, card: clip(JSON.stringify(card), MAX - 400), truncated: true };
        },
      },
      {
        name: "get_schema", annotations: ro,
        description: "The classes of a dataset with instance counts, and the relations between classes. Read it before writing a query.",
        inputSchema: obj({
          key: { type: "string", description: "Default: the open dataset." },
          limit: { type: "integer", minimum: 1, maximum: 500, description: "Classes and relations to return. Default 60." },
        }),
        execute: async (a) => {
          const key = keyOf(a), lim = int(a && a.limit, 60, 500);
          const s = await api("/api/datasets/" + enc(key) + "/schema");
          const sc = s.schema || {}, classes = sc.classes || [], rel = sc.relations || [];
          const f = fit(classes.slice(0, lim), (x) => ({ classes: x }));
          let n = Math.min(rel.length, lim);
          while (n > 0 && size({ classes: f.items, relations: rel.slice(0, n) }) > MAX) n = Math.floor(n * 0.8);
          return { key, info: s.info, classes: f.items, relations: rel.slice(0, n), totalClasses: classes.length, totalRelations: rel.length,
            graphs: (s.graphs || []).slice(0, 20), truncated: f.cut || n < rel.length };
        },
      },
      {
        name: "list_examples", annotations: ro,
        description: "The example queries of a dataset with their SPARQL: the ones in the file's card, else the catalog's.",
        inputSchema: obj({ key: { type: "string", description: "Default: the open dataset." } }),
        execute: async (a) => {
          const key = keyOf(a);
          const ex = await api("/api/datasets/" + enc(key) + "/examples");
          const list = (Array.isArray(ex) ? ex : []).map((e, i) => ({ index: i, title: e.title || e.question, query: clip(e.sparql, 1500) }));
          const f = fit(list, (x) => ({ examples: x }));
          return { key, examples: f.items, total: list.length, truncated: f.cut };
        },
      },
      {
        name: "run_sparql", annotations: ro,
        description: "Run a SPARQL query (SELECT, ASK, CONSTRUCT, DESCRIBE) on a dataset and get the rows back. The page is not changed (use show_query to show it). Values are plain strings as the server returns them. Put a LIMIT on exploratory queries.",
        inputSchema: obj({
          query: { type: "string", description: "The SPARQL text, with its PREFIX lines.", maxLength: 20000 },
          key: { type: "string", description: "Dataset key. Default: the open dataset." },
          limit: { type: "integer", minimum: 1, maximum: 500, description: "Rows to return. Default 50." },
          reason: { type: "boolean", description: "Apply OWL 2 QL reasoning. Default false." },
        }, ["query"]),
        execute: async (a) => {
          const q = String((a && a.query) || "").trim();
          if (!q) throw new Error("query is empty.");
          const key = keyOf(a), lim = int(a && a.limit, 50, 500);
          const t0 = performance.now();
          const res = await api("/api/query", { method: "POST", headers: { "Content-Type": "application/json" },
            body: JSON.stringify({ dataset: key, query: q, reason: !!(a && a.reason), limit: lim }) });
          const out = rows(res, lim);
          out.ms = Math.round(performance.now() - t0);
          if (res.warnings && res.warnings.length) out.warnings = res.warnings.slice(0, 5);
          return out;
        },
      },
      {
        name: "find_entities", annotations: ro,
        description: "Find things by the start of their label, using the label index of the file. Returns labels with their IRIs, to use in a query.",
        inputSchema: obj({
          prefix: { type: "string", description: "The first letters of a label, at least 2.", minLength: 2, maxLength: 100 },
          key: { type: "string", description: "Default: the open dataset." },
          limit: { type: "integer", minimum: 1, maximum: 50, description: "Default 12." },
        }, ["prefix"]),
        execute: async (a) => {
          const p = String((a && a.prefix) || "").trim();
          if (p.length < 2) throw new Error("prefix needs at least 2 characters.");
          const key = keyOf(a);
          const hits = await api("/api/datasets/" + enc(key) + "/search?q=" + encodeURIComponent(p) + "&limit=" + int(a && a.limit, 12, 50));
          return { prefix: p, matches: hits };
        },
      },
      {
        name: "open_dataset",
        description: "Open a dataset in the page: it is selected, its card shows, and the other tools default to it. Use a key from list_datasets.",
        inputSchema: obj({ key: { type: "string", description: "Dataset key, e.g. people or generated/ab12.rete.", maxLength: 300 } }, ["key"]),
        execute: async (a) => {
          const key = String(a.key || "");
          if (!S.datasets.some((d) => d.key === key)) throw new Error('No dataset with key "' + clip(key, 80) + '". Call list_datasets for the keys.');
          const e = await home.select(key);
          $("datasets").scrollIntoView({ block: "start" });
          return { key, title: (e && e.label) || key, selected: S.selected === key };
        },
      },
      {
        name: "show_query",
        description: "Put a query in the page's box and run it there, so the person watching sees the table. To get rows as data, use run_sparql.",
        inputSchema: obj({
          query: { type: "string", maxLength: 20000 },
          key: { type: "string", description: "Dataset to open first. Default: the open dataset." },
        }, ["query"]),
        execute: async (a) => {
          const key = keyOf(a);
          if (key !== S.selected) await home.select(key, true);
          $("query").scrollIntoView({ block: "start" });
          const res = await home.run(String(a.query), key);
          return { shown: true, key, ...rows(res, 20) };
        },
      },
      {
        name: "upload_info", annotations: ro,
        description: "Whether this server stores uploaded files, how large they may be and which kinds, plus the curl line that sends a local file without it entering the conversation.",
        inputSchema: obj(),
        execute: async () => {
          const u = await api("/api/uploads");
          return Object.assign({}, u, { endpoint: location.origin + "/api/uploads?name=<file name>",
            curl: 'curl --data-binary @file.ttl "' + location.origin + '/api/uploads?name=file.ttl"' + (S.token ? ' -H "Authorization: Bearer $RETE_TOKEN"' : "") });
        },
      },
      {
        name: "get_activity", annotations: ro,
        description: "The tool calls made on this page since it loaded, newest first: tool, arguments, whether it worked, how long it took, and a short note on the result.",
        inputSchema: obj({ limit: { type: "integer", minimum: 1, maximum: 100, description: "Default 20." } }),
        execute: async (a) => {
          const list = log.slice(1, 1 + int(a && a.limit, 20, 100)).map((e) => ({
            tool: e.tool, args: e.args, state: e.state, ms: e.ms, note: e.note, at: new Date(e.ts).toISOString() }));
          const f = fit(list, (x) => ({ calls: x }));
          return { calls: f.items, total: Math.max(0, log.length - 1) };
        },
      },
    ];
  }

  // ---- registration ------------------------------------------------------------
  function wire() {
    let ctx = null;
    try { ctx = document.modelContext || null; } catch (_e) { /* none */ }
    if (!ctx) { try { ctx = navigator.modelContext || null; } catch (_e) { /* none */ } }
    const list = tools().map(watched);
    const byName = {};
    list.forEach((t) => { byName[t.name] = t; });
    // The same tools by name for page scripts and tests, with or without WebMCP.
    window.reteHomeTools = {
      list: () => list.map((t) => ({ name: t.name, description: t.description, inputSchema: t.inputSchema, readOnly: !!(t.annotations && t.annotations.readOnlyHint) })),
      call: (name, args) => (byName[name] ? byName[name].execute(args || {}) : Promise.reject(new Error("no tool named " + name))),
    };
    const announce = (detail) => {
      window.reteWebMCP = detail;
      try { home.renderConnect(); } catch (_e) { /* page not ready */ }
      try { document.dispatchEvent(new CustomEvent("rete:webmcp", { detail })); } catch (_e) { /* ignore */ }
    };
    if (!ctx || (typeof ctx.registerTool !== "function" && typeof ctx.provideContext !== "function")) {
      announce({ supported: false, tools: [] });
      return;
    }
    const names = [];
    if (typeof ctx.registerTool === "function") {
      const ac = typeof AbortController === "function" ? new AbortController() : null;
      list.forEach((t) => {
        try { ctx.registerTool(t, ac ? { signal: ac.signal } : undefined); names.push(t.name); }
        catch (e) { console.debug("[rete webmcp] registerTool failed for", t.name, e && e.message); }
      });
      window.addEventListener("pagehide", () => {
        try {
          if (ac) ac.abort();
          else if (typeof ctx.unregisterTool === "function") names.forEach((n) => ctx.unregisterTool(n));
        } catch (_e) { /* ignore */ }
      }, { once: true });
    } else {
      try { ctx.provideContext({ tools: list }); list.forEach((t) => names.push(t.name)); }
      catch (e) { console.debug("[rete webmcp] provideContext failed", e && e.message); }
    }
    announce({ supported: true, tools: names });
  }
  try { wire(); } catch (e) { console.debug("[rete webmcp] off", e && e.message); }
})();
