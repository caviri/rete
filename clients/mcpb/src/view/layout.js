// Where the boxes go in the layered drawing: dagre for what is linked, top
// down, and rows underneath for what is not. Plain arithmetic over sizes, so it
// is tested in Node (test/layout.test.mjs) apart from the page that draws it.
import { layout as dagreLayout, graphlib } from "@dagrejs/dagre";

/**
 * nodes: [{ id, w, h }]; edges: [{ id, source, target }].
 * Answers { pos: { id: { x, y } }, error }: `error` is set when dagre could
 * not lay the linked nodes out, and then every node is packed in rows.
 */
export function layeredPositions(nodes, edges) {
  const size = new Map(nodes.map((n) => [n.id, n]));
  const linked = new Set();
  // One edge per ordered pair: dagre places nodes the same either way, and
  // parallel edges (Battle → Color three times, by three predicates, in the
  // MTG schema) make it throw "Not possible to find intersection inside of the
  // rectangle".
  const pairs = new Set();
  const kept = edges.filter(({ source: a, target: b }) => {
    if (a === b || !size.has(a) || !size.has(b)) return false;
    linked.add(a).add(b);
    const pair = `${a}\u0000${b}`;
    if (pairs.has(pair)) return false;
    pairs.add(pair);
    return true;
  });
  let pos = {};
  let right = 0;
  let bottom = 0;
  let error = null;
  if (linked.size) {
    const g = new graphlib.Graph({ multigraph: true });
    g.setGraph({ rankdir: "TB", nodesep: 30, ranksep: 64, edgesep: 14 });
    g.setDefaultEdgeLabel(() => ({}));
    for (const id of linked) g.setNode(id, { width: size.get(id).w, height: size.get(id).h });
    for (const e of kept) g.setEdge(e.source, e.target, {}, e.id);
    try {
      dagreLayout(g);
      for (const id of linked) {
        const p = g.node(id);
        pos[id] = { x: p.x, y: p.y };
        right = Math.max(right, p.x + p.width / 2);
        bottom = Math.max(bottom, p.y + p.height / 2);
      }
    } catch (e) {
      // Whatever else dagre trips on, the boxes still get drawn: in rows.
      error = e;
      pos = {};
      right = bottom = 0;
      linked.clear();
    }
  }
  const loose = nodes.filter((n) => !linked.has(n.id));
  if (loose.length) {
    const gap = 24;
    const area = loose.reduce((a, n) => a + (n.w + gap) * (n.h + gap), 0);
    const width = Math.max(right, Math.sqrt(area) * 1.6, 360);
    let x = 0;
    let y = linked.size ? bottom + 64 : 0;
    let rowH = 0;
    for (const n of loose) {
      if (x > 0 && x + n.w > width) {
        x = 0;
        y += rowH + gap;
        rowH = 0;
      }
      pos[n.id] = { x: x + n.w / 2, y: y + n.h / 2 };
      x += n.w + gap;
      rowH = Math.max(rowH, n.h);
    }
  }
  return { pos, error };
}
