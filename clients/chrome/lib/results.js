// Search, filter and sort over a query result. The model is plain data (no DOM) so Table, Cards and Graph
// share it; the UI helpers build host-native controls with the same Tailwind-vocabulary classes the other
// components use (HF's own classes on HF, Primer-mapped ones on GitHub, shim.css standalone).
//
// Untrusted values (cells, IRIs, literals) only ever reach the DOM as text nodes (see highlight()), and
// the FILTER text generated for "Add as FILTER to the query" escapes every string literal.
(function () {
  const { h, num, icon } = window.RetelensFmt;
  const XSD = "http://www.w3.org/2001/XMLSchema#";
  const NUMERIC_DT = /(integer|int|long|short|decimal|double|float|byte|nonNegativeInteger|positiveInteger|negativeInteger|nonPositiveInteger|unsigned\w*)$/;
  const NUM_RX = /^[+-]?(\d+\.?\d*|\.\d+)([eE][+-]?\d+)?$/;
  const DATE_RX = /^-?\d{4}(-\d{2}(-\d{2}([T ]\d{2}:\d{2}(:\d{2}(\.\d+)?)?(Z|[+-]\d{2}:?\d{2})?)?)?)?$/;
  const FULL_MAX = 10000;   // up to this many rows every row is fetched, so counts / sort / filters are exact
  const WINDOW = 10000;     // above it: the first WINDOW rows are loaded on demand
  const BATCH = 1000;
  const collator = new Intl.Collator(undefined, { numeric: true, sensitivity: "base" });

  // ---------------------------------------------------------------- cell helpers
  const nsOf = (iri) => { const i = Math.max(iri.lastIndexOf("#"), iri.lastIndexOf("/")); return i >= 0 ? iri.slice(0, i + 1) : iri; };
  const dtKey = (t) => (t.datatype && t.datatype !== XSD + "string" ? t.datatype : t.lang ? "@lang" : XSD + "string");
  function numOf(t) {
    if (!t || t.kind !== "literal") return NaN;
    if (t.datatype && NUMERIC_DT.test(t.datatype)) return parseFloat(t.value);
    if (!t.datatype && !t.lang && NUM_RX.test(t.value.trim())) return parseFloat(t.value);
    return NaN;
  }
  function dateOf(t) {
    if (!t || t.kind !== "literal") return NaN;
    const typed = t.datatype && /(date|dateTime|gYear|gYearMonth)$/.test(t.datatype);
    if (!typed && (t.datatype || t.lang || !DATE_RX.test(t.value.trim()))) return NaN;
    const s = t.value.trim();
    const ms = Date.parse(/^-?\d{4}$/.test(s) ? s + "-01-01" : /^\d{4}-\d{2}$/.test(s) ? s + "-01" : s);
    return Number.isNaN(ms) ? NaN : ms;
  }
  const cellKey = (t) => (t.kind === "iri" ? "i" : t.kind === "bnode" ? "b" : "l") + t.value + (t.lang ? "@" + t.lang : "");

  // ---------------------------------------------------------------- model
  class ResultModel {
    constructor(vars, short) {
      this.vars = vars; this.short = short || ((x) => x);
      this.rows = []; this.total = null; this.complete = false; this.window = false;
      this.q = ""; this.filters = {}; this.sort = null;
      this._ver = 0; this._cache = null; this._prof = {}; this._txt = [];
    }
    setRows(rows, total, complete) { this.rows = rows; this.total = total; this.complete = !!complete; this._bump(); }
    addRows(more) { this.rows = this.rows.concat(more); this._bump(); }
    _bump() { this._ver++; this._cache = null; this._prof = {}; this._txt = []; }
    changed() { this._cache = null; }
    active() { return !!(this.q || this.activeCols().length); }
    activeCols() { return Object.keys(this.filters).filter((v) => filterActive(this.filters[v])); }
    clear() { this.q = ""; this.filters = {}; this._cache = null; }
    setFilter(v, f) { if (f && filterActive(f)) this.filters[v] = f; else delete this.filters[v]; this._cache = null; }

    text(i, v) { // lower-cased search text of a cell: value, shortened IRI, language
      const row = this._txt[i] || (this._txt[i] = {});
      if (row[v] !== undefined) return row[v];
      const t = this.rows[i][v];
      let s = "";
      if (t) { s = t.value; if (t.kind === "iri") s += " " + this.short(t.value); if (t.lang) s += " @" + t.lang; }
      return (row[v] = s.toLowerCase());
    }

    // everything known about one column of the loaded rows
    profile(v) {
      if (this._prof[v]) return this._prof[v];
      const p = { v, n: 0, empty: 0, iri: 0, bnode: 0, lit: 0, num: 0, date: 0, ns: new Map(), lang: new Map(), dt: new Map(), top: new Map(), min: Infinity, max: -Infinity, dmin: Infinity, dmax: -Infinity };
      for (const r of this.rows) {
        const t = r[v];
        if (!t) { p.empty++; continue; }
        p.n++;
        if (t.kind === "iri") { p.iri++; const ns = nsOf(t.value); p.ns.set(ns, (p.ns.get(ns) || 0) + 1); }
        else if (t.kind === "bnode") p.bnode++;
        else {
          p.lit++;
          if (t.lang) p.lang.set(t.lang, (p.lang.get(t.lang) || 0) + 1); else p.lang.set("", (p.lang.get("") || 0) + 1);
          const dk = dtKey(t); p.dt.set(dk, (p.dt.get(dk) || 0) + 1);
          const n = numOf(t); if (!Number.isNaN(n)) { p.num++; if (n < p.min) p.min = n; if (n > p.max) p.max = n; }
          const d = dateOf(t); if (!Number.isNaN(d)) { p.date++; if (d < p.dmin) p.dmin = d; if (d > p.dmax) p.dmax = d; }
        }
        const k = cellKey(t);
        const e = p.top.get(k);
        if (e) e.n++; else if (p.top.size < 5000) p.top.set(k, { n: 1, t });
      }
      p.numeric = p.n > 0 && p.num / p.n >= 0.9;
      p.dates = !p.numeric && p.n > 0 && p.date / p.n >= 0.9;
      p.type = p.numeric ? "number" : p.dates ? "date" : p.iri && !p.lit ? "iri" : p.lit && !p.iri && !p.bnode ? "literal" : p.n ? "mixed" : "empty";
      return (this._prof[v] = p);
    }

    matches(i, skipVar) {
      const r = this.rows[i];
      if (this.q) {
        const q = this.q.toLowerCase();
        let hit = false;
        for (const v of this.vars) if (this.text(i, v).includes(q)) { hit = true; break; }
        if (!hit) return false;
      }
      for (const v of Object.keys(this.filters)) {
        if (v === skipVar) continue;
        if (!this.cellOk(this.filters[v], r[v], v)) return false;
      }
      return true;
    }
    cellOk(f, t, v) {
      if (f.e === "empty") return !t;
      if (f.e === "full" && !t) return false;
      const constrained = f.tx || f.ns || f.lang || f.dt || f.min != null || f.max != null || f.from || f.to || f.set;
      if (!t) return !constrained;
      if (f.tx && f.tx.v) {
        const w = f.tx.v.toLowerCase();
        const a = t.value.toLowerCase(), b = t.kind === "iri" ? this.short(t.value).toLowerCase() : a;
        const ok = f.tx.m === "e" ? a === w || b === w : f.tx.m === "s" ? a.startsWith(w) || b.startsWith(w) : a.includes(w) || b.includes(w);
        if (!ok) return false;
      }
      if (f.ns && f.ns.length && !(t.kind === "iri" && f.ns.includes(nsOf(t.value)))) return false;
      if (f.lang && f.lang.length && !(t.kind === "literal" && f.lang.includes(t.lang || ""))) return false;
      if (f.dt && f.dt.length && !(t.kind === "literal" && f.dt.includes(dtKey(t)))) return false;
      if (f.min != null || f.max != null) {
        const n = numOf(t);
        if (Number.isNaN(n) || (f.min != null && n < f.min) || (f.max != null && n > f.max)) return false;
      }
      if (f.from || f.to) {
        const d = dateOf(t);
        if (Number.isNaN(d)) return false;
        if (f.from && d < Date.parse(f.from)) return false;
        if (f.to && d > Date.parse(f.to) + 86399999) return false;
      }
      if (f.set && f.set.length && !f.set.includes(cellKey(t))) return false;
      return true;
    }

    // indices into this.rows: filtered, then sorted
    view() {
      if (this._cache) return this._cache;
      let idx = [];
      const act = this.active();
      for (let i = 0; i < this.rows.length; i++) if (!act || this.matches(i)) idx.push(i);
      if (this.sort && this.vars.includes(this.sort.v)) {
        const v = this.sort.v, dir = this.sort.dir === "desc" ? -1 : 1, p = this.profile(v);
        const key = (i) => { const t = this.rows[i][v]; if (!t) return null; if (p.numeric) { const n = numOf(t); return Number.isNaN(n) ? null : n; } if (p.dates) { const d = dateOf(t); return Number.isNaN(d) ? null : d; } return t.kind === "iri" ? this.short(t.value) : t.value; };
        const keys = new Map(idx.map((i) => [i, key(i)]));
        idx = idx.map((i, n) => [i, n]).sort((a, b) => {
          const x = keys.get(a[0]), y = keys.get(b[0]);
          if (x === null || y === null) return x === y ? a[1] - b[1] : x === null ? 1 : -1; // unbound last, either direction
          const c = typeof x === "number" ? x - y : collator.compare(x, y);
          return c ? c * dir : a[1] - b[1];
        }).map((x) => x[0]);
      }
      return (this._cache = idx);
    }
    viewRows() { return this.view().map((i) => this.rows[i]); }

    // facet: value counts of one column over the rows every OTHER filter lets through
    facet(v, limit = 8) {
      const counts = new Map();
      for (let i = 0; i < this.rows.length; i++) {
        if (!this.matches(i, v)) continue;
        const t = this.rows[i][v];
        if (!t) continue;
        const k = cellKey(t);
        const e = counts.get(k);
        if (e) e.n++; else counts.set(k, { n: 1, t, k });
      }
      const all = [...counts.values()].sort((a, b) => b.n - a.n || collator.compare(a.t.value, b.t.value));
      return { values: all.slice(0, limit), distinct: all.length, rest: Math.max(0, all.length - limit) };
    }

    // ---- URL state
    toParams() {
      const o = { rq: this.q || null, rs: this.sort ? this.sort.v + ":" + this.sort.dir : null, rf: null };
      const f = {};
      for (const v of this.activeCols()) f[v] = this.filters[v];
      if (Object.keys(f).length) { try { o.rf = JSON.stringify(f); } catch (_) {} }
      return o;
    }
    fromParams(get) {
      const q = get("rq"), s = get("rs"), f = get("rf");
      this.q = q ? q.slice(0, 200) : "";
      this.sort = null; this.filters = {};
      if (s) { const i = s.lastIndexOf(":"); const v = s.slice(0, i), d = s.slice(i + 1); if (this.vars.includes(v) && (d === "asc" || d === "desc")) this.sort = { v, dir: d }; }
      if (f) {
        try {
          const o = JSON.parse(f);
          for (const v of Object.keys(o)) { if (this.vars.includes(v)) { const c = cleanFilter(o[v]); if (c) this.filters[v] = c; } }
        } catch (_) { /* a hand-edited URL: ignore */ }
      }
      this._cache = null;
    }
  }

  function filterActive(f) { return !!f && !!(f.e || (f.tx && f.tx.v) || (f.ns && f.ns.length) || (f.lang && f.lang.length) || (f.dt && f.dt.length) || f.min != null || f.max != null || f.from || f.to || (f.set && f.set.length)); }
  // only known shapes survive a round trip through the URL
  function cleanFilter(f) {
    if (!f || typeof f !== "object") return null;
    const o = {};
    if (f.e === "empty" || f.e === "full") o.e = f.e;
    if (f.tx && typeof f.tx.v === "string" && /^[cse]$/.test(f.tx.m)) o.tx = { m: f.tx.m, v: f.tx.v.slice(0, 200) };
    for (const k of ["ns", "lang", "dt", "set"]) if (Array.isArray(f[k])) o[k] = f[k].filter((x) => typeof x === "string").slice(0, 200);
    for (const k of ["min", "max"]) if (typeof f[k] === "number" && Number.isFinite(f[k])) o[k] = f[k];
    for (const k of ["from", "to"]) if (typeof f[k] === "string" && /^\d{4}-\d{2}-\d{2}$/.test(f[k])) o[k] = f[k];
    return filterActive(o) ? o : null;
  }

  // ---------------------------------------------------------------- highlighting
  // text -> text nodes and <mark> elements (text-node splitting; never innerHTML)
  function highlight(text, q) {
    text = String(text);
    if (!q) return [text];
    const lo = text.toLowerCase(), w = q.toLowerCase();
    if (!w || lo.length !== text.length) return [text];
    const out = [];
    let at = 0, i;
    while ((i = lo.indexOf(w, at)) >= 0) {
      if (i > at) out.push(text.slice(at, i));
      out.push(h("mark", { "data-rl": "hit" }, text.slice(i, i + w.length)));
      at = i + w.length;
    }
    if (at < text.length) out.push(text.slice(at));
    return out;
  }

  // ---------------------------------------------------------------- describing filters (chips)
  function describe(v, f, short) {
    const out = [];
    if (f.e === "empty") out.push("is empty");
    if (f.e === "full") out.push("is not empty");
    if (f.tx && f.tx.v) out.push((f.tx.m === "e" ? "= " : f.tx.m === "s" ? "starts with " : "contains ") + "“" + f.tx.v + "”");
    if (f.ns && f.ns.length) out.push("in " + f.ns.map((n) => short(n)).join(", "));
    if (f.lang && f.lang.length) out.push("lang " + f.lang.map((l) => l || "none").join(", "));
    if (f.dt && f.dt.length) out.push("type " + f.dt.map((d) => (d.startsWith(XSD) ? d.slice(XSD.length) : d === "@lang" ? "langString" : short(d))).join(", "));
    if (f.min != null || f.max != null) out.push((f.min != null ? f.min : "…") + " – " + (f.max != null ? f.max : "…"));
    if (f.from || f.to) out.push((f.from || "…") + " → " + (f.to || "…"));
    if (f.set && f.set.length) out.push(f.set.length === 1 ? "= " + f.set[0].slice(1) : f.set.length + " values");
    return v + " " + out.join(", ");
  }

  // ---------------------------------------------------------------- "Add as FILTER to the query"
  // SPARQL string literal: escape backslash, quote and line breaks. rete (<= 0.7.8) cuts CONTAINS/REGEX/STRSTARTS
  // literals at an embedded double quote, so a needle holding one is split at the quote and every piece is tested
  // on its own (an approximation, flagged in a comment).
  const lit = (s) => '"' + String(s).replace(/\\/g, "\\\\").replace(/"/g, '\\"').replace(/\r?\n/g, "\\n").replace(/\r/g, "\\r") + '"';
  function needleTests(fn, expr, needle, notes) {
    const w = needle.toLowerCase();
    if (!w.includes('"')) return [`${fn}(LCASE(STR(${expr})), ${lit(w)})`];
    notes.add("a text filter holds a double quote: rete cuts CONTAINS/REGEX literals at one, so the text is split at the quote (approximate)");
    const parts = w.split('"').filter(Boolean);
    return parts.map((p, i) => `${i === 0 && fn === "STRSTARTS" ? "STRSTARTS" : "CONTAINS"}(LCASE(STR(${expr})), ${lit(p)})`);
  }
  function filterSparql(query, model) {
    const notes = new Set();
    const conds = [];
    const V = (v) => "?" + v;
    if (model.q) {
      const alts = model.vars.flatMap((v) => needleTests("CONTAINS", V(v), model.q, notes));
      conds.push(model.q.includes('"') ? "(" + model.vars.map((v) => "(" + needleTests("CONTAINS", V(v), model.q, notes).join(" && ") + ")").join(" || ") + ")" : "(" + alts.join(" || ") + ")");
    }
    for (const v of model.activeCols()) {
      const f = model.filters[v], x = V(v);
      if (f.e === "empty") { conds.push(`!BOUND(${x})`); continue; }
      if (f.e === "full") conds.push(`BOUND(${x})`);
      if (f.tx && f.tx.v) {
        const w = f.tx.v;
        if (f.tx.m === "e") conds.push(`LCASE(STR(${x})) = ${lit(w.toLowerCase())}`);
        else conds.push(needleTests(f.tx.m === "s" ? "STRSTARTS" : "CONTAINS", x, w, notes).join(" && "));
      }
      if (f.ns && f.ns.length) conds.push("(" + f.ns.map((n) => `STRSTARTS(STR(${x}), ${lit(n)})`).join(" || ") + ")");
      if (f.lang && f.lang.length) conds.push("(" + f.lang.map((l) => `LANG(${x}) = ${lit(l)}`).join(" || ") + ")");
      if (f.dt && f.dt.length) conds.push("(" + f.dt.map((d) => (d === "@lang" ? `LANG(${x}) != ""` : `DATATYPE(${x}) = <${d}>`)).join(" || ") + ")");
      if (f.min != null) conds.push(`<${XSD}double>(STR(${x})) >= ${f.min}`);
      if (f.max != null) conds.push(`<${XSD}double>(STR(${x})) <= ${f.max}`);
      if (f.from) conds.push(`STR(${x}) >= ${lit(f.from)}`);
      if (f.to) conds.push(`SUBSTR(STR(${x}), 1, 10) <= ${lit(f.to)}`);
      if (f.set && f.set.length) conds.push("(" + f.set.map((k) => `STR(${x}) = ${lit(k.slice(1).replace(/@[\w-]+$/, ""))}`).join(" || ") + ")");
    }
    // prologue (PREFIX / BASE) stays outside; the rest becomes a subquery
    const lines = String(query).split("\n");
    const pro = [], body = [];
    for (const l of lines) (body.length === 0 && /^\s*(PREFIX|BASE)\b/i.test(l) ? pro : body).push(l);
    const order = model.sort ? `\nORDER BY ${model.sort.dir === "desc" ? "DESC" : "ASC"}(?${model.sort.v})` : "";
    const out = [
      ...pro,
      "# retelens: the query below filters the previous result" + (model.active() ? "; run it to filter on the server" : ""),
      ...[...notes].map((n) => "# note: " + n),
      `SELECT ${model.vars.map(V).join(" ")} WHERE {`,
      "  {",
      ...body.map((l) => "    " + l),
      "  }",
      ...conds.map((c) => `  FILTER(${c})`),
      "}" + order,
    ];
    return out.join("\n");
  }

  window.RetelensResults = { ResultModel, highlight, describe, filterSparql, filterActive, FULL_MAX, WINDOW, BATCH, numOf, dateOf, nsOf, cellKey, dtKey, XSD };
})();
