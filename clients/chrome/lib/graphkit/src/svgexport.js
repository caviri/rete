import { GLYPHS, svg, trunc } from "./util.js";

// Own SVG export (cytoscape-svg is GPL-3.0, so it is not bundled). Draws the visible graph in model
// coordinates: edges (curved ones through their control point), node shapes, labels with a halo,
// and the hive / circos decoration if the layout has one. Everything uses presentation attributes.
export function buildSvg(kit) {
  const cy = kit.cy, th = kit.th();
  const nodes = cy.nodes(".gk-n").not(".gk-hide");
  const edges = cy.edges().not(".gk-hide");
  let bb = nodes.length ? nodes.boundingBox() : { x1: 0, y1: 0, x2: 100, y2: 100 };
  let { x1, y1, x2, y2 } = bb;
  if (kit.deco && kit.world.firstChild) {
    try { const d = kit.world.getBBox(); x1 = Math.min(x1, d.x); y1 = Math.min(y1, d.y); x2 = Math.max(x2, d.x + d.width); y2 = Math.max(y2, d.y + d.height); } catch (e) { /* not rendered */ }
  }
  const pad = 70;
  x1 -= pad; y1 -= pad; x2 += pad; y2 += pad;
  const w = x2 - x1, h = y2 - y1;
  const scale = Math.min(1, 2400 / Math.max(w, h));
  const root = svg("svg", { xmlns: "http://www.w3.org/2000/svg", viewBox: x1 + " " + y1 + " " + w + " " + h, width: Math.round(w * scale), height: Math.round(h * scale), "font-family": kit.tokens.fontSans });
  root.appendChild(svg("rect", { x: x1, y: y1, width: w, height: h, fill: th.surface }));
  if (kit.deco && kit.world.firstChild) { const g = svg("g", {}); for (const c of kit.world.childNodes) g.appendChild(c.cloneNode(true)); root.appendChild(g); }
  const eg = svg("g", { fill: "none", "stroke-linecap": "round" });
  const arrows = edges.length <= 1500;
  edges.forEach((e) => {
    const s = e.source().position(), t = e.target().position();
    const col = e.data("col"), ls = e.data("ls");
    const a = { stroke: col, "stroke-width": e.hasClass("gk-curved") ? 0.9 : 1.1, "stroke-opacity": e.hasClass("gk-curved") ? 0.4 : 0.55 };
    if (ls === "dashed") a["stroke-dasharray"] = "5 3"; else if (ls === "dotted") a["stroke-dasharray"] = "1.5 3";
    let d;
    const cp = e.hasClass("gk-curved") ? e.controlPoints() : [];
    if (cp && cp.length) d = "M" + s.x + " " + s.y + " Q" + cp[0].x + " " + cp[0].y + " " + t.x + " " + t.y;
    else if (e.data("source") === e.data("target")) { const r = (e.source().data("size") || 20); d = "M" + s.x + " " + s.y + " c " + r * 1.6 + " " + -r * 1.6 + " " + r * 1.6 + " " + r * 1.6 + " 0 0"; }
    else d = "M" + s.x + " " + s.y + " L" + t.x + " " + t.y;
    eg.appendChild(svg("path", Object.assign({ d }, a)));
    if (arrows && !e.hasClass("gk-curved") && e.data("source") !== e.data("target")) {
      const dx = t.x - s.x, dy = t.y - s.y, len = Math.hypot(dx, dy) || 1, ux = dx / len, uy = dy / len;
      const r = (e.target().data("size") || 20) / 2 + 2;
      const bx = t.x - ux * r, by = t.y - uy * r;
      eg.appendChild(svg("path", { d: "M" + bx + " " + by + " L" + (bx - ux * 7 - uy * 3) + " " + (by - uy * 7 + ux * 3) + " L" + (bx - ux * 7 + uy * 3) + " " + (by - uy * 7 - ux * 3) + " Z", fill: col, stroke: "none", "fill-opacity": 0.7 }));
    }
  });
  root.appendChild(eg);
  const ng = svg("g", {}), lg = svg("g", { "font-size": 11, "text-anchor": "middle", fill: th.text, stroke: th.surface, "stroke-width": 3, "stroke-linejoin": "round", "paint-order": "stroke" });
  nodes.forEach((n) => {
    const p = n.position(), size = n.data("size") || 20, k = size / 10;
    const kind = n.data("kind");
    const attrs = { d: GLYPHS[n.data("shp")] || GLYPHS.ellipse, transform: "translate(" + (p.x - size / 2) + " " + (p.y - size / 2) + ") scale(" + k + ")", fill: n.data("col"), stroke: kind === "iri" ? n.data("bc") : th.text, "stroke-width": 1.5 / k };
    if (kind === "literal") attrs["stroke-dasharray"] = 2 / k + " " + 1.5 / k; else if (kind === "bnode") attrs["stroke-dasharray"] = 0.5 / k + " " + 1.5 / k;
    ng.appendChild(svg("path", attrs));
    if (!n.hasClass("gk-nolabel") && n.data("lbl")) {
      const tx = svg("text", { x: p.x, y: p.y + size / 2 + 12 }, trunc(n.data("lbl"), 46));
      lg.appendChild(tx);
    }
  });
  root.appendChild(ng);
  root.appendChild(lg);
  return new XMLSerializer().serializeToString(root);
}
