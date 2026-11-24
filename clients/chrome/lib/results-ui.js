// Controls for lib/results.js: column filter popover, quick-search bar with chips, Cards facets panel.
// Host-native by construction: HF's Tailwind classes on HF, the same names (mapped to Primer variables by
// content/github.css) on GitHub, shim.css in the standalone viewer. Untrusted text goes in as text nodes only.
(function () {
  const { h, num, icon } = window.RetelensFmt;
  const R = window.RetelensResults;
  const XSD = R.XSD;

  const INPUT = "h-8 min-w-0 rounded-md border border-gray-200 bg-white px-2 text-sm text-gray-900 placeholder:text-gray-400 outline-none focus:border-gray-400 dark:border-gray-800 dark:bg-gray-900 dark:text-gray-100";
  const BTN = "btn shadow-xs rounded-sm border px-2 py-0.5 text-xs font-normal text-gray-700 dark:text-gray-300";
  const label = (v, short) => (v.startsWith("http") ? short(v) : v);

  // ---------------------------------------------------------------- column filter popover
  // api: {model, short, setFilter(v, f), root: element to portal next to (inherits [data-retelens])}
  class ColumnMenu {
    constructor() { this.root = null; }
    close() {
      if (!this.root) return;
      document.removeEventListener("mousedown", this._down, true);
      window.removeEventListener("resize", this._place);
      window.removeEventListener("scroll", this._place, true);
      this.root.remove(); this.root = null;
      if (this.anchor && this.anchor.isConnected) { this.anchor.setAttribute("aria-expanded", "false"); this.anchor.focus(); }
    }
    reanchor(area) {
      if (!this.root || !this.v) return;
      if (this.anchor && this.anchor.isConnected) return;
      const b = area.querySelector('[data-rl="colfilter"][data-col="' + CSS.escape(this.v) + '"]');
      if (b) { this.anchor = b; b.setAttribute("aria-expanded", "true"); this._place && this._place(); }
    }
    open(anchor, v, api) {
      if (this.root && this.anchor === anchor) { this.close(); return; }
      this.close();
      const model = api.model, p = model.profile(v);
      let f = JSON.parse(JSON.stringify(model.filters[v] || {}));
      const root = h("div", { "data-retelens": "" });
      const panel = h("div", { class: "fixed z-[2147483000] flex w-72 max-w-[calc(100vw-1rem)] flex-col overflow-hidden rounded-lg border bg-white text-sm text-gray-900 shadow-lg dark:border-gray-800 dark:bg-gray-950 dark:text-gray-100", role: "dialog", "aria-label": "Filter column " + v, "data-rl": "colmenu" });
      const body = h("div", { class: "flex max-h-[24rem] flex-col gap-3 overflow-auto p-3" });
      panel.append(h("div", { class: "flex items-center justify-between gap-2 border-b px-3 py-2 dark:border-gray-800" },
        h("span", { class: "min-w-0 truncate font-semibold", title: v }, v), h("span", { class: "flex-none text-xs text-gray-500" }, p.type)), body);
      let timer = null;
      const commit = () => { clearTimeout(timer); timer = setTimeout(() => api.setFilter(v, JSON.parse(JSON.stringify(f))), 120); };
      const section = (title, ...kids) => h("div", null, h("div", { class: "mb-1 text-xs font-semibold uppercase tracking-wide text-gray-400" }, title), ...kids);
      const checks = (title, entries, key, fmt) => {
        const box = h("div", { class: "flex max-h-40 flex-col gap-0.5 overflow-auto" });
        for (const [val, n] of entries) {
          const id = "rl-cf-" + key + "-" + Math.random().toString(36).slice(2, 7);
          const cb = h("input", { type: "checkbox", id, class: "flex-none", checked: (f[key] || []).includes(val) });
          cb.addEventListener("change", () => {
            const cur = new Set(f[key] || []);
            if (cb.checked) cur.add(val); else cur.delete(val);
            f[key] = [...cur]; if (!f[key].length) delete f[key];
            commit();
          });
          box.append(h("label", { for: id, class: "flex min-w-0 cursor-pointer items-center gap-2 py-0.5" }, cb, h("span", { class: "min-w-0 flex-1 truncate font-mono text-xs", title: val }, fmt(val)), h("span", { class: "flex-none text-xs text-gray-500" }, num(n))));
        }
        return section(title, box);
      };
      // how the column's values are drawn (lib/cells.js), shared with the Cards view
      if (api.type) body.append(section("Show as", window.RetelensViews.typeSelect(api.type.current, api.type.auto, (ty) => api.type.set(ty))));
      // text
      {
        const sel = h("select", { class: INPUT + " w-28 flex-none", "aria-label": "Match" }, h("option", { value: "c" }, "contains"), h("option", { value: "e" }, "equals"), h("option", { value: "s" }, "starts with"));
        sel.value = (f.tx && f.tx.m) || "c";
        const inp = h("input", { type: "text", class: INPUT + " flex-1", placeholder: "text", "aria-label": "Text", value: (f.tx && f.tx.v) || "" });
        const upd = () => { if (inp.value) f.tx = { m: sel.value, v: inp.value }; else delete f.tx; commit(); };
        sel.addEventListener("change", upd); inp.addEventListener("input", upd);
        body.append(section("Text", h("div", { class: "flex gap-1" }, sel, inp)));
        panel._focus = inp;
      }
      if (p.iri) body.append(checks("Namespace", [...p.ns.entries()].sort((a, b) => b[1] - a[1]).slice(0, 14), "ns", (n) => api.short(n)));
      if (p.lit) {
        if (p.lang.size > 1 || ![...p.lang.keys()].every((k) => k === "")) body.append(checks("Language", [...p.lang.entries()].sort((a, b) => b[1] - a[1]).slice(0, 14), "lang", (l) => (l ? "@" + l : "no tag")));
        if (p.dt.size > 1) body.append(checks("Datatype", [...p.dt.entries()].sort((a, b) => b[1] - a[1]).slice(0, 10), "dt", (d) => (d.startsWith(XSD) ? "xsd:" + d.slice(XSD.length) : d === "@lang" ? "rdf:langString" : api.short(d))));
      }
      if (p.num) {
        const mk = (k, ph) => { const i = h("input", { type: "number", step: "any", class: INPUT + " w-full", placeholder: ph, "aria-label": k === "min" ? "Minimum" : "Maximum", value: f[k] != null ? String(f[k]) : "" }); i.addEventListener("input", () => { const x = parseFloat(i.value); if (Number.isFinite(x)) f[k] = x; else delete f[k]; commit(); }); return i; };
        body.append(section("Number range", h("div", { class: "flex items-center gap-1" }, mk("min", String(p.min)), h("span", { class: "text-gray-400" }, "–"), mk("max", String(p.max)))));
      }
      if (p.date) {
        const iso = (ms) => new Date(ms).toISOString().slice(0, 10);
        const mk = (k, ph) => { const i = h("input", { type: "date", class: INPUT + " w-full", "aria-label": k === "from" ? "From" : "To", value: f[k] || "", placeholder: ph }); i.addEventListener("input", () => { if (/^\d{4}-\d{2}-\d{2}$/.test(i.value)) f[k] = i.value; else delete f[k]; commit(); }); return i; };
        body.append(section("Date range", h("div", { class: "flex items-center gap-1" }, mk("from", iso(p.dmin)), h("span", { class: "text-gray-400" }, "→"), mk("to", iso(p.dmax)))));
      }
      // empty / not empty
      const radios = h("div", { class: "flex flex-col gap-0.5", role: "radiogroup", "aria-label": "Empty" });
      const rname = "rl-e-" + Math.random().toString(36).slice(2, 7);
      for (const [val, text] of [["", "Any"], ["empty", "Is empty (" + num(p.empty) + ")"], ["full", "Is not empty (" + num(p.n) + ")"]]) {
        const r = h("input", { type: "radio", name: rname, value: val, checked: (f.e || "") === val });
        r.addEventListener("change", () => { if (val) f.e = val; else delete f.e; commit(); });
        radios.append(h("label", { class: "flex cursor-pointer items-center gap-2 py-0.5" }, r, h("span", null, text)));
      }
      body.append(section("Empty", radios));
      const clear = h("button", { type: "button", class: BTN, onclick: () => { clearTimeout(timer); f = {}; api.setFilter(v, null); this.close(); } }, "Clear column");
      const done = h("button", { type: "button", class: BTN, onclick: () => this.close() }, "Done");
      panel.append(h("div", { class: "flex justify-between gap-2 border-t px-3 py-2 dark:border-gray-800" }, clear, done));

      root.append(panel);
      document.body.append(root);
      this.root = root; this.anchor = anchor; this.v = v;
      this._down = (e) => { if (!panel.contains(e.target) && !anchor.contains(e.target)) this.close(); };
      this._place = () => {
        const r = anchor.getBoundingClientRect();
        const vw = document.documentElement.clientWidth, vh = innerHeight;
        panel.style.left = Math.max(8, Math.min(r.left, vw - panel.offsetWidth - 8)) + "px";
        const top = r.bottom + 4;
        panel.style.top = Math.min(top, Math.max(8, vh - panel.offsetHeight - 8)) + "px";
      };
      panel.addEventListener("keydown", (e) => { if (e.key === "Escape") { e.preventDefault(); e.stopPropagation(); this.close(); } else if (e.key === "Enter" && e.target.tagName === "INPUT") { e.preventDefault(); clearTimeout(timer); api.setFilter(v, JSON.parse(JSON.stringify(f))); } });
      document.addEventListener("mousedown", this._down, true);
      window.addEventListener("resize", this._place);
      window.addEventListener("scroll", this._place, true);
      anchor.setAttribute("aria-expanded", "true");
      this._place();
      if (panel._focus) panel._focus.focus();
    }
  }

  // ---------------------------------------------------------------- column header controls
  const SORT_ICON = { asc: "sortAsc", desc: "sortDesc", none: "sortNone" };
  function sortState(model, v) { return model.sort && model.sort.v === v ? model.sort.dir : "none"; }
  // the column name as a button: click cycles asc -> desc -> none
  function headerSort(model, v, onSort, label) {
    const st = sortState(model, v);
    const ic = icon(SORT_ICON[st]);
    ic.classList.add("w-3", "h-3"); ic.classList.remove("w-4", "h-4");
    if (st === "none") ic.classList.add("opacity-30");
    const b = h("button", { type: "button", "data-rl": "sort", "data-sort": st, class: "group/sort flex min-w-0 items-center gap-1 text-left font-bold hover:text-blue-600 dark:hover:text-blue-400", title: "Sort by " + v + (st === "asc" ? " (ascending)" : st === "desc" ? " (descending)" : ""), "aria-label": "Sort by " + v, onclick: () => onSort(v) },
      h("span", { class: "min-w-0 truncate", title: label && label !== v ? "?" + v : null }, label || v), ic);
    return b;
  }
  function filterButton(model, v, onOpen) {
    const on = R.filterActive(model.filters[v]);
    const b = h("button", { type: "button", "data-rl": "colfilter", "data-col": v, "aria-haspopup": "dialog", "aria-expanded": "false", class: "flex-none rounded-sm p-0.5 " + (on ? "text-blue-600 dark:text-blue-400" : "text-gray-400 hover:text-gray-700 dark:hover:text-gray-200"), title: "Filter " + v + (on ? " (active)" : ""), "aria-label": "Filter column " + v });
    b.append(icon("filter"));
    const ic = b.firstElementChild; ic.classList.add("w-3.5", "h-3.5"); ic.classList.remove("w-4", "h-4");
    b.addEventListener("click", (e) => { e.stopPropagation(); onOpen(b); });
    return b;
  }

  // ---------------------------------------------------------------- bar: quick search, count, chips
  // ex: {model, short, isCards(), apply(), onClear(), loading: {n, total}|null, addAsFilter(), facetsOpen, toggleFacets()}
  function FilterBar(ex) {
    const input = h("input", { type: "search", dir: "auto", "data-rl": "rsearch", placeholder: "Search these results", "aria-label": "Search these results", class: "h-7 min-w-0 flex-1 border-none bg-transparent px-1 text-sm placeholder:text-gray-400 outline-none focus:ring-0" });
    const form = h("form", { class: "flex w-full min-w-[10rem] max-w-xs items-center gap-1 rounded-md border px-2 dark:border-gray-800", onsubmit: (e) => e.preventDefault() }, h("span", { class: "text-gray-400" }, icon("search")), input);
    let t = null;
    input.addEventListener("input", () => { clearTimeout(t); t = setTimeout(() => { ex.model.q = input.value; ex.model.changed(); ex.apply(); }, 160); });
    const count = h("span", { "data-rl": "rcount", class: "whitespace-nowrap text-xs text-gray-500" });
    const progress = h("span", { "data-rl": "rprogress", class: "whitespace-nowrap text-xs text-gray-500" });
    const chips = h("div", { class: "flex min-w-0 flex-wrap items-center gap-1" });
    const el = h("div", { "data-rl": "fbar", class: "flex flex-wrap items-center gap-x-2 gap-y-1 border-t px-2.5 py-1.5 text-xs dark:border-gray-800" }, form, count, progress, chips);
    const note = h("div", { "data-rl": "rnote", class: "basis-full text-xs text-gray-500", hidden: true });
    el.append(note);
    function update() {
      const m = ex.model;
      if (document.activeElement !== input && input.value !== m.q) input.value = m.q;
      const loaded = m.rows.length, shown = m.active() ? m.view().length : loaded;
      count.textContent = !ex.useModel() ? (m.total != null ? `${num(m.total)} rows` : `${num(loaded)} rows`) : m.total != null && !m.complete ? `${num(shown)} of ${num(loaded)} loaded rows` : `${num(shown)} of ${num(loaded)} rows`;
      progress.textContent = ex.loading ? `Loading rows ${num(ex.loading.n)} / ${num(ex.loading.total)}…` : "";
      chips.textContent = "";
      const mk = (text, onx, title) => h("span", { "data-rl": "chip", class: "inline-flex max-w-xs items-center gap-1 rounded-full border bg-gray-50 px-2 py-0.5 text-xs text-gray-700 dark:border-gray-700 dark:bg-gray-900 dark:text-gray-300", title: title || text },
        h("span", { class: "min-w-0 truncate" }, text), h("button", { type: "button", "aria-label": "Remove filter " + text, class: "flex-none text-gray-400 hover:text-gray-700 dark:hover:text-gray-200", onclick: onx }, icon("x")));
      if (m.q) chips.append(mk("search: “" + m.q + "”", () => { m.q = ""; input.value = ""; m.changed(); ex.apply(); }));
      for (const v of m.activeCols()) chips.append(mk(R.describe(v, m.filters[v], ex.short), () => { m.setFilter(v, null); ex.apply(); }));
      if (m.active()) chips.append(h("button", { type: "button", "data-rl": "clearfilters", class: "text-xs text-blue-600 hover:underline dark:text-blue-400", onclick: () => { m.clear(); input.value = ""; ex.apply(); } }, "Clear filters"));
      if (m.sort) chips.append(mk("sorted by " + m.sort.v + " " + m.sort.dir, () => { m.sort = null; m.changed(); ex.apply(); }));
      if (ex.isCards()) {
        const b = h("button", { type: "button", "data-rl": "facets-toggle", "aria-pressed": String(!!ex.facetsOpen), class: "ml-auto flex items-center gap-1 rounded-sm px-1.5 py-0.5 text-gray-600 hover:bg-gray-100 dark:text-gray-300 dark:hover:bg-gray-800", onclick: () => ex.toggleFacets() }, icon("filter"), "Facets");
        chips.append(b);
      }
      // window note: above FULL_MAX rows the result is filtered in a loaded window only
      if (m.total != null && !m.complete && ex.windowActive()) {
        note.hidden = false; note.textContent = "";
        note.append(`Filtering the first ${num(m.rows.length)} rows of ${num(m.total)}: matches beyond them are not counted. `);
        if (ex.canAddFilter()) note.append(h("button", { type: "button", "data-rl": "addfilter", class: "text-blue-600 hover:underline dark:text-blue-400", onclick: () => ex.addAsFilter() }, "Add as FILTER to the query"), " shows the rewritten query in the console; it is not run for you.");
      } else note.hidden = true;
      if (ex.rewritten) {
        note.hidden = false;
        note.append(h("div", { class: "mt-1" }, "Rewritten query (open Data Studio to run it):"), h("pre", { "data-rl": "code", class: "mt-1 max-h-40 overflow-auto whitespace-pre-wrap rounded-md border bg-gray-50 p-2 font-mono text-xs dark:border-gray-800 dark:bg-gray-900" }, ex.rewritten));
      }
    }
    return { el, update, input };
  }

  // ---------------------------------------------------------------- Cards facets
  function FacetsPanel(ex) {
    const model = ex.model;
    const aside = h("aside", { "data-rl": "facets", class: "sticky top-0 max-h-full w-60 flex-none self-start overflow-auto border-r p-3 text-sm dark:border-gray-800" });
    aside.append(h("div", { class: "mb-2 flex items-center justify-between" }, h("div", { class: "text-xs font-semibold uppercase tracking-wide text-gray-400" }, "Facets"),
      h("button", { type: "button", class: "text-gray-400 hover:text-gray-700 dark:hover:text-gray-200", "aria-label": "Hide facets", onclick: () => ex.toggleFacets() }, icon("x"))));
    const cols = model.vars.map((v) => ({ v, p: model.profile(v) })).filter(({ p }) => p.n && p.top.size >= 2 && p.top.size <= Math.max(30, p.n * 0.6))
      .sort((a, b) => a.p.top.size - b.p.top.size).slice(0, 8);
    if (!cols.length) aside.append(h("div", { class: "text-xs text-gray-500" }, "No column has a few repeated values to facet on."));
    for (const { v } of cols) {
      const open = ex.facetMore.has(v);
      const f = model.facet(v, open ? 30 : 6);
      const sel = new Set((model.filters[v] && model.filters[v].set) || []);
      const sec = h("div", { class: "mb-3", "data-rl": "facet" }, h("div", { class: "mb-1 truncate font-semibold", title: v }, v));
      const shown = new Map(f.values.map((x) => [x.k, x]));
      for (const k of sel) if (!shown.has(k)) shown.set(k, { k, n: 0, t: { kind: k[0] === "i" ? "iri" : "literal", value: k.slice(1) } });
      for (const x of shown.values()) {
        const id = "rl-fc-" + Math.random().toString(36).slice(2, 8);
        const cb = h("input", { type: "checkbox", id, class: "flex-none", checked: sel.has(x.k) });
        cb.addEventListener("change", () => {
          const cur = new Set((model.filters[v] && model.filters[v].set) || []);
          if (cb.checked) cur.add(x.k); else cur.delete(x.k);
          const nf = { ...(model.filters[v] || {}), set: [...cur] };
          model.setFilter(v, nf); ex.apply();
        });
        const text = x.t.kind === "iri" ? ex.short(x.t.value) : x.t.value + (x.t.lang ? " @" + x.t.lang : "");
        sec.append(h("label", { for: id, class: "flex min-w-0 cursor-pointer items-center gap-2 py-0.5" }, cb, h("span", { class: "min-w-0 flex-1 truncate text-xs", title: x.t.value }, text), h("span", { class: "flex-none text-xs text-gray-500" }, num(x.n))));
      }
      if (f.rest > 0 || open) sec.append(h("button", { type: "button", class: "text-xs text-blue-600 hover:underline dark:text-blue-400", onclick: () => { if (open) ex.facetMore.delete(v); else ex.facetMore.add(v); ex.renderResults(); } }, open ? "Show fewer" : `Show more (${num(f.rest)})`));
      aside.append(sec);
    }
    return aside;
  }

  window.RetelensResultsUI = { ColumnMenu, headerSort, filterButton, FilterBar, FacetsPanel };
})();
