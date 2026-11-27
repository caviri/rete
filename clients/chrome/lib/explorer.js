// Explorer: the shared Data Studio component. Modes:
//   "studio"  full width: Graph/Query pills, search, Table|Cards|Graph, pager, status,
//             right side panel (Examples | SPARQL Console | Schema | Card)
//   "box"     HF "Dataset Viewer" card for dataset home / blob / bucket file pages
// It talks to a Session (engine + file list). Everything untrusted is rendered
// as text (see format.js / views.js).
(function () {
  const { h, num, bytes, Prefixes, safeUrl, icon } = window.RetelensFmt;
  const V = window.RetelensViews;
  const R = window.RetelensResults;
  const UI = window.RetelensResultsUI;
  const ENGINE_LABEL = "rete-graph __RETE_VERSION__"; // stamped by build.mjs
  const IRI_OK = /^[^\s<>"{}|\\^`]+$/;
  const BIG_FILE = 5 * 1000 * 1000;

  // ---------------------------------------------------------------- session
  function normalizeExamples(list) {
    const out = list.map((e) => {
      let title = e.title, sparql = e.sparql;
      if (!title) { const m = sparql.match(/^\s*#\s*(.+)$/m); title = m ? m[1].trim() : "Untitled query"; }
      return { ...e, title, group: e.dimension || "card examples", curated: !e.id };
    });
    return [...out.filter((e) => e.curated), ...out.filter((e) => !e.curated)].map((e) => window.RetelensHints.annotate(e));
  }

  // Files whose card has no example queries get generated starters built from schema() and info().
  const PFX = "PREFIX rdfs: <http://www.w3.org/2000/01/rdf-schema#>\nPREFIX skos: <http://www.w3.org/2004/02/skos/core#>\nPREFIX schema: <http://schema.org/>\nPREFIX foaf: <http://xmlns.com/foaf/0.1/>\n";
  function starterQueries(schema) {
    const out = [];
    const add = (title, question, sparql) => out.push({ title, question, sparql, group: "Starter queries", curated: true, starter: true });
    const classes = [...(schema?.classes || [])].sort((a, b) => b[1] - a[1]);
    if (classes.length) add("Classes with counts", "What kinds of things are in this graph, and how many?", "SELECT ?class (COUNT(?s) AS ?instances) WHERE { ?s a ?class } GROUP BY ?class ORDER BY DESC(?instances)");
    add("Predicates with counts", "Which relations are used, and how often?", "SELECT ?predicate (COUNT(*) AS ?triples) WHERE { ?s ?predicate ?o } GROUP BY ?predicate ORDER BY DESC(?triples)");
    if (classes.length && IRI_OK.test(classes[0][0])) {
      add("Instances of the biggest class", `100 instances of ${classes[0][0].split(/[#/]/).pop()}, with a label when there is one.`,
        PFX + `SELECT ?s ?label WHERE {\n  ?s a <${classes[0][0]}> .\n  OPTIONAL { ?s ?lp ?label . FILTER(?lp = rdfs:label || ?lp = skos:prefLabel || ?lp = schema:name || ?lp = foaf:name) }\n} LIMIT 100`);
    }
    add("First 100 triples", "A plain sample of the data.", "SELECT ?s ?p ?o WHERE { ?s ?p ?o } LIMIT 100");
    return out;
  }

  class Session {
    // files: [{path, src, label}]
    constructor(engine, files) {
      this.engine = engine; this.files = files; this.idx = -1;
      this.prefixes = new Prefixes(); this.meta = null; this.examples = []; this.listeners = [];
    }
    onChange(fn) { this.listeners.push(fn); if (this.meta) fn(this); }
    async open(i) {
      this.idx = i; this.meta = null; this.examples = [];
      const f = this.files[i];
      this.tilesP = null;
      if (f.bytes) await this.engine.openBytes(f.bytes);
      else if (f.local) await this.engine.openLocal(f.local, { lazy: f.lazy });
      else await this.engine.open(f.src);
      this.meta = await this.engine.meta();
      this.examples = normalizeExamples(await this.engine.examples()); // card `queries` + `example_queries`, merged by the engine
      this.starters = false;
      if (!this.examples.length && f.examples) this.examples = f.examples.map((e) => window.RetelensHints.annotate({ group: "Starter queries", curated: true, starter: true, ...e }));
      if (!this.examples.length) {
        let schema = null;
        try { schema = await this.engine.schema(); } catch (_) {}
        this.examples = starterQueries(schema).map((e) => window.RetelensHints.annotate(e));
        this.starters = true;
      }
      const c = this.meta.card || {};
      for (const q of [...(c.example_queries || []), ...(c.queries || []).map((x) => x.sparql)]) this.prefixes.learn(q);
      for (const fn of this.listeners) fn(this);
    }
    get file() { return this.files[this.idx]; }
    // a PMTiles archive embedded in the open file, opened once: {pm, header, meta, length} or null
    tiles() {
      if (!this.tilesP) {
        this.tilesP = this.engine.tiles().then(async (sec) => {
          if (!sec) return null;
          const kit = await window.RetelensMaps.ensure();
          const key = (this.file?.src || this.file?.path || "file") + "#tiles@" + sec.offset;
          const t = await window.RetelensMaps.openPmtiles(kit, window.RetelensMaps.engineSource(this.engine, key, sec.offset));
          return { ...t, length: sec.length };
        }).catch(() => null);
      }
      return this.tilesP;
    }
    get fileLength() { return this.meta?.stats?.fileLength || 0; }
    cardData() { return { ...this.meta, fileLength: this.fileLength, name: this.file?.label }; }
    async collect(query, limit) {
      const res = await this.engine.run(query);
      if (res.handle == null) return [];
      const pg = await this.engine.page(res.handle, 0, limit);
      this.engine.free(res.handle).catch(() => {});
      return pg.rows;
    }
  }

  const formOf = (q) => {
    const s = q.replace(/<[^<>\s]*>/g, "<>").replace(/#[^\n]*/g, " ");
    const m = s.match(/\b(SELECT|ASK|CONSTRUCT|DESCRIBE)\b/i);
    return m ? m[1].toUpperCase() : "SELECT";
  };

  const defaultParams = () => ({
    get: (k) => new URLSearchParams(location.search).get(k),
    set(obj) {
      const u = new URL(location.href);
      for (const [k, v] of Object.entries(obj)) { if (v == null) u.searchParams.delete(k); else u.searchParams.set(k, v); }
      history.replaceState(history.state, "", u);
    },
  });

  // ---------------------------------------------------------------- explorer
  class Explorer {
    // opts: {session, mode, params, urlKeys, studioHref(state)->string, isDark}
    constructor(opts) {
      this.o = opts;
      this.s = opts.session;
      this.mode = opts.mode || "studio";
      this.params = opts.params || defaultParams();
      this.urlKeys = new Set(opts.urlKeys || ["file", "q", "view"]);
      this.urlFilters = this.urlKeys.size > 0; // search / filter / sort ride along in the URL where the view state does
      this.isDark = opts.isDark || (() => document.documentElement.classList.contains("dark"));
      this.view = null; this.explicitView = false;
      this.colTypes = {}; this.cardHidden = new Set(); this.hasTiles = false; // render type per variable name; fields the cards leave out
      // the host page's CSP refused pictures (lib/cells.js): say so once, with the file in the extension's viewer
      this.mediaBlocked = false;
      document.addEventListener("retelens:media-blocked", () => { this.mediaBlocked = true; if (this.toolbar) this.renderToolbar(); });
      this.q = 0; this.page = 1; this.tok = 0; this.result = null; this.gv = null; this.status = {};
      this.sideTab = "examples";
      this.qmenu = new window.RetelensQuery.QueryMenu(this);
      this.sparqlOpen = false; // the SPARQL block starts collapsed everywhere (studio, viewer, inline boxes); once opened it stays open for the next queries
      this.lastRun = null; this.lastExample = 0;
      this.skinName = opts.skin || (location.protocol === "chrome-extension:" ? "standalone" : "hf");
      this.gh = this.skinName === "github";
      this.model = null; this.loading = null; this.facetsOpen = false; this.facetMore = new Set();
      this.colMenu = new UI.ColumnMenu();
      this.urlState = this.urlFilters ? { rq: this.params.get("rq"), rs: this.params.get("rs"), rf: this.params.get("rf") } : null;
      if (this.gh) window.RetelensFmt.setSkin("github");
      this.short = (i) => this.s.prefixes.shorten(i);
      this.fbar = UI.FilterBar(this);
      this.el = this.mode === "box" ? this.buildBox() : this.buildStudio();
      this.ctx = {
        short: this.short, isDark: this.isDark, skin: this.skinName,
        collect: (q) => this.s.collect(q, 50), onExpand: () => this.setStatus({}),
      };
    }

    // --- url state
    writeUrl() {
      const o = {};
      if (this.urlKeys.has("file") && this.s.files.length > 1) o.file = this.s.file?.path ?? null;
      if (this.urlKeys.has("q")) o.q = typeof this.q === "number" && this.q > 0 ? String(this.q) : null;
      if (this.urlKeys.has("view") && this.explicitView) o.view = this.view;
      if (this.urlFilters) Object.assign(o, this.model ? this.model.toParams() : { rq: null, rs: null, rf: null });
      try { this.params.set(o); } catch (_) {}
    }
    state() { return { file: this.s.file?.path, q: typeof this.q === "number" ? this.q : null, view: this.explicitView ? this.view : null }; }
    // from an inline box: go to the studio with the console open (the box has no console of its own)
    openStudioConsole() {
      const st = { ...this.state(), console: true };
      if (this.o.onStudio) this.o.onStudio(st);
      else if (this.o.studioHref) location.assign(this.o.studioHref(st));
    }

    // --- build
    // ---- GitHub (Primer) skin: same state and behaviour, Primer markup (btn btn-sm, Label, Counter,
    // UnderlineNav, SegmentedControl) styled by content/github.css from Primer variables only
    buildBoxGH() {
      const root = h("div", { class: "flex flex-col", "data-retelens": "box", "data-rl": "box" });
      this.pills = h("div", { class: "flex min-w-0 flex-wrap items-center gap-2" });
      const studio = this.o.studioHref || this.o.onStudio ? h("a", { class: "btn btn-sm", href: "#", "data-rl": "btn" }, icon("tableView"), h("span", null, "Data Studio")) : null;
      if (studio) { this.studioLink = studio; studio.addEventListener("click", (e) => { if (this.o.onStudio) { e.preventDefault(); this.o.onStudio(this.state()); } }); }
      this.formatPill = h("span", { class: "Label", "data-rl": "label" }, "rete");
      this.dlSlot = h("span", { class: "contents" });
      const head = h("div", { class: "flex flex-wrap items-center gap-2 border-b px-3 py-2" }, this.pills, h("div", { class: "ml-auto flex flex-wrap items-center gap-2" }, this.formatPill, this.o.toolbar ? null : this.dlSlot, this.o.toolbar ? null : studio));
      // `toolbar: true`: the adapter puts Data Studio into GitHub's own blob toolbar (next to Raw / copy / download),
      // which already has the download button, so the box header keeps only the pills and the format label
      if (studio && this.o.toolbar) this.toolbarEl = h("span", { "data-retelens": "", "data-retelens-gh": "", style: "display:inline-flex;margin-right:8px" }, studio);
      this.qblock = h("div", { class: "border-b px-3 py-2 text-sm", hidden: true, "data-rl": "qblock" });
      this.toolbar = h("div", { class: "flex flex-wrap items-center gap-2 border-b px-3 py-2 text-xs" });
      this.area = h("div", { class: "relative flex max-h-[26rem] min-h-[10rem] flex-col overflow-auto" });
      this.pager = h("div", { class: "flex flex-wrap items-center justify-center gap-1 border-t px-3 py-1 text-sm" });
      this.statusText = h("span");
      this.statusEl = h("div", { class: "f6 border-t px-3 py-1 text-xs", "data-rl": "status" }, this.statusText);
      root.append(head, this.qblock, this.toolbar, this.fbar.el, this.area, this.pager, this.statusEl);
      return root;
    }
    buildStudioGH() {
      const root = h("div", { class: "flex min-h-0 flex-1 flex-col md:flex-row", "data-retelens": "studio", "data-rl": "studio" });
      this.pills = h("div", { class: "flex min-w-0 flex-wrap items-center gap-2" });
      this.searchInput = h("input", { class: "min-w-0 flex-1 px-2 py-1 text-sm", placeholder: "Search this graph", type: "search", dir: "auto", "aria-label": "Search this graph", "data-rl": "search" });
      this.searchInput.addEventListener("input", () => { if (!this.searchInput.value && this.q === "search") this.selectExample(0); });
      const form = h("form", { class: "flex min-w-0 flex-1 items-center gap-1", onsubmit: (e) => { e.preventDefault(); this.runSearch(this.searchInput.value); } }, icon("search"), this.searchInput);
      this.sideToggle = h("button", { class: "btn btn-sm btn-octicon", type: "button", "aria-label": "Hide sidepanel", "data-rl": "btn", onclick: () => this.toggleSide() }, icon("panel"));
      const top = h("div", { class: "flex flex-wrap items-center gap-2 border-b px-3 py-2" }, this.pills, form, this.sideToggle);
      this.qblock = h("div", { class: "border-b px-3 py-2 text-sm", hidden: true, "data-rl": "qblock" });
      this.toolbar = h("div", { class: "flex flex-wrap items-center gap-2 border-b px-3 py-2 text-xs" });
      this.area = h("div", { class: "relative flex min-h-[18rem] flex-1 flex-col overflow-auto" });
      this.pager = h("div", { class: "flex flex-wrap items-center justify-center gap-1 border-t px-3 py-1.5 text-sm" });
      this.statusText = h("span");
      this.linksEl = h("div", { class: "ml-auto flex items-center gap-3" });
      this.statusEl = h("div", { class: "f6 flex flex-wrap items-center gap-x-3 border-t px-3 py-1 text-xs", "data-rl": "status" }, this.statusText, this.linksEl);
      const main = h("div", { class: "flex min-w-0 flex-1 flex-col overflow-hidden" }, top, this.qblock, this.toolbar, this.fbar.el, this.area, this.pager, this.statusEl);
      this.sideBody = h("div", { class: "min-h-0 flex-1 overflow-auto" });
      this.sideTabs = h("div", { class: "UnderlineNav", "data-rl": "utabs" });
      this.sidePanel = h("div", { class: "flex min-h-[24rem] w-full flex-none flex-col overflow-hidden border-l md:w-[420px]", "data-rl": "side" }, this.sideTabs, this.sideBody);
      // draggable divider, like GitHub's Symbols pane
      this.resizer = h("div", { role: "separator", "aria-orientation": "vertical", "aria-label": "Resize side panel", tabindex: "0", class: "hidden w-1 flex-none cursor-col-resize md:block", "data-rl": "resizer" });
      this.resizer.addEventListener("pointerdown", (e) => {
        e.preventDefault(); this.resizer.setPointerCapture(e.pointerId);
        const x0 = e.clientX, w0 = this.sidePanel.getBoundingClientRect().width;
        const move = (ev) => { this.sidePanel.style.width = Math.max(260, Math.min(900, w0 + (x0 - ev.clientX))) + "px"; };
        const up = () => { this.resizer.removeEventListener("pointermove", move); this.resizer.removeEventListener("pointerup", up); this.gv && this.gv.resize && this.gv.resize(); };
        this.resizer.addEventListener("pointermove", move); this.resizer.addEventListener("pointerup", up);
      });
      root.append(main, this.resizer, this.sidePanel);
      this.buildSideTabs();
      return root;
    }

    buildStudio() {
      if (this.gh) return this.buildStudioGH();
      const root = h("div", { class: "flex min-h-0 flex-1 flex-col md:flex-row", "data-retelens": "studio" });
      this.pills = h("div", { class: "grid flex-1 grid-cols-1 overflow-hidden text-sm md:grid-cols-2 md:place-content-center md:rounded-l-lg" });
      this.sideToggle = h("button", { class: "group text-gray-500 hover:text-gray-700", type: "button", "aria-label": "Hide sidepanel", onclick: () => this.toggleSide() });
      this.sideToggle.append(icon("panel"));
      const pillBar = h("div", { class: "flex min-w-0 flex-wrap border-t md:my-4 md:ml-4 md:rounded-lg md:border md:border-t dark:border-gray-800" },
        h("div", { class: "flex min-w-0 flex-1 flex-wrap" }, this.pills),
        h("div", { class: "hidden flex-none flex-col items-center gap-0.5 border-l px-1 md:flex justify-end dark:border-gray-800" }, this.sideToggle));
      this.searchInput = h("input", { class: "h-9 w-full border-none bg-transparent px-1 pl-9 pr-3 placeholder:text-gray-400 focus:ring-0", placeholder: "Search this graph", type: "search", dir: "auto", "aria-label": "Search this graph" });
      const form = h("form", { class: "relative flex-1", onsubmit: (e) => { e.preventDefault(); this.runSearch(this.searchInput.value); } },
        h("span", { class: "pointer-events-none absolute left-3 top-2.5 text-gray-400" }, icon("search")), this.searchInput);
      this.searchInput.addEventListener("input", () => { if (!this.searchInput.value && this.q === "search") this.selectExample(0); });
      const search = h("div", { class: "relative flex items-center rounded-t-lg text-smd dark:bg-gray-950" }, form);
      this.toolbar = h("div", { class: "flex flex-wrap items-center gap-2 border-t px-2.5 py-1.5 text-xs dark:border-gray-800" });
      this.area = h("div", { class: "relative flex min-h-[18rem] flex-1 flex-col overflow-auto border-t dark:border-gray-800" });
      this.pager = h("div", { class: "flex flex-wrap items-center justify-center gap-1 border-t px-2.5 py-1.5 text-sm dark:border-gray-800" });
      this.statusText = h("span");
      this.linksEl = h("div", { class: "ml-auto flex items-center gap-3" });
      this.statusEl = h("div", { class: "flex flex-wrap items-center gap-x-3 border-t px-2.5 py-1 text-xs text-gray-400 dark:border-gray-800" }, this.statusText, this.linksEl);
      this.qblock = h("div", { class: "border-t px-3 py-2 text-sm dark:border-gray-800", hidden: true });
      const results = h("div", { class: "flex min-h-0 flex-1 flex-col md:mb-4 md:ml-4 md:rounded-lg md:border dark:border-gray-800" }, search, this.qblock, this.toolbar, this.fbar.el, this.area, this.pager, this.statusEl);
      const main = h("div", { class: "flex min-w-0 flex-1 flex-col overflow-hidden" }, pillBar, results);
      this.sideBody = h("div", { class: "min-h-0 flex-1 overflow-auto" });
      this.sideTabs = h("div", { class: "flex border-b border-gray-200 dark:border-gray-700" });
      this.sidePanel = h("div", { class: "mx-4 mb-4 flex min-h-[24rem] w-auto flex-none flex-col overflow-hidden rounded-lg border shadow-lg md:my-4 md:ml-0 md:mr-4 md:w-[480px] dark:border-gray-800" }, this.sideTabs, this.sideBody);
      root.append(main, this.sidePanel);
      this.buildSideTabs();
      return root;
    }

    buildBox() {
      if (this.gh) return this.buildBoxGH();
      const root = h("div", { class: "flex flex-col overflow-hidden rounded-lg border px-2.5 pt-2 shadow-xs mx-auto dark:ring dark:ring-white/5", "data-retelens": "box" });
      const studio = this.o.studioHref || this.o.onStudio ? h("a", { class: "flex items-center gap-1.5 rounded-full border bg-gradient-to-b from-white to-red-100/90 px-2 py-0.5 text-xs font-medium text-[#2D3648] transition-shadow hover:shadow-inner dark:from-gray-900 dark:to-red-800/30 dark:text-gray-100", href: "#" }, icon("tableView"), h("span", null, "Data Studio")) : null;
      if (studio) {
        this.studioLink = studio;
        studio.addEventListener("click", (e) => { if (this.o.onStudio) { e.preventDefault(); this.o.onStudio(this.state()); } });
      }
      this.formatPill = h("span", { class: "flex items-center gap-1 rounded-sm border px-1 py-0.5 text-xs text-gray-700 dark:text-gray-300" }, "rete");
      const head = h("div", { class: "mb-2 flex flex-wrap items-center gap-2" },
        h("div", { class: "mr-auto flex items-center" }, icon("tableSmall"), h("div", { class: "whitespace-nowrap font-semibold" }, this.skinName === "zenodo" ? "Graph preview" : "Dataset Viewer")),
        this.formatPill, h("span", { class: "flex items-center gap-1 rounded-sm border px-1 py-0.5 text-xs text-gray-700 dark:text-gray-300" }, "SPARQL"), (this.dlSlot = h("span", { class: "contents" })), studio);
      this.pills = h("div", { class: "grid flex-1 grid-cols-1 overflow-hidden text-sm md:grid-cols-2 md:place-content-center sm:mx-2.5" });
      const pillBar = h("div", { class: "-mx-2.5 flex min-w-0 flex-wrap border-t dark:border-gray-800" }, h("div", { class: "flex min-w-0 flex-1 flex-wrap" }, this.pills));
      this.toolbar = h("div", { class: "-mx-2.5 flex flex-wrap items-center gap-2 border-t px-2.5 py-1.5 text-xs dark:border-gray-800" });
      this.area = h("div", { class: "-mx-2.5 relative flex max-h-[26rem] min-h-[10rem] flex-col overflow-auto border-t dark:border-gray-800" });
      this.pager = h("div", { class: "-mx-2.5 flex flex-wrap items-center justify-center gap-1 border-t px-2.5 py-1 text-sm dark:border-gray-800" });
      this.statusText = h("span");
      this.statusEl = h("div", { class: "-mx-2.5 border-t px-2.5 py-1 text-xs text-gray-400 dark:border-gray-800" }, this.statusText);
      this.qblock = h("div", { class: "-mx-2.5 border-t px-2.5 py-2 text-sm dark:border-gray-800", hidden: true });
      root.append(head, pillBar, this.qblock, this.toolbar, this.fbar.el, this.area, this.pager, this.statusEl);
      return root;
    }

    // --- pills
    ghPill(kind, title, name, count, sel, onclick) {
      const label = title.replace(/\s*\(\d+\)$/, "") + ":";
      const kids = [icon(kind === "graph" ? "graph" : "play"), h("span", { class: "color-fg-muted" }, label), h("span", { class: "truncate" }, name),
        count != null ? h("span", { class: "Counter" }, String(count)) : null, icon("caret")];
      // Primer ActionMenu anchor: a regular `btn` with a leading octicon and a triangle-down caret
      const attrs = { class: "btn relative max-w-full", "data-rl": "pill", title: name };
      if (sel) { const l = h("label", attrs, ...kids, sel); return l; }
      return h("button", { ...attrs, type: "button", "aria-haspopup": "listbox", "aria-expanded": "false", onclick }, ...kids);
    }
    pill(title, line1, line2, options, onChange) {
      if (this.gh) {
        const sel = h("select", { class: "absolute inset-0 z-10 w-full cursor-pointer opacity-0", "aria-label": title });
        for (const [v, text, selected] of options) { const o = h("option", { value: v }, text); if (selected) o.selected = true; sel.append(o); }
        sel.addEventListener("change", () => onChange(sel.value));
        return this.ghPill("graph", title, line1, (title.match(/\((\d+)\)/) || [])[1] ?? null, sel);
      }
      const sel = h("select", { class: "absolute inset-0 z-10 w-full cursor-pointer border-0 bg-white text-base opacity-0 dark:bg-gray-900", "aria-label": title });
      for (const [v, text, selected] of options) { const o = h("option", { value: v }, text); if (selected) o.selected = true; sel.append(o); }
      sel.addEventListener("change", () => onChange(sel.value));
      return h("label", { class: "relative block min-w-0 flex-1 px-3 py-2 hover:bg-gray-50 md:border-r dark:hover:bg-gray-900 dark:border-gray-800", title: line1 },
        h("div", { class: "text-gray-500" }, title),
        h("div", { class: "flex items-center overflow-hidden whitespace-nowrap" }, h("span", { class: "truncate" }, line1), h("span", { class: "mx-2 text-gray-500" }, "·"), h("span", { class: "text-gray-500" }, line2), icon("chevron")),
        sel);
    }
    // repository link + Download: the right end of the status line (studio) / an icon in the box header
    renderLinks() {
      const f = this.s.file;
      const dl = f && f.download && !this.o.topbar ? window.RetelensFmt.dlLink(f.download, "") : null;
      if (this.linksEl) {
        this.linksEl.textContent = "";
        if (!this.o.topbar) {
          this.linksEl.append(h("a", { href: "https://github.com/caviri/rete", class: "flex items-center gap-1 text-gray-500 hover:text-blue-600", title: "rete on GitHub" }, icon("graphChip"), "rete"));
          if (dl) { dl.className = "flex items-center gap-1.5 rounded-md border border-gray-800 bg-gray-900 px-2 py-0.5 text-xs font-medium text-white hover:bg-gray-700 dark:border-gray-200 dark:bg-gray-100 dark:text-gray-900"; dl.append(icon("download"), "Download"); this.linksEl.append(dl); }
        }
      }
      if (this.dlSlot && !this.o.toolbar) {
        this.dlSlot.textContent = "";
        const b = f && f.download ? window.RetelensFmt.dlLink(f.download, "btn shadow-xs flex items-center rounded-sm border px-1 py-0.5 text-gray-700 dark:text-gray-300") : null;
        if (b) { b.setAttribute("title", "Download the file"); b.setAttribute("aria-label", "Download the file"); b.append(icon("download")); this.dlSlot.append(b); }
      }
    }

    // The Query pill looks like the other pill but opens a popover listing every query with its description and SPARQL.
    queryPill(title, line1, line2) {
      if (this.gh) { const btn = this.ghPill("query", title, line1, (title.match(/\((\d+)\)/) || [])[1] ?? null, null, () => this.qmenu.open(btn)); return btn; }
      const btn = h("button", { type: "button", class: "relative block w-full min-w-0 flex-1 px-3 py-2 text-left hover:bg-gray-50 md:border-r dark:border-gray-800 dark:hover:bg-gray-900", "aria-haspopup": "listbox", "aria-expanded": "false", title: line1, onclick: () => this.qmenu.open(btn) },
        h("div", { class: "text-gray-500" }, title),
        h("div", { class: "flex items-center overflow-hidden whitespace-nowrap" }, h("span", { class: "truncate" }, line1), h("span", { class: "mx-2 text-gray-500" }, "·"), h("span", { class: "text-gray-500" }, line2), icon("chevron")));
      return btn;
    }

    // description + collapsible, colourised SPARQL of the selected (or custom) query
    renderQBlock() {
      const el = this.qblock, s = this.s;
      if (!el) return;
      el.textContent = "";
      const custom = this.q === "custom";
      const ex = typeof this.q === "number" ? s.examples[this.q] : null;
      const text = custom ? this.lastRun : ex ? ex.sparql : null;
      if (this.openError || !s.meta || !text) { el.hidden = true; return; }
      el.hidden = false;
      if (custom) {
        el.append(h("div", { class: "flex items-center gap-2 text-sm text-gray-600 dark:text-gray-400" }, "Custom query.",
          h("button", { type: "button", class: "text-blue-600 hover:underline dark:text-blue-400", onclick: () => this.selectExample(this.lastExample || 0) }, "Reset to example")));
      } else if (ex.question) el.append(h("p", { class: "text-sm text-gray-600 dark:text-gray-400" }, ex.question));
      const cols = Object.entries(window.RetelensHints.parse(text).columns);
      if (cols.length) el.append(h("dl", { class: "rl-qcols", "data-rl": "qcols" }, cols.flatMap(([v, c]) => [h("dt", { title: "?" + v }, c.label), h("dd", null, c.note || "?" + v)])));
      const pre = h("pre", { "data-rl": "code", id: "rl-sparql-" + this.tok, class: "mt-1 max-h-56 overflow-auto whitespace-pre-wrap rounded-md border bg-gray-50 p-2 font-mono text-xs leading-snug dark:border-gray-800 dark:bg-gray-900", hidden: !this.sparqlOpen }, ...window.RetelensQuery.highlight(text));
      const toggle = h("button", { type: "button", class: "flex items-center gap-1 text-xs font-semibold text-gray-500 hover:text-gray-700 dark:hover:text-gray-300", "aria-expanded": String(this.sparqlOpen), "aria-controls": pre.id }, this.sparqlOpen ? "▾ SPARQL" : "▸ SPARQL");
      toggle.addEventListener("click", () => { this.sparqlOpen = !this.sparqlOpen; pre.hidden = !this.sparqlOpen; toggle.setAttribute("aria-expanded", String(this.sparqlOpen)); toggle.textContent = this.sparqlOpen ? "▾ SPARQL" : "▸ SPARQL"; });
      const copy = h("button", { type: "button", class: "btn shadow-xs rounded-sm border px-1.5 py-0.5 text-xs font-normal text-gray-700 dark:text-gray-300" }, "Copy");
      copy.addEventListener("click", async () => {
        try { await navigator.clipboard.writeText(text); }
        catch (_) { const t = h("textarea", { style: "position:fixed;opacity:0" }); t.value = text; document.body.append(t); t.select(); try { document.execCommand("copy"); } catch (_) {} t.remove(); }
        copy.textContent = "Copied"; setTimeout(() => (copy.textContent = "Copy"), 1200);
      });
      const edit = h("button", { type: "button", class: "btn shadow-xs rounded-sm border px-1.5 py-0.5 text-xs font-normal text-gray-700 dark:text-gray-300", onclick: () => this.editInConsole(text) }, "Edit in console");
      el.append(h("div", { class: "mt-1 flex items-center gap-2" }, toggle, h("div", { class: "ml-auto flex gap-1" }, copy, edit)), pre);
    }

    editInConsole(text) {
      if (this.mode === "box") return this.openStudioConsole();
      this.sideTab = "console";
      this.lastEditor = text;
      if (this.sidePanel && this.sidePanel.classList.contains("hidden")) this.toggleSide();
      this.buildSideTabs(); this.renderSide();
      if (this.editor) this.editor.focus();
    }

    renderPills() {
      const s = this.s;
      const files = s.files;
      const quads = s.meta?.info?.quads;
      const g = this.pill(`Graph (${files.length})`, this.choosing ? "Choose a graph" : s.file?.label || "-", this.choosing ? "" : quads != null ? num(quads) + " quads" : "…",
        files.map((f, i) => [String(i), f.label, i === s.idx]), (v) => this.switchFile(+v));
      let qLine1, qLine2 = this.result && this.result.length != null ? num(this.result.length) + (this.result.form === "SELECT" ? " rows" : " triples") : this.result?.form === "ASK" ? "ASK" : "…";
      const opts = s.examples.map((e, i) => [String(i), e.title, this.q === i]);
      if (this.q === "custom") { qLine1 = "Custom query"; opts.unshift(["custom", "Custom query", true]); }
      else if (this.q === "search") { qLine1 = "Search: " + (this.searchInput ? this.searchInput.value : ""); opts.unshift(["search", qLine1, true]); }
      else qLine1 = s.examples[this.q]?.title || "No example queries";
      this.qmenu.close();
      const qp = this.queryPill(`${s.starters ? "Starter queries" : "Query"} (${s.examples.length})`, qLine1, qLine2);
      this.pills.textContent = "";
      this.pills.append(g, qp);
      if (this.formatPill) this.formatPill.textContent = "rete" + (s.meta?.card?.format_version != null ? " 0x0" + Number(s.meta.card.format_version).toString(16) : "");
      if (this.studioLink) this.studioLink.setAttribute("href", this.o.studioHref ? this.o.studioHref(this.state()) : "#");
      this.renderQBlock();
      this.renderLinks();
    }

    // --- view switch + info
    renderToolbar() {
      this.toolbar.textContent = "";
      const mk = (id, label, ic) => {
        const b = h("button", { type: "button", class: "flex items-center gap-1 px-2 py-1 " + (this.view === id ? "bg-gray-100 font-semibold text-gray-900 dark:bg-gray-800 dark:text-gray-100" : "text-gray-500 hover:bg-gray-50 dark:hover:bg-gray-900") + (id !== "table" ? " border-l dark:border-gray-800" : ""), "data-view": id, "data-rl": "seg-item", "aria-pressed": String(this.view === id), onclick: () => this.setView(id, true) });
        b.append(icon(ic), label);
        return b;
      };
      this.toolbar.append(h("div", { "data-rl": "seg", class: "inline-flex overflow-hidden rounded-md border dark:border-gray-800", role: "group", "aria-label": "Result view" }, mk("table", "Table", "tableView"), mk("cards", "Cards", "cards"), mk("graph", "Graph", "graph"), this.mapAvailable() ? mk("map", "Map", "map") : null));
      const r = this.result;
      if (r) {
        const parts = [];
        if (r.length != null) parts.push(h("span", null, h("b", { class: "font-semibold text-gray-700 dark:text-gray-300" }, num(r.length)), r.form === "SELECT" || r.local ? " rows" : " triples"));
        if (r.ms != null) parts.push(h("span", { class: "text-gray-500" }, Math.round(r.ms) + " ms"));
        this.toolbar.append(h("div", { class: "ml-auto flex items-center gap-3 text-gray-500" }, parts));
      }
      if (r && r.hint) this.toolbar.append(h("div", { class: "basis-full text-amber-700 dark:text-amber-400" }, r.hint));
      const f = this.s.file;
      if (this.mediaBlocked && f && f.src && safeUrl(f.src)) {
        const open = h("button", { type: "button", "data-rl": "open-viewer", class: "btn shadow-xs ml-1 rounded-sm border px-2 py-0.5 text-xs font-normal text-gray-700 dark:text-gray-300",
          onclick: () => chrome.runtime.sendMessage({ retelens: "openViewer", src: f.src, name: f.label || f.path || "" }) }, "Open in the Retelens viewer");
        this.toolbar.append(h("div", { "data-rl": "media-blocked", class: "basis-full text-gray-500" }, "This site's page rules block pictures from other websites, so some are not shown here.", open));
      }
      for (const w of (r && r.warnings) || []) this.toolbar.append(h("div", { class: "basis-full text-gray-500" }, (w.severity === "hint" ? "Hint: " : "Warning: ") + (w.message || w.hint)));
    }

    // the Map view is offered when the result has a geometry (WKT, or latitude + longitude) or the file carries tiles
    mapAvailable() { return this.view === "map" || this.hasTiles || !!(this.result && this.result.geo); }
    // a click on a bar of a column's statistics: filter the column to that value or range (again: clear it)
    pickStat(v, f) {
      const m = this.model;
      if (!m) return;
      const keep = {};
      for (const [k, x] of Object.entries(m.filters[v] || {})) if (!["set", "ns", "min", "max", "from", "to"].includes(k)) keep[k] = x;
      m.setFilter(v, f ? { ...keep, ...f } : keep);
      if (m.changed) m.changed();
      this.apply();
    }
    setColType(v, ty) {
      if (!ty || ty === "auto") delete this.colTypes[v]; else this.colTypes[v] = ty;
      this.renderToolbar();
      this.renderResults();
    }
    setView(v, explicit) {
      if (this.view === v) return;
      this.view = v;
      if (explicit) this.explicitView = true;
      this.page = 1;
      this.writeUrl();
      this.renderToolbar();
      this.renderResults();
    }

    setStatus(extra) {
      Object.assign(this.status, extra);
      const st = this.status.stats;
      const parts = [];
      if (this.status.ms != null) parts.push(`query ${Math.round(this.status.ms)} ms`);
      if (st) parts.push(`fetched ${bytes(st.bytes)} of ${bytes(st.fileLength)} in ${st.requests} requests${st.engineRanges != null ? ` (engine asked ${st.engineRanges} ranges)` : ""}`);
      parts.push(ENGINE_LABEL);
      this.statusText.textContent = parts.join(" · ");
    }

    // hosts that only work with a known CDN next to them are asked for together, in one prompt
    permOrigins(origin) {
      const extra = { "https://github.com": ["https://release-assets.githubusercontent.com", "https://objects.githubusercontent.com", "https://raw.githubusercontent.com"] };
      return [origin, ...(extra[origin] || [])].map((o) => o + "/*");
    }
    errorBox(e) {
      let ask = null;
      if (e.code === "cors" && e.origin && safeUrl(e.origin)) {
        const host = new URL(e.origin).host;
        if (window.chrome && chrome.permissions && chrome.permissions.request) {
          ask = h("div", { class: "mt-2" }, h("div", null, `${host} doesn't allow cross-site reads. Allow retelens to read files from ${host}?`),
            h("button", { type: "button", class: "btn shadow-xs mt-1 rounded-sm border px-2 py-1 text-sm font-medium", onclick: async () => {
              let ok = false;
              try { ok = await chrome.permissions.request({ origins: this.permOrigins(e.origin) }); } catch (_) {}
              if (ok) { this.openError = null; this.s.idx = -1; this.area.textContent = ""; this.start(); }
            } }, `Allow ${host}`));
        } else ask = h("div", { class: "mt-2" }, `${host} doesn't allow cross-site reads from this page. Open the file in the retelens viewer tab (toolbar icon) to grant access.`);
      }
      return h("div", { class: "m-3 rounded-lg border border-red-200 bg-red-50 p-3 text-sm text-red-800 dark:border-red-900 dark:bg-red-950 dark:text-red-300" },
        h("div", null, e.message || String(e)), ask,
        e.detail && e.detail !== e.message ? h("details", { class: "mt-1 text-xs" }, h("summary", { class: "cursor-pointer" }, "Technical detail"), h("pre", { class: "whitespace-pre-wrap" }, String(e.detail))) : null);
    }

    // --- lifecycle
    // chosen: the file the user picked in the chooser. Several files and none named in the URL: ask which one first,
    // nothing is opened before (renderChooser).
    async start(chosen) {
      const s = this.s;
      const want = this.params.get("file");
      if (chosen != null) this.chosenIdx = chosen;
      if (this.chosenIdx == null && !want && s.files.length > 1) return this.renderChooser();
      let i = this.chosenIdx != null ? this.chosenIdx : want ? s.files.findIndex((f) => f.path === want) : 0;
      if (i < 0) i = 0;
      const v = this.params.get("view");
      if (v === "table" || v === "cards" || v === "graph" || v === "map") { this.view = v; this.explicitView = true; }
      const qi = parseInt(this.params.get("q") || "0", 10);
      if (this.params.get("console") === "1" && this.sidePanel) this.sideTab = "console";
      if (this.sidePanel) this.buildSideTabs();
      this.renderPills();
      this.area.append(this.loadingBox(s.files[i]?.label || "the file"));
      try { if (!(s.idx === i && s.meta)) await s.open(i); }
      catch (e) { this.area.textContent = ""; this.area.append(this.errorBox(e)); this.openError = e; this.renderPills(); this.renderToolbar(); if (this.sideBody) this.renderSide(); return; }
      this.q = Number.isInteger(qi) && qi >= 0 && qi < s.examples.length ? qi : 0;
      this.afterOpen();
      if (s.examples.length) await this.selectExample(this.q, true);
      else {
        // no example queries in the card: show a sample of the data so the viewer is not empty
        this.q = "custom";
        this.lastEditor = "SELECT ?s ?p ?o WHERE { ?s ?p ?o } LIMIT 100";
        await this.run(this.lastEditor);
      }
      if (this.sideBody) this.renderSide();
      this.started = true;
    }
    afterOpen() {
      this.setStatus({ stats: this.s.meta.stats }); this.renderPills(); if (this.sideBody) this.renderSide();
      // tiles embedded in the file (lib/maps.js): looked for once, in the background
      this.hasTiles = false;
      const file = this.s.file;
      this.s.tiles().then((t) => { if (t && this.s.file === file) { this.hasTiles = true; this.renderToolbar(); } });
    }

    // what the viewer shows while a file opens: the engine starting (the first time), then the file's header and card
    loadingBox(label) {
      return h("div", { class: "rl-ph p-4", "data-rl": "loading", role: "status", "aria-live": "polite" },
        h("div", { class: "text-sm font-medium" }, "Opening " + label + "…"),
        h("div", { class: "text-xs text-gray-500" }, "Retelens is bringing up the graph viewer: the rete engine reads the file's header and card, then its first query. Nothing is downloaded: a query reads only the bytes it needs."),
        h("div", { class: "rl-ph-bars", "aria-hidden": "true" }, h("span"), h("span"), h("span"), h("span")));
    }

    // ---- the file chooser: the files, each with its card on request (About reads only the card: a few kB)
    renderChooser() {
      const s = this.s, files = s.files;
      this.choosing = true;
      this.renderPills();
      if (this.sideBody) this.renderSide();
      const where = this.skinName === "github" ? "repository" : this.skinName === "zenodo" ? "record" : files.some((f) => /huggingface\.co\/buckets\//.test(f.src || "")) ? "bucket" : "dataset";
      const list = h("ul", { class: "rl-choices", "data-rl": "choices" });
      const filter = files.length > 8 ? h("input", { type: "search", placeholder: "Filter the files", "aria-label": "Filter the files", class: "rl-choose-filter" }) : null;
      const box = h("div", { class: "rl-chooser", "data-rl": "chooser" },
        h("div", { class: "rl-choose-q" }, `This ${where} has ${files.length} graphs. Which one do you want to open?`),
        h("div", { class: "rl-muted rl-choose-note" }, "No graph is opened until you choose one. About reads only that file's card (a few kB): its title, description, licence and size."),
        filter, list);
      // the default file first, then by path (the session keeps its own order: indexes stay the files')
      const order = files.map((f, i) => [f, i]).sort((a, b) => (b[0].default ? 1 : 0) - (a[0].default ? 1 : 0) || String(a[0].label || a[0].path).localeCompare(String(b[0].label || b[0].path), undefined, { numeric: true }));
      for (const [f, i] of order) list.append(this.choiceRow(f, i));
      if (filter) filter.addEventListener("input", () => {
        const w = filter.value.trim().toLowerCase();
        for (const li of list.children) li.hidden = !!w && !li.dataset.name.includes(w);
      });
      this.area.textContent = "";
      this.area.append(box);
      // the file the dataset names as its default, its card shown already
      if (order.length && order[0][0].default) list.children[0].querySelector("[data-rl=about]").click();
    }
    choiceRow(f, i) {
      const s = this.s;
      const parts = String(f.label || f.path || "file").split("/");
      const name = parts.pop(), folder = parts.join("/");
      const info = h("div", { class: "rl-choice-info", "data-rl": "choice-info", hidden: true });
      const sizeEl = h("span", { class: "rl-muted rl-num" }, f.size ? bytes(f.size) : "");
      const about = h("button", { type: "button", class: "rl-btnlink", "data-rl": "about", "aria-expanded": "false" }, "About");
      const open = h("button", { type: "button", class: "rl-btnlink rl-choice-open", "data-rl": "open-choice" }, "Open");
      const li = h("li", { class: "rl-choice", "data-rl": "choice", "data-name": String(f.label || f.path || "").toLowerCase() },
        h("div", { class: "rl-choice-row" },
          h("div", { class: "rl-choice-name" }, h("span", { class: "rl-choice-file" }, name), folder ? h("span", { class: "rl-muted" }, " " + folder + "/") : null,
            f.default ? h("span", { class: "rl-typetag", title: "The file this dataset's README names as its default (configs: default: true)" }, "default") : null),
          sizeEl, about, open),
        info);
      let read = null;
      about.addEventListener("click", () => {
        const show = info.hidden;
        info.hidden = !show; about.setAttribute("aria-expanded", String(show));
        if (!show || read) return;
        if (!f.src) { info.append(h("div", { class: "rl-muted" }, "Open it to read its card.")); read = true; return; }
        info.append(h("div", { class: "rl-muted" }, "Reading the card…"));
        read = s.engine.peekCard(f.src).then((r) => {
          info.textContent = "";
          const c = r.card || {};
          if (r.size && !f.size) sizeEl.textContent = bytes(r.size);
          if (c.title) info.append(h("div", { class: "rl-choice-title" }, String(c.title)));
          if (c.description) {
            const md = window.RetelensCells.markdown(String(c.description).slice(0, 4000));
            md.classList.add("rl-choice-desc");
            info.append(md);
          } else info.append(h("div", { class: "rl-muted" }, "This file's card has no description."));
          const facts = [];
          const n = c.quad_count != null ? c.quad_count : c.triple_count != null ? c.triple_count : r.info && r.info.quads;
          if (n != null) facts.push(num(n) + " statements");
          const terms = c.term_count != null ? c.term_count : r.info && r.info.terms;
          if (terms != null) facts.push(num(terms) + " terms");
          if (c.graph_count) facts.push(num(c.graph_count) + " named graphs");
          if (c.examples) facts.push(c.examples + " example quer" + (c.examples === 1 ? "y" : "ies"));
          if (c.license) facts.push("licence " + String(c.license));
          if (c.source_date) facts.push("source " + String(c.source_date));
          if (c.format_version != null) facts.push("format 0x0" + Number(c.format_version).toString(16));
          if (facts.length) info.append(h("div", { class: "rl-muted rl-choice-facts" }, facts.join(" · ")));
          const src = safeUrl(c.source);
          if (src) info.append(h("div", { class: "rl-choice-facts" }, h("a", { href: src, class: "rl-msrc" }, "Source ↗")));
          if (Array.isArray(c.keywords) && c.keywords.length) info.append(h("div", { class: "rl-qtags" }, ...c.keywords.slice(0, 12).map((k) => h("span", { class: "rl-qtag" }, String(k)))));
        }, (e) => { info.textContent = ""; info.append(h("div", { class: "rl-err" }, "The card could not be read: " + String(e.message || e).slice(0, 200))); });
      });
      open.addEventListener("click", () => this.choose(i));
      return li;
    }
    choose(i) {
      this.choosing = false;
      this.area.textContent = "";
      return this.start(i);
    }

    async switchFile(i) {
      const s = this.s;
      if (this.choosing) return this.choose(i); // picked from the Graph menu while the chooser shows
      this.page = 1; this.q = 0; this.result = null; this.openError = null; this.model = null; this.colMenu.close(); this.fbar.update();
      this.gv && this.gv.destroy(); this.gv = null; this.mv && this.mv.destroy(); this.mv = null;
      this.area.textContent = ""; this.area.append(this.loadingBox(s.files[i].label));
      s.idx = i; this.renderPills();
      try { await s.open(i); } catch (e) { this.area.textContent = ""; this.area.append(this.errorBox(e)); this.openError = e; this.renderPills(); this.renderToolbar(); this.renderSide && this.sideBody && this.renderSide(); this.writeUrl(); return; }
      this.explicitView = this.explicitView; this.writeUrl();
      this.afterOpen();
      if (s.examples.length) await this.selectExample(0, true);
    }

    async selectExample(i, initial) {
      const ex = this.s.examples[i];
      if (!ex) return;
      if (!(initial && this.explicitView) && ex.view) this.pendingView = ex.view;
      this.q = i;
      this.lastExample = i;
      if (this.searchInput) this.searchInput.value = "";
      if (this.editor && !initial) this.editor.value = ex.sparql; else if (this.editor) this.editor.value = ex.sparql;
      this.writeUrl();
      await this.run(ex.sparql);
    }

    async runCustom(text) {
      const hinted = window.RetelensHints.parse(text).view; // "# view: map" in a console query picks the view too
      if (hinted) this.pendingView = hinted;
      this.q = "custom";
      this.writeUrl();
      await this.run(text, { force: true }); // the user pressed Run: no guard
    }

    async runSearch(text) {
      text = text.trim();
      if (!text) return this.selectExample(0);
      const tok = ++this.tok;
      this.q = "search"; this.writeUrl();
      this.area.textContent = ""; this.area.append(h("div", { class: "p-4 text-sm text-gray-500" }, "Searching…"));
      const t0 = performance.now();
      let res;
      try { res = await this.s.engine.search(text, 500); } catch (e) { this.area.textContent = ""; this.area.append(this.errorBox(e)); return; }
      if (tok !== this.tok) return;
      const vars = res.kind === "prefix" ? ["label", "subject"] : ["subject"];
      const rows = res.rows.map((r) => ({ ...(r.label != null ? { label: { kind: "literal", value: r.label } } : {}), subject: { kind: "iri", value: r.subject } }));
      await this.freeResult();
      this.result = { form: "SELECT", local: true, vars, length: rows.length, rows, ms: performance.now() - t0, warnings: [], stats: res.stats, hint: rows.length ? null : "No match. Search is by label prefix, then by words in literals." };
      this.view = this.explicitView ? this.view : "table";
      this.page = 1;
      this.model = null; this.newModel(this.result);
      this.setStatus({ ms: this.result.ms, stats: res.stats });
      this.renderPills(); this.renderToolbar(); await this.renderResults();
    }

    async freeResult() {
      const r = this.result;
      this.result = null;
      if (r && r.handle != null) this.s.engine.free(r.handle).catch(() => {});
    }

    // A CONSTRUCT or DESCRIBE without a LIMIT over a big file is never started on its own: the whole graph
    // can be slow to build or run out of memory. The hint comes first; "Run anyway" (or the console's Run) starts it.
    constructGuard(query, opts) {
      const form = formOf(query);
      const limited = /\bLIMIT\s+\d+\s*$/i.test(query.replace(/#[^\n]*/g, " "));
      return (form === "CONSTRUCT" || form === "DESCRIBE") && !limited && this.s.fileLength > BIG_FILE && !(opts && opts.force);
    }
    async run(query, opts) {
      const tok = ++this.tok;
      this.lastRun = query;
      this.s.prefixes.learn(query);
      await this.freeResult();
      this.model = null; this.loading = null; this.rewritten = null; this.colMenu.close();
      if (this.constructGuard(query, opts)) {
        this.gv && this.gv.destroy(); this.gv = null; this.mv && this.mv.destroy(); this.mv = null;
        this.area.textContent = ""; this.pager.textContent = ""; this.pager.hidden = true;
        this.renderPills(); this.renderToolbar(); this.fbar.update();
        this.area.append(h("div", { "data-rl": "construct-hint", class: "m-3 rounded-lg border border-amber-200 bg-amber-50 p-3 text-sm text-amber-800 dark:border-amber-900 dark:bg-amber-950 dark:text-amber-300" },
          h("div", null, `This CONSTRUCT has no LIMIT and the file is ${bytes(this.s.fileLength)}. Building the whole graph can be slow or run out of memory on a big file, so it was not started.`),
          h("div", { class: "mt-1" }, "Add LIMIT n to see the first triples; the Graph view draws them."),
          h("button", { type: "button", "data-rl": "run-anyway", class: "btn shadow-xs mt-2 rounded-sm border px-2 py-1 text-sm font-medium", onclick: () => this.run(query, { force: true }) }, "Run anyway")));
        return;
      }
      this.area.textContent = ""; this.area.append(h("div", { class: "p-4 text-sm text-gray-500" }, "Running…"));
      let res;
      try { res = await this.s.engine.run(query); }
      catch (e) { if (tok !== this.tok) return; this.gv && this.gv.destroy(); this.gv = null; this.mv && this.mv.destroy(); this.mv = null; this.area.textContent = ""; this.area.append(this.errorBox(e)); this.pager.textContent = ""; this.renderPills(); this.renderToolbar(); return; }
      if (tok !== this.tok) { if (res.handle != null) this.s.engine.free(res.handle); return; }
      res.query = query;
      if ((res.form === "CONSTRUCT" || res.form === "DESCRIBE") && this.s.fileLength > BIG_FILE) {
        res.hint = "CONSTRUCT over a large file can be slow or fail on this engine version. SELECT ?s ?p ?o … LIMIT n gives the same triples and the Graph view draws it just as well.";
      }
      this.result = res;
      if (this.pendingView) { this.view = res.form === "ASK" ? "table" : this.pendingView; this.pendingView = null; }
      else if (!this.explicitView) this.view = res.form === "SELECT" ? "table" : "graph";
      this.page = 1;
      this.setStatus({ ms: res.ms, stats: res.stats });
      this.newModel(res);
      this.renderPills(); this.renderToolbar();
      if (res.form === "ASK") { this.gv && this.gv.destroy(); this.gv = null; this.mv && this.mv.destroy(); this.mv = null; this.area.textContent = ""; this.area.append(h("div", { class: "p-4 text-2xl font-semibold" }, res.boolean ? "true" : "false")); this.pager.textContent = ""; return; }
      await this.renderResults();
      if (tok !== this.tok) return;
      const m = this.model;
      if (m && !m.complete && !m.rows.length && res.length != null && res.length <= R.FULL_MAX && !res.local) this.loadRows(res, res.length);
      else if (m && m.active() && !m.rows.length) this.ensureLoaded(); // a state restored from the URL
    }

    // ---- search / filter / sort state of the current result (lib/results.js)
    newModel(res) {
      if (!res.vars || res.form === "ASK") { this.model = null; this.fbar.update(); return; }
      const m = (this.model = new R.ResultModel(res.vars, this.short));
      if (res.local) m.setRows(res.rows, res.rows.length, true);
      else m.total = res.length;
      if (this.urlState) { const st = this.urlState; this.urlState = null; m.fromParams((k) => st[k]); }
      this.writeUrl();
    }
    // all rows when the result has FULL_MAX or fewer, else the first WINDOW rows, fetched in batches with progress
    async loadRows(res, cap) {
      const m = this.model;
      if (!m || res.handle == null) return;
      const tok = this.tok, total = Math.min(res.length, cap);
      this.loading = { n: 0, total };
      this.fbar.update();
      let acc = [];
      try {
        for (let off = 0; off < total; off += R.BATCH) {
          const pg = await this.s.engine.page(res.handle, off, Math.min(R.BATCH, total - off));
          if (this.model !== m || tok !== this.tok) return;
          acc = acc.concat(pg.rows);
          this.loading = { n: acc.length, total };
          this.fbar.update();
        }
      } catch (e) { if (this.model === m) { this.loading = null; this.fbar.update(); } return; }
      m.setRows(acc, res.length, acc.length >= res.length);
      m.loadedWindow = true;
      this.loading = null;
      await this.apply(true);
    }
    // rows are needed (search, filter, sort, facets): load them if they are not
    async ensureLoaded() {
      const m = this.model, r = this.result;
      if (!m || m.complete || m.loadedWindow || r.local) return;
      if (this.loading) { while (this.loading && this.model === m) await new Promise((res) => setTimeout(res, 100)); return; }
      await this.loadRows(r, r.length <= R.FULL_MAX ? r.length : R.WINDOW);
    }
    wantsModel() { const m = this.model; return !!m && (m.active() || !!m.sort || (this.facetsOpen && this.view === "cards")); }
    useModel() { const m = this.model; return !!m && m.rows.length > 0 && (m.complete || this.wantsModel()); }
    windowActive() { const m = this.model; return !!m && !m.complete && !!m.loadedWindow && this.wantsModel(); }
    canAddFilter() { const r = this.result; return !!r && r.form === "SELECT" && !r.local && !!r.query; }
    isCards() { return this.view === "cards"; }
    // the model changed (typing, chips, sort, facets): persist, re-render
    async apply(fromLoad) {
      const m = this.model;
      if (!m) return;
      this.writeUrl();
      if (!fromLoad && this.wantsModel() && !m.complete && !m.loadedWindow) { await this.ensureLoaded(); return; }
      this.page = 1;
      await this.renderResults();
    }
    toggleSort(v) {
      const m = this.model; if (!m) return;
      const cur = m.sort && m.sort.v === v ? m.sort.dir : null;
      m.sort = cur === null ? { v, dir: "asc" } : cur === "asc" ? { v, dir: "desc" } : null;
      m.changed();
      this.apply();
    }
    async openColumnMenu(btn, v) {
      await this.ensureLoaded();
      if (!this.model || !this.model.rows.length) return;
      const anchor = btn.isConnected ? btn : this.area.querySelector('[data-rl="colfilter"][data-col="' + CSS.escape(v) + '"]') || btn;
      this.colMenu.open(anchor, v, { model: this.model, short: this.short, setFilter: (col, f) => { this.model.setFilter(col, f); this.apply(); },
        type: { current: this.colTypes[v] || "auto", auto: window.RetelensCells.columnAuto(this.model.rows, v), set: (ty) => this.setColType(v, ty) } });
    }
    async toggleFacets() { this.facetsOpen = !this.facetsOpen; if (this.facetsOpen) await this.ensureLoaded(); this.fbar.update(); await this.renderResults(); }
    // "Add as FILTER to the query": the rewritten SPARQL goes to the console for the user to run; nothing runs by itself
    addAsFilter() {
      const text = R.filterSparql(this.result.query, this.model);
      if (this.mode === "box") { this.rewritten = text; this.fbar.update(); return; }
      this.editInConsole(text);
    }

    async fetchRows(offset, limit) {
      const r = this.result;
      if (r.local) return { rows: r.rows.slice(offset, offset + limit) };
      const pg = await this.s.engine.page(r.handle, offset, limit);
      return pg;
    }

    async renderResults() {
      const r = this.result;
      if (!r || r.form === "ASK") return;
      const tok = this.tok;
      const view = this.view;
      const m = this.model;
      const size = view === "cards" ? 36 : view === "graph" ? (r.form === "SELECT" ? 200 : 800) : view === "map" ? 2000 : 100;
      const whole = view === "graph" || view === "map"; // one batch from the start, no pager
      const local = this.useModel();
      let pg, total;
      if (local) {
        const all = m.viewRows();
        total = all.length;
        pg = { rows: whole ? all.slice(0, size) : all.slice((this.page - 1) * size, this.page * size) };
      } else {
        total = r.length;
        const offset = whole ? 0 : (this.page - 1) * size;
        try { pg = await this.fetchRows(offset, size); }
        catch (e) { this.area.textContent = ""; this.area.append(this.errorBox(e)); return; }
        if (tok !== this.tok || view !== this.view) return;
        if (pg.stats) this.setStatus({ stats: pg.stats });
        if (pg.warnings && pg.warnings.length) { r.warnings = pg.warnings; this.renderToolbar(); }
      }
      this.gv && this.gv.destroy(); this.gv = null; this.mv && this.mv.destroy(); this.mv = null;
      this.area.textContent = "";
      const hl = local && m.q ? m.q : "";
      const o = { model: m, hl, filtered: local && m.active(), onSort: (v) => this.toggleSort(v), onFilter: (b, v) => this.openColumnMenu(b, v), maxHeight: this.mode === "box" ? "24rem" : null,
        types: this.colTypes, setType: (v, ty) => this.setColType(v, ty), colInfo: r.query ? window.RetelensHints.parse(r.query).columns : {},
        hidden: this.cardHidden, setHidden: (v, hide) => { if (hide) this.cardHidden.add(v); else this.cardHidden.delete(v); this.renderResults(); } };
      let comp;
      if (view === "cards") comp = V.CardsView(pg.rows, r.vars, this.ctx, { ...o, facets: this.facetsOpen && local ? UI.FacetsPanel(this) : null });
      else if (view === "graph") {
        try { await window.RetelensGraph.ensure(); } catch (e) { this.area.append(this.errorBox(e)); return; }
        if (tok !== this.tok || view !== this.view) return;
        comp = this.gv = window.RetelensGraph.GraphView({ form: r.form, length: total }, pg.rows, r.vars, this.ctx);
      }
      else if (view === "map") {
        comp = this.mv = window.RetelensMaps.MapView(pg.rows, r.vars, this.ctx, { tiles: () => this.s.tiles(), height: this.mode === "box" ? "24rem" : null, total });
      }
      else {
        // column statistics (lib/stats.js) on every loaded row, or on this page until they are loaded
        const statRows = m && m.rows.length ? m.rows : pg.rows;
        const partial = m && m.complete ? 0 : statRows.length;
        o.stats = (v) => window.RetelensStats.chart(window.RetelensStats.summarize(statRows, v, this.short), { current: m && m.filters[v], partial, onPick: m ? (f) => this.pickStat(v, f) : null });
        comp = V.TableView(pg.rows, r.vars, this.ctx, o);
      }
      this.area.append(comp.el);
      this.renderPager(total, size, whole);
      // a geometry noticed in the rows: offer the Map view
      if (!r.geo && r.vars && view !== "map" && window.RetelensMaps.geoColumns(r.vars, pg.rows)) { r.geo = true; this.renderToolbar(); }
      this.lastPageMs = pg.ms;
      this.fbar.update();
      this.colMenu.reanchor(this.area);
      this.afterRender && this.afterRender(view, comp);
    }

    renderPager(total, size, hide) {
      this.pager.textContent = "";
      this.pager.hidden = hide || total <= size;
      if (hide || total <= size) return;
      const pages = Math.ceil(total / size);
      const p = this.page;
      const go = (n) => { this.page = n; this.renderResults(); };
      const btn = (label, n, opts = {}) => h("button", { type: "button", disabled: opts.disabled, "aria-current": opts.current ? "page" : null, class: "rounded-sm px-2 py-0.5 " + (opts.current ? "bg-gray-100 font-semibold ring-1 ring-gray-200 dark:bg-gray-800 dark:ring-gray-700" : "hover:bg-gray-100 dark:hover:bg-gray-800") + (opts.disabled ? " cursor-default opacity-40" : ""), onclick: () => !opts.disabled && go(n) }, label);
      this.pager.append(btn("‹ Previous", p - 1, { disabled: p <= 1 }));
      const set = new Set([1, 2, pages - 1, pages, p - 1, p, p + 1].filter((n) => n >= 1 && n <= pages));
      let last = 0;
      for (const n of [...set].sort((a, b) => a - b)) {
        if (n - last > 1) this.pager.append(h("span", { class: "px-1 text-gray-400" }, "…"));
        this.pager.append(btn(String(n), n, { current: n === p }));
        last = n;
      }
      this.pager.append(btn("Next ›", p + 1, { disabled: p >= pages }));
      this.pager.append(h("span", { class: "ml-2 text-xs text-gray-400" }, `rows ${num((p - 1) * size + 1)}–${num(Math.min(p * size, total))} of ${num(total)}`));
    }

    // --- side panel
    toggleSide() {
      const hide = !this.sidePanel.classList.contains("hidden");
      this.sidePanel.classList.toggle("hidden", hide);
      if (this.resizer) this.resizer.classList.toggle("hidden", hide);
      this.sideToggle.setAttribute("aria-label", hide ? "Show sidepanel" : "Hide sidepanel");
      this.gv && this.gv.resize && setTimeout(() => this.gv && this.gv.resize(), 50);
    }
    buildSideTabs() {
      this.sideTabs.textContent = "";
      if (this.gh) {
        // Primer UnderlineNav: nav > ul.UnderlineNav-body > li > .UnderlineNav-item (+ Counter)
        const item = (id, label, count) => h("li", { class: "UnderlineNav-li", role: "presentation" }, h("button", { type: "button", role: "tab", "data-tab": id, "data-rl": "utab", class: "UnderlineNav-item" + (this.sideTab === id ? " selected" : ""), "aria-selected": String(this.sideTab === id), "aria-current": this.sideTab === id ? "page" : null, onclick: () => { this.sideTab = id; this.buildSideTabs(); this.renderSide(); } },
          h("span", { "data-content": label }, label), count != null ? h("span", { class: "Counter" }, num(count)) : null));
        const classes = this.schemaCache && this.schemaCache.idx === this.s.idx ? this.schemaCache.v.classes.length : null;
        this.sideTabs.append(h("ul", { class: "UnderlineNav-body", role: "tablist", "aria-label": "Side panel" }, item("examples", "Examples", this.s.examples.length || null), item("console", "SPARQL console"), item("schema", "Schema", classes), item("card", "Card")));
        return;
      }
      const tab = (id, label, pill) => h("button", { type: "button", "data-tab": id, class: "flex min-h-[2.5rem] flex-1 items-center justify-center gap-2 px-3 py-3 text-sm font-medium transition-colors " + (this.sideTab === id ? "border-b-2 border-black text-black dark:border-white dark:text-white" : "text-gray-500 hover:text-gray-700 dark:text-gray-400 dark:hover:text-gray-300"), onclick: () => { this.sideTab = id; this.buildSideTabs(); this.renderSide(); } },
        pill ? h("span", { class: "rounded px-2 py-0.5 font-mono text-xs font-semibold leading-none bg-gray-200 text-gray-600 dark:bg-gray-700 dark:text-gray-400" }, pill) : null, h("span", null, label));
      this.sideTabs.append(tab("examples", "Examples"), tab("console", "Console", "SPARQL"), tab("schema", "Schema"), tab("card", "Card"));
    }
    renderSide() {
      const b = this.sideBody, s = this.s;
      b.textContent = "";
      if (this.openError) { b.append(h("div", { class: "p-4 text-sm text-gray-500" }, "The file could not be opened.")); return; }
      if (this.choosing) { b.append(h("div", { class: "p-4 text-sm text-gray-500" }, "Choose a graph to see its example queries, its schema and its card.")); return; }
      if (!s.meta) { b.append(h("div", { class: "p-4 text-sm text-gray-500" }, "Loading…")); return; }
      if (this.sideTab === "examples") {
        if (!s.examples.length) b.append(h("div", { class: "p-4 text-sm text-gray-500" }, "This file has no example queries."));
        // a filter over the queries (words, and a view), shared with the Query pill's list; the list redraws as it is typed
        const Q = window.RetelensQuery, qf = this.qf || (this.qf = { text: "", view: null });
        const list = h("div", { "data-rl": "examples" });
        const views = h("div");
        const input = h("input", { type: "search", "data-rl": "qfilter", class: "h-8 w-full min-w-0 rounded-md border border-gray-200 bg-white px-2 text-sm outline-none placeholder:text-gray-400 focus:border-gray-400 dark:border-gray-800 dark:bg-gray-900", placeholder: "Filter queries: words, a column…", "aria-label": "Filter the example queries" });
        input.value = qf.text;
        if (s.examples.length > 3) b.append(h("div", { class: "space-y-1.5 border-b px-3 py-2 dark:border-gray-800" }, input, views));
        const draw = () => {
          views.textContent = "";
          views.append(Q.viewChips(s.examples, qf, (v) => { qf.view = v; draw(); }));
          list.textContent = "";
          const f = (qf.text.trim() || qf.view) ? qf : null;
          let g = null, shown = 0;
          s.examples.forEach((ex, i) => {
            if (f && !Q.matches(ex, f)) return;
            shown++;
            const grp = ex.starter ? "Starter queries" : ex.curated ? "Card examples" : ex.group;
            if (grp !== g) { g = grp; list.append(h("div", { class: "bg-gray-50 px-3 py-1 text-xs uppercase tracking-wide text-gray-400 dark:bg-gray-900" }, grp)); }
            list.append(h("button", { type: "button", class: "block w-full border-b px-3 py-2 text-left hover:bg-gray-50 dark:border-gray-800 dark:hover:bg-gray-900 " + (this.q === i ? "bg-gray-50 dark:bg-gray-900" : ""), onclick: () => this.selectExample(i) },
            h("div", { class: "text-sm font-medium" }, ex.title), ex.question ? h("div", { class: "text-xs text-gray-500" }, ex.question) : null,
              window.RetelensHints.chips(ex, h, icon)));
          });
          if (f && !shown) list.append(h("div", { class: "p-4 text-sm text-gray-500" }, "No query matches."));
        };
        input.addEventListener("input", () => { qf.text = input.value; draw(); });
        draw();
        b.append(list);
      } else if (this.sideTab === "console") {
        // coloured, with line numbers (lib/editor.js); this.editor stays the textarea
        const ed = window.RetelensEditor.create({ label: "SPARQL query" });
        this.editor = ed.textarea;
        this.editor.value = this.lastEditor ?? (typeof this.q === "number" ? s.examples[this.q]?.sparql || "" : "SELECT ?s ?p ?o WHERE { ?s ?p ?o } LIMIT 20");
        const run = () => { this.lastEditor = this.editor.value; if (this.editor.value.trim()) this.runCustom(this.editor.value); };
        this.editor.addEventListener("input", () => (this.lastEditor = this.editor.value));
        this.editor.addEventListener("keydown", (e) => {
          if ((e.ctrlKey || e.metaKey) && e.key === "Enter") { e.preventDefault(); run(); }
          if (e.key === "Tab" && !e.shiftKey) { e.preventDefault(); const t = e.target; t.setRangeText("  ", t.selectionStart, t.selectionEnd, "end"); }
        });
        const hasLimit = () => /\bLIMIT\b/i.test(this.editor.value);
        const hint = h("div", { class: "text-xs text-gray-400" });
        this.editor.addEventListener("input", () => { hint.textContent = /\bSELECT\b/i.test(this.editor.value) && !hasLimit() ? "No LIMIT: every row is computed (the table pages it). The query is never changed for you." : ""; });
        b.append(h("div", { class: "p-3" }, ed.el,
          h("div", { class: "mt-2 flex items-center gap-3" }, h("button", { type: "button", class: "btn shadow-xs rounded-sm border px-3 py-1 text-sm font-medium", onclick: run }, "Run ", h("span", { class: "ml-1 text-xs text-gray-400" }, "Ctrl+Enter")), hint)));
      } else if (this.sideTab === "schema") {
        b.append(h("div", { class: "p-4 text-sm text-gray-500" }, "Reading schema…"));
        const idx = s.idx;
        (this.schemaCache && this.schemaCache.idx === idx ? Promise.resolve(this.schemaCache.v) : s.engine.schema().then((v) => (this.schemaCache = { idx, v }, this.gh && this.buildSideTabs(), v))).then((sc) => {
          if (this.sideTab !== "schema") return;
          b.textContent = "";
          b.append(h("div", { class: "px-3 pb-1 pt-3 text-xs uppercase tracking-wide text-gray-400" }, "Classes · click to list instances"));
          for (const [c, n] of [...sc.classes].sort((a, z) => z[1] - a[1])) {
            const ok = IRI_OK.test(c);
            b.append(h("button", { type: "button", disabled: !ok, class: "flex w-full items-center justify-between gap-2 border-b px-3 py-1.5 text-left text-sm hover:bg-gray-50 dark:border-gray-800 dark:hover:bg-gray-900", onclick: () => ok && this.runCustom(`SELECT ?s WHERE { ?s a <${c}> } LIMIT 100`) },
              h("span", { class: "min-w-0 truncate font-mono text-xs", title: c }, s.prefixes.shorten(c)), h("span", { class: "flex-none text-gray-500" }, num(n))));
          }
          if (!sc.classes.length) b.append(h("div", { class: "p-3 text-sm text-gray-500" }, "No classes."));
          b.append(h("div", { class: "px-3 pb-1 pt-4 text-xs uppercase tracking-wide text-gray-400" }, "Relations · subject class, predicate, object class"));
          for (const [a, p, o, n] of [...sc.relations].sort((x, y) => y[3] - x[3]).slice(0, 100)) {
            b.append(h("div", { class: "flex items-baseline justify-between gap-2 border-b px-3 py-1 text-xs dark:border-gray-800" }, h("span", { class: "min-w-0 break-words font-mono" }, [a, p, o].map((x) => s.prefixes.shorten(x)).join("  ")), h("span", { class: "flex-none text-gray-500" }, num(n))));
          }
        }).catch((e) => { b.textContent = ""; b.append(this.errorBox(e)); });
      } else {
        const box = h("div", { class: "p-4" });
        window.RetelensCardSidebar.render(box, s.cardData());
        b.append(box);
      }
    }
  }

  window.RetelensExplorer = { Explorer, Session, formOf };
})();
