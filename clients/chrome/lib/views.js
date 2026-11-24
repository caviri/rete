// Result views: Table and Cards (the Graph view is lib/graph-view.js). DOM is built with HF's Tailwind classes.
// Untrusted data (IRIs, literals, labels) enters only via textContent/createTextNode
// (h() in format.js); search hits are <mark> elements made by splitting text nodes.
// Links and thumbnails are http(s) only.
(function () {
  const { h, num, safeUrl, icon } = window.RetelensFmt;
  const XSD = "http://www.w3.org/2001/XMLSchema#";
  const RDF_TYPE = "http://www.w3.org/1999/02/22-rdf-syntax-ns#type";
  const NUMERIC = /(integer|int|long|short|decimal|double|float|byte|nonNegativeInteger|positiveInteger|negativeInteger|nonPositiveInteger|unsigned\w*)$/;
  const IMG = /\.(jpe?g|png|gif|webp|svg)(\?.*)?$/i;
  const NAMEISH = /(^|[^a-z])(name|label|title|prefLabel|fullname|heading)([^a-z]|$)|(name|label|title)$/i;

  const isImage = (t) => t && t.kind === "iri" && IMG.test(t.value) && !!safeUrl(t.value);
  const nameLike = (v) => NAMEISH.test(v);
  const localName = (iri) => decodeURIComponent((iri.split(/[#/]/).filter(Boolean).pop() || iri)).replace(/_/g, " ");

  function typeOf(terms) {
    const kinds = new Set();
    for (const t of terms) {
      if (!t) continue;
      if (t.kind === "iri") kinds.add("IRI");
      else if (t.kind === "bnode") kinds.add("blank node");
      else if (t.kind === "triple") kinds.add("triple");
      else if (t.lang) kinds.add("string@" + t.lang);
      else if (t.datatype) kinds.add(t.datatype.startsWith(XSD) ? t.datatype.slice(XSD.length) : t.datatype.split(/[#/]/).pop());
      else kinds.add("string");
    }
    if (!kinds.size) return "";
    return kinds.size > 2 ? "mixed" : [...kinds].join(" · ");
  }

  // One term as inline nodes. `short` is the prefix shortener.
  const HL = (text, q) => (q ? window.RetelensResults.highlight(text, q) : [text]);
  function termNodes(t, short, opts = {}) {
    if (!t) return [];
    if (t.kind === "iri") {
      const label = HL(short(t.value), opts.hl);
      const url = safeUrl(t.value);
      return [url ? h("a", { href: url, title: t.value, class: "iri text-blue-600 hover:underline dark:text-blue-400" }, label) : h("span", { title: t.value, class: "iri" }, label)];
    }
    if (t.kind === "bnode" || t.kind === "triple") return [h("span", { class: "text-gray-500" }, HL(t.value, opts.hl))];
    const out = HL(t.value, opts.hl);
    if (t.lang) out.push(h("span", { class: "ml-1 text-gray-400" }, "@" + t.lang));
    else if (t.datatype && t.datatype !== XSD + "string" && !NUMERIC.test(t.datatype) && !opts.noDatatype) out.push(h("span", { class: "ml-1 text-gray-400", title: t.datatype }, "^^" + short(t.datatype)));
    return out;
  }

  // ------------------------------------------------------------------ table
  // opts: {model, hl, onSort(v), onFilter(button, v), maxHeight}. The header row sticks inside the box; the box has a
  // maximum height, so its scrollbars (both axes) are in view without scrolling to the end of the table.
  function TableView(rows, vars, ctx, opts = {}) {
    const UI = window.RetelensResultsUI;
    const model = opts.model;
    const wrap = h("div", { "data-rl": "tablebox", class: "relative min-h-0 flex-1 overflow-auto" });
    if (opts.maxHeight) wrap.style.maxHeight = opts.maxHeight;
    const thead = h("tr", { class: "h-full min-w-fit border-b text-left dark:border-gray-800" }, vars.map((v, i) =>
      h("th", { class: "h-full max-w-sm p-2 text-left relative w-auto" + (i ? " border-l dark:border-gray-800" : ""), "aria-sort": model && model.sort && model.sort.v === v ? (model.sort.dir === "asc" ? "ascending" : "descending") : null },
        h("div", { class: "flex min-w-0 items-center justify-between gap-1" },
          model && opts.onSort ? UI.headerSort(model, v, opts.onSort) : h("span", { class: "min-w-0 truncate font-bold", title: v }, v),
          model && opts.onFilter ? UI.filterButton(model, v, (btn) => opts.onFilter(btn, v)) : null),
        h("div", { class: "mb-2 whitespace-nowrap text-xs font-normal text-gray-500" }, h("span", null, typeOf(rows.map((r) => r[v])))))));
    const tbody = h("tbody", { class: "h-16 overflow-scroll" }, rows.map((row) =>
      h("tr", { class: "group cursor-pointer border-b outline-offset-[-2px] odd:bg-gray-50 hover:bg-gray-100 dark:border-gray-800 dark:odd:bg-gray-925 dark:hover:bg-gray-850" },
        vars.map((v, i) => {
          const t = row[v];
          const td = h("td", { class: "group relative min-w-fit max-w-sm break-words p-2" + (i ? " border-l dark:border-gray-800" : "") });
          const inner = h("div", { class: "line-clamp-2" });
          if (isImage(t)) {
            inner.append(h("img", { class: "mr-1 inline-block h-6 w-auto max-w-[3rem] rounded-sm align-middle", src: t.value, loading: "lazy", referrerpolicy: "no-referrer", alt: "", onerror: (e) => e.target.remove() }));
          }
          inner.append(...termNodes(t, ctx.short, { hl: opts.hl }));
          if (t && t.kind === "literal" && t.datatype && NUMERIC.test(t.datatype)) td.classList.add("text-right");
          td.append(inner);
          return td;
        }))));
    wrap.append(h("table", { class: "w-full table-auto rounded-lg font-mono text-xs text-gray-900 dark:text-gray-100" },
      h("thead", { class: "sticky left-0 right-0 top-0 z-20 bg-white align-top dark:bg-gray-950" }, thead), tbody));
    if (!rows.length) wrap.append(h("div", { class: "p-6 text-center text-sm text-gray-500" }, opts.filtered ? "No row matches the search and filters." : "No rows."));
    return { el: wrap };
  }

  // ------------------------------------------------------------------ cards
  function pickTitle(row, vars) {
    for (const v of vars) if (nameLike(v) && row[v] && row[v].kind === "literal") return { v, t: row[v] };
    for (const v of vars) if (row[v] && row[v].kind === "literal") return { v, t: row[v] };
    for (const v of vars) if (row[v] && row[v].kind === "iri") return { v, t: row[v], iri: true };
    return null;
  }
  function CardsView(rows, vars, ctx, opts = {}) {
    const grid = h("div", { class: "grid grid-cols-1 gap-3 p-3 sm:grid-cols-2 xl:grid-cols-3 2xl:grid-cols-4" });
    for (const row of rows) {
      const card = h("div", { "data-rl": "card", class: "relative flex min-w-0 gap-3 overflow-hidden rounded-lg border border-gray-100 bg-white p-3 text-sm shadow-xs hover:border-gray-200 dark:border-gray-800 dark:bg-gray-925 dark:hover:border-gray-700" });
      const imgVar = vars.find((v) => isImage(row[v]));
      if (imgVar) card.append(h("img", { class: "h-16 w-16 flex-none rounded-md border border-gray-100 object-cover dark:border-gray-800", src: row[imgVar].value, loading: "lazy", referrerpolicy: "no-referrer", alt: "", onerror: (e) => e.target.remove() }));
      const body = h("div", { class: "min-w-0 flex-1" });
      const pick = pickTitle(row, vars);
      const firstIri = vars.map((v) => row[v]).find((t) => t && t.kind === "iri" && !isImage(t));
      const titleText = pick ? (pick.iri ? ctx.short(pick.t.value) : pick.t.value) : "(empty row)";
      const titleNodes = opts.hl && pick ? window.RetelensResults.highlight(titleText, opts.hl) : [titleText];
      const url = firstIri && safeUrl(firstIri.value);
      const title = h(url ? "a" : "div", { "data-rl": "card-title", class: "block truncate font-mono text-sm font-semibold text-gray-900 hover:text-blue-600 dark:text-gray-100", href: url, title: titleText }, titleNodes);
      body.append(title);
      const used = new Set([pick && pick.v, imgVar]);
      const rest = vars.filter((v) => !used.has(v) && row[v]);
      const meta = h("div", { "data-rl": "card-meta", class: "mt-1 space-y-0.5 text-xs text-gray-500 dark:text-gray-400" });
      for (const v of rest.slice(0, 6)) {
        meta.append(h("div", { class: "flex min-w-0 gap-1.5" }, h("span", { class: "flex-none font-mono text-gray-400" }, v + ":"), h("span", { class: "min-w-0 truncate" }, termNodes(row[v], ctx.short, { hl: opts.hl }))));
      }
      if (rest.length > 6) meta.append(h("div", { class: "text-gray-400" }, "+" + (rest.length - 6) + " more"));
      body.append(meta);
      card.append(body);
      grid.append(card);
    }
    if (!rows.length) grid.append(h("div", { class: "col-span-full p-6 text-center text-sm text-gray-500" }, opts.filtered ? "No row matches the search and filters." : "No rows."));
    const gridBox = h("div", { class: "min-w-0 flex-1" }, grid);
    return { el: h("div", { "data-rl": "cardsbox", class: "relative flex min-h-0 flex-1 overflow-auto" }, opts.facets || null, gridBox) };
  }

  window.RetelensViews = { TableView, CardsView, typeOf, nameLike };
})();
