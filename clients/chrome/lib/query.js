// Query helpers: SPARQL syntax colouring (plain spans, textContent only), previews, and the
// Query popover (a keyboard-accessible listbox with a filter box) used by the Query pill.
// The popover is portaled to <body> inside a [data-retelens] wrapper so no host overflow clips it.
(function () {
  const { h } = window.RetelensFmt;

  const KEYWORDS = new Set(("SELECT DISTINCT REDUCED WHERE FROM NAMED CONSTRUCT DESCRIBE ASK PREFIX BASE OPTIONAL FILTER UNION MINUS GRAPH SERVICE " +
    "BIND VALUES GROUP BY ORDER HAVING LIMIT OFFSET ASC DESC AS IN NOT EXISTS COUNT SUM AVG MIN MAX SAMPLE GROUP_CONCAT STR LANG DATATYPE " +
    "BOUND IF COALESCE REGEX CONTAINS STRSTARTS STRENDS LCASE UCASE ISIRI ISURI ISBLANK ISLITERAL ISNUMERIC STRLEN SUBSTR REPLACE").split(" "));
  const TOKEN = /(#[^\n]*)|(<[^<>\s"{}|\\^`]*>)|("(?:\\.|[^"\\\n])*"(?:@[\w-]+)?|'(?:\\.|[^'\\\n])*')|([?$][A-Za-z_]\w*)|([A-Za-z_]\w*)|(\d+(?:\.\d+)?)/g;
  const CLS = ["", "text-gray-400", "text-green-700 dark:text-green-400", "text-amber-700 dark:text-amber-400", "text-blue-700 dark:text-blue-300", "", "text-purple-700 dark:text-purple-400"];

  // -> array of text nodes / spans
  function highlight(src) {
    const out = [];
    let last = 0;
    for (const m of String(src).matchAll(TOKEN)) {
      if (m.index > last) out.push(src.slice(last, m.index));
      let gi = 1;
      while (gi < 7 && m[gi] === undefined) gi++;
      let cls = CLS[gi];
      if (gi === 5) cls = KEYWORDS.has(m[0].toUpperCase()) ? "font-semibold text-purple-700 dark:text-purple-400" : "";
      out.push(cls ? h("span", { class: cls }, m[0]) : m[0]);
      last = m.index + m[0].length;
    }
    if (last < src.length) out.push(src.slice(last));
    return out;
  }

  // ---- filtering the queries: words (all of them, in the title, description, SPARQL or a column's name) and a view
  function matches(it, f) {
    if (f && f.view && it.view !== f.view) return false;
    const words = String((f && f.text) || "").toLowerCase().split(/\s+/).filter(Boolean);
    if (!words.length) return true;
    const hay = [it.title, it.question || "", it.sparql, ...Object.values(it.columns || {}).map((c) => c.label + " " + (c.note || ""))].join("\n").toLowerCase();
    return words.every((w) => hay.includes(w));
  }
  const VIEWS = [["map", "Map"], ["table", "Table"], ["cards", "Cards"], ["graph", "Graph"]];
  // the row of view buttons, with how many queries suit each (views no query has are left out)
  function viewChips(items, f, onPick) {
    const n = {};
    for (const it of items) if (it.view) n[it.view] = (n[it.view] || 0) + 1;
    const row = h("div", { class: "rl-qviews", "data-rl": "qviews", role: "group", "aria-label": "Show the queries of one view" });
    const btn = (view, label, count) => {
      const on = (f.view || null) === view;
      const b = h("button", { type: "button", class: "rl-qview" + (on ? " rl-on" : ""), "aria-pressed": String(on), "data-view": view || "all" }, label + " ", h("span", { class: "rl-muted" }, String(count)));
      b.addEventListener("click", (e) => { e.stopPropagation(); onPick(view); });
      return b;
    };
    row.append(btn(null, "All", items.length));
    for (const [v, label] of VIEWS) if (n[v]) row.append(btn(v, label, n[v]));
    return row;
  }

  // the query without PREFIX/BASE lines and comment-only lines, trimmed
  function preview(src) {
    return String(src).split("\n").filter((l) => !/^\s*(PREFIX|BASE)\b/i.test(l) && !/^\s*#/.test(l)).join("\n").replace(/^\s+|\s+$/g, "");
  }

  const cap = (s) => (s ? s.charAt(0).toUpperCase() + s.slice(1) : s);
  const groupOf = (ex) => (ex.starter ? "Starter queries" : ex.curated ? "Card examples" : cap(ex.group || "Other"));

  class QueryMenu {
    constructor(explorer) { this.ex = explorer; this.root = null; this.anchor = null; }
    get isOpen() { return !!this.root; }

    open(anchor) {
      if (this.root) { this.close(); return; }
      const ex = this.ex;
      const items = ex.s.examples;
      if (!items.length) return;
      this.anchor = anchor;
      const root = h("div", { "data-retelens": "" });
      const panel = h("div", { class: "fixed z-[2147483000] flex flex-col overflow-hidden rounded-lg border bg-white text-sm text-gray-900 shadow-lg dark:border-gray-800 dark:bg-gray-950 dark:text-gray-100", role: "dialog", "aria-label": "Choose a query" });
      const qf = ex.qf || (ex.qf = { text: "", view: null }); // the same filter as the Examples panel
      const input = h("input", { type: "text", class: "w-full flex-none border-b bg-transparent px-3 py-2 outline-none placeholder:text-gray-400 dark:border-gray-800", placeholder: "Filter by title, description, query or column", role: "combobox", "aria-expanded": "true", "aria-controls": "rl-qlist", "aria-autocomplete": "list", "aria-label": "Filter queries" });
      input.value = qf.text;
      const views = h("div", { class: "flex-none border-b px-3 py-1.5 dark:border-gray-800" });
      const list = h("div", { id: "rl-qlist", role: "listbox", class: "min-h-0 flex-1 overflow-auto" });
      panel.append(input, views, list);
      root.append(panel);
      document.body.append(root);
      this.root = root; this.panel = panel;

      let opts = []; // [{el, idx}]
      let active = -1;
      const setActive = (n, scroll) => {
        if (opts[active]) opts[active].el.classList.remove("bg-gray-100", "dark:bg-gray-850");
        active = n;
        if (opts[active]) {
          opts[active].el.classList.add("bg-gray-100", "dark:bg-gray-850");
          input.setAttribute("aria-activedescendant", opts[active].el.id);
          if (scroll) opts[active].el.scrollIntoView({ block: "nearest" });
        } else input.removeAttribute("aria-activedescendant");
      };
      const choose = (idx) => { this.close(); ex.selectExample(idx); };
      const render = (filter) => {
        list.textContent = ""; opts = [];
        qf.text = filter;
        const f = (qf.text.trim() || qf.view) ? qf : null;
        views.textContent = "";
        views.append(viewChips(items, qf, (v) => { qf.view = v; render(input.value); input.focus(); }));
        let lastGroup = null, n = 0;
        items.forEach((it, idx) => {
          if (f && !matches(it, f)) return;
          const g = groupOf(it);
          if (g !== lastGroup) { lastGroup = g; list.append(h("div", { role: "presentation", class: "bg-gray-50 px-3 py-1 text-xs uppercase tracking-wide text-gray-400 dark:bg-gray-900" }, g)); }
          const pv = preview(it.sparql);
          const el = h("div", { role: "option", id: "rl-opt-" + idx, "aria-selected": String(idx === ex.q), class: "cursor-pointer border-b px-3 py-2 dark:border-gray-800 " + (idx === ex.q ? "bg-gray-50 dark:bg-gray-900" : "") },
            h("div", { class: "font-medium" }, it.title),
            it.question ? h("div", { class: "text-xs text-gray-500" }, it.question) : null,
            it.view && window.RetelensHints ? window.RetelensHints.chips(it, h, window.RetelensFmt.icon) : null,
            pv ? h("pre", { class: "mt-1 line-clamp-3 whitespace-pre-wrap break-all font-mono text-xs text-gray-600 dark:text-gray-400" }, pv) : null);
          const o = { el, idx };
          opts.push(o);
          el.addEventListener("mousemove", () => { const k = opts.indexOf(o); if (k !== active) setActive(k, false); });
          el.addEventListener("click", () => choose(idx));
          list.append(el);
          n++;
        });
        if (!n) list.append(h("div", { class: "px-3 py-4 text-center text-gray-500" }, "No query matches."));
        if (ex.sideTab === "examples" && ex.renderSide) ex.renderSide(); // the Examples panel shows the same filter
        const sel = opts.findIndex((o) => o.idx === ex.q);
        setActive(f ? (opts.length ? 0 : -1) : sel >= 0 ? sel : (opts.length ? 0 : -1), true);
      };
      input.addEventListener("input", () => render(input.value));
      render(input.value);
      panel.addEventListener("keydown", (e) => {
        if (e.key === "ArrowDown") { e.preventDefault(); if (opts.length) setActive(Math.min(opts.length - 1, active + 1), true); }
        else if (e.key === "ArrowUp") { e.preventDefault(); if (opts.length) setActive(Math.max(0, active - 1), true); }
        else if (e.key === "Home") { e.preventDefault(); setActive(0, true); }
        else if (e.key === "End") { e.preventDefault(); setActive(opts.length - 1, true); }
        else if (e.key === "Enter") { e.preventDefault(); if (opts[active]) choose(opts[active].idx); }
        else if (e.key === "Escape") { e.preventDefault(); e.stopPropagation(); this.close(); }
        else if (e.key === "Tab") { e.preventDefault(); input.focus(); } // focus trap: arrows move through the listbox
      });
      this._down = (e) => { if (!panel.contains(e.target) && !anchor.contains(e.target)) this.close(); };
      this._place = () => {
        const r = anchor.getBoundingClientRect();
        const vw = document.documentElement.clientWidth, vh = window.innerHeight;
        const w = Math.min(560, vw - 16);
        panel.style.width = w + "px";
        panel.style.left = Math.max(8, Math.min(r.left, vw - w - 8)) + "px";
        const top = r.bottom + 4;
        panel.style.top = top + "px";
        panel.style.maxHeight = Math.max(160, Math.min(32 * 16, vh - top - 8)) + "px";
      };
      document.addEventListener("mousedown", this._down, true);
      window.addEventListener("resize", this._place);
      window.addEventListener("scroll", this._place, true);
      anchor.setAttribute("aria-expanded", "true");
      this._place();
      render("");
      input.focus();
    }

    close() {
      if (!this.root) return;
      document.removeEventListener("mousedown", this._down, true);
      window.removeEventListener("resize", this._place);
      window.removeEventListener("scroll", this._place, true);
      this.root.remove();
      this.root = null;
      if (this.anchor && this.anchor.isConnected) { this.anchor.setAttribute("aria-expanded", "false"); this.anchor.focus(); }
    }
  }

  window.RetelensQuery = { highlight, preview, QueryMenu, groupOf, matches, viewChips };
})();
