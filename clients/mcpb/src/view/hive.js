// The hive plot's arithmetic: which axis each node sits on, where along it, and
// how far each edge bows. Plain functions over plain data, so they are tested in
// Node (test/hive.test.mjs) apart from the page that draws them.
//
// A hive plot puts nodes on a few radial axes, each ordered by a measure, and
// draws an edge as a curve between two axes. Where a node sits is decided by
// what it is and how connected it is, not by a layout's mood, so two plots of
// the same graph can be compared.

export const HIVE_AXES = ["auto", "type", "role", "kind", "degree"];
export const HIVE_ORDERS = ["degree", "in", "out", "name"];
const MAX_AXES = 6; // the rest of the classes share the last axis

const ROLE_NAMES = { source: "sources", both: "both", sink: "sinks" };
const KIND_NAMES = { iri: "resources", literal: "literals", bnode: "blank nodes", class: "classes" };

/** In and out degree of every node, over the edges that join two of the given nodes. */
export function degrees(nodes, edges) {
  const d = new Map(nodes.map((n) => [n.id, { in: 0, out: 0 }]));
  for (const e of edges) {
    if (e.source === e.target || !d.has(e.source) || !d.has(e.target)) continue;
    d.get(e.source).out++;
    d.get(e.target).in++;
  }
  return d;
}

const byCount = (a, b) => b[1] - a[1] || String(a[0]).localeCompare(String(b[0]));

/** A key per node, from `by`, and the keys in axis order. */
function keysOf(nodes, deg, by) {
  const keys = new Map();
  if (by === "role") {
    for (const n of nodes) {
      const { in: i, out: o } = deg.get(n.id);
      keys.set(n.id, o > 0 && i === 0 ? ROLE_NAMES.source : i > 0 && o === 0 ? ROLE_NAMES.sink : ROLE_NAMES.both);
    }
    return { keys, order: Object.values(ROLE_NAMES) };
  }
  if (by === "kind") {
    for (const n of nodes) keys.set(n.id, KIND_NAMES[n.kind] ?? "other");
    return { keys, order: Object.values(KIND_NAMES).concat("other") };
  }
  if (by === "degree") {
    // Thirds by total degree: the axes of a graph with nothing else to tell its nodes apart.
    const total = (n) => deg.get(n.id).in + deg.get(n.id).out;
    const ranked = [...nodes].sort((a, b) => total(a) - total(b) || String(a.id).localeCompare(String(b.id)));
    const names = ["few links", "some links", "many links"];
    ranked.forEach((n, i) => keys.set(n.id, names[Math.min(2, Math.floor((i * 3) / ranked.length))]));
    return { keys, order: names };
  }
  // by type: the commonest classes, one axis each, then one for the rest.
  const count = new Map();
  for (const n of nodes) {
    const t = n.type || "(untyped)";
    keys.set(n.id, t);
    count.set(t, (count.get(t) ?? 0) + 1);
  }
  const ranked = [...count].sort(byCount).map(([t]) => t);
  if (ranked.length > MAX_AXES) {
    const keep = ranked.slice(0, MAX_AXES - 1);
    for (const [id, t] of keys) if (!keep.includes(t)) keys.set(id, "other");
    return { keys, order: [...keep, "other"] };
  }
  return { keys, order: ranked };
}

/**
 * Which axis each node is on. nodes: [{ id, kind, type }]; edges:
 * [{ source, target }]; by: "auto" | "type" | "role" | "kind" | "degree".
 * "auto" takes the first of type, role, kind that tells at least two groups
 * apart, and degree thirds when none does.
 * Answers { by, axes: [{ key, count }], axisOf: Map(id -> index), deg }.
 */
export function hiveAxes(nodes, edges, by = "auto") {
  const deg = degrees(nodes, edges);
  const tries = by === "auto" ? ["type", "role", "kind", "degree"] : [by];
  let made = null;
  for (const b of tries) {
    const k = keysOf(nodes, deg, b);
    const present = new Set(k.keys.values());
    made = { by: b, k, used: k.order.filter((key) => present.has(key)) };
    if (made.used.length >= 2) break;
  }
  const { by: resolved, k, used } = made;
  const axisOf = new Map();
  const count = new Map(used.map((key) => [key, 0]));
  for (const n of nodes) {
    const key = k.keys.get(n.id);
    axisOf.set(n.id, used.indexOf(key));
    count.set(key, count.get(key) + 1);
  }
  return { by: resolved, axes: used.map((key) => ({ key, count: count.get(key) })), axisOf, deg };
}

/** The angle of axis i of n, in radians, the first pointing up; two axes sit a quarter turn apart. */
export function axisAngle(i, n) {
  const step = n === 2 ? Math.PI / 2 : (2 * Math.PI) / Math.max(n, 1);
  return -Math.PI / 2 + i * step;
}

/** An angle brought into (-pi, pi]. */
const wrap = (a) => {
  const t = ((((a + Math.PI) % (2 * Math.PI)) + 2 * Math.PI) % (2 * Math.PI)) - Math.PI;
  return t === -Math.PI ? Math.PI : t;
};

/**
 * Where everything goes. Along an axis nodes run from the inner radius out in
 * ascending order of `order` (ties by name), so the best connected sit outermost.
 * Answers { pos: { id: { x, y } }, axes: [{ key, count, angle, from, to }],
 * bow: { edgeId: distance }, by }. `bow` is the signed distance, from the
 * middle of the edge, of the control point that bends it between its axes: what
 * cytoscape calls control-point-distances at weight 0.5, measured along the
 * normal (-dy, dx) of the edge's direction.
 */
export function hivePositions(nodes, edges, { by = "auto", order = "degree", inner = 40 } = {}) {
  const made = hiveAxes(nodes, edges, by);
  const n = made.axes.length;
  const label = new Map(nodes.map((x) => [x.id, String(x.label ?? x.id)]));
  const measure = (id) => {
    const d = made.deg.get(id);
    return order === "in" ? d.in : order === "out" ? d.out : order === "name" ? 0 : d.in + d.out;
  };
  const members = made.axes.map(() => []);
  for (const node of nodes) members[made.axisOf.get(node.id)].push(node.id);
  const longest = Math.max(1, ...members.map((m) => m.length));
  const length = Math.min(1200, Math.max(280, longest * 10));
  const pos = {};
  const radius = {};
  members.forEach((ids, a) => {
    ids.sort((p, q) => measure(p) - measure(q) || label.get(p).localeCompare(label.get(q)) || String(p).localeCompare(String(q)));
    const angle = axisAngle(a, n);
    ids.forEach((id, k) => {
      const r = ids.length === 1 ? inner + length / 2 : inner + (length * k) / (ids.length - 1);
      radius[id] = r;
      pos[id] = { x: r * Math.cos(angle), y: r * Math.sin(angle) };
    });
  });
  const bow = {};
  for (const e of edges) {
    const s = pos[e.source];
    const t = pos[e.target];
    if (!s || !t || e.source === e.target) continue;
    const dx = t.x - s.x;
    const dy = t.y - s.y;
    const len = Math.hypot(dx, dy);
    if (len < 1e-6) continue;
    const mx = (s.x + t.x) / 2;
    const my = (s.y + t.y) / 2;
    const as = made.axisOf.get(e.source);
    const at = made.axisOf.get(e.target);
    let cx;
    let cy;
    if (as === at) {
      // Along one axis: an arc to the clockwise side, further out the longer it is.
      const a = axisAngle(as, n);
      const out = 12 + 0.35 * Math.abs(radius[e.source] - radius[e.target]);
      cx = mx - Math.sin(a) * out;
      cy = my + Math.cos(a) * out;
    } else {
      // Between axes: toward the angle between them, at the radius they share on average.
      const a = axisAngle(as, n);
      const mid = a + wrap(axisAngle(at, n) - a) / 2;
      const rho = (radius[e.source] + radius[e.target]) / 2;
      cx = rho * Math.cos(mid);
      cy = rho * Math.sin(mid);
    }
    bow[e.id] = ((cx - mx) * -dy + (cy - my) * dx) / len;
  }
  const axes = made.axes.map((x, a) => ({ ...x, angle: axisAngle(a, n), from: inner, to: inner + length }));
  return { pos, axes, bow, by: made.by };
}
