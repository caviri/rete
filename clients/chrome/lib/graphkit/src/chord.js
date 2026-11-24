import { chordDirected, ribbon } from "d3-chord";
import { arc as d3arc } from "d3-shape";
import { svg, trunc, fmt } from "./util.js";

// Aggregate group -> group view. All styling is done with presentation attributes so the
// serialised SVG stands alone (SVG / PNG export).
// groups: [{name, color, count}], matrix[i][j] = number of edges from group i to group j.
export function renderChord(host, groups, matrix, t, tip) {
  while (host.firstChild) host.removeChild(host.firstChild);
  const W = Math.max(host.clientWidth, 300), H = Math.max(host.clientHeight, 300);
  const root = svg("svg", { xmlns: "http://www.w3.org/2000/svg", viewBox: "0 0 " + W + " " + H, width: W, height: H, role: "img", "aria-label": "Chord diagram of edges between groups" });
  root.appendChild(svg("rect", { x: 0, y: 0, width: W, height: H, fill: t.surface }));
  host.appendChild(root);
  const total = matrix.reduce((s, r) => s + r.reduce((a, b) => a + b, 0), 0);
  if (!groups.length || !total) {
    root.appendChild(svg("text", { x: W / 2, y: H / 2, "text-anchor": "middle", fill: t.muted, "font-size": 14, "font-family": t.font }, groups.length ? "No visible edges between the groups" : "Nothing to show"));
    return;
  }
  const labelRoom = 175;
  const outer = Math.max(60, Math.min(W - 2 * labelRoom, H - 70) / 2);
  const inner = outer - 16;
  const g = svg("g", { transform: "translate(" + W / 2 + " " + H / 2 + ")" });
  root.appendChild(g);
  const chords = chordDirected().padAngle(groups.length > 1 ? 0.05 : 0).sortSubgroups((a, b) => b - a)(matrix);
  const arc = d3arc().innerRadius(inner).outerRadius(outer);
  const rib = ribbon().radius(inner - 2);
  const outTotal = matrix.map((r) => r.reduce((a, b) => a + b, 0));
  const inTotal = groups.map((_, j) => matrix.reduce((s, r) => s + r[j], 0));
  const ribbons = [];
  const rg = svg("g", { fill: "none" });
  g.appendChild(rg);
  for (const c of chords) {
    const p = svg("path", { d: rib(c), fill: groups[c.source.index].color, "fill-opacity": 0.62, stroke: t.surface, "stroke-width": 0.8 });
    p.addEventListener("pointerenter", (ev) => {
      mark(ribbons, (x) => x === p);
      const a = c.source.index, b = c.target.index;
      tip.show(ev, [groups[a].name + " → " + groups[b].name + ": " + fmt(matrix[a][b]) + " edges"].concat(a !== b ? [groups[b].name + " → " + groups[a].name + ": " + fmt(matrix[b][a]) + " edges"] : []));
    });
    p.addEventListener("pointermove", (ev) => tip.move(ev));
    p.addEventListener("pointerleave", () => { mark(ribbons, null); tip.hide(); });
    ribbons.push(p); rg.appendChild(p);
    p._c = c;
  }
  function mark(list, pred) { for (const r of list) r.setAttribute("fill-opacity", pred === null ? 0.62 : pred(r) ? 0.95 : 0.08); }
  const labels = [];
  const arcs = svg("g", {});
  g.appendChild(arcs);
  for (const gr of chords.groups) {
    const i = gr.index;
    const a = svg("path", { d: arc(gr), fill: groups[i].color, stroke: t.surface, "stroke-width": 1, tabindex: 0 });
    const over = (ev) => {
      mark(ribbons, (r) => r._c.source.index === i || r._c.target.index === i);
      tip.show(ev, [groups[i].name + " (" + fmt(groups[i].count) + " nodes)", "out: " + fmt(outTotal[i]) + " edges · in: " + fmt(inTotal[i]) + " edges", "within the group: " + fmt(matrix[i][i])]);
    };
    a.addEventListener("pointerenter", over);
    a.addEventListener("pointermove", (ev) => tip.move(ev));
    a.addEventListener("pointerleave", () => { mark(ribbons, null); tip.hide(); });
    arcs.appendChild(a);
    const mid = (gr.startAngle + gr.endAngle) / 2;
    const lr = outer + 12;
    labels.push({ i, right: Math.sin(mid) >= 0, ax: outer * Math.sin(mid), ay: -outer * Math.cos(mid), x: lr * Math.sin(mid), y: -lr * Math.cos(mid) });
  }
  // labels: push apart vertically on each side so small arcs do not collide, with a leader line when moved
  const GAP = 19;
  for (const side of [true, false]) {
    const L = labels.filter((l) => l.right === side).sort((a, b) => a.y - b.y);
    for (let k = 1; k < L.length; k++) L[k].y = Math.max(L[k].y, L[k - 1].y + GAP);
    const lim = H / 2 - 12;
    if (L.length && L[L.length - 1].y > lim) { L[L.length - 1].y = lim; for (let k = L.length - 2; k >= 0; k--) L[k].y = Math.min(L[k].y, L[k + 1].y - GAP); }
    for (const l of L) {
      const gi = l.i;
      const lx = l.x + (side ? 6 : -6);
      {
        g.appendChild(svg("polyline", { points: l.ax + "," + l.ay + " " + l.x + "," + l.y + " " + lx + "," + l.y, fill: "none", stroke: t.muted, "stroke-width": 0.8, opacity: 0.6 }));
      }
      const lab = svg("text", { x: lx + (side ? 3 : -3), y: l.y, "text-anchor": side ? "start" : "end", "dominant-baseline": "middle", "font-size": 12.5, "font-family": t.font, fill: t.text, "font-weight": 600 });
      lab.appendChild(document.createTextNode(trunc(groups[gi].name, 24) + " "));
      lab.appendChild(svg("tspan", { "font-weight": 400, fill: t.muted, "font-size": 11 }, "· " + fmt(outTotal[gi] + inTotal[gi] - matrix[gi][gi]) + " edges"));
      g.appendChild(lab);
    }
  }
}
