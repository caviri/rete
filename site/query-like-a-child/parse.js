// SPARQL text → sketch, for the queries a file's card brings along.
//
// Only the part of SPARQL the blocks can say is read. Anything else (a
// subquery, HAVING, a path with alternatives, an expression the filter
// blocks do not have) makes fromSparql return null, and the page then
// offers the query as text. A sketch is only returned when the query it
// writes back means the same as the one read.
import { CALC_FNS, CMP_OPS, GENERIC_LABELS, NS, emptySketch, iri, lit, make, toSparql, vr } from "./model.js";

class Unsupported extends Error {}
const no = (why) => { throw new Unsupported(why); };

const unesc = (s) => s.replace(/\\(u[0-9a-fA-F]{4}|U[0-9a-fA-F]{8}|.)/g, (m, c) =>
  (c[0] === "u" || c[0] === "U" ? String.fromCodePoint(parseInt(c.slice(1), 16)) : { n: "\n", t: "\t", r: "\r", b: "\b", f: "\f" }[c] ?? c));

function tokenize(src) {
  const out = [];
  let i = 0;
  while (i < src.length) {
    const c = src[i];
    if (/\s/.test(c)) { i++; continue; }
    if (c === "#") { while (i < src.length && src[i] !== "\n") i++; continue; }
    const rest = src.slice(i, i + 4000);
    let m;
    if ((m = /^<([^<>"{}|^`\\\s]*:[^<>"{}|^`\\\s]*)>/.exec(rest))) { out.push({ t: "iri", v: m[1] }); i += m[0].length; continue; }
    if ((m = /^[?$]([\p{L}\p{N}_]+)/u.exec(rest))) { out.push({ t: "var", v: m[1] }); i += m[0].length; continue; }
    if (rest.startsWith('"""') || rest.startsWith("'''")) no("long string");
    if ((m = /^"((?:[^"\\\n]|\\.)*)"/.exec(rest) || /^'((?:[^'\\\n]|\\.)*)'/.exec(rest))) {
      const tok = { t: "str", v: unesc(m[1]) };
      i += m[0].length;
      const after = src.slice(i, i + 300);
      let x;
      if ((x = /^@([A-Za-z]+(?:-[A-Za-z0-9]+)*)/.exec(after))) { tok.lang = x[1]; i += x[0].length; }
      else if (after.startsWith("^^")) { tok.dtAt = true; i += 2; }
      out.push(tok);
      continue;
    }
    if ((m = /^(\d+\.\d+|\d+)([eE][+-]?\d+)?/.exec(rest))) { out.push({ t: "num", v: m[0] }); i += m[0].length; continue; }
    if ((m = /^([A-Za-z][\w-]*)?:((?:[\w\-%]|\.(?=[\w\-%]))*)/.exec(rest))) { out.push({ t: "pname", p: m[1] || "", l: m[2] }); i += m[0].length; continue; }
    if ((m = /^[A-Za-z_]\w*/.exec(rest))) { out.push({ t: "word", v: m[0], u: m[0].toUpperCase() }); i += m[0].length; continue; }
    const two = src.slice(i, i + 2);
    if (["!=", "<=", ">=", "&&", "||"].includes(two)) { out.push({ t: "p", v: two }); i += 2; continue; }
    if ("{}().;,|/^+*?=<>![]-".includes(c)) { out.push({ t: "p", v: c }); i++; continue; }
    no("character " + c);
  }
  return out;
}

const XSD = NS.xsd;
const NUMERIC = new Set(["integer", "decimal", "double", "float", "int", "long", "short", "nonNegativeInteger", "positiveInteger"]);
const FN_NAMES = new Set(CALC_FNS.map((f) => f[0]));

/**
 * The sketch that says the same as `text`, or null.
 * ctx: { labelPredicate?: iri }: with it, a statement through that property
 * becomes a "has a name" block.
 */
export function fromSparql(text, ctx = {}) {
  try {
    const sketch = parse(tokenize(String(text)), ctx);
    // A sketch that would leave something out when written back is not the same query.
    return toSparql(sketch, ctx).problems.length ? null : sketch;
  } catch (e) {
    if (e instanceof Unsupported) return null;
    return null;
  }
}

function parse(toks, ctx) {
  let i = 0;
  const peek = () => toks[i];
  const next = () => toks[i++] || no("end");
  const isP = (v) => toks[i] && toks[i].t === "p" && toks[i].v === v;
  const isW = (u) => toks[i] && toks[i].t === "word" && toks[i].u === u;
  const eatP = (v) => (isP(v) ? (i++, true) : false);
  const eatW = (u) => (isW(u) ? (i++, true) : false);
  const needP = (v) => eatP(v) || no("expected " + v);
  const needW = (u) => eatW(u) || no("expected " + u);

  const prefixes = {};
  const used = new Set(toks.filter((t) => t.t === "var").map((t) => t.v));
  const fresh = (base) => { let n = base, k = 2; while (used.has(n)) n = base + k++; used.add(n); return n; };

  const full = (t) => {
    if (t.t === "iri") return t.v;
    if (t.t === "pname") {
      const ns = prefixes[t.p] ?? NS[t.p];
      if (ns == null) no("prefix " + t.p);
      return ns + t.l;
    }
    return no("not an IRI");
  };
  const isIri = (t) => t && (t.t === "iri" || t.t === "pname");

  const literal = (t) => {
    if (t.t === "num") return lit(t.v, "number");
    if (t.t !== "str") return no("not a literal");
    if (t.dtAt) {
      const dt = full(next());
      if (!dt.startsWith(XSD)) no("datatype");
      const local = dt.slice(XSD.length);
      if (local === "string") return lit(t.v, "text");
      if (NUMERIC.has(local) && /^[+-]?(\d+\.?\d*|\.\d+)([eE][+-]?\d+)?$/.test(t.v)) return lit(t.v, "number");
      // The page writes a date back in the shape it was read in.
      if (local === "date" && /^\d{4}-\d{2}-\d{2}$/.test(t.v) && !/^\d{4}-01-01$/.test(t.v)) return lit(t.v, "date");
      if (local === "dateTime" && /^\d{4}-\d{2}-\d{2}T/.test(t.v)) return lit(t.v, "date");
      return no("datatype " + local);
    }
    // Untyped text that looks like a date would be written back typed.
    if (!t.lang && /^\d{4}(-\d{2}-\d{2}(T.*)?)?$/.test(t.v.trim())) return lit(t.v, "text");
    return lit(t.v, "text", t.lang);
  };

  const term = (asObject) => {
    const t = next();
    if (t.t === "var") return vr(t.v);
    if (isIri(t)) return iri(full(t));
    if (asObject && (t.t === "str" || t.t === "num")) return literal(t);
    return no("term");
  };

  // One step of a property: a, an IRI or a variable, with ^ before or + * after.
  const step = () => {
    if (isW("A") && peek().v === "a") { i++; return { type: true }; }
    const inverse = eatP("^");
    const t = next();
    if (t.t === "var") { if (inverse) no("inverse of a variable"); return { p: vr(t.v), path: "" }; }
    if (!isIri(t)) no("property");
    const p = iri(full(t));
    let path = inverse ? "^" : "";
    if (isP("+") || isP("*")) { if (inverse) no("inverse with repeat"); path = next().v; }
    if (isP("?")) no("optional step");
    return { p, path };
  };
  const path = () => {
    if (isP("(") || isP("!")) no("grouped path");
    const steps = [step()];
    while (eatP("/")) steps.push(step());
    if (isP("|")) {
      // The one alternative the blocks write: "has a name" without a file,
      // which tries the usual label properties.
      const alt = [steps[0]];
      while (eatP("|")) alt.push(step());
      const plain = alt.every((x) => x.p && x.p.k === "iri" && !x.path);
      if (steps.length !== 1 || !plain || ctx.labelPredicate || alt.map((x) => x.p.v).join(" ") !== GENERIC_LABELS.join(" ")) no("alternative path");
      return [{ label: true }];
    }
    if (steps.length > 1 && steps.some((s) => s.type)) no("type inside a path");
    return steps;
  };

  const statement = (s, st, o) => {
    if (st.label) return o.k === "var" ? { ...make("name"), s, o } : no("name of a value");
    if (st.type) {
      if (o.k === "lit") no("literal kind");
      return { ...make("kind"), s, c: o };
    }
    if (ctx.labelPredicate && st.p.k === "iri" && st.p.v === ctx.labelPredicate && !st.path && o.k === "var")
      return { ...make("name"), s, o };
    return { ...make("link"), s, p: st.p, o, path: st.path };
  };

  const triples = (out) => {
    const s = term(false);
    for (;;) {
      const steps = path();
      for (;;) {
        const o = term(true);
        // a/b/c: one link per step, through variables of their own.
        let from = s;
        steps.forEach((st, k) => {
          const to = k === steps.length - 1 ? o : vr(fresh("step"));
          out.push(statement(from, st, to));
          from = to;
        });
        if (!eatP(",")) break;
      }
      if (!eatP(";")) break;
      if (isP(".") || isP("}")) break;
    }
  };

  // ── filters ──
  const primary = () => {
    const t = next();
    if (t.t === "var") return { k: "var", v: t.v };
    if (t.t === "str" || t.t === "num") return { k: "lit", lit: literal(t), raw: t };
    if (t.t === "word" && isP("(")) {
      i++;
      const args = [];
      if (!isP(")")) { args.push(expr()); while (eatP(",")) args.push(expr()); }
      needP(")");
      return { k: "call", fn: t.u, args };
    }
    if (t.t === "p" && t.v === "(") { const e = expr(); needP(")"); return e; }
    return no("expression");
  };
  const comparison = () => {
    const left = primary();
    const t = peek();
    if (t && t.t === "p" && CMP_OPS.includes(t.v)) { i++; return { k: "cmp", op: t.v, left, right: primary() }; }
    return left;
  };
  const expr = () => {
    const parts = [comparison()];
    while (eatP("&&")) parts.push(comparison());
    if (isP("||")) no("or");
    return parts.length === 1 ? parts[0] : { k: "and", parts };
  };
  const call = (e, fn, n) => e.k === "call" && e.fn === fn && e.args.length === n;
  const strOf = (e) => (call(e, "STR", 1) && e.args[0].k === "var" ? e.args[0].v : null);
  const lowerStrOf = (e) => (call(e, "LCASE", 1) ? strOf(e.args[0]) : null);
  const text = (e) => (e.k === "lit" && e.raw.t === "str" && !e.raw.lang && !e.raw.dtAt ? e.raw.v : null);

  const filterBlock = (e) => {
    if (call(e, "CONTAINS", 2) || call(e, "STRSTARTS", 2)) {
      const v = lowerStrOf(e.args[0]), q = text(e.args[1]);
      // The blocks compare in lower case; a query that does not is another query.
      if (v == null || q == null || !q || q !== q.toLowerCase()) no("text test");
      return { ...make("text"), v: vr(v), op: e.fn === "CONTAINS" ? "contains" : "starts", q };
    }
    if (call(e, "REGEX", 3)) {
      const v = strOf(e.args[0]), q = text(e.args[1]);
      if (v == null || !q || text(e.args[2]) !== "i") no("regex");
      return { ...make("text"), v: vr(v), op: "regex", q };
    }
    if (e.k !== "cmp") no("filter");
    const { left, right, op } = e;
    if (op === "=" && strOf(left) != null && text(right)) return { ...make("text"), v: vr(strOf(left)), op: "equals", q: text(right) };
    if (op === "=" && call(left, "LANG", 1) && left.args[0].k === "var" && text(right)) return { ...make("lang"), v: vr(left.args[0].v), lang: text(right) };
    if (call(left, "YEAR", 1) && left.args[0].k === "var" && right.k === "lit" && right.raw.t === "num" && /^\d{4}$/.test(right.raw.v))
      return { ...make("compare"), v: vr(left.args[0].v), op, val: lit(right.raw.v, "date") };
    if (left.k !== "var") no("comparison");
    if (right.k === "var") return { ...make("compare"), v: vr(left.v), op, val: vr(right.v) };
    if (right.k === "lit" && !right.lit.lang) {
      if (right.lit.dt === "text" && /^[+-]?(\d+\.?\d*|\.\d+)([eE][+-]?\d+)?$/.test(String(right.lit.v).trim())) no("number as text");
      return { ...make("compare"), v: vr(left.v), op, val: right.lit };
    }
    return no("comparison");
  };

  const group = () => {
    const out = [];
    while (!isP("}")) {
      if (!peek()) no("unclosed group");
      if (eatW("OPTIONAL")) { needP("{"); out.push({ ...make("optional"), kids: group() }); }
      else if (eatW("FILTER")) {
        if (eatW("NOT")) { needW("EXISTS"); needP("{"); out.push({ ...make("without"), kids: group() }); }
        else if (isW("EXISTS")) no("exists");
        else {
          const e = isP("(") ? (i++, (() => { const x = expr(); needP(")"); return x; })()) : primary();
          for (const part of e.k === "and" ? e.parts : [e]) out.push(filterBlock(part));
        }
      } else if (eatW("BIND")) {
        needP("(");
        const f = next();
        if (f.t !== "word" || !FN_NAMES.has(f.u)) no("bind");
        needP("(");
        const v = next();
        if (v.t !== "var") no("bind argument");
        needP(")"); needW("AS");
        const as = next();
        if (as.t !== "var") no("bind name");
        needP(")");
        out.push({ ...make("calc"), fn: f.u, v: vr(v.v), as: vr(as.v) });
      } else if (eatW("VALUES")) {
        const v = next();
        if (v.t !== "var") no("values");
        needP("{");
        const vals = [];
        while (!isP("}")) vals.push(iri(full(next())));
        i++;
        if (!vals.length) no("empty values");
        out.push({ ...make("is"), v: vr(v.v), vals });
      } else if (eatP("{")) {
        if (isW("SELECT")) no("subquery");
        const parts = [group()];
        while (eatW("UNION")) { needP("{"); parts.push(group()); }
        if (parts.length < 2 || parts.some((p) => !p.length)) no("plain group");
        // a UNION b UNION c: either a, or (either b, or c).
        let acc = parts.pop();
        while (parts.length) acc = [{ ...make("either"), kids: parts.pop(), kids2: acc }];
        out.push(acc[0]);
      } else if (peek().t === "word" && peek().v !== "a") no(peek().u);
      else triples(out);
      eatP(".");
    }
    i++;
    return out;
  };

  // ── the query ──
  while (isW("PREFIX")) {
    i++;
    const p = next(), u = next();
    if (p.t !== "pname" || p.l !== "" || u.t !== "iri") no("prefix");
    prefixes[p.p] = u.v;
  }
  needW("SELECT");
  const sketch = emptySketch();
  const a = sketch.answer;
  a.limit = null;
  a.distinct = eatW("DISTINCT");
  if (isW("REDUCED")) no("reduced");
  const plain = [];
  let count = null;
  if (!eatP("*")) {
    while (peek() && (peek().t === "var" || isP("("))) {
      const t = next();
      if (t.t === "var") { plain.push(t.v); continue; }
      if (count) no("two aggregates");
      needW("COUNT"); needP("(");
      let of = null;
      if (!eatP("*")) { needW("DISTINCT"); const v = next(); if (v.t !== "var") no("count"); of = v.v; }
      needP(")"); needW("AS");
      const alias = next();
      if (alias.t !== "var") no("alias");
      needP(")");
      count = { of, alias: alias.v };
    }
    if (!plain.length && !count) no("projection");
  }
  eatW("WHERE");
  needP("{");
  sketch.blocks = group();
  if (!sketch.blocks.length) no("empty");

  let grouped = null;
  if (eatW("GROUP")) { needW("BY"); grouped = []; while (peek() && peek().t === "var") grouped.push(next().v); if (!grouped.length) no("group by"); }
  if (isW("HAVING")) no("having");
  const order = [];
  if (eatW("ORDER")) {
    needW("BY");
    for (;;) {
      if (peek() && peek().t === "var") order.push({ v: next().v, dir: "asc" });
      else if (isW("ASC") || isW("DESC")) { const dir = next().u.toLowerCase(); needP("("); const v = next(); if (v.t !== "var") no("order"); needP(")"); order.push({ v: v.v, dir }); }
      else break;
    }
    if (order.length !== 1) no("order by");
  }
  if (eatW("LIMIT")) { const n = next(); if (n.t !== "num" || !/^\d+$/.test(n.v) || n.v === "0") no("limit"); a.limit = Number(n.v); }
  if (peek()) no("trailing " + (peek().u || peek().v));

  if (count) {
    if (a.distinct) no("distinct count");
    if (used.has("count") && count.alias !== "count") no("a variable named count");
    if (plain.length ? !grouped || grouped.length !== plain.length || !plain.every((v) => grouped.includes(v)) : !!grouped) no("grouping");
    a.mode = "count"; a.of = count.of; a.per = plain;
    if (order.length) {
      const o = order[0];
      if (o.v === count.alias) a.sort = { v: "count", dir: o.dir };
      else if (plain.includes(o.v)) a.sort = { v: o.v, dir: o.dir };
      else no("order");
    }
  } else {
    if (grouped) no("group without count");
    a.show = plain;
    if (order.length) a.sort = order[0];
  }
  return sketch;
}
