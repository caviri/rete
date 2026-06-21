// schema-d3.js — an interactive ontology/schema graph for the detail page.
// Nodes are the card's classes (size ∝ instance count), links are its
// class_links (the class-to-class predicate relations), coloured by vocabulary.
// Hover/click a class to highlight its neighbourhood and show details (IRI,
// instances, vocabulary, top relations). Force-directed, draggable, zoomable.
import { hueOf } from "./procgen.js";

const fmt = (n) => (n == null ? "—" : Intl.NumberFormat().format(n));
const esc = (s) =>
  String(s == null ? "" : s).replace(/[&<>"]/g, (c) =>
    ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;" }[c])
  );
const strip = (iri) => String(iri).replace(/^[<(]|[>)]$/g, "");
const nsOf = (iri) => {
  const s = strip(iri);
  const m = s.match(/^(.*[#/])[^#/]+$/);
  return m ? m[1] : null;
};
const localName = (iri) => {
  const s = strip(iri);
  const m = s.match(/[#/]([^#/]+)\/?$/);
  return (m ? m[1] : s).slice(0, 22);
};

const HINT = `<div class="si-hint">Hover a class to see its relations; click to pin. Drag to rearrange, scroll to zoom.</div>`;

export function mountSchemaGraph(card, graphEl, infoEl) {
  if (typeof d3 === "undefined") {
    graphEl.innerHTML = `<div class="notice" style="padding:14px">d3 unavailable — schema graph skipped.</div>`;
    return;
  }
  const classes = card.classes || [];
  const rawLinks = card.class_links || [];
  if (!classes.length && !rawLinks.length) return;

  const countOf = new Map(classes.map(([iri, n]) => [iri, n]));
  const degree = new Map();
  for (const l of rawLinks) {
    degree.set(l.s_class, (degree.get(l.s_class) || 0) + (l.count || 0));
    degree.set(l.o_class, (degree.get(l.o_class) || 0) + (l.count || 0));
  }
  // Node set: every class plus every class_link endpoint, ranked by prominence,
  // capped so the graph stays legible.
  const ids = new Set([...countOf.keys()]);
  for (const l of rawLinks) { ids.add(l.s_class); ids.add(l.o_class); }
  const idList = [...ids]
    .sort((a, b) => (countOf.get(b) || 0) + (degree.get(b) || 0) - ((countOf.get(a) || 0) + (degree.get(a) || 0)))
    .slice(0, 48);
  const keep = new Set(idList);
  const nodes = idList.map((id) => ({
    id,
    count: countOf.get(id) || 0,
    deg: degree.get(id) || 0,
    ns: nsOf(id),
    label: localName(id),
  }));

  // Aggregate parallel class_links into one weighted edge (keep predicate tallies).
  const lmap = new Map();
  for (const l of rawLinks) {
    if (!keep.has(l.s_class) || !keep.has(l.o_class) || l.s_class === l.o_class) continue;
    const k = l.s_class + "" + l.o_class;
    const e = lmap.get(k) || { source: l.s_class, target: l.o_class, count: 0, preds: new Map() };
    e.count += l.count || 0;
    e.preds.set(l.predicate, (e.preds.get(l.predicate) || 0) + (l.count || 0));
    lmap.set(k, e);
  }
  const links = [...lmap.values()];

  // Neighbour sets (built from string ids before forceLink turns them into objects).
  const nbr = new Map(nodes.map((n) => [n.id, new Set()]));
  for (const l of links) { nbr.get(l.source).add(l.target); nbr.get(l.target).add(l.source); }

  const maxC = Math.max(1, ...nodes.map((n) => n.count || n.deg || 1));
  const radius = (n) => 5 + 13 * Math.sqrt((n.count || n.deg || 1) / maxC);
  const maxW = Math.max(1, ...links.map((l) => l.count));

  const W = graphEl.clientWidth || 640;
  const H = 440;
  graphEl.innerHTML = "";
  const svg = d3.select(graphEl).append("svg").attr("viewBox", `0 0 ${W} ${H}`).attr("width", "100%").attr("height", H);
  const g = svg.append("g");
  svg.call(d3.zoom().scaleExtent([0.3, 4]).on("zoom", (ev) => g.attr("transform", ev.transform)));

  const link = g.append("g").selectAll("line").data(links).join("line")
    .attr("class", "slink").attr("stroke-width", (d) => 1 + 3 * Math.sqrt(d.count / maxW));
  const nodeG = g.append("g").selectAll("g.snode-g").data(nodes).join("g").attr("class", "snode-g");
  nodeG.append("circle").attr("class", "snode").attr("r", radius)
    .style("fill", (d) => `hsl(${hueOf(d.ns || d.id)} 60% 55%)`);
  // Little radial ticks around each node — how many grows with its connectivity.
  nodeG.each(function (d) {
    const r = radius(d);
    const t = Math.min(14, Math.max(4, (nbr.get(d.id) || new Set()).size));
    const sel = d3.select(this);
    for (let i = 0; i < t; i++) {
      const a = (i / t) * Math.PI * 2;
      sel.append("line").attr("class", "stick")
        .attr("x1", Math.cos(a) * r).attr("y1", Math.sin(a) * r)
        .attr("x2", Math.cos(a) * (r + 3.5)).attr("y2", Math.sin(a) * (r + 3.5));
    }
  });
  const label = g.append("g").selectAll("text").data(nodes).join("text")
    .attr("class", "slabel").attr("text-anchor", "middle").text((d) => d.label);

  const sim = d3.forceSimulation(nodes)
    .force("link", d3.forceLink(links).id((d) => d.id).distance(64).strength(0.25))
    .force("charge", d3.forceManyBody().strength(-200))
    .force("center", d3.forceCenter(W / 2, H / 2))
    .force("collide", d3.forceCollide().radius((d) => radius(d) + 5))
    .on("tick", () => {
      link.attr("x1", (d) => d.source.x).attr("y1", (d) => d.source.y).attr("x2", (d) => d.target.x).attr("y2", (d) => d.target.y);
      nodeG.attr("transform", (d) => `translate(${d.x},${d.y})`);
      label.attr("x", (d) => d.x).attr("y", (d) => d.y - radius(d) - 3);
    });

  nodeG.call(
    d3.drag()
      .on("start", (ev, d) => { if (!ev.active) sim.alphaTarget(0.3).restart(); d.fx = d.x; d.fy = d.y; })
      .on("drag", (ev, d) => { d.fx = ev.x; d.fy = ev.y; })
      .on("end", (ev, d) => { if (!ev.active) sim.alphaTarget(0); d.fx = null; d.fy = null; })
  );

  let pinned = null;
  nodeG.on("mouseover", (ev, d) => { if (!pinned) highlight(d); })
    .on("mouseout", () => { if (!pinned) clearHi(); })
    .on("click", (ev, d) => { pinned = pinned === d ? null : d; pinned ? highlight(d) : clearHi(); ev.stopPropagation(); });
  svg.on("click", () => { pinned = null; clearHi(); });

  infoEl.innerHTML = HINT;

  function highlight(d) {
    const keepSet = new Set(nbr.get(d.id) || []);
    keepSet.add(d.id);
    nodeG.classed("dim", (n) => !keepSet.has(n.id)).classed("hot", (n) => n.id === d.id);
    label.classed("dim", (n) => !keepSet.has(n.id));
    link.classed("dim", (l) => l.source.id !== d.id && l.target.id !== d.id)
      .classed("hot", (l) => l.source.id === d.id || l.target.id === d.id);
    infoEl.innerHTML = details(d);
  }
  function clearHi() {
    nodeG.classed("dim", false).classed("hot", false);
    label.classed("dim", false);
    link.classed("dim", false).classed("hot", false);
    infoEl.innerHTML = HINT;
  }

  function details(d) {
    const rels = links
      .filter((l) => l.source.id === d.id || l.target.id === d.id)
      .map((l) => {
        const out = l.source.id === d.id;
        const other = out ? l.target : l.source;
        const top = [...l.preds.entries()].sort((a, b) => b[1] - a[1])[0];
        return { other, pred: top ? top[0] : "", count: l.count, dir: out ? "→" : "←" };
      })
      .sort((a, b) => b.count - a.count)
      .slice(0, 10);
    return `
      <div class="si-title">${esc(d.label)}</div>
      <div class="si-iri">${esc(strip(d.id))}</div>
      <dl class="kv">
        ${d.count ? `<dt>instances</dt><dd>${fmt(d.count)}</dd>` : ""}
        ${d.ns ? `<dt>vocabulary</dt><dd class="mono">${esc(d.ns)}</dd>` : ""}
        <dt>connections</dt><dd>${rels.length}</dd>
      </dl>
      <div class="si-rels">${rels
        .map((r) => `<div class="si-rel"><span class="dir">${r.dir}</span> <b>${esc(localName(r.pred))}</b> ${esc(r.other.label)} <span class="n">${fmt(r.count)}</span></div>`)
        .join("")}</div>`;
  }
}
