// The sketch and the SPARQL it stands for. No DOM here: the page (app.js)
// draws a sketch and this file turns it into a query, names its variables
// and says in words what each one is.
//
// A term in a slot is one of
//   { k: "var", v: "person" }            a variable
//   { k: "iri", v: "http://…", l: "…" }  a thing, kind or property (l: a label to show)
//   { k: "lit", v: "…", dt: "text" | "number" | "date", lang?: "en" }
//   { k: "hole" }                        an empty slot; it becomes a variable of its own
//
// A block is { id, t, … }:
//   kind     s a c                        "?s is a c"
//   link     s p o, path                  "?s —p→ ?o"
//   name     s, o                         the label of s
//   is       v, vals[]                    ?v is one of these things
//   text     v, op, q                     contains / starts / equals / matches
//   compare  v, op, val                   < > = …
//   lang     v, lang
//   calc     fn, v, as                    BIND(YEAR(?v) AS ?as)
//   optional / without  kids[]            OPTIONAL { } / FILTER NOT EXISTS { }
//   either   kids[], kids2[]              { } UNION { }

export const NS = {
  rdf: "http://www.w3.org/1999/02/22-rdf-syntax-ns#",
  rdfs: "http://www.w3.org/2000/01/rdf-schema#",
  owl: "http://www.w3.org/2002/07/owl#",
  xsd: "http://www.w3.org/2001/XMLSchema#",
  skos: "http://www.w3.org/2004/02/skos/core#",
  foaf: "http://xmlns.com/foaf/0.1/",
  schema: "https://schema.org/",
  sdo: "http://schema.org/",
  dcterms: "http://purl.org/dc/terms/",
  dc: "http://purl.org/dc/elements/1.1/",
  prov: "http://www.w3.org/ns/prov#",
  dcat: "http://www.w3.org/ns/dcat#",
  void: "http://rdfs.org/ns/void#",
  geo: "http://www.opengis.net/ont/geosparql#",
  wgs: "http://www.w3.org/2003/01/geo/wgs84_pos#",
  wd: "http://www.wikidata.org/entity/",
  wdt: "http://www.wikidata.org/prop/direct/",
  dbo: "http://dbpedia.org/ontology/",
  dbr: "http://dbpedia.org/resource/",
  bibo: "http://purl.org/ontology/bibo/",
  cito: "http://purl.org/spar/cito/",
  fabio: "http://purl.org/spar/fabio/",
  rel: "http://purl.org/vocab/relationship/",
  sh: "http://www.w3.org/ns/shacl#",
};
export const RDF_TYPE = NS.rdf + "type";
export const RDFS_LABEL = NS.rdfs + "label";
// Without a file there is no way to know which property names things, so the
// generic sketch tries the usual ones.
export const GENERIC_LABELS = [RDFS_LABEL, NS.schema + "name", NS.sdo + "name", NS.foaf + "name", NS.skos + "prefLabel", NS.dcterms + "title"];

export const hole = () => ({ k: "hole" });
export const vr = (v) => ({ k: "var", v });
export const iri = (v, l) => (l ? { k: "iri", v, l } : { k: "iri", v });
export const lit = (v, dt = "text", lang) => (lang ? { k: "lit", v, dt, lang } : { k: "lit", v, dt });

let seq = 0;
export const nid = () => "b" + (++seq).toString(36) + Math.random().toString(36).slice(2, 6);

export const CONTAINERS = new Set(["optional", "without", "either"]);

/** A fresh block of type `t`, wired to the variables a child would expect. */
export function make(t, focus = "thing") {
  const id = nid();
  switch (t) {
    case "kind": return { id, t, s: vr(focus), c: hole() };
    case "link": return { id, t, s: vr(focus), p: hole(), o: hole(), path: "" };
    case "name": return { id, t, s: vr(focus), o: vr(focus === "thing" ? "name" : focus + "Name") };
    case "is": return { id, t, v: vr(focus), vals: [] };
    case "text": return { id, t, v: hole(), op: "contains", q: "" };
    case "compare": return { id, t, v: hole(), op: ">", val: lit("", "number") };
    case "lang": return { id, t, v: hole(), lang: "en" };
    case "calc": return { id, t, fn: "YEAR", v: hole(), as: vr("year") };
    case "optional": case "without": return { id, t, kids: [] };
    case "either": return { id, t, kids: [], kids2: [] };
  }
  throw new Error("unknown block " + t);
}

export const emptySketch = () => ({
  blocks: [],
  answer: { mode: "list", show: [], distinct: false, of: null, per: [], sort: null, limit: 50 },
});

export const clone = (o) => JSON.parse(JSON.stringify(o));

/** Every block, depth first, with the list that holds it. */
export function* walk(blocks) {
  for (const b of blocks) {
    yield [b, blocks];
    if (b.kids) yield* walk(b.kids);
    if (b.kids2) yield* walk(b.kids2);
  }
}
export function find(sketch, id) {
  for (const [b, list] of walk(sketch.blocks)) if (b.id === id) return { block: b, list, index: list.indexOf(b) };
  return null;
}
export function contains(block, id) {
  if (block.id === id) return true;
  for (const [b] of walk([...(block.kids || []), ...(block.kids2 || [])])) if (b.id === id) return true;
  return false;
}

export const normVar = (s) => {
  let v = String(s || "").trim().replace(/^[?$]/, "").replace(/[^\p{L}\p{N}_]+/gu, "_").replace(/^_+|_+$/g, "");
  if (/^\d/.test(v)) v = "v" + v;
  return v;
};

// ── names ────────────────────────────────────────────────────────────────

export function splitIri(v) {
  const m = /^(.*[#/:])([^#/:]*)$/.exec(v);
  return m ? [m[1], m[2]] : ["", v];
}
/** "knowsOf" → "knows of", "birth_date" → "birth date". */
export function humanize(local) {
  let s = local;
  try { s = decodeURIComponent(local); } catch { /* keep as it is */ }
  s = s.replace(/[_\-+]+/g, " ").replace(/([a-z\d])([A-Z])/g, "$1 $2").replace(/([A-Z]+)([A-Z][a-z])/g, "$1 $2").trim();
  if (!s) return local;
  // A name written in capitals throughout ("Mary Lou Williams", "Person")
  // stays; a camel-cased phrase ("knows Of") reads as lower-case words.
  const words = s.split(" ");
  if (words.every((w) => /^[A-Z\d]/.test(w))) return s;
  return words.map((w) => (/^[A-Z][a-z]/.test(w) ? w[0].toLowerCase() + w.slice(1) : w)).join(" ");
}
/** The words to show for an IRI: its label when one is known, else its last part. */
export function nameOf(term) {
  if (term.l) return term.l;
  const [, local] = splitIri(term.v);
  if (!local) return term.v;
  // Opaque ids (Q42, P31, 12345) read better as they are.
  if (/^[A-Z]?\d+$/.test(local)) return local;
  return humanize(local);
}

/** Prefix table for a set of IRIs: the well-known ones, then made-up names. */
export function prefixer(extra = {}) {
  const byNs = new Map();
  for (const [p, ns] of Object.entries(NS)) if (!byNs.has(ns)) byNs.set(ns, p);
  for (const [p, ns] of Object.entries(extra)) if (!byNs.has(ns)) byNs.set(ns, p);
  const taken = new Set(byNs.values());
  const used = new Map();
  const invent = (ns) => {
    const words = ns.replace(/^[a-z]+:\/\/(www\.)?/, "").split(/[^A-Za-z]+/).filter((w) => w && !/^(org|com|net|www|http|https|ns|vocab|ontology|resource|purl|w3id|data|def|core|terms)$/i.test(w));
    const base = (words[words.length - 1] || words[0] || "ns").toLowerCase().slice(0, 8);
    let p = base, i = 2;
    while (taken.has(p)) p = base + i++;
    taken.add(p); byNs.set(ns, p);
    return p;
  };
  const ok = (ns, local) => ns && local && /^[A-Za-z_][\w-]*$/.test(local) && /^[a-z][a-z0-9+.-]*:/i.test(ns) && ns.length > 8;
  return {
    /** `<iri>` or `prefix:local`, remembering the prefix for the header. */
    term(v) {
      const [ns, local] = splitIri(v);
      if (ok(ns, local)) {
        const p = byNs.get(ns) || invent(ns);
        used.set(p, ns);
        return p + ":" + local;
      }
      return "<" + v + ">";
    },
    /** Same shortening for display, only with prefixes already known. */
    show(v) {
      const [ns, local] = splitIri(v);
      const p = byNs.get(ns);
      return p && local ? p + ":" + local : v;
    },
    header() {
      return [...used].sort((a, b) => a[0].localeCompare(b[0])).map(([p, ns]) => `PREFIX ${p}: <${ns}>`).join("\n");
    },
  };
}

// ── variables ────────────────────────────────────────────────────────────

const HOLE_NAMES = { s: "thing", c: "kind", p: "property", o: "value", v: "value" };
const TERM_FIELDS = ["s", "c", "p", "o", "v", "val", "as"];

/**
 * Give every empty slot a variable of its own, so a half-drawn sketch is
 * still a query. Returns (block, field) → variable name (or null).
 */
function holeNamer(sketch) {
  const taken = new Set();
  for (const [b] of walk(sketch.blocks)) for (const f of TERM_FIELDS) if (b[f] && b[f].k === "var" && b[f].v) taken.add(b[f].v);
  const names = new Map();
  for (const [b] of walk(sketch.blocks)) {
    for (const f of ["s", "c", "p", "o", "v"]) {
      const t = b[f];
      if (!t || t.k !== "hole") continue;
      const base = HOLE_NAMES[f];
      let n = base, i = 2;
      while (taken.has(n)) n = base + i++;
      taken.add(n);
      names.set(b.id + "." + f, n);
    }
  }
  return (b, f) => names.get(b.id + "." + f) || null;
}

/** The name an open slot stands for. */
export function holeName(sketch, block, field) {
  return holeNamer(sketch)(block, field);
}

/** The variables of a sketch in the order they first appear. */
export function variables(sketch) {
  const namer = holeNamer(sketch);
  const out = [], seen = new Set();
  const add = (v, open) => { if (v && !seen.has(v)) { seen.add(v); out.push({ name: v, open: !!open }); } };
  for (const [b] of walk(sketch.blocks)) {
    for (const f of TERM_FIELDS) {
      const t = b[f];
      if (!t) continue;
      if (t.k === "var") add(t.v);
      else if (t.k === "hole" && namer(b, f)) add(namer(b, f), true);
    }
  }
  return out;
}

/** In words, what the sketch says about each variable. */
export function describe(sketch) {
  const namer = holeNamer(sketch);
  const info = new Map();
  const say = (v, text) => { if (!v) return; if (!info.has(v)) info.set(v, []); if (!info.get(v).includes(text)) info.get(v).push(text); };
  const vOf = (b, f) => (b[f].k === "var" ? b[f].v : b[f].k === "hole" ? namer(b, f) : null);
  const word = (b, f) => { const t = b[f]; return t.k === "iri" ? nameOf(t) : t.k === "lit" ? `“${t.v}”` : "?" + vOf(b, f); };
  for (const [b] of walk(sketch.blocks)) {
    if (b.t === "kind") {
      say(vOf(b, "s"), b.c.k === "iri" ? `a ${nameOf(b.c)}` : `something of kind ?${vOf(b, "c")}`);
      if (b.c.k !== "iri") say(vOf(b, "c"), `the kind of ${word(b, "s")}`);
    } else if (b.t === "link") {
      const p = b.p.k === "iri" ? nameOf(b.p) : "?" + vOf(b, "p");
      say(vOf(b, "s"), `has ${p} ${word(b, "o")}`);
      say(vOf(b, "o"), `the ${p} of ${word(b, "s")}`);
      if (b.p.k !== "iri") say(vOf(b, "p"), `a property that links ${word(b, "s")} to ${word(b, "o")}`);
    } else if (b.t === "name") {
      say(vOf(b, "o"), `the name of ${word(b, "s")}`);
      say(vOf(b, "s"), `named ?${vOf(b, "o")}`);
    } else if (b.t === "is") {
      if (b.vals.length) say(vOf(b, "v"), b.vals.length === 1 ? `exactly ${nameOf(b.vals[0])}` : `one of ${b.vals.length} things`);
    } else if (b.t === "calc") {
      say(vOf(b, "as"), `the ${(CALC_FNS.find((f) => f[0] === b.fn) || [0, b.fn])[1]} of ${word(b, "v")}`);
    } else if (b.t === "text" && b.q) {
      say(vOf(b, "v"), `${TEXT_OPS[b.op]} “${b.q}”`);
    } else if (b.t === "compare" && (b.val.k !== "lit" || b.val.v !== "")) {
      say(vOf(b, "v"), `${b.op} ${b.val.k === "var" ? "?" + b.val.v : b.val.v}`);
    }
  }
  return info;
}

/** The kinds a variable is said to be (for ranking the properties offered). */
export function kindsOf(sketch, name) {
  const out = [];
  for (const [b] of walk(sketch.blocks)) if (b.t === "kind" && b.s.k === "var" && b.s.v === name && b.c.k === "iri") out.push(b.c.v);
  return out;
}

// ── SPARQL ───────────────────────────────────────────────────────────────

export const TEXT_OPS = { contains: "contains", starts: "starts with", equals: "is exactly", regex: "matches" };
export const CMP_OPS = [">", ">=", "<", "<=", "=", "!="];
export const CALC_FNS = [["YEAR", "year"], ["MONTH", "month"], ["STRLEN", "length"], ["LCASE", "lower case"], ["UCASE", "upper case"], ["STR", "plain text"], ["LANG", "language"], ["DATATYPE", "datatype"]];
export const PATHS = [["", "directly"], ["+", "in one or more steps"], ["*", "in any number of steps"], ["^", "the other way round"]];

const escStr = (s) => String(s).replace(/\\/g, "\\\\").replace(/"/g, '\\"').replace(/\n/g, "\\n").replace(/\r/g, "\\r").replace(/\t/g, "\\t");

function litOut(t, px) {
  if (t.dt === "number" && /^[+-]?(\d+\.?\d*|\.\d+)([eE][+-]?\d+)?$/.test(String(t.v).trim())) return String(t.v).trim();
  if (t.dt === "date") {
    const v = String(t.v).trim();
    if (/^\d{4}$/.test(v)) return `"${v}-01-01"^^${px.term(NS.xsd + "date")}`;
    if (/^\d{4}-\d{2}-\d{2}$/.test(v)) return `"${v}"^^${px.term(NS.xsd + "date")}`;
    if (/^\d{4}-\d{2}-\d{2}T/.test(v)) return `"${v}"^^${px.term(NS.xsd + "dateTime")}`;
  }
  return `"${escStr(t.v)}"` + (t.lang ? "@" + t.lang : "");
}

/**
 * The query a sketch stands for.
 * ctx: { labelPredicate?: iri, prefixes?: {p: ns} }
 * Returns { text, vars, problems[] }; `problems` are plain sentences about
 * what is still open, never a reason not to produce a query.
 */
export function toSparql(sketch, ctx = {}, opts = {}) {
  const px = prefixer(ctx.prefixes);
  const tpl = []; // the statements a CONSTRUCT of this sketch writes
  const namer = holeNamer(sketch);
  const problems = [];
  const T = (b, f) => {
    const t = b[f];
    if (t.k === "var") return "?" + t.v;
    if (t.k === "hole") return "?" + namer(b, f);
    if (t.k === "iri") return px.term(t.v);
    return litOut(t, px);
  };
  const labelPath = () => (ctx.labelPredicate ? px.term(ctx.labelPredicate) : GENERIC_LABELS.map((l) => px.term(l)).join("|"));

  const body = (blocks, ind) => {
    const pad = "  ".repeat(ind);
    const out = [];
    for (const b of blocks) {
      switch (b.t) {
        case "kind": out.push(`${pad}${T(b, "s")} a ${T(b, "c")} .`); tpl.push(`${T(b, "s")} a ${T(b, "c")} .`); break;
        case "link": {
          let p = T(b, "p");
          if (b.path && b.p.k !== "iri") problems.push("“" + PATHS.find((x) => x[0] === b.path)[1] + "” needs a named property; it is left out while the property is open.");
          else if (b.path === "^") p = "^" + p;
          else if (b.path) p = p + b.path;
          out.push(`${pad}${T(b, "s")} ${p} ${T(b, "o")} .`);
          // A path is followed to find the two ends; the statement written is the plain link.
          tpl.push(b.path === "^" ? `${T(b, "o")} ${T(b, "p")} ${T(b, "s")} .` : `${T(b, "s")} ${T(b, "p")} ${T(b, "o")} .`);
          break;
        }
        case "name": out.push(`${pad}${T(b, "s")} ${labelPath()} ${T(b, "o")} .`); tpl.push(`${T(b, "s")} ${px.term(ctx.labelPredicate || RDFS_LABEL)} ${T(b, "o")} .`); break;
        case "is":
          if (b.vals.length) out.push(`${pad}VALUES ${T(b, "v")} { ${b.vals.map((x) => px.term(x.v)).join(" ")} }`);
          else problems.push("An “is exactly” block has nothing picked yet, so it is left out.");
          break;
        case "text": {
          if (!b.q) { problems.push("A text filter is empty, so it is left out."); break; }
          const v = T(b, "v"), q = `"${escStr(b.q)}"`, lq = `"${escStr(b.q.toLowerCase())}"`;
          const e = b.op === "contains" ? `CONTAINS(LCASE(STR(${v})), ${lq})`
            : b.op === "starts" ? `STRSTARTS(LCASE(STR(${v})), ${lq})`
            : b.op === "equals" ? `STR(${v}) = ${q}`
            : `REGEX(STR(${v}), ${q}, "i")`;
          out.push(`${pad}FILTER(${e})`);
          break;
        }
        case "compare": {
          if (b.val.k === "lit" && b.val.v === "") { problems.push("A comparison has no value, so it is left out."); break; }
          const v = T(b, "v");
          // A bare year compares against the year of a date.
          if (b.val.k === "lit" && b.val.dt === "date" && /^\d{4}$/.test(String(b.val.v).trim()))
            out.push(`${pad}FILTER(YEAR(${v}) ${b.op} ${String(b.val.v).trim()})`);
          else out.push(`${pad}FILTER(${v} ${b.op} ${T(b, "val")})`);
          break;
        }
        case "lang": out.push(`${pad}FILTER(LANG(${T(b, "v")}) = "${escStr(b.lang)}")`); break;
        case "calc": out.push(`${pad}BIND(${b.fn}(${T(b, "v")}) AS ${T(b, "as")})`); break;
        case "optional":
          if (b.kids.length) out.push(`${pad}OPTIONAL {`, ...body(b.kids, ind + 1), `${pad}}`);
          break;
        case "without":
          if (b.kids.length) out.push(`${pad}FILTER NOT EXISTS {`, ...body(b.kids, ind + 1), `${pad}}`);
          break;
        case "either":
          if (b.kids.length && b.kids2.length) out.push(`${pad}{`, ...body(b.kids, ind + 1), `${pad}} UNION {`, ...body(b.kids2, ind + 1), `${pad}}`);
          else if (b.kids.length || b.kids2.length) out.push(...body(b.kids.length ? b.kids : b.kids2, ind));
          break;
      }
    }
    return out;
  };

  const where = body(sketch.blocks, 1);
  const all = variables(sketch).map((v) => v.name);
  const known = new Set(all);
  const a = sketch.answer;
  const keep = (list) => list.filter((v) => known.has(v));
  const tail = [];
  let head, vars;

  if (!where.length) {
    // Nothing drawn yet: the pattern that matches every statement.
    where.push("  ?thing ?property ?value .");
    for (const v of ["thing", "property", "value"]) if (!known.has(v)) { known.add(v); all.push(v); }
  }

  if (a.mode === "count") {
    const per = keep(a.per);
    const of = a.of && known.has(a.of) ? a.of : null;
    const what = of ? `COUNT(DISTINCT ?${of})` : "COUNT(*)";
    head = "SELECT " + per.map((v) => "?" + v + " ").join("") + `(${what} AS ?count)`;
    if (per.length) tail.push("GROUP BY " + per.map((v) => "?" + v).join(" "));
    const s = a.sort && (a.sort.v === "count" || per.includes(a.sort.v)) ? a.sort : { v: "count", dir: "desc" };
    if (per.length) tail.push(`ORDER BY ${s.dir === "desc" ? `DESC(?${s.v})` : `?${s.v}`}`);
    vars = [...per, "count"];
  } else {
    const show = keep(a.show);
    head = "SELECT " + (a.distinct ? "DISTINCT " : "") + (show.length ? show.map((v) => "?" + v).join(" ") : "*");
    if (a.sort && known.has(a.sort.v)) tail.push(`ORDER BY ${a.sort.dir === "desc" ? `DESC(?${a.sort.v})` : `?${a.sort.v}`}`);
    vars = show.length ? show : all;
  }
  const n = Number(a.limit);
  if (Number.isFinite(n) && n > 0) tail.push("LIMIT " + Math.floor(n));

  if (opts.construct) {
    const lines = [...new Set(tpl)];
    if (!lines.length) lines.push("?thing ?property ?value .");
    const limit = Number.isFinite(n) && n > 0 ? "\nLIMIT " + Math.floor(n) : "";
    const pre = px.header();
    const text = (pre ? pre + "\n\n" : "") + "CONSTRUCT {\n" + lines.map((l) => "  " + l).join("\n") + "\n}\nWHERE {\n" + where.join("\n") + "\n}" + limit;
    return { text, vars: [], problems };
  }
  const prefixes = px.header();
  const text = (prefixes ? prefixes + "\n\n" : "") + head + "\nWHERE {\n" + where.join("\n") + "\n}" + (tail.length ? "\n" + tail.join("\n") : "");
  return { text, vars, problems };
}

/**
 * What a graph of the result draws: the links of the sketch, and the names
 * it asks for. Open slots appear as the variables they stand for.
 */
export function graphTemplate(sketch) {
  const namer = holeNamer(sketch);
  const t = (b, f) => (b[f].k === "hole" ? vr(namer(b, f)) : b[f]);
  const links = [], names = [];
  for (const [b] of walk(sketch.blocks)) {
    if (b.t === "link") links.push(b.path === "^" ? { s: t(b, "o"), p: t(b, "p"), o: t(b, "s") } : { s: t(b, "s"), p: t(b, "p"), o: t(b, "o") });
    else if (b.t === "name") names.push({ s: t(b, "s"), o: t(b, "o") });
  }
  return { links, names };
}

/** Rename a variable everywhere it is used. */
export function rename(sketch, from, to) {
  to = normVar(to);
  if (!to || to === from) return;
  for (const [b] of walk(sketch.blocks)) for (const f of TERM_FIELDS) if (b[f] && b[f].k === "var" && b[f].v === from) b[f].v = to;
  const a = sketch.answer;
  a.show = a.show.map((v) => (v === from ? to : v));
  a.per = a.per.map((v) => (v === from ? to : v));
  if (a.of === from) a.of = to;
  if (a.sort && a.sort.v === from) a.sort.v = to;
}

/** Drop what the answer names but the sketch no longer has. */
export function tidy(sketch) {
  const known = new Set(variables(sketch).map((v) => v.name));
  const a = sketch.answer;
  a.show = a.show.filter((v) => known.has(v));
  a.per = a.per.filter((v) => known.has(v));
  if (a.of && !known.has(a.of)) a.of = null;
  if (a.sort && a.sort.v !== "count" && !known.has(a.sort.v)) a.sort = null;
}

// ── ready-made sketches ──────────────────────────────────────────────────
//
// Each takes what the open file's card says (or nothing) and returns a
// sketch. Without a file the slots stay open.
//   d.topClass  { v, l? }                 the most common kind
//   d.topLink   { s?: {v}, p: {v} }       the most common link between kinds

const K = (s, c) => ({ ...make("kind"), s: vr(s), c: c ? iri(c.v, c.l) : hole() });
const L = (s, p, o) => ({ ...make("link"), s: vr(s), p: p ? iri(p.v, p.l) : hole(), o: typeof o === "string" ? vr(o) : o });
const N = (s, o) => ({ ...make("name"), s: vr(s), o: vr(o) });

export const RECIPES = [
  {
    id: "list", title: "List things of a kind", hint: "Every thing of one kind, with its name.",
    build(d) {
      const s = emptySketch();
      s.blocks = [K("thing", d.topClass), N("thing", "name")];
      s.answer.show = ["thing", "name"];
      return s;
    },
  },
  {
    id: "find", title: "Find something by name", hint: "Things whose name contains a word.",
    build() {
      const s = emptySketch();
      s.blocks = [N("thing", "name"), { ...make("text"), v: vr("name"), op: "contains", q: "" }];
      s.answer.show = ["thing", "name"]; s.answer.limit = 25;
      return s;
    },
  },
  {
    id: "about", title: "Everything about one thing", hint: "Pick a thing and read all it says.",
    build() {
      const s = emptySketch();
      s.blocks = [{ ...make("is"), v: vr("thing"), vals: [] }, L("thing", null, "value")];
      s.blocks[1].p = vr("property");
      s.answer.show = ["property", "value"]; s.answer.limit = 200;
      return s;
    },
  },
  {
    id: "kinds", title: "How many of each kind", hint: "Count the things per kind.",
    build() {
      const s = emptySketch();
      s.blocks = [{ ...make("kind"), s: vr("thing"), c: vr("kind") }];
      s.answer.mode = "count"; s.answer.of = "thing"; s.answer.per = ["kind"]; s.answer.limit = 25;
      return s;
    },
  },
  {
    id: "top", title: "Who has the most", hint: "Rank things by how many links they have.",
    build(d) {
      const s = emptySketch();
      const l = d.topLink;
      s.blocks = [...(l && l.s ? [K("thing", l.s)] : []), L("thing", l && l.p, "other"), { ...make("optional"), kids: [N("thing", "name")] }];
      s.answer.mode = "count"; s.answer.of = "other"; s.answer.per = ["thing", "name"]; s.answer.limit = 20;
      return s;
    },
  },
  {
    id: "linked", title: "Things linked to one thing", hint: "Its neighbours, and how they are linked.",
    build() {
      const s = emptySketch();
      const l = L("thing", null, "other");
      l.p = vr("property");
      s.blocks = [{ ...make("is"), v: vr("thing"), vals: [] }, l, { ...make("optional"), kids: [N("other", "name")] }];
      s.answer.show = ["property", "other", "name"]; s.answer.limit = 100;
      return s;
    },
  },
  {
    id: "missing", title: "Things missing something", hint: "Of a kind, but without a given property.",
    build(d) {
      const s = emptySketch();
      s.blocks = [K("thing", d.topClass), { ...make("without"), kids: [make("link")] }];
      s.answer.show = ["thing"];
      return s;
    },
  },
  {
    id: "range", title: "Filter by a number or year", hint: "Keep the values above or below a bound.",
    build(d) {
      const s = emptySketch();
      s.blocks = [...(d.topClass ? [K("thing", d.topClass)] : []), L("thing", null, "value"), { ...make("compare"), v: vr("value"), op: ">", val: lit("", "number") }];
      s.answer.show = ["thing", "value"]; s.answer.sort = { v: "value", dir: "desc" };
      return s;
    },
  },
  {
    id: "either", title: "Either of two kinds", hint: "Things that are one kind or another (a union).",
    build(d) {
      const s = emptySketch();
      s.blocks = [{ ...make("either"), kids: [K("thing", d.topClass)], kids2: [K("thing", null)] }, { ...make("optional"), kids: [N("thing", "name")] }];
      s.answer.show = ["thing", "name"];
      return s;
    },
  },
  {
    id: "anyof", title: "One property or another", hint: "A value found under either of two properties.",
    build(d) {
      const s = emptySketch();
      s.blocks = [{ ...make("either"), kids: [L("thing", d.topLink && d.topLink.p, "value")], kids2: [L("thing", null, "value")] }];
      s.answer.show = ["thing", "value"];
      return s;
    },
  },
  {
    id: "optional", title: "With a detail, if there is one", hint: "Keep every thing, add a value where it exists.",
    build(d) {
      const s = emptySketch();
      s.blocks = [K("thing", d.topClass), N("thing", "name"), { ...make("optional"), kids: [L("thing", null, "detail")] }];
      s.answer.show = ["thing", "name", "detail"];
      return s;
    },
  },
  {
    id: "both", title: "One thing but not another", hint: "Has a property and lacks a second one.",
    build(d) {
      const s = emptySketch();
      s.blocks = [L("thing", d.topLink && d.topLink.p, "value"), { ...make("without"), kids: [make("link")] }];
      s.answer.show = ["thing", "value"];
      return s;
    },
  },
  {
    id: "shared", title: "Two things that share something", hint: "Pairs linked to the same third thing.",
    build(d) {
      const s = emptySketch();
      const p = d.topLink && d.topLink.p;
      s.blocks = [L("first", p, "shared"), L("second", p, "shared"), { ...make("compare"), v: vr("first"), op: "!=", val: vr("second") }];
      s.answer.show = ["first", "second", "shared"];
      return s;
    },
  },
  {
    id: "chain", title: "Follow a chain", hint: "Everything reached through a property, step after step.",
    build(d) {
      const s = emptySketch();
      const l = L("thing", d.topLink && d.topLink.p, "reached");
      l.path = "+";
      s.blocks = [{ ...make("is"), v: vr("thing"), vals: [] }, l, { ...make("optional"), kids: [N("reached", "name")] }];
      s.answer.show = ["reached", "name"]; s.answer.distinct = true; s.answer.limit = 100;
      return s;
    },
  },
  {
    id: "values", title: "Most common values", hint: "Count how often each value of a property appears.",
    build(d) {
      const s = emptySketch();
      s.blocks = [L("thing", d.topLink && d.topLink.p, "value")];
      s.answer.mode = "count"; s.answer.of = "thing"; s.answer.per = ["value"]; s.answer.limit = 25;
      return s;
    },
  },
  {
    id: "years", title: "How many per year", hint: "Take the year of a date and count per year.",
    build() {
      const s = emptySketch();
      s.blocks = [L("thing", null, "date"), { ...make("calc"), fn: "YEAR", v: vr("date"), as: vr("year") }];
      s.answer.mode = "count"; s.answer.of = "thing"; s.answer.per = ["year"]; s.answer.sort = { v: "year", dir: "asc" }; s.answer.limit = 200;
      return s;
    },
  },
  {
    id: "language", title: "Names in one language", hint: "Only the names written in a given language.",
    build(d) {
      const s = emptySketch();
      s.blocks = [K("thing", d.topClass), N("thing", "name"), { ...make("lang"), v: vr("name"), lang: "en" }];
      s.answer.show = ["thing", "name"];
      return s;
    },
  },
];
