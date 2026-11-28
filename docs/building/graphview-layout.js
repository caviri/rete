// Force layout for the clustered graph view (no libraries, no DOM, deterministic).
//
// The scene is a list of items (circles) that sit in a tree of groups: every item has a `path`,
// the keys of the expanded clusters it is inside (outermost first, [] at the top level). Each group
// is laid out as a unit: its children (items or sub-groups) repel as circles, a sub-group moves as
// one piece, and its items are pulled to its centre, so an expanded cluster stays a blob.
// Links pull their two items together. A `rank` per item (flow mode) pulls the item to a column.
//
//   const lay = new Layout();
//   lay.set(items, links, ranks?)  items: [{key, r, x?, y?, path, seed?: {x,y,r}}] (x/y kept from the
//                                  previous call by key; new items without x/y start inside `seed`
//                                  or on a spiral), links: [{a, b, w}] indices into items
//   lay.wake(alpha)                heat it up; lay.step() runs one step, returns true while moving
//   lay.group(key) -> {x, y, r}    the circle around an expanded cluster
//
// The same input always gives the same positions: new items are placed from a hash of their key.

export function hash32(s) {
  let h = 2166136261;
  for (let i = 0; i < s.length; i++) { h ^= s.charCodeAt(i); h = Math.imul(h, 16777619); }
  return h >>> 0;
}
export function rng(seed) {
  let a = seed >>> 0;
  return () => { a = (a + 0x6D2B79F5) >>> 0; let t = a; t = Math.imul(t ^ (t >>> 15), t | 1); t ^= t + Math.imul(t ^ (t >>> 7), t | 61); return ((t ^ (t >>> 14)) >>> 0) / 4294967296; };
}

const ALPHA_MIN = 0.02;
const DECAY = 0.965;

export class Layout {
  constructor() {
    this.items = []; this.links = []; this.rank = null; this.alpha = 0;
    this.groups = new Map(); this._root = null; this._pos = new Map();
    this.flowGap = 150; this.aspect = 1;
  }

  set(items, links, ranks) {
    for (const it of this.items) this._pos.set(it.key, it);
    this.items = items; this.links = links || []; this.rank = ranks || null;
    const rootR = Math.max(60, Math.sqrt(items.length) * 30);
    for (const it of items) {
      const old = this._pos.get(it.key);
      if (old && old !== it) { it.x = old.x; it.y = old.y; }
      if (it.x == null || Number.isNaN(it.x)) {
        const rnd = rng(hash32(it.key));
        const s = it.seed;
        if (s) { const a = rnd() * 6.2832, d = Math.sqrt(rnd()) * Math.max(4, s.r * 0.7); it.x = s.x + Math.cos(a) * d; it.y = s.y + Math.sin(a) * d; }
        else { const a = rnd() * 6.2832, d = Math.sqrt(rnd()) * rootR; it.x = Math.cos(a) * d; it.y = Math.sin(a) * d; }
      }
      it.vx = it.vx || 0; it.vy = it.vy || 0;
    }
    this._pos = new Map(items.map((i) => [i.key, i]));
    this.deg = new Float32Array(items.length);
    for (const l of this.links) { this.deg[l.a]++; this.deg[l.b]++; }
    this._build();
  }

  _build() {
    // group tree from the item paths
    const groups = new Map(); const root = { key: '', parent: null, depth: 0, children: [], leaves: [], x: 0, y: 0, r: 0 };
    groups.set('', root);
    const gp = (path, n) => { // group for the first n entries of path
      if (n === 0) return root; const k = path[n - 1];
      let g = groups.get(k);
      if (!g) { g = { key: k, parent: gp(path, n - 1), depth: n, children: [], leaves: [], x: 0, y: 0, r: 0 }; groups.set(k, g); g.parent.children.push(g); }
      return g;
    };
    for (const it of this.items) {
      const g = gp(it.path, it.path.length); g.children.push(it); it.group = g;
      for (let p = g; p; p = p.parent) p.leaves.push(it);
    }
    this.groups = groups; this._root = root;
    this._order = [...groups.values()].sort((a, b) => b.depth - a.depth);
  }

  group(key) { const g = this.groups.get(key); return g ? { x: g.x, y: g.y, r: g.r, n: g.leaves.length } : null; }

  wake(a) { this.alpha = Math.max(this.alpha, a); }
  get active() { return this.alpha > ALPHA_MIN; }

  _stats() {
    for (const g of this._order) {
      if (!g.leaves.length) continue;
      let sx = 0, sy = 0; for (const l of g.leaves) { sx += l.x; sy += l.y; }
      g.x = sx / g.leaves.length; g.y = sy / g.leaves.length;
      let r = 0; for (const l of g.leaves) { const d = Math.hypot(l.x - g.x, l.y - g.y) + l.r; if (d > r) r = d; }
      g.r = r + (g.key ? 16 : 0);
    }
  }

  step() {
    const items = this.items, n = items.length;
    if (!n) { this.alpha = 0; return false; }
    const a = this.alpha;
    this._stats();
    const fx = new Float32Array(n), fy = new Float32Array(n);
    const idx = new Map(); items.forEach((it, i) => idx.set(it, i));
    // links
    for (const l of this.links) {
      const p = items[l.a], q = items[l.b];
      let dx = q.x - p.x, dy = q.y - p.y; let d = Math.hypot(dx, dy) || 0.01;
      const L = p.r + q.r + 46;
      const same = p.group === q.group;
      const k = (same ? 0.16 : 0.05) / Math.sqrt(Math.min(this.deg[l.a], this.deg[l.b]) || 1);
      const f = (d - L) * k * a; dx /= d; dy /= d;
      fx[l.a] += dx * f; fy[l.a] += dy * f; fx[l.b] -= dx * f; fy[l.b] -= dy * f;
    }
    // repulsion of the children of every group (leaves are circles, sub-groups move as one)
    for (const g of this._order) {
      const ch = g.children, m = ch.length;
      for (let i = 0; i < m; i++) {
        const A = ch[i], Ag = !!A.leaves, ax = A.x, ay = A.y, ar = A.r;
        for (let j = i + 1; j < m; j++) {
          const B = ch[j];
          let dx = B.x - ax, dy = B.y - ay; let d2 = dx * dx + dy * dy;
          const minD = ar + B.r + 10;
          if (d2 > 240000 && d2 > minD * minD * 9) continue;
          let d = Math.sqrt(d2);
          if (d < 0.01) { dx = ((i * 7 + j) % 5) - 2 + 0.5; dy = ((i + j * 3) % 5) - 2 + 0.3; d = Math.hypot(dx, dy); }
          dx /= d; dy /= d;
          let f = 5.5 * Math.sqrt(ar * B.r) / (d + 20) * a;
          if (d < minD) f += (minD - d) * 0.5 * (0.5 + 0.5 * a);
          if (Ag) { for (const l of A.leaves) { const k = idx.get(l); fx[k] -= dx * f; fy[k] -= dy * f; } } else { const k = idx.get(A); fx[k] -= dx * f; fy[k] -= dy * f; }
          if (B.leaves) { for (const l of B.leaves) { const k = idx.get(l); fx[k] += dx * f; fy[k] += dy * f; } } else { const k = idx.get(B); fx[k] += dx * f; fy[k] += dy * f; }
        }
      }
    }
    // cohesion of expanded clusters
    for (const g of this._order) {
      if (!g.key) continue;
      const k = 0.05 * a;
      for (const l of g.leaves) { const i = idx.get(l); fx[i] += (g.x - l.x) * k; fy[i] += (g.y - l.y) * k; }
    }
    // gravity of the top-level entities, flow columns
    const root = this._root;
    for (const c of root.children) {
      const asp = Math.min(2.5, this.aspect || 1), kx = 0.016 * a / asp, ky = 0.016 * a * asp;
      const ls = c.leaves || [c];
      for (const l of ls) { const i = idx.get(l); fx[i] -= c.x * kx; fy[i] -= c.y * ky; }
    }
    if (this.rank) {
      let mx = 0, cnt = 0; for (let i = 0; i < n; i++) if (this.rank[i] != null) { mx += items[i].x - this.rank[i] * this.flowGap; cnt++; }
      const x0 = cnt ? mx / cnt : 0;
      for (let i = 0; i < n; i++) if (this.rank[i] != null) fx[i] += (x0 + this.rank[i] * this.flowGap - items[i].x) * (items[i].noflow ? 0.03 : 0.3) * (0.5 + 0.5 * a);
    }
    for (let i = 0; i < n; i++) {
      const it = items[i];
      if (it.pin) { it.vx = it.vy = 0; continue; }
      it.vx = (it.vx + fx[i]) * 0.62; it.vy = (it.vy + fy[i]) * 0.62;
      const sp = Math.hypot(it.vx, it.vy); if (sp > 40) { it.vx *= 40 / sp; it.vy *= 40 / sp; }
      it.x += it.vx; it.y += it.vy;
    }
    this.alpha *= DECAY;
    if (this.alpha <= ALPHA_MIN) { this.alpha = 0; for (const it of items) it.vx = it.vy = 0; this._relax(30); this._stats(); return false; }
    return true;
  }

  // pull overlapping circles apart (children of each group; a sub-group moves as one piece)
  _relax(iters) {
    for (let t = 0; t < iters; t++) {
      this._stats(); let moved = false;
      for (const g of this._order) {
        const ch = g.children, m = ch.length;
        for (let i = 0; i < m; i++) for (let j = i + 1; j < m; j++) {
          const A = ch[i], B = ch[j]; let dx = B.x - A.x, dy = B.y - A.y; let d = Math.hypot(dx, dy);
          const minD = A.r + B.r + 4; if (d >= minD) continue;
          if (d < 0.01) { dx = 1; dy = 0.3; d = Math.hypot(dx, dy); }
          const push = (minD - d) / 2 * 0.8; dx /= d; dy /= d; moved = true;
          for (const l of A.leaves || [A]) { if (!l.pin) { l.x -= dx * push; l.y -= dy * push; } }
          for (const l of B.leaves || [B]) { if (!l.pin) { l.x += dx * push; l.y += dy * push; } }
        }
      }
      if (!moved) break;
    }
  }

  run(steps) { let i = 0; while (i++ < steps && this.step()); return this.alpha; }

  // bounding box of the items (or of the given keys)
  bounds(keys) {
    let x0 = Infinity, y0 = Infinity, x1 = -Infinity, y1 = -Infinity;
    for (const it of this.items) {
      if (keys && !keys.has(it.key)) continue;
      x0 = Math.min(x0, it.x - it.r); y0 = Math.min(y0, it.y - it.r); x1 = Math.max(x1, it.x + it.r); y1 = Math.max(y1, it.y + it.r);
    }
    return x0 === Infinity ? null : { x0, y0, x1, y1 };
  }
}

// Longest-path rank of each item along the given links (source left, sink right); cycles are cut.
export function flowRanks(count, links) {
  const out = Array.from({ length: count }, () => []), indeg = new Int32Array(count), inv = Array.from({ length: count }, () => []);
  const has = new Uint8Array(count);
  for (const l of links) { if (l.a === l.b) continue; out[l.a].push(l.b); inv[l.b].push(l.a); has[l.a] = has[l.b] = 1; }
  // DFS to find back edges, then relax on the acyclic remainder
  const state = new Uint8Array(count), back = new Set();
  for (let s = 0; s < count; s++) {
    if (state[s] || !has[s]) continue;
    const stack = [[s, 0]]; state[s] = 1;
    while (stack.length) {
      const top = stack[stack.length - 1], u = top[0];
      if (top[1] < out[u].length) { const v = out[u][top[1]++]; if (state[v] === 1) back.add(u * count + v); else if (!state[v]) { state[v] = 1; stack.push([v, 0]); } }
      else { state[u] = 2; stack.pop(); }
    }
  }
  const rank = new Array(count).fill(null);
  for (let i = 0; i < count; i++) if (has[i]) rank[i] = 0;
  for (const l of links) if (l.a !== l.b && !back.has(l.a * count + l.b)) indeg[l.b]++;
  const q = []; for (let i = 0; i < count; i++) if (has[i] && !indeg[i]) q.push(i);
  while (q.length) {
    const u = q.shift();
    for (const v of out[u]) { if (back.has(u * count + v)) continue; if (rank[v] < rank[u] + 1) rank[v] = rank[u] + 1; if (--indeg[v] === 0) q.push(v); }
  }
  return rank;
}
