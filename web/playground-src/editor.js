// ---------------------------------------------------------------------------
// PlaygroundEditor — a CodeMirror 6 wrapper for the playground's query/shape
// editors. Built on the bundled `window.CM` (web/playground-src/cm6.bundle.js).
//
// It mounts a CM6 editor over the original <textarea>, hides the textarea, and
// mirrors the document back into `textarea.value` on every change — so the rest
// of the app keeps reading `$("q").value` unchanged.
//
// Features: SPARQL/Turtle highlighting, keyword/schema/entity autocomplete, and
// an **IRI label decode** mode — true inline atomic chips: each recognised IRI
// gets a low-opacity pill, with a full-opacity human-label chip rendered inline
// to its right (a CM6 widget, not part of the document). Editing the IRI makes
// it stop matching, so the chip reverts to raw text ("autoformatting").
//
// IMPORTANT — it NEVER changes what a query does. Decoration widgets/marks are
// view-only; the query is always `view.state.doc` (mirrored to textarea.value).
//
// API (on window for the baked app.js):
//   PlaygroundEditor.enhance(id, lang, ctx)   // lang: "sparql" | "ttl"
//   PlaygroundEditor.setText(id, text)
//   PlaygroundEditor.insert(id, text)         // insert at the cursor
//   PlaygroundEditor.setDecode(id, on) / toggleDecode(id)
//   PlaygroundEditor.editors                  // { id -> editor }
//
// ctx (all optional): {
//   schema:        () => ({ classes:[[iri,n]], relations:[[s,p,o,n]] }) | null,
//   searchEntities:(prefix) => [{ label, subject }],
//   labelHints:    () => ({ iri: label }) | null,
//   resolveLabels: (iris) => ({ iri: label }) | Promise<{ iri: label }>
// }
// ---------------------------------------------------------------------------
;(function () {
  "use strict";

  const $ = (id) => document.getElementById(id);
  const shorten = (s, n) => { s = String(s); return s.length > n ? s.slice(0, n - 1) + "…" : s; };

  const EDITORS = {};

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
    CONCAT: "Concatenate strings.",
    SUBSTR: "Extract a substring.",
    COALESCE: "First argument that is bound / not an error.",
    LANG: "Language tag of a literal.",
    DATATYPE: "Datatype IRI of a literal.",
    BOUND: "True if a variable is bound.",
    IRI: "Make an IRI from a string.",
    REGEX: "Match a string against a regular expression.",
    EXISTS: "True if a pattern has any solution.",
    NOT: "Negation (NOT EXISTS / NOT IN).",
    AS: "Name a computed value (expr AS ?var).",
    a: "Shorthand for rdf:type."
  };

  const SPARQL_COMPLETIONS = ["SELECT", "CONSTRUCT", "ASK", "DESCRIBE", "WHERE", "PREFIX",
    "FILTER", "OPTIONAL", "UNION", "MINUS", "GRAPH", "BIND", "VALUES", "DISTINCT",
    "ORDER BY", "ORDER BY DESC(", "GROUP BY", "HAVING", "LIMIT", "OFFSET", "FROM NAMED",
    "COUNT", "SUM", "AVG", "MIN", "MAX", "SAMPLE", "GROUP_CONCAT", "STR", "STRLEN", "UCASE",
    "LCASE", "CONTAINS", "STRSTARTS", "STRENDS", "CONCAT", "SUBSTR", "COALESCE", "LANG",
    "DATATYPE", "BOUND", "REGEX", "EXISTS", "NOT EXISTS", "AS"];
  const TTL_COMPLETIONS = ["@prefix", "sh:NodeShape", "sh:PropertyShape", "sh:targetClass",
    "sh:targetSubjectsOf", "sh:targetObjectsOf", "sh:property", "sh:path", "sh:minCount",
    "sh:maxCount", "sh:datatype", "sh:class", "sh:nodeKind", "sh:pattern", "sh:message",
    "sh:severity", "sh:in", "sh:or", "sh:and", "sh:not", "xsd:integer", "xsd:double",
    "xsd:date", "xsd:boolean", "xsd:string"];

  // Built-in labels for ubiquitous vocabulary; per-dataset hints + live lookups
  // come from ctx.labelHints()/ctx.resolveLabels().
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
    "http://schema.org/description": "description",
    "http://schema.org/name": "name",
    "http://schema.org/image": "image",
    "http://www.w3.org/2002/07/owl#Class": "class",
    "http://www.opengis.net/ont/geosparql#asWKT": "geometry (WKT)",
    "http://www.opengis.net/ont/geosparql#hasGeometry": "has geometry",
    "http://purl.org/spar/cito/cites": "cites"
  };

  // prefix -> namespace, from PREFIX/@prefix declarations.
  function prefixMap(text) {
    const map = {};
    const re = /(?:PREFIX|@prefix)\s+([A-Za-z_][\w.-]*)?:\s*<([^>\s]*)>/gi;
    let m;
    while ((m = re.exec(text))) map[m[1] || ""] = m[2];
    return map;
  }
  function tokenIri(tokenText, prefixes) {
    const t = String(tokenText).trim();
    if (!t) return null;
    if (t[0] === "<") return t.replace(/^<|>$/g, "") || null;
    const i = t.indexOf(":");
    if (i < 0) return null;
    const ns = prefixes[t.slice(0, i)];
    return ns != null ? ns + t.slice(i + 1) : null;
  }
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

  // ---- CodeMirror glue (lazy: window.CM exists once the bundle has loaded) ----
  let CM = null;            // window.CM
  let bumpEffect = null;    // a StateEffect to force a decoration rebuild
  let LabelWidget = null;   // the inline label-chip widget class
  let highlightStyle = null;
  let baseTheme = null;

  function ensureCM() {
    if (CM) return true;
    if (!window.CM) return false;
    CM = window.CM;
    bumpEffect = CM.StateEffect.define();
    LabelWidget = class extends CM.WidgetType {
      constructor(label) { super(); this.label = label; }
      eq(o) { return o.label === this.label; }
      toDOM() { const s = document.createElement("span"); s.className = "cm-iri-chip"; s.textContent = this.label; return s; }
      ignoreEvent() { return true; }
    };
    const T = CM.tags;
    highlightStyle = CM.HighlightStyle.define([
      { tag: T.keyword, color: "#8a3d5a", fontWeight: "700" },
      { tag: T.operatorKeyword, color: "#8a3d5a", fontWeight: "700" },
      { tag: T.string, color: "#0b745f" },
      { tag: T.comment, color: "#78877f", fontStyle: "italic" },
      { tag: T.number, color: "#a85424" },
      { tag: T.bool, color: "#a85424" },
      { tag: T.variableName, color: "#a85424" },
      { tag: T.atom, color: "#0b6f5e" },
      { tag: T.operator, color: "#66746e" },
      { tag: T.punctuation, color: "#66746e" },
      { tag: T.propertyName, color: "#0b6f5e" },
      { tag: T.typeName, color: "#0b6f5e" },
      { tag: T.namespace, color: "#0b6f5e" },
      { tag: T.labelName, color: "#0b6f5e" },
      { tag: [T.url, T.literal], color: "#0b6f5e" }
    ]);
    baseTheme = CM.EditorView.theme({
      "&": { fontSize: "13px", border: "1px solid var(--code-border)", borderRadius: "8px",
        background: "var(--code)", color: "var(--ink)", height: "100%" },
      "&.cm-focused": { outline: "none" },
      ".cm-scroller": { fontFamily: "'Cascadia Mono','SF Mono',Consolas,ui-monospace,monospace",
        lineHeight: "1.55", minHeight: "240px" },
      ".cm-content": { padding: "10px 4px", caretColor: "var(--ink)" },
      ".cm-gutters": { background: "rgba(20,125,105,.05)", color: "#8ba197",
        border: "none", borderRight: "1px solid var(--code-border)" },
      ".cm-activeLine": { background: "rgba(20,125,105,.045)" },
      ".cm-activeLineGutter": { background: "rgba(20,125,105,.09)" },
      ".cm-selectionBackground, &.cm-focused .cm-selectionBackground": { background: "rgba(20,125,105,.18)" },
      ".cm-cursor": { borderLeftColor: "var(--ink)" },
      ".cm-tooltip": { border: "1px solid var(--line-strong)", borderRadius: "7px", background: "var(--surface)", boxShadow: "var(--shadow)" },
      ".cm-tooltip-autocomplete > ul": { fontFamily: "'Cascadia Mono','SF Mono',Consolas,ui-monospace,monospace", fontSize: "12.5px", maxHeight: "16em" },
      ".cm-tooltip-autocomplete > ul > li[aria-selected]": { background: "#e3f0ec", color: "var(--accent-dark)" },
      ".cm-completionDetail": { color: "var(--muted)", fontStyle: "normal", marginLeft: "1em" }
    });
    return true;
  }

  function langExtension(lang) {
    return CM.StreamLanguage.define(lang === "ttl" ? CM.turtle : CM.sparql);
  }

  // Build the decode decoration set: a low-opacity mark over each recognised IRI
  // + an inline label-chip widget to its right. Unknown IRIs kick a best-effort
  // (possibly async/remote) lookup, then a rebuild.
  const IRI_RE = /<[^>\s]+>|[A-Za-z_][\w.-]*:[A-Za-z_][\w.-]*/g;
  // Pure (no-CodeMirror) scan: find every IRI/prefixed-name in `text`, skip
  // PREFIX-declaration namespaces, and split into recognised (with a label) vs
  // unknown. Shared by the CM decoration builder and the unit test.
  function scanIris(text, ed) {
    const prefixes = prefixMap(text);
    const namespaces = new Set(Object.keys(prefixes).map((k) => prefixes[k]));
    const known = [], unknown = [];
    let m;
    IRI_RE.lastIndex = 0;
    while ((m = IRI_RE.exec(text))) {
      const from = m.index, to = from + m[0].length;
      const iri = tokenIri(m[0], prefixes);
      if (!iri || namespaces.has(iri)) continue;
      const lab = decodeLabel(ed, iri);
      if (lab) known.push({ from, to, iri, label: lab });
      else if (!ed.labels.has(iri) && !ed.pending.has(iri)) unknown.push(iri);
    }
    return { known, unknown };
  }
  function chipsFor(text, ctx) {
    return scanIris(String(text || ""), { labels: new Map(), pending: new Set(), ctx: ctx || {} });
  }
  function decodeDecorations(view, ed) {
    if (!ed.decode) return CM.Decoration.none;
    const { known, unknown } = scanIris(view.state.doc.toString(), ed);
    const decos = [];
    for (const k of known) {
      decos.push(CM.Decoration.mark({ class: "cm-iri-decoded" }).range(k.from, k.to));
      decos.push(CM.Decoration.widget({ widget: new LabelWidget(k.label), side: 1 }).range(k.to));
    }
    if (unknown.length && ed.ctx.resolveLabels) {
      const want = Array.from(new Set(unknown));
      want.forEach((i) => ed.pending.add(i));
      Promise.resolve(ed.ctx.resolveLabels(want)).then((got) => {
        let added = false;
        want.forEach((i) => { ed.pending.delete(i); const v = (got && got[i]) || null; ed.labels.set(i, v); if (v) added = true; });
        if (added && ed.decode && ed.view) ed.view.dispatch({ effects: bumpEffect.of(null) });
      }).catch(() => { want.forEach((i) => ed.pending.delete(i)); });
    }
    return CM.Decoration.set(decos, true);
  }

  function decodePlugin(ed) {
    return CM.ViewPlugin.fromClass(class {
      constructor(view) { this.decorations = decodeDecorations(view, ed); }
      update(u) {
        if (u.docChanged || u.viewportChanged ||
            u.transactions.some((tr) => tr.effects.some((e) => e.is(bumpEffect))))
          this.decorations = decodeDecorations(u.view, ed);
      }
    }, { decorations: (v) => v.decorations });
  }

  // Keyword + variable + schema + entity completions.
  function completionSource(ed) {
    return (context) => {
      const word = context.matchBefore(/[<A-Za-z0-9_?$:@/#.\-]+/);
      if (!word || (word.from === word.to && !context.explicit)) return null;
      const token = word.text;
      const tl = token.toLowerCase();
      const bare = tl.replace(/^</, "");
      const options = [];
      const seen = new Set();
      const add = (o) => { if (!seen.has(o.label)) { seen.add(o.label); options.push(o); } };
      const base = ed.lang === "sparql" ? SPARQL_COMPLETIONS : TTL_COMPLETIONS;
      base.forEach((text) => {
        if (text.toLowerCase().startsWith(tl) && text.toLowerCase() !== tl)
          add({ label: text, type: "keyword", detail: KEYWORD_INFO[text.toUpperCase()] || "" });
      });
      if (ed.lang === "sparql") {
        const vars = new Set(ed.view.state.doc.toString().match(/[?$][A-Za-z_]\w*/g) || []);
        vars.forEach((v) => { if (v.toLowerCase().startsWith(tl) && v.toLowerCase() !== tl) add({ label: v, type: "variable" }); });
      }
      const schema = ed.ctx.schema && ed.ctx.schema();
      if (schema && bare.length >= 2) {
        (schema.classes || []).slice(0, 60).forEach((c) => {
          const iri = String(c[0]).replace(/^<|>$/g, "");
          if (iri.toLowerCase().includes(bare)) add({ label: "<" + iri + ">", displayLabel: shorten(iri, 46), type: "class", detail: "class" });
        });
        const preds = new Set();
        (schema.relations || []).forEach((r) => preds.add(String(r[1]).replace(/^<|>$/g, "")));
        Array.from(preds).slice(0, 80).forEach((p) => {
          if (p.toLowerCase().includes(bare)) add({ label: "<" + p + ">", displayLabel: shorten(p, 46), type: "property", detail: "predicate" });
        });
      }
      if (ed.ctx.searchEntities && /^[A-Za-z]/.test(token) && token.length >= 2) {
        let hits = [];
        try { hits = ed.ctx.searchEntities(token) || []; } catch (_e) { hits = []; }
        hits.slice(0, 6).forEach((h) => {
          const iri = String(h.subject).replace(/^<|>$/g, "");
          add({ label: "<" + iri + ">", displayLabel: shorten(h.label || iri, 44), type: "namespace", detail: shorten(iri, 38) });
        });
      }
      if (!options.length) return null;
      return { from: word.from, options: options.slice(0, 24), validFor: /^[<A-Za-z0-9_?$:@/#.\-]*$/ };
    };
  }

  function enhance(id, lang, ctx) {
    const ta = $(id);
    if (!ta || EDITORS[id] || !ensureCM()) return;
    const ed = { ta, lang, ctx: ctx || {}, labels: new Map(), pending: new Set(), decode: false, view: null };
    const host = document.createElement("div");
    host.className = "cm-host";
    ta.parentNode.insertBefore(host, ta);
    ta.style.display = "none";
    ta.setAttribute("aria-hidden", "true");
    const extensions = [
      CM.lineNumbers(),
      CM.history(),
      CM.drawSelection(),
      CM.highlightActiveLine(),
      CM.highlightActiveLineGutter(),
      CM.bracketMatching(),
      CM.closeBrackets(),
      CM.indentOnInput(),
      CM.indentUnit.of("  "),
      langExtension(lang),
      CM.syntaxHighlighting(highlightStyle),
      CM.autocompletion({ override: [completionSource(ed)], icons: true, activateOnTyping: true }),
      decodePlugin(ed),
      // Phones: wrap long lines (no sideways scrolling to read a PREFIX) and
      // bump the type to 16px — iOS zooms the whole page when a focused
      // field's text is any smaller. Listed BEFORE baseTheme: for CM6 themes,
      // earlier in the extension array = higher precedence, so this fontSize
      // beats baseTheme's 13px.
      ...(window.matchMedia && window.matchMedia("(max-width: 560px)").matches
        ? [CM.EditorView.lineWrapping, CM.EditorView.theme({ "&": { fontSize: "16px" } })]
        : []),
      baseTheme,
      CM.keymap.of([].concat(CM.closeBracketsKeymap, CM.defaultKeymap, CM.historyKeymap, CM.completionKeymap, [CM.indentWithTab])),
      CM.EditorView.updateListener.of((u) => {
        if (!u.docChanged) return;
        const text = u.state.doc.toString();
        ed.ta.value = text;
        if (ed.ctx.onChange) { try { ed.ctx.onChange(text); } catch (_e) { /* never let a listener break editing */ } }
      })
    ];
    ed.view = new CM.EditorView({ doc: ta.value || "", extensions, parent: host });
    ed.refresh = () => { ed.ta.value = ed.view.state.doc.toString(); };
    EDITORS[id] = ed;
    return ed;
  }

  function setText(id, text) {
    const ed = EDITORS[id];
    if (!ed) { const ta = $(id); if (ta) ta.value = text || ""; return; }
    ed.view.dispatch({ changes: { from: 0, to: ed.view.state.doc.length, insert: text || "" } });
    ed.ta.value = text || "";
  }

  function insert(id, text) {
    const ed = EDITORS[id];
    if (!ed) return false;
    const sel = ed.view.state.selection.main;
    ed.view.dispatch({ changes: { from: sel.from, to: sel.to, insert: text }, selection: { anchor: sel.from + text.length } });
    ed.ta.value = ed.view.state.doc.toString();
    ed.view.focus();
    return true;
  }

  function setDecode(id, on) {
    const ed = EDITORS[id];
    if (!ed) return false;
    ed.decode = !!on;
    ed.view.dispatch({ effects: bumpEffect.of(null) });
    return ed.decode;
  }
  function toggleDecode(id) { const ed = EDITORS[id]; return ed ? setDecode(id, !ed.decode) : false; }

  // Drop every cached label (incl. negatives) and re-resolve — used when the
  // label predicate changes so live lookups run again against the new property.
  function clearLabels(id) {
    const ed = EDITORS[id];
    if (!ed) return;
    ed.labels.clear();
    ed.pending.clear();
    if (ed.decode) renderDecorations(ed);
  }

  window.PlaygroundEditor = { enhance, setText, insert, setDecode, toggleDecode, clearLabels, editors: EDITORS, KEYWORD_INFO, __chips: chipsFor };
})();
