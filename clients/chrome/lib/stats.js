// Column statistics in the table header, as Hugging Face's dataset viewer shows them: under each column's name and
// type, a small bar chart of its values and a caption.
//   numbers, dates      histogram (at most 12 bins), min and max under it
//   few distinct values one bar per value, commonest first ("5 values"); numbers in value order
//   IRIs                one bar per namespace when there are 2 to 12, else the count of distinct values
//   other text          histogram of string lengths
// Hovering a bar puts its range or value and its count in the caption; clicking it filters the column to it (again
// to clear). Computed on the rows the explorer holds: every row up to 10,000 (lib/results.js), else the first rows,
// which the caption says. Values are text nodes; the chart is SVG built from numbers only.
(function () {
  const { h, num } = window.RetelensFmt;
  const R = () => window.RetelensResults;
  const SVGNS = "http://www.w3.org/2000/svg";
  const MAX_BARS = 12;
  const cache = new WeakMap(); // rows array -> Map(var -> summary)

  const nice = (x) => { if (!(x > 0)) return 1; const p = Math.pow(10, Math.floor(Math.log10(x))), f = x / p; return (f <= 1 ? 1 : f <= 2 ? 2 : f <= 2.5 ? 2.5 : f <= 5 ? 5 : 10) * p; };
  const fmtNum = (x) => (Math.abs(x) >= 1e6 || (Math.abs(x) < 1e-3 && x !== 0) ? x.toExponential(2) : new Intl.NumberFormat("en-US", { maximumFractionDigits: Math.abs(x) < 10 ? 3 : 1 }).format(x));
  const iso = (ms) => new Date(ms).toISOString().slice(0, 10);
  const pct = (n, of) => (of ? (n / of) * 100 : 0);
  const pctText = (p) => (p >= 10 || p === 0 ? p.toFixed(0) : p.toFixed(1)) + "%";

  // equal-width bins over [min, max] at a nice step; each bin keeps the smallest and largest value it holds (the
  // filter a click sets selects exactly those rows)
  function histogram(xs, ints) {
    let min = Infinity, max = -Infinity;
    for (const x of xs) { if (x < min) min = x; if (x > max) max = x; }
    if (min === max) return { min, max, bins: [{ a: min, b: max, n: xs.length, lo: min, hi: max }] };
    let step = nice((max - min) / 10);
    if (ints) step = Math.max(1, Math.ceil(step));
    let start = Math.floor(min / step) * step;
    let count = Math.floor((max - start) / step) + 1;
    while (count > MAX_BARS) { step = nice(step * 1.01 + step); start = Math.floor(min / step) * step; count = Math.floor((max - start) / step) + 1; }
    const bins = Array.from({ length: count }, (_, i) => ({ a: start + i * step, b: start + (i + 1) * step, n: 0, lo: Infinity, hi: -Infinity }));
    for (const x of xs) { const i = Math.min(count - 1, Math.floor((x - start) / step)); const bn = bins[i]; bn.n++; if (x < bn.lo) bn.lo = x; if (x > bn.hi) bn.hi = x; }
    return { min, max, bins };
  }

  function compute(rows, v, short) {
    const vals = [];
    let empty = 0;
    for (const r of rows) { const t = r[v]; if (t) vals.push(t); else empty++; }
    const s = { rows: rows.length, empty, count: vals.length, unique: 0, kind: "empty", bars: [], left: "", right: "" };
    if (!vals.length) { s.left = "no values"; return s; }
    const keys = new Map();
    for (const t of vals) { const k = R().cellKey(t); const e = keys.get(k); if (e) e.n++; else keys.set(k, { n: 1, t, k }); }
    s.unique = keys.size;
    const nums = vals.map((t) => R().numOf(t)).filter((x) => !Number.isNaN(x));
    const numeric = nums.length / vals.length >= 0.9;
    const dms = numeric ? [] : vals.map((t) => R().dateOf(t)).filter((x) => !Number.isNaN(x));
    const dates = !numeric && dms.length / vals.length >= 0.9;
    const label = (t) => (t.kind === "iri" ? short(t.value) : String(t.value)) + (t.lang ? " @" + t.lang : "");
    if (vals.length < 4) { s.kind = "few"; s.left = `${num(s.unique)} ${s.unique === 1 ? "value" : "values"}`; return s; }

    if (keys.size <= MAX_BARS && keys.size < vals.length) {
      // classes: one bar per value
      let list = [...keys.values()];
      if (numeric) list.sort((a, b) => R().numOf(a.t) - R().numOf(b.t)); else list.sort((a, b) => b.n - a.n);
      s.kind = "classes";
      s.bars = list.map((e) => ({ n: e.n, text: label(e.t), filter: { set: [e.k] } }));
      s.left = `${num(keys.size)} ${keys.size === 1 ? "value" : "values"}`;
    } else if (numeric) {
      const ints = vals.every((t) => !/\./.test(t.value) && !/e/i.test(t.value));
      const hst = histogram(nums, ints);
      s.kind = "number";
      s.bars = hst.bins.map((bn) => ({ n: bn.n, text: bn.n ? (bn.lo === bn.hi ? fmtNum(bn.lo) : `${fmtNum(bn.lo)} – ${fmtNum(bn.hi)}`) : `${fmtNum(bn.a)} – ${fmtNum(bn.b)}`, filter: bn.n ? { min: bn.lo, max: bn.hi } : null }));
      s.left = fmtNum(hst.min); s.right = fmtNum(hst.max);
    } else if (dates) {
      const hst = histogram(dms, false);
      const years = hst.max - hst.min > 2 * 365.25 * 864e5;
      const fd = (ms) => (years ? String(new Date(ms).getUTCFullYear()) : iso(ms));
      s.kind = "date";
      s.bars = hst.bins.map((bn) => ({ n: bn.n, text: bn.n ? (fd(bn.lo) === fd(bn.hi) ? fd(bn.lo) : `${fd(bn.lo)} – ${fd(bn.hi)}`) : `${fd(bn.a)} – ${fd(bn.b)}`, filter: bn.n ? { from: iso(bn.lo), to: iso(bn.hi) } : null }));
      s.left = fd(hst.min); s.right = fd(hst.max);
    } else if (vals.every((t) => t.kind === "iri")) {
      const ns = new Map();
      for (const t of vals) { const k = R().nsOf(t.value); ns.set(k, (ns.get(k) || 0) + 1); }
      if (ns.size >= 2 && ns.size <= MAX_BARS) {
        s.kind = "namespaces";
        s.bars = [...ns.entries()].sort((a, b) => b[1] - a[1]).map(([k, n]) => ({ n, text: short(k) || k, filter: { ns: [k] } }));
        s.left = `${ns.size} namespaces`;
      } else { s.kind = "unique"; s.left = `${num(keys.size)} distinct`; }
    } else {
      const lens = vals.map((t) => [...String(t.value)].length);
      const hst = histogram(lens, true);
      s.kind = "lengths";
      s.bars = hst.bins.map((bn) => ({ n: bn.n, text: (bn.lo === bn.hi || !bn.n ? `${bn.n ? bn.lo : bn.a}` : `${bn.lo} – ${bn.hi}`) + " characters", filter: null }));
      s.left = `${hst.min}`; s.right = `${hst.max} characters`;
    }
    return s;
  }
  function summarize(rows, v, short) {
    let m = cache.get(rows);
    if (!m) { m = new Map(); cache.set(rows, m); }
    if (!m.has(v)) m.set(v, compute(rows, v, short));
    return m.get(v);
  }

  // is a bar's filter the column's current filter (drawn selected)
  function same(bar, f) {
    if (!bar.filter || !f) return false;
    const b = bar.filter;
    if (b.set) return !!(f.set && f.set.length === 1 && f.set[0] === b.set[0]);
    if (b.ns) return !!(f.ns && f.ns.length === 1 && f.ns[0] === b.ns[0]);
    if (b.min != null) return f.min === b.min && f.max === b.max;
    if (b.from) return f.from === b.from && f.to === b.to;
    return false;
  }

  // the header block: chart + caption. opts: {current: the column's filter, onPick(filter|null), partial: rows count}
  function chart(s, opts = {}) {
    const box = h("div", { class: "rl-stat", "data-rl": "stat", "data-kind": s.kind });
    const capL = h("span", { class: "rl-stat-l" }), capR = h("span", { class: "rl-stat-r" });
    const cap = h("div", { class: "rl-stat-cap" }, capL, capR);
    const emptyP = pct(s.empty, s.rows);
    const rest = () => {
      capL.textContent = s.left;
      const right = [s.right, s.empty ? pctText(emptyP) + " empty" : ""].filter(Boolean).join(" · ");
      capR.textContent = right;
    };
    rest();
    if (s.bars.length) {
      const W = 100, H = 28, gap = s.bars.length > 1 ? Math.min(2.2, 30 / s.bars.length) : 0;
      const bw = (W - gap * (s.bars.length - 1)) / s.bars.length;
      const max = Math.max(...s.bars.map((b) => b.n)) || 1;
      const svg = document.createElementNS(SVGNS, "svg");
      svg.setAttribute("viewBox", `0 0 ${W} ${H}`);
      svg.setAttribute("preserveAspectRatio", "none");
      svg.setAttribute("class", "rl-stat-svg");
      svg.setAttribute("role", "img");
      svg.setAttribute("aria-label", `${s.kind === "classes" || s.kind === "namespaces" ? "Values" : "Distribution"} of the column: ${s.bars.map((b) => `${b.text} ${b.n}`).join(", ")}`.slice(0, 600));
      s.bars.forEach((b, i) => {
        const x = i * (bw + gap);
        const g = document.createElementNS(SVGNS, "g");
        g.setAttribute("class", "rl-stat-g" + (same(b, opts.current) ? " rl-on" : "") + (b.filter && opts.onPick ? " rl-pick" : ""));
        const hit = document.createElementNS(SVGNS, "rect"); // the whole column height is the hover target, not just the bar
        hit.setAttribute("x", x.toFixed(2)); hit.setAttribute("y", "0"); hit.setAttribute("width", bw.toFixed(2)); hit.setAttribute("height", String(H));
        hit.setAttribute("class", "rl-stat-hit");
        const bh = b.n ? Math.max(1.5, (b.n / max) * (H - 1)) : 0.6;
        const bar = document.createElementNS(SVGNS, "rect");
        bar.setAttribute("x", x.toFixed(2)); bar.setAttribute("y", (H - bh).toFixed(2)); bar.setAttribute("width", bw.toFixed(2)); bar.setAttribute("height", bh.toFixed(2));
        bar.setAttribute("class", "rl-stat-bar");
        g.append(hit, bar);
        g.addEventListener("mouseenter", () => { capL.textContent = b.text; capR.textContent = `${num(b.n)} · ${pctText(pct(b.n, s.count))}`; });
        g.addEventListener("mouseleave", rest);
        if (b.filter && opts.onPick) g.addEventListener("click", (e) => { e.stopPropagation(); opts.onPick(same(b, opts.current) ? null : b.filter); });
        svg.append(g);
      });
      box.append(svg);
    }
    box.append(cap);
    if (opts.partial) box.title = `Statistics of the first ${num(opts.partial)} rows`;
    return box;
  }

  window.RetelensStats = { summarize, chart };
})();
