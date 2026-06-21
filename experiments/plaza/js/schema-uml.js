// schema-uml.js — the ontology/schema as a compact UML class diagram (à la
// OpenPULSE). Boxes start COLLAPSED (class name only) so only the object
// properties that *join* classes are shown; click a box to expand its literal
// properties (re-layouts). Layout + PCB-style orthogonal edge routing by ELK,
// flowing left→right so the main classes read from the top-left.
import { hueOf } from "./procgen.js";

const esc = (s) =>
  String(s == null ? "" : s).replace(/[&<>"]/g, (c) =>
    ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;" }[c])
  );
const fmt = (n) => (n == null ? "—" : Intl.NumberFormat().format(n));
const strip = (iri) => String(iri).replace(/^[<(]|[>)]$/g, "");
const isClass = (iri) => /^</.test(String(iri));
const nsOf = (iri) => { const s = strip(iri); const m = s.match(/^(.*[#/])[^#/]+$/); return m ? m[1] : null; };
const localName = (iri) => { const s = strip(iri); const m = s.match(/[#/]([^#/]+)\/?$/); return (m ? m[1] : s).slice(0, 28); };

const TITLE_FONT = "600 13px ui-sans-serif, system-ui, sans-serif";
const ATTR_FONT = "11px ui-monospace, Consolas, monospace";
const HEADER_H = 24, ROW_H = 15, PAD_X = 9;

export async function mountSchemaUML(card, graphEl, infoEl) {
  if (typeof ELK === "undefined") { graphEl.innerHTML = `<div class="notice" style="padding:14px">ELK unavailable — diagram skipped.</div>`; return; }
  const classes = card.classes || [];
  const links = card.class_links || [];
  if (!classes.length && !links.length) return;
  graphEl.innerHTML = `<div class="notice" style="padding:14px">laying out the schema…</div>`;

  // --- pick boxes (real classes), ranked by instances + connectivity ---
  const countOf = new Map(classes.map(([i, n]) => [i, n]));
  const degree = new Map();
  for (const l of links) {
    degree.set(l.s_class, (degree.get(l.s_class) || 0) + (l.count || 0));
    if (isClass(l.o_class)) degree.set(l.o_class, (degree.get(l.o_class) || 0) + (l.count || 0));
  }
  const ids = new Set([...countOf.keys()].filter(isClass));
  for (const l of links) { if (isClass(l.s_class)) ids.add(l.s_class); if (isClass(l.o_class)) ids.add(l.o_class); }
  const idList = [...ids]
    .sort((a, b) => (countOf.get(b) || 0) + (degree.get(b) || 0) - ((countOf.get(a) || 0) + (degree.get(a) || 0)))
    .slice(0, 16);
  const keep = new Set(idList);

  // --- attributes (literal props) + object-property edges ---
  const attrs = new Map(), emap = new Map();
  for (const l of links) {
    if (!keep.has(l.s_class)) continue;
    if (isClass(l.o_class)) {
      if (!keep.has(l.o_class) || l.s_class === l.o_class) continue;
      const k = l.s_class + "|" + l.o_class + "|" + l.predicate;
      const e = emap.get(k) || { s: l.s_class, o: l.o_class, pred: l.predicate, count: 0 };
      e.count += l.count || 0; emap.set(k, e);
    } else {
      const a = attrs.get(l.s_class) || [];
      a.push({ pred: l.predicate, count: l.count || 0, dt: /untyped/i.test(l.o_class) ? "untyped" : "literal" });
      attrs.set(l.s_class, a);
    }
  }
  const edges = [...emap.values()].sort((a, b) => b.count - a.count).slice(0, 48);

  const ctx = document.createElement("canvas").getContext("2d");
  const tw = (t, f) => { ctx.font = f; return ctx.measureText(t).width; };
  const nodes = idList.map((id, i) => {
    const at = (attrs.get(id) || []).sort((a, b) => b.count - a.count).slice(0, 8).map((a) => `${localName(a.pred)} : ${a.dt}`);
    return {
      id, eid: "n" + i, title: localName(id), attrs: at, count: countOf.get(id) || 0, hue: hueOf(nsOf(id) || id),
      titleW: tw(localName(id), TITLE_FONT),
      attrW: at.reduce((m, a) => Math.max(m, tw(a, ATTR_FONT)), 0),
    };
  });
  const byId = new Map(nodes.map((n) => [n.id, n]));
  const adj = new Map(nodes.map((n) => [n.id, new Set()]));
  for (const e of edges) { adj.get(e.s).add(e.o); adj.get(e.o).add(e.s); }

  const elk = new ELK();
  const expanded = new Set();
  let pinned = null;
  const HINT = `<div class="si-hint">Compact UML — boxes show only the properties that join classes; <b>click a class to expand</b> its literal properties. Arrows are object properties (orthogonally routed). Scroll to zoom.</div>`;
  infoEl.innerHTML = HINT;

  await relayout();

  async function relayout() {
    for (const n of nodes) {
      const exp = expanded.has(n.id);
      const badge = n.attrs.length ? 30 : 14;
      n.w = Math.ceil(Math.min(360, Math.max(n.titleW + badge, exp ? n.attrW + 2 * PAD_X : 0)));
      n.h = HEADER_H + (exp ? n.attrs.length * ROW_H + 6 : 0);
    }
    let layout;
    try {
      layout = await elk.layout({
        id: "root",
        layoutOptions: {
          "elk.algorithm": "layered",
          "elk.direction": "RIGHT",
          "elk.edgeRouting": "ORTHOGONAL",
          "elk.layered.spacing.nodeNodeBetweenLayers": "52",
          "elk.spacing.nodeNode": "24",
          "elk.spacing.edgeNode": "16",
          "elk.spacing.edgeEdge": "10",
          "elk.layered.mergeEdges": "true",
          "elk.layered.nodePlacement.strategy": "NETWORK_SIMPLEX",
        },
        children: nodes.map((n) => ({ id: n.eid, width: n.w, height: n.h })),
        edges: edges.map((e, i) => ({
          id: "e" + i, sources: [byId.get(e.s).eid], targets: [byId.get(e.o).eid],
          labels: [{ text: localName(e.pred), width: Math.ceil(tw(localName(e.pred), ATTR_FONT)) + 6, height: 14 }],
        })),
      });
    } catch (err) { graphEl.innerHTML = `<div class="warnbox" style="border-radius:10px">Layout failed: ${esc(err)}</div>`; return; }
    draw(layout);
  }

  function draw(layout) {
    const pos = new Map(layout.children.map((c) => [c.id, c]));
    const W = Math.ceil(Math.max(1, layout.width || 1)), H = Math.ceil(Math.max(1, layout.height || 1));
    let svg = `<svg viewBox="0 0 ${W} ${H}" preserveAspectRatio="xMidYMid meet" class="uml">`;
    svg += `<defs><marker id="uarrow" viewBox="0 0 10 10" refX="9" refY="5" markerWidth="7" markerHeight="7" orient="auto-start-reverse"><path d="M0 0 L10 5 L0 10 z" class="uarrow"/></marker></defs><g class="uzoom">`;

    for (let i = 0; i < edges.length; i++) {
      const e = layout.edges[i];
      if (!e || !e.sections || !e.sections.length) continue;
      const s = e.sections[0];
      const pts = [s.startPoint, ...(s.bendPoints || []), s.endPoint].map((p) => `${p.x.toFixed(1)},${p.y.toFixed(1)}`).join(" ");
      svg += `<polyline class="uedge" data-s="${esc(edges[i].s)}" data-o="${esc(edges[i].o)}" points="${pts}" marker-end="url(#uarrow)"/>`;
      const lab = e.labels && e.labels[0];
      if (lab) svg += `<g class="uedge-label" data-s="${esc(edges[i].s)}" data-o="${esc(edges[i].o)}"><rect x="${(lab.x - 2).toFixed(1)}" y="${lab.y.toFixed(1)}" width="${(lab.width + 4).toFixed(1)}" height="${lab.height.toFixed(1)}" rx="2"/><text x="${lab.x.toFixed(1)}" y="${(lab.y + lab.height - 3).toFixed(1)}">${esc(localName(edges[i].pred))}</text></g>`;
    }

    for (const n of nodes) {
      const p = pos.get(n.eid); if (!p) continue;
      const w = p.width, h = p.height, exp = expanded.has(n.id);
      const tint = `hsl(${n.hue} 55% 55% / 0.9)`;
      svg += `<g class="ubox ${exp ? "expanded" : "collapsed"}" data-id="${esc(n.id)}" transform="translate(${p.x.toFixed(1)},${p.y.toFixed(1)})">`;
      if (exp) {
        svg += `<rect class="ubox-bg" width="${w}" height="${h}" rx="5"/>`;
        svg += `<path class="ubox-hd" d="M0 ${HEADER_H} L0 5 Q0 0 5 0 L${w - 5} 0 Q${w} 0 ${w} 5 L${w} ${HEADER_H} Z" style="fill:${tint}"/>`;
      } else {
        svg += `<rect class="ubox-hdfull" width="${w}" height="${h}" rx="5" style="fill:${tint}"/>`;
      }
      svg += `<text class="ubox-title" x="${PAD_X}" y="16">${esc(n.title)}</text>`;
      if (n.attrs.length) svg += `<text class="ubox-badge" x="${w - 7}" y="16" text-anchor="end">${exp ? "–" : "+" + n.attrs.length}</text>`;
      if (exp) n.attrs.forEach((a, i) => { svg += `<text class="ubox-attr" x="${PAD_X}" y="${HEADER_H + i * ROW_H + 11}">${esc(a)}</text>`; });
      svg += `</g>`;
    }
    svg += `</g></svg>`;
    graphEl.innerHTML = svg;

    const svgEl = graphEl.querySelector("svg");
    const boxes = [...graphEl.querySelectorAll(".ubox")];
    const polys = [...graphEl.querySelectorAll(".uedge")];
    const labels = [...graphEl.querySelectorAll(".uedge-label")];
    const clear = () => {
      boxes.forEach((b) => b.classList.remove("dim", "hot"));
      polys.forEach((p) => p.classList.remove("dim", "hot"));
      labels.forEach((l) => l.classList.remove("dim", "hot"));
      infoEl.innerHTML = HINT;
    };
    const hi = (id) => {
      const keepSet = new Set(adj.get(id) || []); keepSet.add(id);
      boxes.forEach((b) => { b.classList.toggle("dim", !keepSet.has(b.dataset.id)); b.classList.toggle("hot", b.dataset.id === id); });
      const on = (el) => el.dataset.s === id || el.dataset.o === id;
      polys.forEach((p) => { p.classList.toggle("hot", on(p)); p.classList.toggle("dim", !on(p)); });
      labels.forEach((l) => { l.classList.toggle("hot", on(l)); l.classList.toggle("dim", !on(l)); });
      infoEl.innerHTML = details(id, nodes, edges, byId, card, expanded.has(id));
    };
    boxes.forEach((b) => {
      const id = b.dataset.id, n = byId.get(id);
      b.addEventListener("mouseenter", () => { if (!pinned) hi(id); });
      b.addEventListener("mouseleave", () => { if (!pinned) clear(); });
      b.addEventListener("click", (ev) => {
        ev.stopPropagation();
        pinned = id;
        if (n && n.attrs.length) { expanded.has(id) ? expanded.delete(id) : expanded.add(id); relayout(); }
        else hi(id);
      });
    });
    svgEl.addEventListener("click", () => { pinned = null; clear(); });
    if (pinned) hi(pinned);

    if (typeof d3 !== "undefined") {
      const g = d3.select(graphEl).select("g.uzoom");
      d3.select(svgEl).call(d3.zoom().scaleExtent([0.2, 4]).on("zoom", (ev) => g.attr("transform", ev.transform)));
    }
  }
}

function details(id, nodes, edges, byId, card, isExpanded) {
  const n = byId.get(id);
  const rels = edges.filter((e) => e.s === id || e.o === id)
    .map((e) => ({ out: e.s === id, other: byId.get(e.s === id ? e.o : e.s), pred: e.pred, count: e.count }))
    .sort((a, b) => b.count - a.count).slice(0, 12);
  return `
    <div class="si-title">${esc(n.title)}</div>
    <div class="si-iri">${esc(strip(id))}</div>
    <dl class="kv">
      ${n.count ? `<dt>instances</dt><dd>${fmt(n.count)}</dd>` : ""}
      ${n.attrs.length ? `<dt>properties</dt><dd>${n.attrs.length}${isExpanded ? "" : " (click box to show)"}</dd>` : ""}
      <dt>relations</dt><dd>${rels.length}</dd>
    </dl>
    ${n.attrs.length ? `<div class="si-rels">${n.attrs.map((a) => `<div class="si-rel mono">${esc(a)}</div>`).join("")}</div>` : ""}
    <div class="si-rels">${rels.map((r) => `<div class="si-rel"><span class="dir">${r.out ? "→" : "←"}</span> <b>${esc(localName(r.pred))}</b> ${esc(r.other ? r.other.title : "?")} <span class="n">${fmt(r.count)}</span></div>`).join("")}</div>`;
}
