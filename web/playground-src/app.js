(function () {
  "use strict";

  const CATALOG = window.RETE_PLAYGROUND_CATALOG;
  const state = {
    bytes: null,
    dataset: CATALOG.defaultDataset,
    mode: "sparql",
    family: "All",
    selectedExample: -1,
    activeSource: "bundled",
    schema: null,
    lastProgressive: null,
    lastProvenance: null,
    built: null
  };

  const BUILD_SAMPLE = `# Paste N-Triples here (or open a file), pick the format, then Build.
<http://ex/Alice> <http://ex/knows> <http://ex/Bob> .
<http://ex/Bob> <http://ex/knows> <http://ex/Carol> .
<http://ex/Alice> <http://ex/age> "30"^^<http://www.w3.org/2001/XMLSchema#integer> .
<http://ex/Carol> <http://ex/worksAt> <http://ex/AcmeLabs> .
`;

  const $ = (id) => document.getElementById(id);
  const $$ = (sel, root = document) => Array.from(root.querySelectorAll(sel));
  const W = () => wasm_bindgen;

  function esc(value) {
    return String(value == null ? "" : value)
      .replace(/&/g, "&amp;")
      .replace(/</g, "&lt;")
      .replace(/>/g, "&gt;")
      .replace(/"/g, "&quot;")
      .replace(/'/g, "&#39;");
  }

  function shorten(value, max = 82) {
    const s = String(value == null ? "" : value);
    if (s.length <= max) return s;
    const iri = s.match(/^<(.+)>$/);
    if (iri) {
      const body = iri[1];
      const cut = Math.max(body.lastIndexOf("/"), body.lastIndexOf("#"));
      if (cut >= 0 && body.length - cut < max - 8) return "<..." + body.slice(cut) + ">";
    }
    return s.slice(0, Math.max(0, max - 3)) + "...";
  }

  function formatBytes(n) {
    const v = Number(n || 0);
    if (v < 1024) return v + " B";
    if (v < 1024 * 1024) return (v / 1024).toFixed(1) + " KB";
    return (v / 1024 / 1024).toFixed(1) + " MB";
  }

  function b64ToBytes(b64) {
    const bin = atob(b64);
    const out = new Uint8Array(bin.length);
    for (let i = 0; i < bin.length; i++) out[i] = bin.charCodeAt(i);
    return out;
  }

  function setStatus(text) {
    $("meta").textContent = text;
  }

  function sourceLabel() {
    if (state.activeSource === "file") return "local file";
    if (state.activeSource === "url") return "url";
    if (state.activeSource === "built") return "built in browser";
    return "bundled";
  }

  function updateSourcePill() {
    $("sourcePill").textContent = sourceLabel();
  }

  function datasetInfo(key) {
    return CATALOG.datasets.find((d) => d.key === key) || CATALOG.datasets[0];
  }

  // --- Code editors: gutter + syntax highlight overlay + autocomplete -----
  // A textarea is wrapped with a line-number gutter and a <pre> overlay that
  // renders the same text highlighted (the textarea's own text is transparent,
  // only its caret/selection show). Same token theme as the documentation.
  const EDITORS = {};

  const ED_TOKENS = (() => {
    const COM = /#.*/y;
    const STR = /"(?:\\.|[^"\\])*"|'(?:\\.|[^'\\])*'/y;
    const IRI = /<[^>\s]*>?/y;
    const VAR = /[?$][A-Za-z_]\w*/y;
    const NUM = /\b\d[\d_]*(?:\.\d+)?\b/y;
    const WS = /\s+/y;
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

  function highlightCode(text, lang) {
    const rules = ED_TOKENS[lang] || ED_TOKENS.ttl;
    let out = "";
    let i = 0;
    const n = text.length;
    const WS = /\s+/y;
    outer: while (i < n) {
      WS.lastIndex = i;
      const w = WS.exec(text);
      if (w && w.index === i) {
        out += w[0];
        i += w[0].length;
        continue;
      }
      for (const [re, cls] of rules) {
        re.lastIndex = i;
        const m = re.exec(text);
        if (m && m.index === i && m[0].length) {
          out += cls ? `<span class="tok-${cls}">${esc(m[0])}</span>` : esc(m[0]);
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

  function completionItems(ed) {
    const items = [];
    const base = ed.lang === "sparql" ? SPARQL_COMPLETIONS : TTL_COMPLETIONS;
    base.forEach((text) => items.push({ text, kind: "kw" }));
    if (ed.lang === "sparql") {
      // Variables already used in this query.
      const vars = new Set(ed.ta.value.match(/[?$][A-Za-z_]\w*/g) || []);
      vars.forEach((v) => items.push({ text: v, kind: "var" }));
    }
    // Terms from the loaded graph: classes and predicates from the schema view.
    if (state.schema) {
      (state.schema.classes || []).slice(0, 40).forEach((c) =>
        items.push({ text: String(c[0]), kind: "class" }));
      const preds = new Set();
      (state.schema.relations || []).forEach((r) => preds.add(String(r[1])));
      Array.from(preds).slice(0, 60).forEach((p) => items.push({ text: p, kind: "pred" }));
    }
    return items;
  }

  function currentToken(ta) {
    const pos = ta.selectionStart;
    const head = ta.value.slice(0, pos);
    const m = head.match(/[<A-Za-z0-9_?$:@\/#.-]+$/);
    return m ? { token: m[0], start: pos - m[0].length, end: pos } : null;
  }

  function matchCompletions(ed, token) {
    const t = token.toLowerCase();
    const bare = t.replace(/^</, "");
    const seen = new Set();
    const out = [];
    for (const item of completionItems(ed)) {
      const lower = item.text.toLowerCase();
      if (seen.has(lower)) continue;
      const isIri = lower.startsWith("<");
      const hit = isIri
        ? bare.length >= 2 && lower.includes(bare)
        : lower.startsWith(t) && lower !== t;
      if (hit) {
        seen.add(lower);
        out.push(item);
        if (out.length >= 8) break;
      }
    }
    return out;
  }

  function caretOffset(ed) {
    // Mirror the text up to the caret in a hidden element with the same text
    // metrics, then read the marker's position.
    const ta = ed.ta;
    const probe = document.createElement("div");
    probe.style.cssText =
      "position:absolute;visibility:hidden;white-space:pre;padding:12px;" +
      "font:13px/1.46 'Cascadia Mono','SF Mono',Consolas,ui-monospace,monospace;";
    probe.textContent = ta.value.slice(0, ta.selectionStart);
    const mark = document.createElement("span");
    mark.textContent = "\u200b";
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
    const items = matchCompletions(ed, tok.token);
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
      `<div class="sg ${i === ed.sel ? "active" : ""}" data-sg="${i}">` +
        `<span>${esc(shorten(item.text, 46))}</span>` +
        `<span class="sg-kind">${esc(item.kind)}</span>` +
      `</div>`
    ).join("");
    $$("[data-sg]", ed.sug).forEach((el) => {
      el.onmousedown = (e) => {
        e.preventDefault();
        acceptSuggest(ed, Number(el.dataset.sg));
      };
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

  function hideSuggest(ed) {
    ed.sug.classList.add("hidden");
    ed.items = [];
  }

  function suggestKeydown(ed, e) {
    if (ed.sug.classList.contains("hidden")) return;
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

  function enhanceEditor(id, lang) {
    const ta = $(id);
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

    ta.parentNode.insertBefore(wrap, ta);
    wrap.appendChild(gutter);
    wrap.appendChild(body);
    body.appendChild(hl);
    body.appendChild(ta);
    body.appendChild(sug);
    ta.setAttribute("wrap", "off");
    ta.spellcheck = false;

    const ed = { ta, gutter, code, hl, sug, body, lang, items: [], sel: 0, tok: null };
    ed.refresh = () => {
      const text = ta.value;
      code.innerHTML = highlightCode(text, lang);
      const lines = text.split("\n").length;
      gutter.textContent = Array.from({ length: lines }, (_, i) => i + 1).join("\n");
      ed.sync();
    };
    ed.sync = () => {
      hl.scrollTop = ta.scrollTop;
      hl.scrollLeft = ta.scrollLeft;
      gutter.scrollTop = ta.scrollTop;
    };
    ta.addEventListener("input", () => {
      ed.refresh();
      updateSuggest(ed);
    });
    ta.addEventListener("scroll", ed.sync);
    ta.addEventListener("keydown", (e) => suggestKeydown(ed, e));
    ta.addEventListener("blur", () => setTimeout(() => hideSuggest(ed), 120));
    EDITORS[id] = ed;
    ed.refresh();
  }

  function setEd(id, text) {
    $(id).value = text;
    if (EDITORS[id]) EDITORS[id].refresh();
  }

  function renderDatasetOptions() {
    $("ds").innerHTML = CATALOG.datasets.map((d) =>
      `<option value="${esc(d.key)}">${esc(d.label)}</option>`
    ).join("");
    $("ds").value = state.dataset;
  }

  function loadBytes(bytes, source) {
    state.bytes = bytes;
    state.activeSource = source;
    updateSourcePill();

    const info = JSON.parse(W().info(bytes));
    const graphNames = JSON.parse(W().graph_names(bytes));
    const graphText = graphNames.length ? " | graphs " + graphNames.length : "";
    setStatus(`${info.quads} quads | ${info.terms} terms | ${info.pyramidLevels} pyramid levels${graphText}`);

    const schema = JSON.parse(W().schema(bytes));
    state.schema = schema;
    renderSchema(schema);
    renderProgressiveInfo(null);
    renderProvenanceSummary(null);
    renderReachDefaults();
    renderShaclExamples();
    renderProvenanceDefaults();

    const infoRow = datasetInfo(state.dataset);
    $("dsDesc").textContent = source === "bundled"
      ? infoRow.description
      : "Custom graph loaded into the same in-browser engine.";
  }

  function loadDataset(key) {
    const b64 = RETE_DATASETS_B64[key];
    if (!b64) {
      setStatus("dataset not embedded: " + key);
      return;
    }
    state.dataset = key;
    $("ds").value = key;
    loadBytes(b64ToBytes(b64), "bundled");
    renderExamples();
    const list = examplesForDataset();
    if (list.length) selectExample(0);
    updateHash();
  }

  async function loadFromUrl() {
    const url = $("urlInput").value.trim();
    if (!url) return;
    setStatus("loading url...");
    try {
      const res = await fetch(url);
      if (!res.ok) throw new Error(res.status + " " + res.statusText);
      const buf = new Uint8Array(await res.arrayBuffer());
      loadBytes(buf, "url");
      state.dataset = $("ds").value;
    } catch (e) {
      showError("out", "URL load failed: " + e.message);
    }
  }

  async function loadFromFile(file) {
    if (!file) return;
    try {
      const buf = new Uint8Array(await file.arrayBuffer());
      loadBytes(buf, "file");
      setStatus(`${file.name} | ${formatBytes(buf.byteLength)} | custom file`);
    } catch (e) {
      showError("out", "File load failed: " + e.message);
    }
  }

  function examplesForDataset() {
    return CATALOG.examples[state.dataset] || [];
  }

  function filteredExamples() {
    const q = $("exampleSearch").value.trim().toLowerCase();
    return examplesForDataset()
      .map((ex, index) => ({ ex, index }))
      .filter(({ ex }) => state.family === "All" || ex.family === state.family)
      .filter(({ ex }) => {
        if (!q) return true;
        return [ex.label, ex.family, ex.tip, ex.q].join(" ").toLowerCase().includes(q);
      });
  }

  function renderFamilyFilters() {
    const families = ["All"].concat(CATALOG.families);
    $("familyFilters").innerHTML = families.map((family) =>
      `<button type="button" data-family="${esc(family)}" class="${family === state.family ? "active" : ""}">${esc(family)}</button>`
    ).join("");
    $$("#familyFilters button").forEach((btn) => {
      btn.onclick = () => {
        state.family = btn.dataset.family;
        renderExamples();
      };
    });
  }

  function renderExamples() {
    renderFamilyFilters();
    const items = filteredExamples();
    if (!items.length) {
      $("examples").innerHTML = `<p class="microcopy">No matching examples for this dataset.</p>`;
      return;
    }
    $("examples").innerHTML = items.map(({ ex, index }) =>
      `<article class="example-card" data-family="${esc(ex.family)}">` +
        `<button type="button" class="example-button ${index === state.selectedExample ? "active" : ""}" data-example="${index}">` +
          `<span>${esc(ex.label)}</span>` +
        `</button>` +
        `<div class="tagline">${esc(ex.family)} | ${esc(ex.tip)}</div>` +
      `</article>`
    ).join("");
    $$("#examples [data-example]").forEach((btn) => {
      btn.onclick = () => selectExample(Number(btn.dataset.example));
    });
  }

  function selectExample(index) {
    const ex = examplesForDataset()[index];
    if (!ex) return;
    state.selectedExample = index;
    setEd("q", ex.q);
    setView(ex.view || "table");
    setStrategy(ex.strategy || "whole");
    setMode("sparql");
    $("exampleInfo").innerHTML =
      `<div><strong>${esc(ex.label)}</strong></div>` +
      `<div>${esc(ex.family)}</div>` +
      `<div>${esc(ex.tip)}</div>`;
    renderExamples();
    updateHash();
  }

  function setMode(mode) {
    state.mode = mode;
    $$("#modeTabs button").forEach((btn) => btn.classList.toggle("active", btn.dataset.mode === mode));
    $$(".panel").forEach((panel) => panel.classList.toggle("active", panel.dataset.panel === mode));
    updateResultVisibility();
    updateHash();
  }

  function updateResultVisibility() {
    $$(".result-pane").forEach((pane) => pane.classList.add("hidden"));
    if (state.mode === "sparql") {
      $("out").classList.remove("hidden");
      if ($("commOut").innerHTML.trim()) $("commOut").classList.remove("hidden");
    } else if (state.mode === "shacl") {
      $("shaclOut").classList.remove("hidden");
    } else if (state.mode === "reach") {
      $("reachOut").classList.remove("hidden");
    } else if (state.mode === "schema") {
      $("schemaOut").classList.remove("hidden");
    } else if (state.mode === "provenance") {
      $("provOut").classList.remove("hidden");
    } else if (state.mode === "build") {
      $("buildOut").classList.remove("hidden");
    }
  }

  function setView(view) {
    $("fmt").value = view;
    $$("#viewSeg button").forEach((btn) => btn.classList.toggle("active", btn.dataset.view === view));
  }

  function setStrategy(strategy) {
    $("strategy").value = strategy || "whole";
    $("roundWrap").classList.toggle("hidden", $("strategy").value !== "community");
  }

  function renderTable(vars, rows) {
    const cap = 500;
    const shown = (rows || []).slice(0, cap);
    const head = (vars || []).map((v) => `<th>${esc(v)}</th>`).join("");
    const body = shown.map((row) =>
      `<tr>${(vars || []).map((v) => `<td class="iri">${esc(shorten(row[v], 120))}</td>`).join("")}</tr>`
    ).join("");
    const more = (rows || []).length > cap
      ? `<p class="microcopy">Showing first ${cap} of ${rows.length} rows.</p>`
      : "";
    return more + `<table><thead><tr>${head}</tr></thead><tbody>${body}</tbody></table>`;
  }

  function renderTriplesTable(triples) {
    const cap = 500;
    const shown = (triples || []).slice(0, cap);
    const body = shown.map((t) =>
      `<tr><td class="iri">${esc(shorten(t[0], 120))}</td><td class="iri">${esc(shorten(t[1], 120))}</td><td class="iri">${esc(shorten(t[2], 120))}</td></tr>`
    ).join("");
    const more = (triples || []).length > cap
      ? `<p class="microcopy">Showing first ${cap} of ${triples.length} triples.</p>`
      : "";
    return more + `<table><thead><tr><th>subject</th><th>predicate</th><th>object</th></tr></thead><tbody>${body}</tbody></table>`;
  }

  function triplesForGraph(res) {
    if (res.triples) return res.triples;
    if (res.kind !== "select") return [];
    const vars = res.vars || [];
    if (vars.length >= 3) return res.rows.map((r) => [r[vars[0]], r[vars[1]], r[vars[2]]]);
    if (vars.length === 2) return res.rows.map((r) => [r[vars[0]], "related", r[vars[1]]]);
    return [];
  }

  function renderGraph(triples) {
    const out = $("out");
    if (!triples || !triples.length) {
      out.innerHTML = `<div class="note">Graph view needs triples. Use a CONSTRUCT query or a SELECT with at least two columns.</div>`;
      return "graph: 0 edges";
    }

    const cap = 90;
    const nodeMap = new Map();
    const nodes = [];
    const edges = [];
    const addNode = (term) => {
      if (!nodeMap.has(term) && nodes.length < cap) {
        const i = nodes.length;
        const angle = i * 2.399963;
        const radius = 34 + 7 * Math.sqrt(i);
        nodeMap.set(term, i);
        nodes.push({
          term,
          label: shorten(term, 28),
          x: 460 + Math.cos(angle) * radius,
          y: 260 + Math.sin(angle) * radius
        });
      }
      return nodeMap.get(term);
    };

    triples.forEach((t) => {
      const s = addNode(String(t[0]));
      const o = addNode(String(t[2]));
      if (s != null && o != null) edges.push({ s, o, p: String(t[1]) });
    });

    for (let iter = 0; iter < 110; iter++) {
      for (let i = 0; i < nodes.length; i++) {
        for (let j = i + 1; j < nodes.length; j++) {
          const a = nodes[i], b = nodes[j];
          const dx = a.x - b.x || 0.01;
          const dy = a.y - b.y || 0.01;
          const d2 = dx * dx + dy * dy;
          const f = Math.min(260 / d2, 0.035);
          a.x += dx * f; a.y += dy * f;
          b.x -= dx * f; b.y -= dy * f;
        }
      }
      edges.forEach((e) => {
        const a = nodes[e.s], b = nodes[e.o];
        const dx = b.x - a.x, dy = b.y - a.y;
        a.x += dx * 0.012; a.y += dy * 0.012;
        b.x -= dx * 0.012; b.y -= dy * 0.012;
      });
      nodes.forEach((n) => {
        n.x += (460 - n.x) * 0.01;
        n.y += (260 - n.y) * 0.01;
        n.x = Math.max(28, Math.min(892, n.x));
        n.y = Math.max(28, Math.min(492, n.y));
      });
    }

    let svg = `<svg viewBox="0 0 920 520" role="img" aria-label="Graph result">`;
    svg += `<defs><marker id="arrow" markerWidth="7" markerHeight="7" refX="6" refY="3.5" orient="auto"><path d="M0,0 L7,3.5 L0,7 z" fill="#99a991"></path></marker></defs>`;
    edges.forEach((e, i) => {
      const a = nodes[e.s], b = nodes[e.o];
      svg += `<line class="gedge" data-s="${e.s}" data-t="${e.o}" x1="${a.x.toFixed(1)}" y1="${a.y.toFixed(1)}" x2="${b.x.toFixed(1)}" y2="${b.y.toFixed(1)}" marker-end="url(#arrow)"></line>`;
      if (i < 70) {
        svg += `<text class="gedge-label" data-s="${e.s}" data-t="${e.o}" x="${((a.x + b.x) / 2).toFixed(1)}" y="${((a.y + b.y) / 2).toFixed(1)}">${esc(shorten(e.p, 22))}</text>`;
      }
    });
    nodes.forEach((n, i) => {
      svg += `<g class="gnodeg" data-i="${i}"><circle class="gnode" cx="${n.x.toFixed(1)}" cy="${n.y.toFixed(1)}" r="7"><title>${esc(n.term)}</title></circle><text class="gnode-label" x="${(n.x + 10).toFixed(1)}" y="${(n.y + 4).toFixed(1)}">${esc(n.label)}</text></g>`;
    });
    svg += `</svg>`;

    const truncated = nodeMap.size >= cap;
    out.innerHTML = `<p class="microcopy">${nodes.length} nodes | ${edges.length} edges | drag nodes to adjust layout.</p>` +
      (truncated ? `<div class="note">Graph capped at ${cap} nodes for legibility.</div>` : "") +
      `<div class="graphwrap">${svg}</div>`;
    enableGraphDrag(out.querySelector("svg"), nodes);
    return `graph: ${nodes.length} nodes, ${edges.length} edges`;
  }

  function enableGraphDrag(svg, nodes) {
    if (!svg) return;
    let dragging = null;
    const point = (ev) => {
      const rect = svg.getBoundingClientRect();
      return {
        x: (ev.clientX - rect.left) / rect.width * 920,
        y: (ev.clientY - rect.top) / rect.height * 520
      };
    };
    $$(".gnodeg", svg).forEach((g) => {
      g.addEventListener("mousedown", (ev) => {
        dragging = Number(g.dataset.i);
        svg.classList.add("grabbing");
        ev.preventDefault();
      });
    });
    svg.addEventListener("mousemove", (ev) => {
      if (dragging == null) return;
      const p = point(ev);
      const n = nodes[dragging];
      n.x = Math.max(28, Math.min(892, p.x));
      n.y = Math.max(28, Math.min(492, p.y));
      const g = svg.querySelector(`.gnodeg[data-i="${dragging}"]`);
      g.querySelector("circle").setAttribute("cx", n.x.toFixed(1));
      g.querySelector("circle").setAttribute("cy", n.y.toFixed(1));
      g.querySelector("text").setAttribute("x", (n.x + 10).toFixed(1));
      g.querySelector("text").setAttribute("y", (n.y + 4).toFixed(1));
      $$(`line.gedge[data-s="${dragging}"], line.gedge[data-t="${dragging}"]`, svg).forEach((line) => {
        const a = nodes[Number(line.dataset.s)], b = nodes[Number(line.dataset.t)];
        line.setAttribute("x1", a.x.toFixed(1));
        line.setAttribute("y1", a.y.toFixed(1));
        line.setAttribute("x2", b.x.toFixed(1));
        line.setAttribute("y2", b.y.toFixed(1));
      });
      $$(`text.gedge-label[data-s="${dragging}"], text.gedge-label[data-t="${dragging}"]`, svg).forEach((txt) => {
        const a = nodes[Number(txt.dataset.s)], b = nodes[Number(txt.dataset.t)];
        txt.setAttribute("x", ((a.x + b.x) / 2).toFixed(1));
        txt.setAttribute("y", ((a.y + b.y) / 2).toFixed(1));
      });
    });
    const end = () => {
      dragging = null;
      svg.classList.remove("grabbing");
    };
    svg.addEventListener("mouseup", end);
    svg.addEventListener("mouseleave", end);
  }

  function renderProgressiveInfo(meta) {
    state.lastProgressive = meta;
    if (!meta) {
      $("progressiveInfo").innerHTML = `<div>Run a Summary-family example with the progressive strategy.</div>`;
      return;
    }
    $("progressiveInfo").innerHTML =
      `<div class="metric-grid">` +
      metric("Exact", meta.exact ? "yes" : "no") +
      metric("Index skipped", meta.readsIndex ? "no" : "yes") +
      metric("Bytes", formatBytes(meta.bytes)) +
      metric("Range reads", String(meta.requests || 0)) +
      `</div>` +
      `<div>Shape: <code>${esc(meta.queryShape || "summary")}</code></div>` +
      (meta.predicate ? `<div>Predicate: <span class="iri">${esc(shorten(meta.predicate))}</span></div>` : "");
  }

  function metric(label, value) {
    return `<div class="metric"><strong>${esc(value)}</strong><span>${esc(label)}</span></div>`;
  }

  function progressiveBanner(meta) {
    if (!meta) return "";
    return `<div class="meta-strip">` +
      `<span class="meta-chip"><strong>exact</strong> ${meta.exact ? "yes" : "no"}</span>` +
      `<span class="meta-chip"><strong>index</strong> ${meta.readsIndex ? "read" : "skipped"}</span>` +
      `<span class="meta-chip"><strong>bytes</strong> ${formatBytes(meta.bytes)}</span>` +
      `<span class="meta-chip"><strong>ranges</strong> ${esc(meta.requests || 0)}</span>` +
      `</div>`;
  }

  function renderResult(res, fmt) {
    const progressive = res.progressive || null;
    renderProgressiveInfo(progressive);

    if (fmt === "graph") {
      let triples = triplesForGraph(res);
      if (!triples.length && res.kind === "construct" && res.format) {
        const rerun = JSON.parse(W().query(state.bytes, $("q").value, "table"));
        triples = triplesForGraph(rerun);
      }
      return renderGraph(triples);
    }

    if (res.kind === "ask") {
      $("out").innerHTML = progressiveBanner(progressive) +
        `<div class="banner">ASK result: <strong>${esc(res.boolean)}</strong></div>`;
      return `ASK ${res.boolean}`;
    }

    if (res.kind === "select") {
      $("out").innerHTML = progressiveBanner(progressive) + renderTable(res.vars || [], res.rows || []);
      return `${(res.rows || []).length} row(s)`;
    }

    if (res.format === "ttl" || res.format === "jsonld") {
      $("out").innerHTML = `<pre>${esc(res.text || "")}</pre>`;
      return `CONSTRUCT ${res.format}`;
    }

    $("out").innerHTML = renderTriplesTable(res.triples || []);
    return `${(res.triples || []).length} triple(s)`;
  }

  function runQuery() {
    if (!state.bytes) return showError("out", "Load a graph first.");
    const q = $("q").value.trim();
    if (!q) return showError("out", "Enter a SPARQL query.");
    const fmt = $("fmt").value;
    const strategy = $("strategy").value;
    const queryFmt = strategy === "progressive" || fmt === "graph" ? "table" : fmt;
    $("commOut").innerHTML = "";
    updateResultVisibility();

    const t0 = performance.now();
    try {
      const raw = strategy === "progressive"
        ? W().progressive_query(state.bytes, q)
        : W().query(state.bytes, q, queryFmt);
      const res = JSON.parse(raw);
      const summary = renderResult(res, strategy === "progressive" && fmt === "graph" ? "table" : fmt);
      const dt = performance.now() - t0;
      $("qmeta").textContent = `${summary} | ${dt.toFixed(1)} ms`;
      if (strategy === "community") runCommunity();
      saveHistory({ query: q, format: fmt, strategy, dataset: state.dataset, ts: Date.now(), resultSummary: summary });
      updateHash();
    } catch (e) {
      $("qmeta").textContent = "";
      showError("out", String(e));
      renderProgressiveInfo(null);
    }
  }

  function runCommunity() {
    const roundText = $("round").value.trim();
    const round = roundText === "" ? undefined : Number(roundText);
    const t0 = performance.now();
    try {
      const rows = JSON.parse(W().communities(state.bytes, round));
      const dt = performance.now() - t0;
      $("commOut").innerHTML =
        `<div class="note">Community split is shown as decomposition metadata in this single-threaded static build.</div>` +
        `<p class="microcopy">${rows.length} communities | ${dt.toFixed(1)} ms</p>` +
        `<table><thead><tr><th>community</th><th>members</th><th>triples</th></tr></thead><tbody>` +
        rows.map((r) => `<tr><td>C${r.community}</td><td>${r.size}</td><td>${r.triples}</td></tr>`).join("") +
        `</tbody></table>`;
      updateResultVisibility();
    } catch (e) {
      showError("commOut", "community error: " + e.message);
    }
  }

  function renderShaclExamples() {
    const list = CATALOG.shacl[state.dataset] || [];
    if (!list.length) {
      $("shaclExamples").innerHTML = `<p class="microcopy">No SHACL examples for this dataset.</p>`;
      setEd("shapeText", "");
      return;
    }
    $("shaclExamples").innerHTML = list.map((ex, i) =>
      `<article class="example-card"><button type="button" class="example-button" data-shacl="${i}">${esc(ex.label)}</button><div class="tagline">${esc(ex.tip)}</div></article>`
    ).join("");
    $$("#shaclExamples [data-shacl]").forEach((btn) => {
      btn.onclick = () => {
        const ex = list[Number(btn.dataset.shacl)];
        setEd("shapeText", ex.shape);
        $("exampleInfo").innerHTML = `<strong>${esc(ex.label)}</strong><div>${esc(ex.tip)}</div>`;
        setMode("shacl");
      };
    });
    setEd("shapeText", list[0].shape);
  }

  function renderShaclJson(report, raw) {
    if (report.conforms) {
      return `<div class="banner">Conforms. No validation results.</div><pre>${esc(raw)}</pre>`;
    }
    const rows = (report.results || []).slice(0, 250).map((r) =>
      `<tr><td class="iri">${esc(shorten(r.focusNode || ""))}</td><td class="iri">${esc(shorten(r.resultPath || ""))}</td><td>${esc(shorten(r.sourceConstraintComponent || ""))}</td><td>${esc(shorten((r.messages || []).join(" "), 120))}</td></tr>`
    ).join("");
    return `<div class="note">Does not conform: ${(report.results || []).length} validation result(s).</div>` +
      `<table><thead><tr><th>focus</th><th>path</th><th>component</th><th>message</th></tr></thead><tbody>${rows}</tbody></table>`;
  }

  function runShacl() {
    if (!state.bytes) return showError("shaclOut", "Load a graph first.");
    const shapes = $("shapeText").value.trim();
    if (!shapes) return showError("shaclOut", "Enter a SHACL shape.");
    const fmt = $("shaclFormat").value;
    const t0 = performance.now();
    try {
      const text = W().shacl(state.bytes, shapes, null, fmt);
      const dt = performance.now() - t0;
      if (fmt === "json") {
        const report = JSON.parse(text);
        $("shaclOut").innerHTML = renderShaclJson(report, text);
        $("shaclMeta").textContent = `${report.conforms ? "conforms" : "violations"} | ${dt.toFixed(1)} ms`;
      } else {
        $("shaclOut").innerHTML = `<pre>${esc(text)}</pre>`;
        $("shaclMeta").textContent = `${text.startsWith("conforms: true") ? "conforms" : "report"} | ${dt.toFixed(1)} ms`;
      }
      updateResultVisibility();
    } catch (e) {
      $("shaclMeta").textContent = "";
      showError("shaclOut", String(e));
    }
  }

  function renderReachDefaults() {
    const cfg = CATALOG.reach[state.dataset] || {};
    $("reachPred").value = cfg.pred || "";
    $("reachSeeds").value = cfg.seeds || "";
    $("reachReverse").checked = false;
    const list = cfg.examples || [];
    $("reachExamples").innerHTML = list.map((ex, i) =>
      `<article class="example-card"><button type="button" class="example-button" data-reach="${i}">${esc(ex.label)}</button><div class="tagline">${esc(ex.pred)} | ${ex.reverse ? "reverse" : "forward"}</div></article>`
    ).join("");
    $$("#reachExamples [data-reach]").forEach((btn) => {
      btn.onclick = () => {
        const ex = list[Number(btn.dataset.reach)];
        $("reachPred").value = ex.pred;
        $("reachSeeds").value = ex.seeds;
        $("reachReverse").checked = !!ex.reverse;
        $("exampleInfo").innerHTML = `<strong>${esc(ex.label)}</strong><div>${esc(ex.pred)}</div>`;
        setMode("reach");
      };
    });
  }

  function runReach() {
    if (!state.bytes) return showError("reachOut", "Load a graph first.");
    const pred = $("reachPred").value.trim();
    const seeds = $("reachSeeds").value.split(",").map((s) => s.trim()).filter(Boolean);
    if (!pred || !seeds.length) return showError("reachOut", "Enter a predicate and at least one seed.");
    const reverse = $("reachReverse").checked;
    const t0 = performance.now();
    try {
      const results = JSON.parse(W().reach(state.bytes, pred, JSON.stringify(seeds), reverse));
      const dt = performance.now() - t0;
      const rows = results.map((r) => {
        if (r.error) return `<tr><td class="iri">${esc(shorten(r.seed))}</td><td colspan="2">${esc(r.error)}</td></tr>`;
        const shown = (r.reached || []).slice(0, 250).map((x) => `<div class="iri">${esc(shorten(x, 90))}</div>`).join("");
        const more = r.count > 250 ? `<div class="microcopy">Showing first 250 of ${r.count}.</div>` : "";
        return `<tr><td class="iri">${esc(shorten(r.seed))}</td><td>${r.count}</td><td>${shown}${more}</td></tr>`;
      }).join("");
      $("reachMeta").textContent = `${results.length} seed(s) | ${reverse ? "reverse" : "forward"} | ${dt.toFixed(1)} ms`;
      $("reachOut").innerHTML = `<table><thead><tr><th>seed</th><th>count</th><th>reached</th></tr></thead><tbody>${rows}</tbody></table>`;
      updateResultVisibility();
    } catch (e) {
      $("reachMeta").textContent = "";
      showError("reachOut", String(e));
    }
  }

  function renderSchema(schema) {
    const classes = schema.classes || [];
    const relations = schema.relations || [];
    $("schemaSummary").innerHTML =
      `<div class="metric-grid">${metric("classes", classes.length)}${metric("relations", relations.length)}</div>` +
      `<div>${classes.slice(0, 5).map((c) => `<span class="chip">${esc(shorten(c[0], 38))} (${esc(c[1])})</span>`).join(" ")}</div>`;
    $("classes").innerHTML = `<div class="chip-list">` + classes.slice(0, 80)
      .map((c) => `<span class="chip">${esc(shorten(c[0], 50))} <strong>${esc(c[1])}</strong></span>`)
      .join("") + `</div>`;
    $("relations").innerHTML = renderTable(["subjectClass", "predicate", "objectClass", "count"],
      relations.slice(0, 120).map((r) => ({
        subjectClass: r[0],
        predicate: r[1],
        objectClass: r[2],
        count: String(r[3])
      })));
    $("ontologyDiagram").innerHTML = renderOntologyDiagram(classes, relations);
    $("schemaOut").innerHTML = `<div class="banner">${classes.length} classes and ${relations.length} class-level relations.</div>`;
  }

  function localName(term) {
    const m = String(term).match(/[\/#]([^\/#>]+)>?$/);
    return m ? m[1] : String(term).replace(/[<>]/g, "");
  }

  // UML-style schema: each class is a box whose rows are its datatype
  // properties (relations whose object class is "(literal)"); object
  // properties between shown classes are drawn as labelled edges.
  function renderOntologyDiagram(classes, relations) {
    if (!classes.length) return `<div class="note">No rdf:type-derived classes found.</div>`;
    const top = classes.slice(0, 8);
    const idx = new Map(top.map((c, i) => [c[0], i]));

    // Per-class datatype properties (top 5 by count) + object edges.
    const attrs = top.map(() => []);
    const edgeMap = new Map(); // "s>t" -> {s, t, preds: Map(pred -> count)}
    relations.forEach((r) => {
      const [sc, p, oc, n] = r;
      if (!idx.has(sc)) return;
      if (oc === "(literal)") {
        attrs[idx.get(sc)].push([p, Number(n)]);
      } else if (idx.has(oc)) {
        const s = idx.get(sc), t = idx.get(oc);
        const key = s + ">" + t;
        if (!edgeMap.has(key)) edgeMap.set(key, { s, t, preds: new Map() });
        const e = edgeMap.get(key);
        e.preds.set(p, (e.preds.get(p) || 0) + Number(n));
      }
    });
    attrs.forEach((list) => list.sort((a, b) => b[1] - a[1]).splice(5));

    // Grid layout: up to 4 columns; row height grows with the tallest box.
    const cols = Math.min(4, top.length);
    const boxW = 196;
    const gapX = 36;
    const gapY = 46;
    const headH = 24;
    const rowH = 13;
    const width = 24 + cols * boxW + (cols - 1) * gapX + 24;
    const boxH = (i) => headH + 7 + attrs[i].length * rowH + (attrs[i].length ? 5 : 0);
    const boxes = top.map((c, i) => {
      const col = i % cols;
      const row = Math.floor(i / cols);
      return { iri: c[0], count: c[1], i, col, row, w: boxW, h: boxH(i) };
    });
    const rowHeights = [];
    boxes.forEach((b) => {
      rowHeights[b.row] = Math.max(rowHeights[b.row] || 0, b.h);
    });
    let y = 18;
    const rowY = rowHeights.map((h) => {
      const at = y;
      y += h + gapY;
      return at;
    });
    boxes.forEach((b) => {
      b.x = 24 + b.col * (boxW + gapX);
      b.y = rowY[b.row];
    });
    const height = y - gapY + 18;

    const anchor = (b) => ({ x: b.x + b.w / 2, y: b.y + b.h / 2 });
    let svg = `<svg viewBox="0 0 ${width} ${Math.max(height, 160)}" role="img" aria-label="Schema diagram">`;
    svg += `<defs><marker id="sarrow" markerWidth="7" markerHeight="7" refX="6" refY="3.5" orient="auto"><path d="M0,0 L7,3.5 L0,7 z" fill="#9fb5ac"></path></marker></defs>`;

    // Edges beneath the boxes. Self-references (e.g. Person coauthor Person)
    // are drawn as a small loop on top of the box.
    const edges = Array.from(edgeMap.values()).slice(0, 18);
    edges.forEach((e) => {
      const label = Array.from(e.preds.entries())
        .sort((a, b) => b[1] - a[1])
        .slice(0, 2)
        .map(([p]) => localName(p))
        .join(", ");
      if (e.s === e.t) {
        const b = boxes[e.s];
        const cx = b.x + b.w - 26;
        const cy = b.y;
        svg += `<path class="cls-edge" d="M ${cx - 12} ${cy} C ${cx - 12} ${cy - 26}, ${cx + 12} ${cy - 26}, ${cx + 12} ${cy}" marker-end="url(#sarrow)"></path>`;
        svg += `<text class="cls-edge-label" x="${cx}" y="${cy - 28}" text-anchor="middle">${esc(shorten(label, 22))}</text>`;
        return;
      }
      const a = anchor(boxes[e.s]);
      const b = anchor(boxes[e.t]);
      svg += `<line class="cls-edge" x1="${a.x.toFixed(1)}" y1="${a.y.toFixed(1)}" x2="${b.x.toFixed(1)}" y2="${b.y.toFixed(1)}" marker-end="url(#sarrow)"></line>`;
      svg += `<text class="cls-edge-label" x="${((a.x + b.x) / 2).toFixed(1)}" y="${((a.y + b.y) / 2 - 4).toFixed(1)}" text-anchor="middle">${esc(shorten(label, 26))}</text>`;
    });

    boxes.forEach((b) => {
      svg += `<g><title>${esc(b.iri)} (${esc(b.count)} instances)</title>`;
      svg += `<rect class="cls-box" x="${b.x}" y="${b.y}" width="${b.w}" height="${b.h}" rx="6"></rect>`;
      svg += `<rect class="cls-head" x="${b.x}" y="${b.y}" width="${b.w}" height="${headH}" rx="6"></rect>`;
      svg += `<rect class="cls-head" x="${b.x}" y="${b.y + headH - 6}" width="${b.w}" height="6"></rect>`;
      svg += `<text class="cls-title" x="${b.x + 9}" y="${b.y + 16}">${esc(shorten(localName(b.iri), 18))}</text>`;
      svg += `<text class="cls-count" x="${b.x + b.w - 9}" y="${b.y + 16}" text-anchor="end">${esc(b.count)}</text>`;
      attrs[b.i].forEach(([p, n], j) => {
        const ay = b.y + headH + 14 + j * rowH;
        svg += `<text class="cls-attr" x="${b.x + 9}" y="${ay}">${esc(shorten(localName(p), 20))}</text>`;
        svg += `<text class="cls-attr-count" x="${b.x + b.w - 9}" y="${ay}" text-anchor="end">${esc(n)}</text>`;
      });
      svg += `</g>`;
    });
    svg += `</svg>`;
    return svg;
  }

  function renderProvenanceDefaults() {
    const cfg = CATALOG.provenance[state.dataset] || {};
    $("whySubject").value = cfg.subject || "";
    $("whyPredicate").value = cfg.predicate || "";
    $("whyObject").value = cfg.object || "";
  }

  function optText(id) {
    const v = $(id).value.trim();
    return v ? v : undefined;
  }

  function runProvenance() {
    if (!state.bytes) return showError("provOut", "Load a graph first.");
    const subject = optText("whySubject");
    const predicate = optText("whyPredicate");
    const object = optText("whyObject");
    const t0 = performance.now();
    try {
      const out = JSON.parse(W().why_triples(state.bytes, subject, predicate, object));
      const dt = performance.now() - t0;
      renderProvenance(out);
      $("whyMeta").textContent = `${out.resultCount} match(es) | ${dt.toFixed(1)} ms`;
      updateResultVisibility();
    } catch (e) {
      $("whyMeta").textContent = "";
      showError("provOut", String(e));
    }
  }

  function renderRange(range) {
    if (!range) return "absent";
    return `${formatBytes(range.len)} @ ${range.offset}..${range.end}`;
  }

  function renderProvenance(out) {
    state.lastProvenance = out;
    renderProvenanceSummary(out);
    const rows = (out.results || []).slice(0, 250).map((r) => {
      const p = r.provenance || {};
      return `<tr>` +
        `<td class="iri">${esc(shorten(r.terms.subject, 80))}</td>` +
        `<td class="iri">${esc(shorten(r.terms.predicate, 70))}</td>` +
        `<td class="iri">${esc(shorten(r.terms.object, 80))}</td>` +
        `<td>${esc(p.indexPermutation)} / ${esc(p.indexSection)}` +
        `<span class="cell-note">payload ${esc(renderRange(p.indexSectionRange))}</span></td>` +
        `<td>${esc(renderRange(p.dictionaryRange))}</td>` +
        `<td>${esc(renderRange(p.indexRange))}</td>` +
        `<td>${esc(p.tile && p.tile.available ? p.tile.id : "not_materialized")}</td>` +
      `</tr>`;
    }).join("");
    $("provOut").innerHTML =
      `<div class="banner">${out.resultCount} result(s) matched by the selected triple pattern.</div>` +
      `<table><thead><tr><th>subject</th><th>predicate</th><th>object</th><th>index section</th><th>dictionary range</th><th>index container</th><th>tile</th></tr></thead><tbody>${rows}</tbody></table>`;
  }

  function renderProvenanceSummary(out) {
    if (!out) {
      $("provSummary").innerHTML = `<div>Run Provenance mode to see index permutation and byte ranges.</div>`;
      return;
    }
    const first = (out.results || [])[0];
    if (!first) {
      $("provSummary").innerHTML = `<div>No matches for the current pattern.</div>`;
      return;
    }
    const p = first.provenance;
    $("provSummary").innerHTML =
      `<div class="metric-grid">` +
      metric("matches", out.resultCount) +
      metric("index", p.indexPermutation) +
      metric("section", p.indexSection) +
      metric("tile", p.tile.available ? "available" : p.tile.reason) +
      `</div>` +
      `<div>Dictionary: ${esc(renderRange(p.dictionaryRange))}</div>` +
      `<div>Index container: ${esc(renderRange(p.indexRange))}</div>` +
      `<div>Selected payload: ${esc(renderRange(p.indexSectionRange))}</div>` +
      `<div>Pyramid: ${esc(renderRange(p.pyramidRange))}</div>`;
  }

  function buildFileName() {
    const base = (state.built && state.built.name) || "graph";
    return base.replace(/\.(nt|nq|nquads|ttl|turtle|txt)$/i, "") + ".rete";
  }

  function runBuild() {
    const text = $("buildText").value;
    if (!text.trim()) return showError("buildOut", "Paste some RDF first (or open a file).");
    const fmt = $("buildFormat").value;
    const t0 = performance.now();
    try {
      const bytes = W().build(text, fmt);
      const dt = performance.now() - t0;
      state.built = { bytes, name: (state.built && state.built.name) || "graph" };
      const info = JSON.parse(W().info(bytes));
      $("buildDownload").disabled = false;
      $("buildOpen").disabled = false;
      $("buildMeta").textContent = `${formatBytes(bytes.length)} | ${dt.toFixed(1)} ms`;
      $("buildOut").innerHTML =
        `<div class="banner">Built <strong>${esc(buildFileName())}</strong> — a complete, queryable .rete file.</div>` +
        `<div class="metric-grid">` +
        metric("Quads", info.quads) +
        metric("Terms", info.terms) +
        metric("Pyramid levels", info.pyramidLevels) +
        metric("Named graphs", info.namedGraphs) +
        metric("Size", formatBytes(bytes.length)) +
        `</div>` +
        `<p class="microcopy">Download it, or open it in this console to query it immediately. ` +
        `In-browser builds write uncompressed sections (the wasm engine ships no zstd encoder); ` +
        `<code>rete build</code> produces a smaller file from the same input.</p>`;
      updateResultVisibility();
    } catch (e) {
      state.built = null;
      $("buildDownload").disabled = true;
      $("buildOpen").disabled = true;
      $("buildMeta").textContent = "";
      showError("buildOut", "Build failed: " + String(e));
    }
  }

  function downloadBuilt() {
    if (!state.built) return;
    const blob = new Blob([state.built.bytes], { type: "application/octet-stream" });
    const url = URL.createObjectURL(blob);
    const a = document.createElement("a");
    a.href = url;
    a.download = buildFileName();
    document.body.appendChild(a);
    a.click();
    a.remove();
    URL.revokeObjectURL(url);
  }

  function openBuilt() {
    if (!state.built) return;
    loadBytes(state.built.bytes, "built");
    setStatus(`${buildFileName()} | ${formatBytes(state.built.bytes.byteLength)} | built in browser`);
    $("dsDesc").textContent = "Graph built from RDF text in this session — query it like any dataset.";
    setMode("sparql");
  }

  async function loadBuildFile(file) {
    if (!file) return;
    try {
      const text = await file.text();
      setEd("buildText", text);
      state.built = { bytes: null, name: file.name };
      $("buildDownload").disabled = true;
      $("buildOpen").disabled = true;
      const ext = (file.name.match(/\.(\w+)$/) || [])[1] || "";
      const fmt = { nq: "nq", nquads: "nq", ttl: "ttl", turtle: "ttl" }[ext.toLowerCase()] || "nt";
      $("buildFormat").value = fmt;
      $("buildMeta").textContent = `${file.name} | ${formatBytes(file.size)} | ready to build`;
    } catch (e) {
      showError("buildOut", "File read failed: " + e.message);
    }
  }

  function showError(targetId, message) {
    $(targetId).innerHTML = `<div class="error-box">${esc(message)}</div>`;
    updateResultVisibility();
  }

  const HIST_KEY = "rete.playground.history";
  function loadHistory() {
    try {
      return JSON.parse(localStorage.getItem(HIST_KEY) || "[]");
    } catch (_e) {
      return [];
    }
  }

  function saveHistory(entry) {
    let history = loadHistory();
    history.unshift(entry);
    history = history.slice(0, 18);
    try {
      localStorage.setItem(HIST_KEY, JSON.stringify(history));
    } catch (_e) {
      return;
    }
    renderHistory();
  }

  function renderHistory() {
    const history = loadHistory();
    if (!history.length) {
      $("histList").innerHTML = `<div>No runs yet.</div>`;
      return;
    }
    $("histList").innerHTML = history.map((h, i) =>
      `<article class="history-item" data-hist="${i}">` +
      `<div class="mono">${esc(shorten((h.query || "").replace(/\s+/g, " "), 90))}</div>` +
      `<div>${esc(h.dataset)} | ${esc(h.strategy)} | ${esc(h.resultSummary || "")}</div>` +
      `</article>`
    ).join("");
    $$("#histList [data-hist]").forEach((el) => {
      el.onclick = () => {
        const h = loadHistory()[Number(el.dataset.hist)];
        if (!h) return;
        setEd("q", h.query || "");
        setView(h.format || "table");
        setStrategy(h.strategy || "whole");
        if (h.dataset && h.dataset !== state.dataset && RETE_DATASETS_B64[h.dataset]) loadDataset(h.dataset);
        setMode("sparql");
      };
    });
  }

  function updateHash() {
    const params = new URLSearchParams();
    params.set("dataset", state.dataset);
    params.set("mode", state.mode);
    const q = $("q").value.trim();
    if (q) params.set("q", q);
    history.replaceState(null, "", "#" + params.toString());
  }

  function readHash() {
    return new URLSearchParams(location.hash.replace(/^#/, ""));
  }

  async function shareUrl() {
    updateHash();
    const url = location.href;
    try {
      await navigator.clipboard.writeText(url);
      $("shareBtn").title = "Copied";
    } catch (_e) {
      $("qmeta").textContent = "Share URL: " + url;
    }
  }

  function wireEvents() {
    $("ds").onchange = () => loadDataset($("ds").value);
    $("run").onclick = runQuery;
    $("strategy").onchange = () => setStrategy($("strategy").value);
    $$("#viewSeg button").forEach((btn) => {
      btn.onclick = () => setView(btn.dataset.view);
    });
    $$("#modeTabs button").forEach((btn) => {
      btn.onclick = () => setMode(btn.dataset.mode);
    });
    $("exampleSearch").oninput = renderExamples;
    $("urlLoad").onclick = loadFromUrl;
    $("fileInput").onchange = (e) => loadFromFile(e.target.files[0]);
    $("shareBtn").onclick = shareUrl;
    $("shaclRun").onclick = runShacl;
    $("reachRun").onclick = runReach;
    $("whyRun").onclick = runProvenance;
    $("buildRun").onclick = runBuild;
    $("buildDownload").onclick = downloadBuilt;
    $("buildOpen").onclick = openBuilt;
    $("buildFile").onchange = (e) => loadBuildFile(e.target.files[0]);
    $("clearHist").onclick = () => {
      localStorage.removeItem(HIST_KEY);
      renderHistory();
    };

    const drop = $("dropZone");
    ["dragenter", "dragover"].forEach((ev) => {
      drop.addEventListener(ev, (e) => {
        e.preventDefault();
        drop.classList.add("drag");
      });
    });
    ["dragleave", "drop"].forEach((ev) => {
      drop.addEventListener(ev, (e) => {
        e.preventDefault();
        drop.classList.remove("drag");
      });
    });
    drop.addEventListener("drop", (e) => {
      const file = e.dataTransfer && e.dataTransfer.files && e.dataTransfer.files[0];
      loadFromFile(file);
    });
  }

  async function boot() {
    renderDatasetOptions();
    wireEvents();
    renderHistory();
    enhanceEditor("q", "sparql");
    enhanceEditor("shapeText", "ttl");
    enhanceEditor("buildText", "ttl");
    setEd("buildText", BUILD_SAMPLE);

    await wasm_bindgen(b64ToBytes(RETE_WASM_B64));

    const params = readHash();
    const ds = params.get("dataset") || CATALOG.defaultDataset;
    if (RETE_DATASETS_B64[ds]) state.dataset = ds;
    loadDataset(state.dataset);

    const q = params.get("q");
    if (q) {
      setEd("q", q);
      state.selectedExample = -1;
      renderExamples();
    }
    setMode(params.get("mode") || "sparql");
    updateResultVisibility();
  }

  boot().catch((e) => {
    setStatus("boot failed");
    showError("out", String(e && e.stack ? e.stack : e));
  });
})();
