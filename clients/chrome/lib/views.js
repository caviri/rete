// Result views: Table and Cards (the Graph view is lib/graph-view.js, the Map view lib/maps.js). Every value is
// drawn by lib/cells.js, so images, IIIF manifests, geometries, audio, video and links look the same in both, and
// a column's render type (Auto, or forced from the column menu / the Fields dialog) applies to both.
// Untrusted data (IRIs, literals, labels) enters only via text nodes; search hits are <mark> elements made by
// splitting text nodes; links and media are http(s) only.
(function () {
  const { h, safeUrl, icon, num } = window.RetelensFmt;
  const C = window.RetelensCells;
  const XSD = "http://www.w3.org/2001/XMLSchema#";
  const NAMEISH = /(^|[^a-z])(name|label|title|prefLabel|fullname|heading|headline)([^a-z]|$)|(name|label|title|headline)$/i;
  const nameLike = (v) => NAMEISH.test(v);

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
  const typeFor = (opts, v) => (opts.types && opts.types[v]) || "auto";

  // ------------------------------------------------------------------ table
  // opts: {model, hl, onSort(v), onFilter(button, v), maxHeight, types}. The header row sticks inside the box; the
  // box has a maximum height, so its scrollbars (both axes) are in view without scrolling to the end of the table.
  function TableView(rows, vars, ctx, opts = {}) {
    const UI = window.RetelensResultsUI;
    const model = opts.model;
    const wrap = h("div", { "data-rl": "tablebox", class: "relative min-h-0 flex-1 overflow-auto" });
    if (opts.maxHeight) wrap.style.maxHeight = opts.maxHeight;
    const thead = h("tr", { class: "h-full min-w-fit border-b text-left dark:border-gray-800" }, vars.map((v, i) => {
      const forced = typeFor(opts, v) !== "auto";
      return h("th", { class: "h-full max-w-sm p-2 text-left relative w-auto" + (i ? " border-l dark:border-gray-800" : ""), "aria-sort": model && model.sort && model.sort.v === v ? (model.sort.dir === "asc" ? "ascending" : "descending") : null },
        h("div", { class: "flex min-w-0 items-center justify-between gap-1" },
          model && opts.onSort ? UI.headerSort(model, v, opts.onSort, C.prettyLabel(v)) : h("span", { class: "min-w-0 truncate font-bold", title: "?" + v }, C.prettyLabel(v)),
          model && opts.onFilter ? UI.filterButton(model, v, (btn) => opts.onFilter(btn, v)) : null),
        h("div", { class: "mb-2 whitespace-nowrap text-xs font-normal text-gray-500" }, h("span", null, typeOf(rows.map((r) => r[v]))),
          forced ? h("span", { class: "rl-typetag", title: "Shown as " + C.TYPE_LABEL[typeFor(opts, v)] + " (column menu)" }, C.TYPE_LABEL[typeFor(opts, v)]) : null),
        opts.stats ? opts.stats(v) : null);
    }));
    const tbody = h("tbody", { class: "h-16 overflow-scroll" }, rows.map((row) =>
      h("tr", { class: "group cursor-pointer border-b outline-offset-[-2px] odd:bg-gray-50 hover:bg-gray-100 dark:border-gray-800 dark:odd:bg-gray-925 dark:hover:bg-gray-850" },
        vars.map((v, i) => {
          const t = row[v];
          const type = typeFor(opts, v);
          const kind = type === "auto" ? C.autoType(t) : type;
          const td = h("td", { class: "group relative min-w-fit max-w-sm break-words p-2" + (i ? " border-l dark:border-gray-800" : "") + (kind === "number" ? " text-right" : "") });
          if (!t) return td;
          const media = C.isMediaType(kind) || kind === "audio" || kind === "markdown";
          td.append(media ? C.render(t, type, ctx, { hl: opts.hl, caption: captionOf(row, vars, v, ctx) }) : h("div", { class: "line-clamp-2" }, C.render(t, type, ctx, { hl: opts.hl })));
          return td;
        }))));
    wrap.append(h("table", { class: "w-full table-auto rounded-lg font-mono text-xs text-gray-900 dark:text-gray-100" },
      h("thead", { class: "sticky left-0 right-0 top-0 z-20 bg-white align-top dark:bg-gray-950" }, thead), tbody));
    if (!rows.length) wrap.append(h("div", { class: "p-6 text-center text-sm text-gray-500" }, opts.filtered ? "No row matches the search and filters." : "No rows."));
    return { el: wrap };
  }
  // a short name for a row (the lightbox caption, a map label): its first name-like literal
  function captionOf(row, vars, skip, ctx) {
    const p = pickTitle(row, vars.filter((v) => v !== skip));
    return p ? (p.iri ? ctx.short(p.t.value) : String(p.t.value)) : "";
  }

  // ------------------------------------------------------------------ cards
  function pickTitle(row, vars) {
    const isLit = (t) => t && t.kind === "literal" && !C.urlOf(t) && !C.isWkt(t);
    for (const v of vars) if (nameLike(v) && isLit(row[v])) return { v, t: row[v] };
    for (const v of vars) if (isLit(row[v]) && C.autoType(row[v]) === "text") return { v, t: row[v] };
    for (const v of vars) if (row[v] && row[v].kind === "iri" && C.autoType(row[v]) !== "image") return { v, t: row[v], iri: true };
    return null;
  }
  const LEAD = new Set(["image", "iiif", "video"]);
  // One card: the lead picture (the first image / IIIF / video field) full width, a title, then the fields label
  // over value. `full`: the focus view (every field, nothing clamped, media loaded now).
  function cardEl(row, vars, ctx, opts, full) {
    const hidden = opts.hidden || new Set();
    const shown = vars.filter((v) => row[v] && !hidden.has(v));
    const kindOf = (v) => { const ty = typeFor(opts, v); return ty === "auto" ? C.autoType(row[v]) : ty; };
    const leadVar = shown.find((v) => LEAD.has(kindOf(v)));
    const pick = pickTitle(row, shown.filter((v) => v !== leadVar));
    const card = h("article", { "data-rl": "card", class: "rl-card rounded-lg border border-gray-100 bg-white text-sm shadow-xs dark:border-gray-800 dark:bg-gray-925" + (full ? " rl-card-full" : "") });
    if (leadVar) card.append(h("div", { class: "rl-card-media" }, C.render(row[leadVar], typeFor(opts, leadVar), ctx, { card: true, eager: full || opts.eager, caption: pick ? String(pick.t.value) : "" })));
    const body = h("div", { class: "rl-card-body" });
    const titleText = pick ? (pick.iri ? ctx.short(pick.t.value) : String(pick.t.value)) : "";
    const firstIri = shown.map((v) => row[v]).find((t) => t && t.kind === "iri" && C.autoType(t) !== "image");
    const url = firstIri && safeUrl(firstIri.value);
    // no text to name the row (a count, a list of IRIs): the fields say it all, no "(no label)" title
    if (pick) body.append(h(url ? "a" : "div", { "data-rl": "card-title", class: "rl-card-title text-gray-900 hover:text-blue-600 dark:text-gray-100", href: url, title: titleText }, opts.hl ? window.RetelensResults.highlight(titleText, opts.hl) : titleText));
    const rest = shown.filter((v) => v !== leadVar && !(pick && v === pick.v));
    const cap = full ? rest.length : 8;
    const dl = h("dl", { "data-rl": "card-meta", class: "rl-fields text-gray-500 dark:text-gray-400" });
    for (const v of rest.slice(0, cap)) {
      dl.append(h("div", { class: "rl-f" }, h("dt", { title: "?" + v }, C.prettyLabel(v)),
        h("dd", { class: "text-gray-900 dark:text-gray-100" }, C.render(row[v], typeFor(opts, v), ctx, { hl: opts.hl, card: true, eager: full, caption: titleText }))));
    }
    body.append(dl);
    if (rest.length > cap) body.append(h("div", { class: "rl-more text-gray-400" }, `+${rest.length - cap} more`));
    card.append(body);
    return card;
  }
  // opts: {hl, facets, types, setType(v, type), hidden (Set), setHidden(v, hide), filtered}
  function CardsView(rows, vars, ctx, opts = {}) {
    const grid = h("div", { class: "rl-cards" });
    rows.forEach((row, i) => {
      const card = cardEl(row, vars, ctx, { ...opts, eager: i < 12 }, false);
      card.tabIndex = 0;
      card.setAttribute("role", "button");
      card.setAttribute("aria-label", "Open card " + (i + 1));
      const open = (e) => {
        if (e.target.closest("a, button, input, select, textarea, audio, video, iframe, .rl-iiif")) return;
        focusCards(rows, vars, ctx, opts, i);
      };
      card.addEventListener("click", open);
      card.addEventListener("keydown", (e) => { if (e.key === "Enter" && e.target === card) open(e); });
      grid.append(card);
    });
    if (!rows.length) grid.append(h("div", { class: "col-span-full p-6 text-center text-sm text-gray-500" }, opts.filtered ? "No row matches the search and filters." : "No rows."));
    const bar = h("div", { class: "rl-cards-bar text-gray-500" }, h("span", null, `${num(rows.length)} ${rows.length === 1 ? "card" : "cards"} on this page · click a card to see it whole`));
    if (opts.setType) bar.append(h("button", { type: "button", class: "btn shadow-xs rounded-sm border px-2 py-0.5 text-xs font-normal text-gray-700 dark:text-gray-300", onclick: () => fieldsDialog(rows, vars, opts) }, "Fields"));
    const gridBox = h("div", { class: "min-w-0 flex-1" }, bar, grid);
    return { el: h("div", { "data-rl": "cardsbox", class: "relative flex min-h-0 flex-1 overflow-auto" }, opts.facets || null, gridBox) };
  }

  // ------------------------------------------------------------------ focus: one card at a time, swipeable
  // Native horizontal scroll with snap: touch swipes, trackpads, Shift+wheel, ←/→, the ‹ › buttons and a drag on
  // the card background all move it; the cards on both sides peek in.
  function focusCards(rows, vars, ctx, opts, start) {
    const track = h("div", { class: "rl-focus-track" });
    const count = h("span", { class: "rl-focus-count" });
    const slides = rows.map(() => h("div", { class: "rl-focus-slide" }));
    track.append(...slides);
    const filled = new Set();
    const fill = (i) => { if (i < 0 || i >= rows.length || filled.has(i)) return; filled.add(i); slides[i].append(cardEl(rows[i], vars, ctx, opts, true)); };
    const at = () => { const w = slides[0].offsetWidth || 1; const gap = slides.length > 1 ? slides[1].offsetLeft - slides[0].offsetLeft : w; return Math.round(track.scrollLeft / gap); };
    const go = (i, smooth) => { i = Math.max(0, Math.min(rows.length - 1, i)); fill(i - 1); fill(i); fill(i + 1); slides[i].scrollIntoView({ behavior: smooth ? "smooth" : "instant", inline: "center", block: "nearest" }); };
    const sync = () => { const i = at(); fill(i - 1); fill(i); fill(i + 1); count.textContent = `${i + 1} / ${rows.length}`; prev.disabled = i <= 0; next.disabled = i >= rows.length - 1; };
    const m = C.modal({ label: "Card", cls: "rl-focus", onKey: (e) => { if (e.key === "ArrowLeft") { e.preventDefault(); go(at() - 1, true); } else if (e.key === "ArrowRight") { e.preventDefault(); go(at() + 1, true); } } });
    const prev = h("button", { type: "button", class: "rl-navbtn", "aria-label": "Previous card", onclick: () => go(at() - 1, true) }, "‹");
    const next = h("button", { type: "button", class: "rl-navbtn", "aria-label": "Next card", onclick: () => go(at() + 1, true) }, "›");
    m.box.append(track, h("div", { class: "rl-focus-bar" }, prev, count, next));
    let t = 0;
    track.addEventListener("scroll", () => { cancelAnimationFrame(t); t = requestAnimationFrame(sync); });
    track.addEventListener("wheel", (e) => { if (e.shiftKey && Math.abs(e.deltaY) > Math.abs(e.deltaX)) { e.preventDefault(); track.scrollLeft += e.deltaY; } }, { passive: false });
    if (matchMedia("(hover: hover) and (pointer: fine)").matches) {
      let drag = null;
      track.addEventListener("pointerdown", (e) => {
        if (e.button !== 0 || e.target.closest("a, button, input, select, textarea, audio, video, iframe, .rl-iiif, .rl-geo, .rl-md")) return;
        drag = { x: e.clientX, left: track.scrollLeft, moved: false };
        track.setPointerCapture(e.pointerId);
      });
      track.addEventListener("pointermove", (e) => { if (!drag) return; const dx = e.clientX - drag.x; if (Math.abs(dx) > 4) { drag.moved = true; track.classList.add("rl-dragging"); } track.scrollLeft = drag.left - dx; });
      const end = () => { if (!drag) return; const moved = drag.moved; drag = null; track.classList.remove("rl-dragging"); if (moved) go(at(), true); };
      track.addEventListener("pointerup", end);
      track.addEventListener("pointercancel", end);
    }
    requestAnimationFrame(() => { go(start, false); sync(); });
  }

  // ------------------------------------------------------------------ Fields: show / hide and render type per field
  function fieldsDialog(rows, vars, opts) {
    const m = C.modal({ label: "Card fields", cls: "rl-fieldsdlg" });
    const list = h("div", { class: "rl-fieldlist" });
    for (const v of vars) {
      const cb = h("input", { type: "checkbox", checked: !(opts.hidden && opts.hidden.has(v)), "aria-label": "Show " + v });
      cb.addEventListener("change", () => opts.setHidden && opts.setHidden(v, !cb.checked));
      const sel = typeSelect(opts.types && opts.types[v], C.columnAuto(rows, v), (ty) => opts.setType(v, ty));
      list.append(h("label", { class: "rl-fieldrow" }, cb, h("span", { class: "rl-fieldname", title: "?" + v }, C.prettyLabel(v)), sel));
    }
    m.box.append(h("div", { class: "rl-dlg-h" }, "Card fields"), h("p", { class: "rl-muted" }, "Pick the fields the cards show and how each value is drawn. Tables use the same choice."), list);
  }
  // the render-type picker shared by the column menu and the Fields dialog
  function typeSelect(cur, auto, onChange) {
    const sel = h("select", { class: "rl-select", "aria-label": "Show as" });
    for (const [id, label] of C.TYPES) {
      const op = h("option", { value: id }, id === "auto" ? `Auto (${C.TYPE_LABEL[auto] || (auto === "iri" ? "IRI" : "Text")})` : label);
      if ((cur || "auto") === id) op.selected = true;
      sel.append(op);
    }
    sel.addEventListener("change", () => onChange(sel.value));
    return sel;
  }

  window.RetelensViews = { TableView, CardsView, typeOf, nameLike, termNodes: C.termNodes, typeSelect, pickTitle };
})();
