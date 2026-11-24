import { arc as d3arc } from "d3-shape";
import { svg, trunc } from "./util.js";

// Angles follow d3: 0 = 12 o'clock, clockwise.
const pt = (th, r) => ({ x: r * Math.sin(th), y: -r * Math.cos(th) });

// Split nodes into at most 5 axes. `keyOf(node) -> {key,name,slot}`; the biggest groups
// get their own axis, the rest share one "Other" axis. Fewer than 3 distinct keys: the caller
// should pass a finer key (communities).
export function makeAxes(nodes, keyOf, maxAxes = 5) {
  const by = new Map();
  for (const n of nodes) {
    const k = keyOf(n);
    let a = by.get(k.key);
    if (!a) by.set(k.key, (a = { key: k.key, name: k.name, slot: k.slot, nodes: [] }));
    a.nodes.push(n);
  }
  let axes = [...by.values()].sort((a, b) => b.nodes.length - a.nodes.length);
  if (axes.length > maxAxes) {
    const rest = axes.slice(maxAxes - 1);
    const merged = { key: "#rest", name: "Other", slot: 8, nodes: [].concat(...rest.map((a) => a.nodes)) };
    axes = axes.slice(0, maxAxes - 1).concat(merged);
  }
  return axes;
}

export function hive(axes, edges) {
  const k = axes.length;
  const maxCount = Math.max(...axes.map((a) => a.nodes.length), 1);
  const L = Math.max(320, Math.min(1000, maxCount * 8));
  const r0 = 50;
  const pos = new Map(), axisOf = new Map();
  const deco = { type: "hive", axes: [] };
  axes.forEach((a, i) => {
    const th = (i * 2 * Math.PI) / k;
    const sorted = a.nodes.slice().sort((p, q) => p.deg - q.deg || (p.id < q.id ? -1 : 1));
    const m = sorted.length;
    sorted.forEach((n, j) => {
      const r = r0 + (L - r0) * (m === 1 ? 0.5 : j / (m - 1));
      const p = pt(th, r);
      const off = ((j % 3) - 1) * 7; // spread nodes with equal rank sideways a little
      pos.set(n.id, { x: p.x + off * Math.cos(th), y: p.y + off * Math.sin(th) });
      axisOf.set(n.id, i);
    });
    deco.axes.push({ th, r0, L, name: a.name, slot: a.slot, count: m });
  });
  const curves = edgeCurves(edges, pos, (e) => {
    const sa = axisOf.get(e.source), ta = axisOf.get(e.target);
    return sa === ta ? { mode: "bulge", th: axes.length ? deco.axes[sa].th : 0 } : { mode: "centre", pull: 0.18 };
  });
  return { pos, curves, deco };
}

export function circos(axes, edges) {
  const total = axes.reduce((s, a) => s + a.nodes.length, 0) || 1;
  const gap = axes.length > 1 ? 0.045 : 0;
  const avail = 2 * Math.PI - gap * axes.length;
  const R = Math.max(300, Math.min(1500, total * 3.2));
  const pos = new Map(), deco = { type: "circos", R, arcs: [] };
  let th = 0;
  for (const a of axes) {
    const span = avail * (a.nodes.length / total);
    const sorted = a.nodes.slice().sort((p, q) => q.deg - p.deg || (p.id < q.id ? -1 : 1));
    sorted.forEach((n, j) => pos.set(n.id, pt(th + span * ((j + 0.5) / sorted.length), R)));
    deco.arcs.push({ a0: th, a1: th + span, name: a.name, slot: a.slot, count: a.nodes.length });
    th += span + gap;
  }
  const curves = edgeCurves(edges, pos, () => ({ mode: "centre", pull: 0.12 }));
  return { pos, curves, deco };
}

// Control point of a quadratic curve per edge, expressed the way cytoscape's unbundled-bezier
// wants it: signed distance from the straight line and the weight along it.
function edgeCurves(edges, pos, how) {
  const out = new Map();
  for (const e of edges) {
    const s = pos.get(e.source), t = pos.get(e.target);
    if (!s || !t) continue;
    const dx = t.x - s.x, dy = t.y - s.y, len = Math.hypot(dx, dy);
    if (len < 1e-6) continue;
    const h = how(e);
    let c;
    if (h.mode === "bulge") {
      const nx = Math.cos(h.th), ny = Math.sin(h.th); // sideways of an axis at angle th
      c = { x: (s.x + t.x) / 2 + nx * len * 0.3, y: (s.y + t.y) / 2 + ny * len * 0.3 };
    } else {
      const mx = (s.x + t.x) / 2, my = (s.y + t.y) / 2, mr = Math.hypot(mx, my) || 1;
      // pulled toward the centre, but short edges between neighbours only dip a little
      const dip = Math.min(mr * (1 - h.pull), Math.max(len * 1.6, 24));
      c = { x: mx - (mx / mr) * dip, y: my - (my / mr) * dip };
    }
    const ux = dx / len, uy = dy / len;
    const vx = c.x - s.x, vy = c.y - s.y;
    out.set(e.id, { w: (vx * ux + vy * uy) / len, d: -(vx * -uy + vy * ux) });
  }
  return out;
}

// Decoration drawn in model coordinates in the SVG under the cytoscape canvas.
export function drawDeco(g, deco, ctx) {
  while (g.firstChild) g.removeChild(g.firstChild);
  if (!deco) return;
  const { color, text, muted, fontSans, border } = ctx;
  if (deco.type === "hive") {
    for (const a of deco.axes) {
      const p0 = pt(a.th, a.r0 - 14), p1 = pt(a.th, a.L + 14);
      g.appendChild(svg("line", { x1: p0.x, y1: p0.y, x2: p1.x, y2: p1.y, stroke: muted, "stroke-width": 2, "stroke-linecap": "round", opacity: 0.55 }));
      const fs = Math.max(15, a.L * 0.04);
      const lp = pt(a.th, a.L + 46);
      const sn = Math.sin(a.th);
      const side = Math.abs(sn) < 0.35 ? "middle" : sn > 0 ? "start" : "end";
      const dx = side === "start" ? fs * 1.1 : side === "end" ? -fs * 1.1 : 0;
      const dy = side === "middle" ? (Math.cos(a.th) > 0 ? -fs * 1.5 : fs * 1.2) : 0;
      const col = color(a.slot);
      g.appendChild(svg("circle", { cx: lp.x + dx + (side === "start" ? -fs * 0.8 : side === "end" ? fs * 0.8 : -(Math.min(a.name.length, 28) * fs * 0.3 + fs * 0.8)), cy: lp.y + dy - fs * 0.3, r: fs * 0.3, fill: col }));
      g.appendChild(svg("text", { x: lp.x + dx, y: lp.y + dy, "text-anchor": side, "font-size": fs, "font-family": fontSans, fill: text, "font-weight": 600 }, trunc(a.name, 28)));
      g.appendChild(svg("text", { x: lp.x + dx, y: lp.y + dy + fs * 1.25, "text-anchor": side, "font-size": fs * 0.78, "font-family": fontSans, fill: muted }, a.count + " nodes"));
    }
  } else if (deco.type === "circos") {
    const R = deco.R;
    const ring = d3arc().innerRadius(R + 18).outerRadius(R + 40);
    const fs = Math.max(16, R * 0.05);
    g.appendChild(svg("circle", { cx: 0, cy: 0, r: R + 11, fill: "none", stroke: border, "stroke-width": 1 }));
    for (const a of deco.arcs) {
      const d = ring({ startAngle: a.a0, endAngle: Math.max(a.a1, a.a0 + 0.004) });
      g.appendChild(svg("path", { d, fill: color(a.slot) }));
      const mid = (a.a0 + a.a1) / 2;
      const lp = pt(mid, R + 52);
      const right = Math.sin(mid) >= 0;
      const deg = (mid * 180) / Math.PI - 90;
      const rot = right ? deg : deg + 180;
      const t = svg("text", { x: 0, y: fs * 0.35, "text-anchor": right ? "start" : "end", "font-size": fs, "font-family": fontSans, fill: text, transform: "translate(" + lp.x + " " + lp.y + ") rotate(" + rot + ")" }, trunc(a.name, 30) + " (" + a.count + ")");
      g.appendChild(t);
    }
  }
}
