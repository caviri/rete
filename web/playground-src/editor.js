// ---------------------------------------------------------------------------
// PlaygroundEditor — a self-contained code-editor component for the playground.
//
// Wraps a <textarea> with: a line-number gutter, a syntax-highlight overlay,
// keyword/variable/schema autocomplete, **entity search** (by label, over the
// loaded graph's label index), and hover tooltips for keywords.
//
// IMPORTANT — it CANNOT change what a query does. The component only ever reads
// and writes the textarea's *text*; the query/shape is still evaluated from the
// textarea's literal value at run time, exactly as if it were typed by hand.
// Autocomplete just inserts text; highlighting is a visual overlay; hover is a
// passive tooltip. Nothing here touches the engine or the run path.
//
// API (attached to window so the baked app.js can call it):
//   PlaygroundEditor.enhance(id, lang, ctx)   // lang: "sparql" | "ttl"
//   PlaygroundEditor.setText(id, text)
//   PlaygroundEditor.editors                  // { id -> editor }
//
// ctx (all optional): {
//   schema:        () => ({ classes:[[iri,n]], relations:[[s,p,o,n]] }) | null,
//   searchEntities:(prefix) => [{ label, subject }]   // e.g. wasm prefix_search
// }
// ---------------------------------------------------------------------------
;(function () {
  "use strict";

  const $ = (id) => document.getElementById(id);
  const $$ = (sel, el) => Array.from((el || document).querySelectorAll(sel));
  const esc = (s) => String(s).replace(/[&<>"]/g, (c) =>
    ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;" }[c]));
  const shorten = (s, n) => { s = String(s); return s.length > n ? s.slice(0, n - 1) + "…" : s; };

  const EDITORS = {};

  // What each keyword/function means — shown as a hover tooltip (native title)
  // and as the suggestion's right-hand hint, so you can "remember what's what".
  const KEYWORD_INFO = {
    SELECT: "Project variables — the columns to return.",
    CONSTRUCT: "Build a new RDF graph from a template.",
    ASK: "Return true/false: does any solution exist?",
    DESCRIBE: "Return the triples that describe a resource.",
    WHERE: "The graph pattern to match.",
    PREFIX: "Declare a namespace abbreviation.",
    BASE: "Set the base IRI for relative references.",
    FILTER: "Keep only solutions where the expression is true.",
    OPTIONAL: "Left-join: include solutions even when this part is absent.",
    UNION: "Combine the solutions of two alternative patterns.",
    MINUS: "Remove solutions that match this pattern.",
    GRAPH: "Match inside a named graph.",
    SERVICE: "Delegate a sub-pattern to a remote endpoint (not supported here).",
    BIND: "Compute a value and bind it to a variable.",
    VALUES: "Supply an inline table of bindings.",
    DISTINCT: "Drop duplicate solutions.",
    REDUCED: "Permit (but don't require) duplicate removal.",
    ORDER: "ORDER BY — sort the solutions.",
    BY: "Used by ORDER BY / GROUP BY.",
    ASC: "Ascending sort order.",
    DESC: "Descending sort order.",
    GROUP: "GROUP BY — aggregate solutions into groups.",
    HAVING: "Filter groups after aggregation.",
    LIMIT: "Cap the number of solutions returned.",
    OFFSET: "Skip the first N solutions.",
    FROM: "Choose the default graph.",
    NAMED: "FROM NAMED — make a graph visible to GRAPH.",
    COUNT: "Aggregate: count solutions.",
    SUM: "Aggregate: sum a numeric expression.",
    AVG: "Aggregate: average a numeric expression.",
    MIN: "Aggregate: minimum value.",
    MAX: "Aggregate: maximum value.",
    SAMPLE: "Aggregate: any one value from the group.",
    GROUP_CONCAT: "Aggregate: join group values into one string.",
    STR: "The lexical string of a term.",
    STRLEN: "Length of a string.",
    UCASE: "Upper-case a string.",
    LCASE: "Lower-case a string.",
    CONTAINS: "True if a string contains a substring.",
    STRSTARTS: "True if a string starts with a prefix.",
    STRENDS: "True if a string ends with a suffix.",
    STRBEFORE: "The part of a string before a separator.",
    STRAFTER: "The part of a string after a separator.",
    CONCAT: "Concatenate strings.",
    SUBSTR: "Extract a substring.",
    ABS: "Absolute value.",
    CEIL: "Round up.",
    FLOOR: "Round down.",
    ROUND: "Round to the nearest integer.",
    COALESCE: "First argument that is bound / not an error.",
    LANG: "Language tag of a literal.",
    DATATYPE: "Datatype IRI of a literal.",
    BOUND: "True if a variable is bound.",
    IRI: "Make an IRI from a string.",
    URI: "Make an IRI from a string.",
    REGEX: "Match a string against a regular expression.",
    EXISTS: "True if a pattern has any solution.",
    NOT: "Negation (NOT EXISTS / NOT IN).",
    IN: "True if a term is in a list.",
    AS: "Name a computed value (expr AS ?var).",
    UNDEF: "An unbound cell in a VALUES table.",
    "a": "Shorthand for rdf:type."
  };

  const ED_TOKENS = (() => {
    const COM = /#.*/y;
    const STR = /"(?:\\.|[^"\\])*"|'(?:\\.|[^'\\])*'/y;
    const IRI = /<[^>\s]*>?/y;
    const VAR = /[?$][A-Za-z_]\w*/y;
    const NUM = /\b\d[\d_]*(?:\.\d+)?\b/y;
    const IDENT = /[A-Za-z_]\w*/y;
    const PNAME = /[A-Za-z_][\w.-]*:[A-Za-z_][\w.-]*|:[A-Za-z_][\w.-]*/y;
    const A_KW = /\ba\b/y;
    const AT_KW = /@[A-Za-z]+/y;
    const kw = (words, flags) => new RegExp("\\b(?:" + words.join("|") + ")\\b", (flags || "") + "y");
    const SPARQL = kw(["SELECT", "CONSTRUCT", "ASK", "DESCRIBE", "WHERE", "PREFIX", "BASE",
      "FILTER", "OPTIONAL", "UNION", "MINUS", "GRAPH", "SERVICE", "BIND", "VALUES", "DISTINCT",
      "REDUCED", "ORDER", "BY", "ASC", "DESC", "GROUP", "HAVING", "LIMIT", "OFFSET", "FROM",
      "NAMED", "COUNT", "SUM", "AVG", "MIN", "MAX", "SAMPLE", "GROUP_CONCAT", "STR", "STRLEN",
      "UCASE", "LCASE", "CONTAINS", "STRSTARTS", "STRENDS", "STRBEFORE", "STRAFTER", "CONCAT",
      "SUBSTR", "ABS", "CEIL", "FLOOR", "ROUND", "COALESCE", "LANG", "DATATYPE", "BOUND",
      "IRI", "URI", "REGEX", "EXISTS", "NOT", "IN", "AS", "UNDEF"], "i");
    return {
      sparql: [[COM, "com"], [IRI, "iri"], [STR, "str"], [VAR, "var"], [A_KW, "kw"],
        [SPARQL, "kw"], [PNAME, "fn"], [NUM, "num"], [IDENT, null]],
      ttl: [[COM, "com"], [IRI, "iri"], [STR, "str"], [AT_KW, "kw"], [A_KW, "kw"],
        [PNAME, "fn"], [NUM, "num"], [IDENT, null]]
    };
  })();

  // Syntax-highlight `text`. Keyword spans get a `title` so hovering them shows
  // what they do.
  function highlightCode(text, lang) {
    const rules = ED_TOKENS[lang] || ED_TOKENS.ttl;
    let out = "";
    let i = 0;
    const n = text.length;
    const WS = /\s+/y;
    outer: while (i < n) {
      WS.lastIndex = i;
      const w = WS.exec(text);
      if (w && w.index === i) { out += w[0]; i += w[0].length; continue; }
      for (const [re, cls] of rules) {
        re.lastIndex = i;
        const m = re.exec(text);
        if (m && m.index === i && m[0].length) {
          if (cls) {
            const info = cls === "kw" ? KEYWORD_INFO[m[0].toUpperCase()] || KEYWORD_INFO[m[0]] : null;
            const title = info ? ` title="${esc(info)}"` : "";
            out += `<span class="tok-${cls}"${title}>${esc(m[0])}</span>`;
          } else {
            out += esc(m[0]);
          }
          i += m[0].length;
          continue outer;
        }
      }
      out += esc(text[i]);
      i++;
    }
    return out;
  }

  const SPARQL_COMPLETIONS = ["SELECT", "CONSTRUCT", "ASK", "DESCRIBE", "WHERE", "PREFIX",
    "FILTER", "OPTIONAL", "UNION", "MINUS", "GRAPH", "BIND", "VALUES", "DISTINCT",
    "ORDER BY", "ORDER BY DESC(", "GROUP BY", "HAVING", "LIMIT", "OFFSET", "FROM NAMED",
    "COUNT", "SUM", "AVG", "MIN", "MAX", "SAMPLE", "GROUP_CONCAT", "STR", "STRLEN", "UCASE",
    "LCASE", "CONTAINS", "STRSTARTS", "STRENDS", "CONCAT", "SUBSTR", "COALESCE", "LANG",
    "DATATYPE", "BOUND", "REGEX", "EXISTS", "NOT EXISTS", "AS", "UNDEF"];
  const TTL_COMPLETIONS = ["@prefix", "sh:NodeShape", "sh:PropertyShape", "sh:targetClass",
    "sh:targetSubjectsOf", "sh:targetObjectsOf", "sh:property", "sh:path", "sh:minCount",
    "sh:maxCount", "sh:datatype", "sh:class", "sh:nodeKind", "sh:pattern", "sh:message",
    "sh:severity", "sh:in", "sh:or", "sh:and", "sh:not", "xsd:integer", "xsd:double",
    "xsd:date", "xsd:boolean", "xsd:string"];

  // Keyword + variable + schema (class/predicate) suggestions, like before.
  function keywordItems(ed) {
    const items = [];
    const base = ed.lang === "sparql" ? SPARQL_COMPLETIONS : TTL_COMPLETIONS;
    base.forEach((text) => items.push({ text, kind: "kw", hint: KEYWORD_INFO[text.toUpperCase()] || "" }));
    if (ed.lang === "sparql") {
      const vars = new Set(ed.ta.value.match(/[?$][A-Za-z_]\w*/g) || []);
      vars.forEach((v) => items.push({ text: v, kind: "var" }));
    }
    const schema = ed.ctx.schema && ed.ctx.schema();
    if (schema) {
      (schema.classes || []).slice(0, 40).forEach((c) =>
        items.push({ text: "<" + String(c[0]).replace(/^<|>$/g, "") + ">", display: shorten(String(c[0]).replace(/^<|>$/g, ""), 46), kind: "class" }));
      const preds = new Set();
      (schema.relations || []).forEach((r) => preds.add(String(r[1])));
      Array.from(preds).slice(0, 60).forEach((p) =>
        items.push({ text: "<" + p.replace(/^<|>$/g, "") + ">", display: shorten(p.replace(/^<|>$/g, ""), 46), kind: "pred" }));
    }
    return items;
  }

  function currentToken(ta) {
    const pos = ta.selectionStart;
    const head = ta.value.slice(0, pos);
    const m = head.match(/[<A-Za-z0-9_?$:@/#.-]+$/);
    return m ? { token: m[0], start: pos - m[0].length, end: pos } : null;
  }

  function matchKeywords(ed, token) {
    const t = token.toLowerCase();
    const bare = t.replace(/^</, "");
    const seen = new Set();
    const out = [];
    for (const item of keywordItems(ed)) {
      const lower = item.text.toLowerCase();
      if (seen.has(lower)) continue;
      const isIri = lower.startsWith("<");
      const hit = isIri
        ? bare.length >= 2 && lower.includes(bare)
        : lower.startsWith(t) && lower !== t;
      if (hit) { seen.add(lower); out.push(item); if (out.length >= 8) break; }
    }
    return out;
  }

  // Entity (instance) search: a plain word looks up matching labels in the
  // loaded graph's label index and offers their IRIs. Inserting one drops in
  // <iri>; the label + (optional) type are shown so you pick the right thing.
  function matchEntities(ed, token) {
    if (!ed.ctx.searchEntities) return [];
    if (!/^[A-Za-z]/.test(token) || token.length < 2) return [];   // not for ?vars, <iris>, prefixes
    let hits;
    try { hits = ed.ctx.searchEntities(token) || []; } catch (_e) { return []; }
    return hits.slice(0, 6).map((h) => ({
      text: "<" + String(h.subject).replace(/^<|>$/g, "") + ">",
      display: shorten(h.label || h.subject, 44),
      hint: shorten(String(h.subject).replace(/^<|>$/g, ""), 40),
      kind: "entity"
    }));
  }

  function caretOffset(ed) {
    const ta = ed.ta;
    const probe = document.createElement("div");
    probe.style.cssText =
      "position:absolute;visibility:hidden;white-space:pre;padding:12px;" +
      "font:13px/1.46 'Cascadia Mono','SF Mono',Consolas,ui-monospace,monospace;";
    probe.textContent = ta.value.slice(0, ta.selectionStart);
    const mark = document.createElement("span");
    mark.textContent = "​";
    probe.appendChild(mark);
    ed.body.appendChild(probe);
    const x = mark.offsetLeft - ta.scrollLeft;
    const y = mark.offsetTop - ta.scrollTop;
    probe.remove();
    return { x, y };
  }

  function updateSuggest(ed) {
    const tok = currentToken(ed.ta);
    const min = tok && (tok.token.startsWith("?") || tok.token.startsWith("<")) ? 1 : 2;
    if (!tok || tok.token.length < min) return hideSuggest(ed);
    // Keywords / schema first, then matching entities from the graph.
    const items = matchKeywords(ed, tok.token).concat(matchEntities(ed, tok.token)).slice(0, 10);
    if (!items.length) return hideSuggest(ed);
    ed.items = items;
    ed.tok = tok;
    ed.sel = 0;
    const at = caretOffset(ed);
    ed.sug.style.left = Math.max(0, Math.min(at.x, ed.body.clientWidth - 240)) + "px";
    ed.sug.style.top = (at.y + 21) + "px";
    renderSuggest(ed);
    ed.sug.classList.remove("hidden");
  }

  function renderSuggest(ed) {
    ed.sug.innerHTML = ed.items.map((item, i) =>
      `<div class="sg ${i === ed.sel ? "active" : ""}" data-sg="${i}"` +
        (item.hint ? ` title="${esc(item.hint)}"` : "") + ">" +
        `<span>${esc(item.display || item.text)}</span>` +
        `<span class="sg-kind sg-${item.kind}">${esc(item.kind)}</span>` +
      "</div>"
    ).join("");
    $$("[data-sg]", ed.sug).forEach((el) => {
      el.onmousedown = (e) => { e.preventDefault(); acceptSuggest(ed, Number(el.dataset.sg)); };
    });
  }

  function acceptSuggest(ed, index) {
    const item = ed.items[index];
    if (!item || !ed.tok) return;
    const ta = ed.ta;
    ta.value = ta.value.slice(0, ed.tok.start) + item.text + ta.value.slice(ed.tok.end);
    const caret = ed.tok.start + item.text.length;
    ta.setSelectionRange(caret, caret);
    hideSuggest(ed);
    ed.refresh();
    ta.focus();
  }

  function hideSuggest(ed) { ed.sug.classList.add("hidden"); ed.items = []; }

  function suggestKeydown(ed, e) {
    if (ed.sug.classList.contains("hidden")) return;
    if (e.key === "Enter" && (e.ctrlKey || e.metaKey)) { hideSuggest(ed); return; }
    if (e.key === "ArrowDown" || e.key === "ArrowUp") {
      e.preventDefault();
      const d = e.key === "ArrowDown" ? 1 : -1;
      ed.sel = (ed.sel + d + ed.items.length) % ed.items.length;
      renderSuggest(ed);
    } else if (e.key === "Tab" || e.key === "Enter") {
      e.preventDefault();
      acceptSuggest(ed, ed.sel);
    } else if (e.key === "Escape") {
      hideSuggest(ed);
    }
  }

  // ── IRI → human-label decode (optional, purely visual) ──────────────────
  // A toggle floats a human-readable label over each IRI/prefixed-name token.
  // Labels come from: a built-in vocabulary, the dataset's predefined hints
  // (ctx.labelHints), and a best-effort live lookup (ctx.resolveLabels). This
  // NEVER edits the query — it only draws an overlay and sets hover titles.
  const VOCAB = {
    "http://www.w3.org/1999/02/22-rdf-syntax-ns#type": "type",
    "http://www.w3.org/2000/01/rdf-schema#label": "label",
    "http://www.w3.org/2000/01/rdf-schema#subClassOf": "subclass of",
    "http://www.w3.org/2000/01/rdf-schema#subPropertyOf": "subproperty of",
    "http://www.w3.org/2000/01/rdf-schema#comment": "comment",
    "http://www.w3.org/2000/01/rdf-schema#domain": "domain",
    "http://www.w3.org/2000/01/rdf-schema#range": "range",
    "http://www.w3.org/2004/02/skos/core#prefLabel": "preferred label",
    "http://www.w3.org/2004/02/skos/core#altLabel": "alternative label",
    "http://www.w3.org/2004/02/skos/core#broader": "broader",
    "http://www.w3.org/2004/02/skos/core#narrower": "narrower",
    "http://xmlns.com/foaf/0.1/name": "name",
    "http://xmlns.com/foaf/0.1/gender": "gender",
    "http://purl.org/dc/terms/title": "title",
    "http://purl.org/dc/terms/date": "date",
    "http://purl.org/dc/terms/creator": "creator",
    "http://www.w3.org/2002/07/owl#Class": "class",
    "http://www.opengis.net/ont/geosparql#asWKT": "geometry (WKT)",
    "http://www.opengis.net/ont/geosparql#hasGeometry": "has geometry",
    "http://purl.org/spar/cito/cites": "cites"
  };

  // prefix -> namespace, parsed from PREFIX/@prefix declarations in the text.
  function prefixMap(text) {
    const map = {};
    const re = /(?:PREFIX|@prefix)\s+([A-Za-z_][\w.-]*)?:\s*<([^>\s]*)>/gi;
    let m;
    while ((m = re.exec(text))) map[m[1] || ""] = m[2];
    return map;
  }
  // A token's full IRI: "<…>" verbatim, or a "pfx:local" expanded via prefixes.
  function tokenIri(tokenText, prefixes) {
    const t = String(tokenText).trim();
    if (!t) return null;
    if (t[0] === "<") return t.replace(/^<|>$/g, "") || null;
    const i = t.indexOf(":");
    if (i < 0) return null;
    const ns = prefixes[t.slice(0, i)];
    return ns != null ? ns + t.slice(i + 1) : null;
  }
  // VOCAB / hints lookup (sync). Returns a label string, or null if unknown
  // (the live resolver may fill it in and cache it later).
  function decodeLabel(ed, iri) {
    if (ed.labels.has(iri)) return ed.labels.get(iri);
    let lab = VOCAB[iri] || null;
    if (lab == null && ed.ctx.labelHints) {
      const h = ed.ctx.labelHints();
      if (h && h[iri] != null) lab = h[iri];
    }
    if (lab != null) { ed.labels.set(iri, lab); return lab; }
    return null;
  }
  function placeAnno(ed, sp, lab) {
    const a = document.createElement("span");
    a.className = "ed-anno";
    a.textContent = lab;
    a.style.left = (sp.offsetLeft - ed.ta.scrollLeft) + "px";
    a.style.top = (sp.offsetTop - ed.ta.scrollTop - 14) + "px";
    ed.anno.appendChild(a);
  }
  function renderDecorations(ed) {
    if (!ed.anno) return;
    ed.anno.innerHTML = "";
    if (!ed.decode) return;
    const prefixes = prefixMap(ed.ta.value);
    // The <…> in a PREFIX/BASE declaration is a namespace, not an entity — skip
    // those so we don't annotate (or try to resolve) prefix targets.
    const namespaces = new Set(Object.keys(prefixes).map((k) => prefixes[k]));
    const spans = ed.code.querySelectorAll(".tok-iri, .tok-fn");
    const unknown = [];
    spans.forEach((sp) => {
      const iri = tokenIri(sp.textContent, prefixes);
      if (!iri || namespaces.has(iri)) return;
      const lab = decodeLabel(ed, iri);
      if (lab) { sp.title = lab; placeAnno(ed, sp, lab); }
      else if (!ed.labels.has(iri) && !ed.pending.has(iri)) unknown.push(iri);
    });
    // Resolve still-unknown IRIs live (best-effort), tracking each as pending so
    // a render mid-flight doesn't re-request or drop it. Negatives are cached so
    // unresolved IRIs aren't asked again.
    const want = Array.from(new Set(unknown));
    if (want.length && ed.ctx.resolveLabels) {
      want.forEach((iri) => ed.pending.add(iri));
      Promise.resolve(ed.ctx.resolveLabels(want)).then((got) => {
        let added = false;
        want.forEach((iri) => {
          ed.pending.delete(iri);
          const v = (got && got[iri]) || null;
          ed.labels.set(iri, v);
          if (v) added = true;
        });
        if (added && ed.decode) renderDecorations(ed);
      }).catch(() => { want.forEach((iri) => ed.pending.delete(iri)); });
    }
  }
  function setDecode(id, on) {
    const ed = EDITORS[id];
    if (!ed) return false;
    ed.decode = !!on;
    if (ed.wrap) ed.wrap.classList.toggle("decode-on", ed.decode);
    renderDecorations(ed);
    return ed.decode;
  }
  function toggleDecode(id) { const ed = EDITORS[id]; return ed ? setDecode(id, !ed.decode) : false; }

  function enhance(id, lang, ctx) {
    const ta = $(id);
    if (!ta || EDITORS[id]) return;
    const wrap = document.createElement("div");
    wrap.className = "editor";
    const gutter = document.createElement("div");
    gutter.className = "ed-gutter";
    const body = document.createElement("div");
    body.className = "ed-body";
    const hl = document.createElement("pre");
    hl.className = "ed-hl";
    const code = document.createElement("code");
    hl.appendChild(code);
    const sug = document.createElement("div");
    sug.className = "ed-suggest hidden";
    const anno = document.createElement("div");
    anno.className = "ed-anno-layer";

    ta.parentNode.insertBefore(wrap, ta);
    wrap.appendChild(gutter);
    wrap.appendChild(body);
    body.appendChild(hl);
    body.appendChild(ta);
    body.appendChild(anno);
    body.appendChild(sug);
    ta.setAttribute("wrap", "off");
    ta.spellcheck = false;

    const ed = { ta, gutter, code, hl, sug, body, wrap, anno, lang, ctx: ctx || {},
      labels: new Map(), pending: new Set(), decode: false, items: [], sel: 0, tok: null };
    ed.refresh = () => {
      const text = ta.value;
      code.innerHTML = highlightCode(text, lang);
      const lines = text.split("\n").length;
      gutter.textContent = Array.from({ length: lines }, (_, i) => i + 1).join("\n");
      ed.sync();
      renderDecorations(ed);
    };
    ed.sync = () => {
      hl.scrollTop = ta.scrollTop;
      hl.scrollLeft = ta.scrollLeft;
      gutter.scrollTop = ta.scrollTop;
    };
    ta.addEventListener("input", () => { ed.refresh(); updateSuggest(ed); });
    ta.addEventListener("scroll", () => { ed.sync(); if (ed.decode) renderDecorations(ed); });
    ta.addEventListener("keydown", (e) => suggestKeydown(ed, e));
    ta.addEventListener("blur", () => setTimeout(() => hideSuggest(ed), 120));
    EDITORS[id] = ed;
    ed.refresh();
    return ed;
  }

  function setText(id, text) {
    const ta = $(id);
    if (ta) ta.value = text;
    if (EDITORS[id]) EDITORS[id].refresh();
  }

  window.PlaygroundEditor = { enhance, setText, editors: EDITORS, highlightCode, KEYWORD_INFO, setDecode, toggleDecode };
})();
