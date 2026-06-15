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
    built: null,
    exploreClass: null,
    exploreReady: false,
    remote: null
  };

  const RDF_TYPE = "<http://www.w3.org/1999/02/22-rdf-syntax-ns#type>";

  // --- Remote lazy SPARQL worker ----------------------------------------
  // The engine is synchronous and wasm can't block on fetch, so remote
  // range-querying uses synchronous XHR — allowed only inside a Web Worker.
  // We build that worker from the page's own inlined wasm glue (the #reteGlue
  // script's source) plus a tiny harness, so the offline single-file page
  // gains real lazy remote querying with no extra files. Same mechanism
  // DuckDB uses over httpfs: fetch only the bytes a query touches.
  const REMOTE_HARNESS = `
;(function () {
  var ready = null;
  self.onmessage = function (e) {
    var m = e.data;
    if (m.type === "init") {
      ready = wasm_bindgen(m.bytes);
      ready.then(function () { self.postMessage({ type: "ready" }); });
      return;
    }
    if (m.type === "query") {
      Promise.resolve(ready).then(function () {
        try {
          self.postMessage({ type: "result", id: m.id, ok: true,
            json: wasm_bindgen.sparql_url(m.url, m.query, m.format) });
        } catch (err) {
          self.postMessage({ type: "result", id: m.id, ok: false, error: String(err) });
        }
      });
    }
  };
})();`;
  let remoteWorker = null, remoteReady = null, remoteResolveReady = null, remoteSeq = 0;
  const remotePending = new Map();

  function ensureRemoteWorker() {
    if (remoteWorker) return remoteReady;
    const glue = document.getElementById("reteGlue").textContent;
    const blob = new Blob([glue + REMOTE_HARNESS], { type: "text/javascript" });
    remoteWorker = new Worker(URL.createObjectURL(blob));
    remoteWorker.onmessage = (e) => {
      const m = e.data;
      if (m.type === "ready") { if (remoteResolveReady) remoteResolveReady(); return; }
      if (m.type === "result") {
        const p = remotePending.get(m.id);
        if (!p) return;
        remotePending.delete(m.id);
        m.ok ? p.resolve(m.json) : p.reject(new Error(m.error));
      }
    };
    remoteReady = new Promise((res) => { remoteResolveReady = res; });
    remoteWorker.postMessage({ type: "init", bytes: b64ToBytes(RETE_WASM_B64) });
    return remoteReady;
  }

  function remoteSparql(url, query, fmt) {
    return ensureRemoteWorker().then(() => new Promise((resolve, reject) => {
      const id = ++remoteSeq;
      remotePending.set(id, { resolve, reject });
      remoteWorker.postMessage({ type: "query", id, url, query, format: fmt || "table" });
    }));
  }

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
    if (state.activeSource === "remote") return "remote (lazy)";
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

  // Show the loaded dataset's short name on the topbar chip (which opens the
  // Datasets browser). Replaces the old <select> dropdown.
  function setDatasetName(key) {
    const d = datasetInfo(key);
    $("dsName").textContent = d ? d.label.split(" - ")[0] : key;
  }

  function renderDatasetOptions() {
    setDatasetName(state.dataset);
  }

  function loadBytes(bytes, source) {
    state.bytes = bytes;
    state.activeSource = source;
    state.remote = null; // an in-memory load leaves remote-lazy mode
    state.exploreReady = false;
    updateSourcePill();

    const info = JSON.parse(W().info(bytes));
    const graphNames = JSON.parse(W().graph_names(bytes));
    const graphText = graphNames.length ? " | graphs " + graphNames.length : "";
    setStatus(`${info.quads} quads | ${info.terms} terms | ${info.pyramidLevels} pyramid levels${graphText}`);

    const schema = JSON.parse(W().schema(bytes));
    state.schema = schema;
    state.exploreClass = null;
    state.exploreReady = false;
    renderSchema(schema);
    if (state.mode === "explore") ensureExplore();
    renderProgressiveInfo(null);
    renderProvenanceSummary(null);
    renderReachDefaults();
    renderShaclExamples();
    renderProvenanceDefaults();

    const infoRow = datasetInfo(state.dataset);
    $("dsDesc").textContent = source === "bundled"
      ? infoRow.description
      : "Custom graph loaded into the same in-browser engine.";
    if (source !== "bundled") {
      $("dsName").textContent = source === "file" ? "Local file" : source === "url" ? "Custom .rete" : "Custom";
    }
  }

  function loadDataset(key) {
    const b64 = RETE_DATASETS_B64[key];
    if (!b64) {
      setStatus("dataset not embedded: " + key);
      return;
    }
    state.dataset = key;
    setDatasetName(key);
    loadBytes(b64ToBytes(b64), "bundled");
    renderExamples();
    const list = examplesForDataset();
    if (list.length) selectExample(0);
    updateHash();
  }

  async function loadFromUrl() {
    const url = $("remoteUrl").value.trim();
    if (!url) return;
    setStatus("downloading...");
    try {
      const res = await fetch(url);
      if (!res.ok) throw new Error(res.status + " " + res.statusText);
      const buf = new Uint8Array(await res.arrayBuffer());
      loadBytes(buf, "url");
      closeSource();
    } catch (e) {
      showError("out", "URL load failed: " + e.message);
    }
  }

  // Enter remote lazy mode: query a remote .rete over HTTP range via the
  // worker, no full download. Only the SPARQL tab applies (the other tabs need
  // the whole graph in memory). `datasetKey` ties it to a catalog entry so its
  // example query library shows; a custom URL (no key) gets no library.
  function enterRemote(url, datasetKey) {
    if (!url) return;
    state.bytes = null;
    state.remote = { url };
    state.activeSource = "remote";
    state.schema = null;
    if (datasetKey) {
      state.dataset = datasetKey;
      setDatasetName(datasetKey);
    }
    state.selectedExample = -1;
    updateSourcePill();
    setStatus("remote (lazy) — queries range-fetch only what they touch");
    const info = datasetKey ? datasetInfo(datasetKey) : null;
    $("dsDesc").textContent = info ? info.description : "Remote graph, queried lazily over HTTP range: " + url;
    renderExamples();
    closeSource();
    setMode("sparql");
    const lib = examplesForDataset().length
      ? "Pick an example from the library, or write your own."
      : "Write a SPARQL query (a bound subject keeps the fetch small). No example library for a custom URL.";
    $("out").innerHTML = `<div class="note">Connected to a remote .rete, queried lazily — ` +
      `each query fetches only the dictionary chunks and index tiles it touches (the first also ` +
      `pulls the header and directories). ${lib} Other tabs need a graph loaded into memory.</div>`;
  }

  function connectRemote() {
    enterRemote($("remoteUrl").value.trim(), null);
  }

  // Dataset dropdown / catalog selection: bundled keys load into memory;
  // remote-lazy keys connect over HTTP range.
  function selectDataset(key) {
    const info = datasetInfo(key);
    if (info && info.kind === "remote-lazy") {
      enterRemote(info.url, key);
    } else {
      loadDataset(key);
    }
  }

  // The dataset metadata table: triples, .rete size, type (bundled/remote),
  // license, and a link to where the data came from. Driven by CATALOG.datasetMeta.
  function renderDatasetTable() {
    const fmtTri = (t) => (t == null ? "—" : typeof t === "number" ? t.toLocaleString() : esc(t));
    const host = (u) => { try { return new URL(u).host.replace(/^www\./, ""); } catch (e) { return u; } };
    const rows = CATALOG.datasets.map((d) => {
      const m = (CATALOG.datasetMeta && CATALOG.datasetMeta[d.key]) || {};
      const type = d.kind === "remote-lazy" ? "🛰 Remote · lazy" : "📦 Bundled";
      const src = m.source
        ? `<a href="${esc(m.source)}" target="_blank" rel="noopener">${esc(host(m.source))} ↗</a>`
        : "—";
      return `<tr><td><button type="button" class="ds-rowload" data-ds="${esc(d.key)}">${esc(d.label.split(" - ")[0])}</button></td>` +
        `<td class="num">${fmtTri(m.triples)}</td>` +
        `<td class="num">${esc(m.size || "—")}</td>` +
        `<td>${type}</td><td>${esc(m.license || "—")}</td><td>${src}</td></tr>`;
    }).join("");
    return `<table class="ds-table"><thead><tr>` +
      `<th>Dataset</th><th class="num">Triples</th><th class="num">.rete size</th>` +
      `<th>Type</th><th>License</th><th>Source</th></tr></thead><tbody>${rows}</tbody></table>`;
  }

  // The "Datasets" browser: one card per catalog dataset, with its description
  // and a source badge (the source is part of the dataset). Loading a card calls
  // selectDataset, which picks the right source — bundled→memory, remote→lazy.
  // A Cards/Table toggle switches to the metadata table above.
  function openSource() {
    $("datasetList").innerHTML = CATALOG.datasets.map((d) => {
      const remote = d.kind === "remote-lazy";
      const m = (d.description || "").match(/~?\s*([\d.]+\s*[MG]B)\b/);
      const size = m ? " · " + m[1].replace(/\s+/g, "") : "";
      const badge = remote
        ? `<span class="ds-badge remote">🛰 Remote · lazy${esc(size)}</span>`
        : `<span class="ds-badge bundled">📦 Bundled · in page</span>`;
      const active = d.key === state.dataset;
      return `<div class="ds-card${active ? " active" : ""}" data-ds="${esc(d.key)}">` +
        `<div class="ds-card-head"><b>${esc(d.label.split(" - ")[0])}</b>${badge}</div>` +
        `<p class="ds-card-desc">${esc(d.description || "(no description)")}</p>` +
        `<div class="ds-card-foot">` +
          `<button type="button" class="ds-load" data-ds="${esc(d.key)}">${active ? "Reload" : "Load"}</button>` +
          `<button type="button" class="ds-more" aria-label="Toggle full description">more</button>` +
        `</div></div>`;
    }).join("");
    $$("#datasetList .ds-load").forEach((btn) => {
      btn.onclick = () => { selectDataset(btn.dataset.ds); closeSource(); };
    });
    $$("#datasetList .ds-more").forEach((btn) => {
      btn.onclick = () => {
        const card = btn.closest(".ds-card");
        const open = card.classList.toggle("expanded");
        btn.textContent = open ? "less" : "more";
      };
    });
    // The metadata table (same datasets, at-a-glance) + the Cards/Table toggle.
    $("datasetTable").innerHTML = renderDatasetTable();
    $$("#datasetTable .ds-rowload").forEach((b) => {
      b.onclick = () => { selectDataset(b.dataset.ds); closeSource(); };
    });
    $$("#sourceModal [data-dsview]").forEach((b) => {
      b.onclick = () => {
        const table = b.dataset.dsview === "table";
        $("datasetList").classList.toggle("hidden", table);
        $("datasetTable").classList.toggle("hidden", !table);
        $$("#sourceModal [data-dsview]").forEach((x) => x.classList.toggle("active", x === b));
      };
    });
    $("sourceModal").classList.remove("hidden");
    // Show "more/less" only where the description is actually clamped — measured
    // now that the modal is visible (a hidden element reports zero heights).
    requestAnimationFrame(() => {
      $$("#datasetList .ds-card").forEach((card) => {
        const desc = card.querySelector(".ds-card-desc");
        const more = card.querySelector(".ds-more");
        if (more && desc && desc.scrollHeight <= desc.clientHeight + 1) more.style.display = "none";
      });
    });
  }

  function closeSource() {
    $("sourceModal").classList.add("hidden");
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
    if (mode === "explore") ensureExplore();
    updateResultVisibility();
    updateHash();
  }

  // --- Explore: entity tables + the community pyramid -------------------
  function ensureExplore() {
    if (!state.bytes || state.exploreReady) return;
    state.exploreReady = true;
    renderExploreClasses();
    renderPyramid();
    renderLayout();
  }

  // The predicate this dataset uses for typing — rdf:type by default, but some
  // graphs use another (e.g. Wikidata's "instance of" = wdt:P31). Declared per
  // dataset in the catalog as `typePredicate`.
  function currentTypePredicate() {
    const d = datasetInfo(state.dataset);
    return (d && d.typePredicate) || RDF_TYPE;
  }

  // Top classes for the Explore tab. For rdf:type we reuse the (fast, single-pass)
  // schema summary; for a custom type predicate we derive the top classes live via
  // SPARQL — a scan, heavier on big files, but the only way without a baked schema
  // that already knows the predicate.
  function exploreClassList() {
    const tp = currentTypePredicate();
    if (tp === RDF_TYPE) return ((state.schema && state.schema.classes) || []).slice(0, 12);
    try {
      const res = JSON.parse(W().query(state.bytes,
        `SELECT ?c (COUNT(?s) AS ?n) WHERE { ?s ${tp} ?c } GROUP BY ?c ORDER BY DESC(?n) LIMIT 12`, "table"));
      return (res.rows || []).map((r) => [r.c, (String(r.n).match(/\d+/) || ["?"])[0]]);
    } catch (e) { return []; }
  }

  function renderExploreClasses() {
    const tp = currentTypePredicate();
    const classes = exploreClassList();
    if (!classes.length) {
      const via = tp === RDF_TYPE ? "rdf:type" : shorten(localName(tp), 24);
      $("exploreClasses").innerHTML =
        `<p class="microcopy">No ${esc(via)} classes in this graph — showing raw triples.</p>`;
      const res = JSON.parse(W().query(state.bytes, "SELECT ?s ?p ?o WHERE { ?s ?p ?o } LIMIT 300", "table"));
      $("exploreTable").innerHTML = renderTable(res.vars || [], res.rows || []);
      return;
    }
    if (!state.exploreClass || !classes.some(([c]) => c === state.exploreClass))
      state.exploreClass = classes[0][0];
    $("exploreClasses").innerHTML = classes.map(([c, n]) =>
      `<button type="button" data-cls="${esc(c)}" class="${c === state.exploreClass ? "active" : ""}">` +
        `${esc(shorten(localName(c), 22))} (${esc(n)})` +
      `</button>`).join("");
    $$("#exploreClasses [data-cls]").forEach((btn) => {
      btn.onclick = () => {
        state.exploreClass = btn.dataset.cls;
        renderExploreClasses();
      };
    });
    renderEntityTable(state.exploreClass);
  }

  // Pivot one class's instances into an entity table: rows = entities,
  // columns = their most frequent properties (multi-values joined).
  function renderEntityTable(cls) {
    const tp = currentTypePredicate();
    let res;
    try {
      res = JSON.parse(W().query(state.bytes,
        `SELECT ?s ?p ?o WHERE { ?s ${tp} ${cls} . ?s ?p ?o } LIMIT 6000`, "table"));
    } catch (e) {
      $("exploreTable").innerHTML = `<div class="error-box">${esc(String(e))}</div>`;
      return;
    }
    const entities = new Map();
    const predCount = new Map();
    for (const row of res.rows || []) {
      if (row.p === tp) continue;
      if (!entities.has(row.s)) entities.set(row.s, new Map());
      const props = entities.get(row.s);
      if (!props.has(row.p)) props.set(row.p, []);
      props.get(row.p).push(row.o);
      predCount.set(row.p, (predCount.get(row.p) || 0) + 1);
    }
    const cols = Array.from(predCount.entries())
      .sort((a, b) => b[1] - a[1])
      .slice(0, 8)
      .map(([p]) => p);
    const rows = Array.from(entities.entries()).slice(0, 100);
    const cell = (vals) => {
      if (!vals) return "";
      const shown = vals.slice(0, 3).map((v) => shorten(v, 44)).join("; ");
      return vals.length > 3 ? `${shown} (+${vals.length - 3})` : shown;
    };
    const sampled = (res.rows || []).length >= 6000 ? " (sampled)" : "";
    const head = `<tr><th>${esc(localName(cls))}</th>` +
      cols.map((c) => `<th>${esc(shorten(localName(c), 20))}</th>`).join("") + `</tr>`;
    const rowHtmls = rows.map(([s, props]) =>
      `<tr><td class="iri">${esc(shorten(s, 42))}</td>` +
      cols.map((c) => `<td>${esc(cell(props.get(c)))}</td>`).join("") +
      `</tr>`);
    $("exploreTable").innerHTML = collapsedTable(head, rowHtmls,
      `<p class="microcopy">${entities.size} ${esc(localName(cls))} entit${entities.size === 1 ? "y" : "ies"}${sampled} — ` +
      `showing up to ${rows.length}, top ${cols.length} properties. Use the SPARQL tab for full values.</p>`);
  }

  // The "cluster of clusters": outer circles are the coarsest dendrogram
  // round; nested circles are the next finer round's communities they merge.
  function renderPyramid() {
    let tree;
    try {
      tree = JSON.parse(W().pyramid_tree(state.bytes));
    } catch (e) {
      $("pyramidNote").textContent = "pyramid error: " + String(e);
      return;
    }
    if (!tree.rounds) {
      $("pyramidNote").textContent = "This graph has no community structure (one community holds everything).";
      $("pyramidViz").innerHTML = "";
      $("pyramidLevels").innerHTML = "";
      return;
    }
    const chain = tree.levels.map((l) => l.length).reverse().join(" → ");
    $("pyramidNote").textContent =
      `A community is a group of subjects more densely connected to each other than to the rest ` +
      `of the graph, found by repeated Louvain clustering. Each clustering round merges ` +
      `communities into coarser ones — the pyramid. This file: ${tree.rounds} round(s), ` +
      `coarsest → finest ${chain} communities. These are the same rounds the “Split by ` +
      `community” Round field selects, and the units the pyramid summary aggregates.`;
    $("pyramidLegend").innerHTML =
      `<span class="lg"><span class="sw sw-pyr-outer"></span>outer circle = one coarsest-round community (area ∝ member nodes)</span>` +
      `<span class="lg"><span class="sw sw-pyr-inner"></span>nested bubble = a finer-round community it absorbs — the cluster of clusters</span>` +
      `<span class="lg">hover any circle for its exact node and triple counts</span>`;

    const outer = tree.levels[tree.rounds - 1].slice().sort((a, b) => b.nodes - a.nodes);
    const inner = tree.rounds >= 2 ? tree.levels[tree.rounds - 2] : null;
    const children = new Map();
    if (inner) {
      for (const c of inner) {
        if (!children.has(c.parent)) children.set(c.parent, []);
        children.get(c.parent).push(c);
      }
    }
    const shown = outer.slice(0, 24);
    const totalNodes = shown.reduce((a, c) => a + c.nodes, 0) || 1;
    const width = 920;
    const items = shown.map((c) => ({ c, R: Math.max(24, Math.sqrt(c.nodes / totalNodes) * 240) }));
    let x = 14, y = 16, rowH = 0;
    for (const it of items) {
      const d = it.R * 2 + 14;
      if (x + d > width) { x = 14; y += rowH + 14; rowH = 0; }
      it.cx = x + it.R;
      it.cy = y + it.R;
      x += d;
      rowH = Math.max(rowH, it.R * 2);
    }
    const height = y + rowH + 16;
    let svg = `<svg viewBox="0 0 ${width} ${Math.max(height, 140)}" role="img" aria-label="Community pyramid">`;
    for (const it of items) {
      svg += `<circle class="pyr-outer" cx="${it.cx.toFixed(1)}" cy="${it.cy.toFixed(1)}" r="${it.R.toFixed(1)}">` +
        `<title>round ${tree.rounds - 1} community C${it.c.id}: ${it.c.nodes} nodes, ${it.c.triples} triples</title></circle>`;
      const kids = (children.get(it.c.id) || []).sort((a, b) => b.nodes - a.nodes).slice(0, 40);
      const kTotal = kids.reduce((a, k) => a + k.nodes, 0) || 1;
      kids.forEach((k, i) => {
        const angle = i * 2.399963;
        const dist = (it.R * 0.6) * Math.sqrt((i + 0.5) / kids.length);
        const r = Math.max(3, Math.sqrt(k.nodes / kTotal) * it.R * 0.42);
        svg += `<circle class="pyr-inner" cx="${(it.cx + Math.cos(angle) * dist).toFixed(1)}" ` +
          `cy="${(it.cy + Math.sin(angle) * dist).toFixed(1)}" r="${r.toFixed(1)}">` +
          `<title>round ${tree.rounds - 2} community C${k.id}: ${k.nodes} nodes, ${k.triples} triples</title></circle>`;
      });
      if (it.R >= 30) {
        svg += `<text class="pyr-label" x="${it.cx.toFixed(1)}" y="${(it.cy - it.R + 14).toFixed(1)}" text-anchor="middle">C${it.c.id}</text>`;
      }
    }
    svg += `</svg>`;
    $("pyramidViz").innerHTML = svg +
      (outer.length > 24 ? `<p class="microcopy" style="padding:4px 10px">Showing the 24 largest of ${outer.length} top-level communities.</p>` : "");
    $("pyramidLevels").innerHTML =
      `<table><thead><tr><th>round</th><th>communities</th><th>largest (nodes)</th><th>largest (triples)</th></tr></thead><tbody>` +
      tree.levels.map((l, r) =>
        `<tr><td>${r}${r === tree.rounds - 1 ? " (coarsest)" : r === 0 ? " (finest)" : ""}</td>` +
        `<td>${l.length}</td><td>${Math.max(...l.map((c) => c.nodes))}</td>` +
        `<td>${Math.max(...l.map((c) => c.triples))}</td></tr>`).join("") +
      `</tbody></table>`;
  }

  // The byte map: every byte of the file as a wrapped grid of cells, colored
  // by the section it belongs to — where the data physically lives.
  const LAYOUT_COLORS = {
    header: "#17211d",
    metadata: "#7b5ea7",
    dictionary: "#147d69",
    directory: "#9fb5ac",
    pyramid: "#b98112",
    "named-graphs": "#235c7c",
    framing: "#e3e9e6"
  };
  const TILE_COLORS = ["#c84f2f", "#e0876a"];

  // Byte ranges the last Provenance run touched — the heat overlay.
  function touchedRanges() {
    const results = (state.lastProvenance && state.lastProvenance.results) || [];
    const out = [];
    for (const r of results.slice(0, 500)) {
      const p = r.provenance || {};
      for (const key of ["dictionaryRange", "indexSectionRange"]) {
        if (p[key]) out.push(p[key]);
      }
      if (p.tile && p.tile.range) out.push(p.tile.range);
    }
    return out;
  }

  function renderLayout() {
    if (!state.bytes) return;
    let lay;
    try {
      lay = JSON.parse(W().file_layout(state.bytes));
    } catch (e) {
      $("layoutNote").textContent = "layout error: " + String(e);
      return;
    }
    const segs = lay.segments;
    const total = lay.fileLength || 1;
    // Pre-index tiles for alternating shades.
    let tileSeq = 0;
    segs.forEach((s) => { if (s.kind === "tile") s.tile = tileSeq++; });

    // Cell size: each square is exactly `perCell` bytes. Auto picks the
    // smallest power of two that keeps the grid under ~1536 cells; an explicit
    // choice that would exceed 4096 cells falls back to auto with a note.
    const MAX_CELLS = 4096;
    const choice = $("layoutCell").value;
    const autoSize = () => {
      let s = 16;
      while (Math.ceil(total / s) > 1536) s *= 2;
      return s;
    };
    let perCell = choice === "auto" ? autoSize() : Number(choice);
    let fellBackCell = false;
    if (Math.ceil(total / perCell) > MAX_CELLS) {
      perCell = autoSize();
      fellBackCell = true;
    }
    const cells = Math.max(1, Math.ceil(total / perCell));
    const cols = Math.min(96, Math.max(24, Math.ceil(Math.sqrt(cells * 3))));
    const size = 9;
    const rows = Math.ceil(cells / cols);

    // Heat overlay: cells intersecting the byte ranges of the last
    // Provenance run (what a remote client would actually fetch).
    const hot = touchedRanges();
    const isHot = (lo, hi) => hot.some((r) => r.offset < hi && lo < r.offset + r.len);

    // Dominant-section coloring: a cell takes the section owning most of its
    // bytes; its opacity is that section's share, so paler = a boundary cell.
    let si = 0;
    let svg = `<svg viewBox="0 0 ${cols * size} ${rows * size}" role="img" aria-label="File byte map" style="max-width:100%">`;
    for (let i = 0; i < cells; i++) {
      const lo = i * perCell;
      const hi = Math.min(total, lo + perCell);
      while (si < segs.length && segs[si].offset + segs[si].len <= lo) si++;
      let best = null, bestBytes = 0, covered = 0;
      for (let j = si; j < segs.length && segs[j].offset < hi; j++) {
        const ov = Math.min(hi, segs[j].offset + segs[j].len) - Math.max(lo, segs[j].offset);
        if (ov > 0) {
          covered += ov;
          if (ov > bestBytes) { bestBytes = ov; best = segs[j]; }
        }
      }
      const framingBytes = (hi - lo) - covered;
      const useFraming = framingBytes > bestBytes;
      const frac = (useFraming ? framingBytes : bestBytes) / (hi - lo);
      const color = useFraming || !best ? LAYOUT_COLORS.framing
        : best.kind === "tile" ? TILE_COLORS[best.tile % 2]
        : (LAYOUT_COLORS[best.kind] || LAYOUT_COLORS.framing);
      const label = (useFraming || !best
        ? "container framing (section directories, length fields)"
        : `${best.label} — bytes ${best.offset}–${best.offset + best.len} (${formatBytes(best.len)})`) +
        ` | cell: bytes ${lo}–${hi}` + (frac < 1 ? ` (${Math.round(frac * 100)}% of cell)` : "");
      const heat = isHot(lo, hi);
      svg += `<rect x="${(i % cols) * size}" y="${Math.floor(i / cols) * size}" width="${size - 1}" height="${size - 1}" ` +
        `fill="${color}" fill-opacity="${(0.35 + 0.65 * frac).toFixed(2)}"` +
        (heat ? ` stroke="#17211d" stroke-width="1.4"` : "") +
        `><title>${esc(label + (heat ? " | touched by your last Provenance query" : ""))}</title></rect>`;
    }
    svg += `</svg>`;
    $("layoutViz").innerHTML = svg;
    $("layoutNote").textContent =
      `Each square is exactly ${formatBytes(perCell)} of the ${formatBytes(total)} file, in byte order ` +
      `(left→right, top→bottom)${fellBackCell ? " — the requested cell size was too fine for this file, using auto" : ""}. ` +
      `A paler square spans a section boundary. This is the surface a range query navigates: read the ` +
      `header, then jump straight to the squares you need.` +
      (hot.length ? " Outlined squares are the bytes your last Provenance query touched." :
        " Run a Provenance example and come back: the touched bytes get outlined.");
    const legendKinds = [
      ["header", "header"], ["metadata", "metadata"], ["dictionary", "dictionary"],
      ["directory", "tile directories"], ["tile", "index tiles (alternating per tile)"],
      ["pyramid", "pyramid summary"], ["named-graphs", "named graphs"], ["framing", "framing"]
    ];
    $("layoutLegend").innerHTML = legendKinds
      .filter(([k]) => k === "framing" || k === "tile" || segs.some((s) => s.kind === k))
      .map(([k, label]) =>
        `<span class="lg"><span class="sw" style="background:${k === "tile" ? TILE_COLORS[0] : LAYOUT_COLORS[k]}"></span>${esc(label)}</span>`)
      .join("") +
      (hot.length ? `<span class="lg"><span class="sw" style="background:#fff;border:2px solid #17211d"></span>touched by last Provenance query</span>` : "");
    // Per-kind byte totals.
    const sums = new Map();
    segs.forEach((s) => sums.set(s.kind, (sums.get(s.kind) || 0) + s.len));
    const coveredTotal = Array.from(sums.values()).reduce((a, b) => a + b, 0);
    sums.set("framing", Math.max(0, total - coveredTotal));
    $("layoutTable").innerHTML = collapsedTable(
      `<tr><th>section</th><th>bytes</th><th>share</th></tr>`,
      Array.from(sums.entries()).sort((a, b) => b[1] - a[1]).map(([k, n]) =>
        `<tr><td>${esc(k)}</td><td>${formatBytes(n)}</td><td>${(100 * n / total).toFixed(1)}%</td></tr>`)
    );
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
    } else if (state.mode === "coherence") {
      $("coherenceOut").classList.remove("hidden");
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
    const noRound = $("strategy").value !== "community";
    $("roundWrap").classList.toggle("hidden", noRound);
    $("roundHelp").classList.toggle("hidden", noRound);
  }

  // How many rows a table shows before its "Show more" button, and how many
  // each click reveals.
  const TABLE_HEAD_ROWS = 12;
  const TABLE_MORE_STEP = 50;

  /// Wrap table row strings into a collapsed table: the first TABLE_HEAD_ROWS
  /// rows show; the rest hide behind a "Show more" button (a delegated click
  /// handler in wireEvents reveals them in steps).
  function collapsedTable(headRowHtml, rowHtmls, note) {
    const hidden = Math.max(0, rowHtmls.length - TABLE_HEAD_ROWS);
    const body = rowHtmls
      .map((r, i) => (i < TABLE_HEAD_ROWS ? r : r.replace("<tr", `<tr class="tr-hidden"`)))
      .join("");
    return (note || "") +
      `<div class="tbl"><table><thead>${headRowHtml}</thead><tbody>${body}</tbody></table>` +
      (hidden > 0
        ? `<button type="button" class="tbl-more secondary">Show ${Math.min(hidden, TABLE_MORE_STEP)} more (${hidden} hidden)</button>`
        : "") +
      `</div>`;
  }

  function renderTable(vars, rows) {
    const cap = 500;
    const shown = (rows || []).slice(0, cap);
    const head = `<tr>${(vars || []).map((v) => `<th>${esc(v)}</th>`).join("")}</tr>`;
    const rowHtmls = shown.map((row) =>
      `<tr>${(vars || []).map((v) => `<td class="iri">${esc(shorten(row[v], 120))}</td>`).join("")}</tr>`);
    const note = (rows || []).length > cap
      ? `<p class="microcopy">Showing first ${cap} of ${rows.length} rows.</p>`
      : "";
    return collapsedTable(head, rowHtmls, note);
  }

  function renderTriplesTable(triples) {
    const cap = 500;
    const shown = (triples || []).slice(0, cap);
    const rowHtmls = shown.map((t) =>
      `<tr><td class="iri">${esc(shorten(t[0], 120))}</td><td class="iri">${esc(shorten(t[1], 120))}</td><td class="iri">${esc(shorten(t[2], 120))}</td></tr>`);
    const note = (triples || []).length > cap
      ? `<p class="microcopy">Showing first ${cap} of ${triples.length} triples.</p>`
      : "";
    return collapsedTable(`<tr><th>subject</th><th>predicate</th><th>object</th></tr>`, rowHtmls, note);
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
    const q = $("q").value.trim();
    if (!q) return showError("out", "Enter a SPARQL query.");
    const fmt = $("fmt").value;

    // Remote lazy mode: route through the worker (range reads), render async.
    if (state.remote) {
      $("commOut").innerHTML = "";
      updateResultVisibility();
      $("qmeta").textContent = "querying remote…";
      const t0 = performance.now();
      remoteSparql(state.remote.url, q, "table").then((raw) => {
        const res = JSON.parse(raw);
        const summary = renderResult(res, fmt === "graph" ? "table" : fmt);
        const r = res.remote || {};
        const pct = r.fileLength ? (100 * r.bytes / r.fileLength).toFixed(1) : "?";
        const dt = performance.now() - t0;
        $("qmeta").textContent = `${summary} | ${r.requests || 0} range req · ` +
          `${formatBytes(r.bytes || 0)} of ${formatBytes(r.fileLength || 0)} (${pct}%) · ${dt.toFixed(0)} ms`;
        saveHistory({ query: q, format: fmt, strategy: "remote", dataset: "(remote)", ts: Date.now(), resultSummary: summary });
      }).catch((e) => {
        $("qmeta").textContent = "";
        showError("out", "Remote query failed: " + String(e.message || e));
      });
      return;
    }

    if (!state.bytes) return showError("out", "Load a graph first.");
    const strategy = $("strategy").value;
    const queryFmt = strategy === "progressive" || fmt === "graph" ? "table" : fmt;
    $("commOut").innerHTML = "";
    updateResultVisibility();

    const t0 = performance.now();
    try {
      let raw;
      let fellBack = false;
      if (strategy === "progressive") {
        // Progressive is a contract, not a speedup: answer exactly from the
        // pyramid summary or don't. Shapes that need index/dictionary bytes
        // (any query returning values) fall back to the whole index — run,
        // and *say so* rather than refusing.
        try {
          raw = W().progressive_query(state.bytes, q);
        } catch (pe) {
          const m = String(pe);
          if (m.includes("not exactly answerable") || m.includes("no pyramid summary")) {
            raw = W().query(state.bytes, q, queryFmt);
            fellBack = true;
          } else {
            throw pe;
          }
        }
      } else if (strategy === "community") {
        const roundText = $("round").value.trim();
        raw = W().query_communities(state.bytes, q, roundText === "" ? undefined : Number(roundText));
      } else {
        raw = W().query(state.bytes, q, queryFmt);
      }
      const res = JSON.parse(raw);
      const summary = renderResult(res, strategy !== "whole" && fmt === "graph" ? "table" : fmt);
      const dt = performance.now() - t0;
      $("qmeta").textContent = `${summary} | ${dt.toFixed(1)} ms${fellBack ? " | fell back to whole index" : ""}`;
      if (fellBack) {
        $("out").innerHTML =
          `<div class="note">Not summary-answerable: this query returns values (titles, scores, …), ` +
          `which live in the dictionary and triple index — the pyramid summary holds only community ` +
          `structure and per-predicate counts, so the progressive contract (answer from the summary ` +
          `alone, never touch the index) cannot apply. <strong>Ran the whole index instead.</strong> ` +
          `Progressive shines on shapes like the “Predicate totals” example.</div>` +
          $("out").innerHTML;
        $("progressiveInfo").innerHTML =
          `<div>Fell back to the whole index — this query needs index bytes the summary does not hold.</div>`;
      }
      if (strategy === "community") renderCommunityPartials(res.communities);
      saveHistory({ query: q, format: fmt, strategy, dataset: state.dataset, ts: Date.now(), resultSummary: summary });
      updateHash();
    } catch (e) {
      $("qmeta").textContent = "";
      let msg = String(e);
      if (strategy === "progressive") {
        msg += " — Progressive answers COUNT/ASK shapes straight from the pyramid summary.";
      }
      showError("out", msg);
      renderProgressiveInfo(null);
    }
  }

  function renderCommunityPartials(parts) {
    if (!parts || !parts.length) return;
    const total = parts.reduce((a, p) => a + p.rows, 0);
    const contributing = parts.filter((p) => p.rows > 0);
    $("commOut").innerHTML =
      `<div class="banner">Subject stars computed per pyramid community, recombined with global ` +
      `joins, modifiers applied once: ${contributing.length} of ${parts.length} communities ` +
      `contributed ${total} partial row(s) — the merged result is identical to the whole-index answer.</div>` +
      collapsedTable(
        `<tr><th>community</th><th>subjects</th><th>partial rows</th></tr>`,
        contributing.map((p) =>
          `<tr><td>C${p.community}</td><td>${p.subjects}</td><td>${p.rows}</td></tr>`)
      );
    updateResultVisibility();
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

  function runCoherence() {
    if (!state.bytes) return showError("coherenceOut", "Load a graph first.");
    const t0 = performance.now();
    try {
      const schema = JSON.parse(W().check_schema(state.bytes));
      const full = JSON.parse(W().reason(state.bytes, null));
      const dt = performance.now() - t0;
      const block = (title, sub, coherent, points) => {
        const items = (points || []).map((p) =>
          `<li><code>${esc(p.kind)}</code> — ${esc(p.detail)}</li>`).join("");
        const verdict = coherent ? "coherent ✓" : `${points.length} incoherent point(s)`;
        return `<section class="coherence-block"><h3>${esc(title)}</h3>` +
          `<p class="microcopy">${esc(sub)}</p>` +
          `<p><strong>${verdict}</strong></p>` +
          (items ? `<ul>${items}</ul>` : "") + `</section>`;
      };
      $("coherenceOut").innerHTML =
        block("Schema (Tier-0, index-free)", "subClassOf cycles + unsatisfiable classes, from the schema pyramid", schema.coherent, schema.schemaPoints) +
        block("Full reasoner (instance-level)", `${full.inferredCount} triple(s) entailed; disjoint-class / sameAs / functional clashes`, full.coherent, full.inconsistencies);
      const ok = schema.coherent && full.coherent;
      $("coherenceMeta").textContent = `${ok ? "coherent" : "incoherent"} | ${dt.toFixed(1)} ms`;
      updateResultVisibility();
    } catch (e) {
      $("coherenceMeta").textContent = "";
      showError("coherenceOut", String(e));
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
    const list = cfg.examples || [];
    $("provExamples").innerHTML = list.map((ex, i) =>
      `<article class="example-card"><button type="button" class="example-button" data-prov="${i}">${esc(ex.label)}</button>` +
      `<div class="tagline">${esc(ex.tip)}</div></article>`).join("");
    $$("#provExamples [data-prov]").forEach((btn) => {
      btn.onclick = () => {
        const ex = list[Number(btn.dataset.prov)];
        $("whySubject").value = ex.subject || "";
        $("whyPredicate").value = ex.predicate || "";
        $("whyObject").value = ex.object || "";
        $("exampleInfo").innerHTML = `<strong>${esc(ex.label)}</strong><div>${esc(ex.tip)}</div>`;
        setMode("provenance");
        runProvenance();
      };
    });
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
      // Refresh the Explore byte map so the touched ranges light up there.
      if (state.exploreReady) renderLayout();
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
    $("buildBtn").onclick = () => setMode("build");
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
    $("coherenceRun").onclick = runCoherence;
    $("reachRun").onclick = runReach;
    $("whyRun").onclick = runProvenance;
    $("buildRun").onclick = runBuild;
    $("buildDownload").onclick = downloadBuilt;
    $("buildOpen").onclick = openBuilt;
    $("buildFile").onchange = (e) => loadBuildFile(e.target.files[0]);

    $("strategyHelp").onclick = () => $("strategyModal").classList.remove("hidden");
    $("roundHelp").onclick = () => $("strategyModal").classList.remove("hidden");
    $("layoutCell").onchange = renderLayout;
    $("dsButton").onclick = openSource;
    $("sourceModalClose").onclick = closeSource;
    $("remoteConnect").onclick = connectRemote;
    $("sourceModal").addEventListener("click", (e) => {
      if (e.target === $("sourceModal")) closeSource();
    });
    $("strategyModalClose").onclick = () => $("strategyModal").classList.add("hidden");
    $("strategyModal").addEventListener("click", (e) => {
      if (e.target === $("strategyModal")) $("strategyModal").classList.add("hidden");
    });
    document.addEventListener("keydown", (e) => {
      if (e.key === "Escape") {
        $("strategyModal").classList.add("hidden");
        closeSource();
      }
    });

    // Collapsed tables: every "Show more" button reveals the next step of
    // hidden rows (delegated, so it works for any dynamically-rendered table).
    document.addEventListener("click", (e) => {
      const btn = e.target.closest(".tbl-more");
      if (!btn) return;
      const wrap = btn.closest(".tbl");
      const hidden = $$("tr.tr-hidden", wrap);
      hidden.slice(0, TABLE_MORE_STEP).forEach((tr) => tr.classList.remove("tr-hidden"));
      const left = Math.max(0, hidden.length - TABLE_MORE_STEP);
      if (left === 0) btn.remove();
      else btn.textContent = `Show ${Math.min(left, TABLE_MORE_STEP)} more (${left} hidden)`;
    });
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
