// Query hints: what a query's comments say about its result. A Retelens convention inside the SPARQL text, so it
// needs nothing new in the .rete format and every SPARQL engine ignores it:
//   # Title                          the first comment line: the query's title (as card examples always had)
//   # A plain comment line           more plain lines: the description
//   # view: map                      the view that suits the result: table, cards, graph or map
//   # column ?km2: Area (km²) — …    what a column holds: a label, and a note after " — "
// Without a view line the view is guessed from the query: a WKT geometry -> map, CONSTRUCT / DESCRIBE or a
// triple pattern of variables -> graph, a picture predicate -> cards, else table. A starter query's dimension
// helps too (geo -> map, topology -> graph).
(function () {
  const VIEWS = new Set(["table", "cards", "graph", "map"]);
  const VIEW_LABEL = { table: "Table", cards: "Cards", graph: "Graph", map: "Map" };

  // only comment lines that start a line (a # inside an IRI or a string is not a comment there)
  function comments(sparql) {
    const out = [];
    for (const line of String(sparql || "").split(/\r?\n/)) {
      const m = /^\s*#\s?(.*)$/.exec(line);
      if (m) out.push(m[1].trim());
    }
    return out;
  }

  function parse(sparql) {
    const h = { title: null, description: [], view: null, columns: {} };
    for (const c of comments(sparql)) {
      let m;
      if ((m = /^view\s*:\s*([a-z]+)\s*$/i.exec(c))) { if (VIEWS.has(m[1].toLowerCase())) h.view = m[1].toLowerCase(); continue; }
      if ((m = /^col(?:umn)?\s+\?([A-Za-z_]\w*)\s*:\s*(.+)$/i.exec(c))) {
        const [label, ...note] = m[2].split(/\s+[—–-]{1,2}\s+/);
        h.columns[m[1]] = { label: label.trim().slice(0, 80), note: note.join(" — ").trim().slice(0, 300) || null };
        continue;
      }
      if (!c) continue;
      if (h.title == null) h.title = c.slice(0, 200); else h.description.push(c);
    }
    h.description = h.description.join(" ").slice(0, 600) || null;
    return h;
  }

  const DIMENSION_VIEW = { geo: "map", spatial: "map", topology: "graph", links: "graph", graph: "graph", community: "graph" };
  function infer(sparql, dimension) {
    const s = String(sparql || "").replace(/^\s*#.*$/gm, " ");
    if (/^\s*(CONSTRUCT|DESCRIBE)\b/i.test(s.replace(/^\s*(PREFIX|BASE)\b[^\n]*$/gim, ""))) return "graph";
    if (/geo:asWKT|#asWKT>|wktLiteral|geo:hasGeometry|\?(wkt|geom|geometry|shape|boundary|footprint)\b|wgs84_pos#(lat|long)|geo:lat\b|schema:latitude/i.test(s)) return "map";
    if (dimension && DIMENSION_VIEW[String(dimension).toLowerCase()]) return DIMENSION_VIEW[String(dimension).toLowerCase()];
    if (/SELECT\s+(DISTINCT\s+)?\?s\s+\?p\s+\?o\b/i.test(s) && !/GROUP\s+BY|COUNT\s*\(/i.test(s)) return "graph";
    if (/schema:image|foaf:depiction|dbo:thumbnail|wdt:P18\b|schema:thumbnailUrl/i.test(s)) return "cards";
    return "table";
  }

  // an example as the explorer keeps it: {title, question, sparql, …} + view, viewHinted, columns
  function annotate(ex) {
    const hint = parse(ex.sparql);
    const view = hint.view || infer(ex.sparql, ex.dimension);
    return { ...ex, title: ex.title || hint.title || "Untitled query", question: ex.question || hint.description, view, viewHinted: !!hint.view, columns: hint.columns };
  }

  // the chips a query list shows: the view, then the explained columns
  function chips(ex, h, icon) {
    const box = h("div", { class: "rl-qtags", "data-rl": "qtags" });
    const v = h("span", { class: "rl-qtag rl-qtag-view" + (ex.viewHinted ? "" : " rl-qtag-guess"), title: ex.viewHinted ? "The view this query's author recommends" : "The view that suits this result (guessed from the query)" });
    const ic = icon(ex.view === "map" ? "map" : ex.view === "graph" ? "graph" : ex.view === "cards" ? "cards" : "tableView");
    if (ic) v.append(ic);
    v.append(VIEW_LABEL[ex.view] || ex.view);
    box.append(v);
    const cols = Object.entries(ex.columns || {});
    for (const [name, c] of cols.slice(0, 4)) box.append(h("span", { class: "rl-qtag", title: "?" + name + (c.note ? " — " + c.note : "") }, c.label));
    if (cols.length > 4) box.append(h("span", { class: "rl-qtag", title: cols.slice(4).map(([n, c]) => c.label).join(", ") }, "+" + (cols.length - 4)));
    return box;
  }

  window.RetelensHints = { parse, infer, annotate, chips, VIEW_LABEL };
})();
