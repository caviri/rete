// Shared playground widgets: a code editor (line-number gutter, syntax-highlight
// overlay, basic autocomplete) and result renderers (table, graph), exposed as
// `window.RetePG`. Used by the standalone explorer; the same machinery the main
// playground app uses inline. Pure, dependency-free, works from a static page.
(function () {
  "use strict";
  const esc = (v) => String(v == null ? "" : v)
    .replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;")
    .replace(/"/g, "&quot;").replace(/'/g, "&#39;");
  const shorten = (v, max = 90) => {
    const s = String(v == null ? "" : v);
    return s.length <= max ? s : s.slice(0, max - 1) + "…";
  };

  // --- syntax highlighting (sticky-regex tokenizers; same theme as docgen) ---
  const TOK = (() => {
    const COM = /#.*/y, DASH = /--.*/y, STR = /"(?:\\.|[^"\\])*"|'(?:\\.|[^'\\])*'/y;
    const IRI = /<[^>\s]*>?/y, VAR = /[?$][A-Za-z_]\w*/y, NUM = /\b\d[\d_]*(?:\.\d+)?\b/y;
    const WS = /\s+/y, IDENT = /[A-Za-z_]\w*/y;
    const PNAME = /[A-Za-z_][\w.-]*:[A-Za-z_][\w.-]*|:[A-Za-z_][\w.-]*/y, A = /\ba\b/y;
    const kw = (w, f) => new RegExp("\\b(?:" + w.join("|") + ")\\b", (f || "") + "y");
    const SPARQL = kw(["SELECT","CONSTRUCT","ASK","DESCRIBE","WHERE","PREFIX","BASE","FILTER","OPTIONAL",
      "UNION","MINUS","GRAPH","BIND","VALUES","DISTINCT","REDUCED","ORDER","BY","ASC","DESC","GROUP",
      "HAVING","LIMIT","OFFSET","FROM","NAMED","COUNT","SUM","AVG","MIN","MAX","SAMPLE","STR","LANG",
      "DATATYPE","BOUND","IRI","URI","REGEX","EXISTS","NOT","IN","AS","UNDEF"], "i");
    const SQL = kw(["SELECT","FROM","WHERE","GROUP","BY","ORDER","HAVING","LIMIT","OFFSET","AS","JOIN",
      "LEFT","RIGHT","INNER","OUTER","ON","AND","OR","NOT","IN","IS","NULL","LIKE","DISTINCT","COUNT",
      "SUM","AVG","MIN","MAX","UNION","ALL","DESC","ASC","CASE","WHEN","THEN","ELSE","END","WITH",
      "ATTACH","read_parquet","len","unnest","json_array_length","map_keys","map_values"], "i");
    return {
      sparql: [[COM,"com"],[IRI,"iri"],[STR,"str"],[VAR,"var"],[A,"kw"],[SPARQL,"kw"],[PNAME,"fn"],[NUM,"num"],[IDENT,null]],
      sql: [[DASH,"com"],[STR,"str"],[SQL,"kw"],[NUM,"num"],[IDENT,null]],
    };
  })();
  function highlight(text, lang) {
    const rules = TOK[lang] || TOK.sparql;
    let out = "", i = 0; const n = text.length, WS = /\s+/y;
    outer: while (i < n) {
      WS.lastIndex = i; const w = WS.exec(text);
      if (w && w.index === i) { out += w[0]; i += w[0].length; continue; }
      for (const [re, cls] of rules) {
        re.lastIndex = i; const m = re.exec(text);
        if (m && m.index === i && m[0].length) {
          out += cls ? `<span class="tok-${cls}">${esc(m[0])}</span>` : esc(m[0]);
          i += m[0].length; continue outer;
        }
      }
      out += esc(text[i]); i++;
    }
    return out;
  }

  // --- editor: gutter + highlight overlay + autocomplete ---
  function enhanceEditor(ta, opts) {
    opts = opts || {};
    const lang = () => (typeof opts.lang === "function" ? opts.lang() : opts.lang) || "sparql";
    const completions = () => (opts.completions ? opts.completions() : []);
    const wrap = document.createElement("div"); wrap.className = "ed";
    const gutter = document.createElement("div"); gutter.className = "ed-gutter";
    const body = document.createElement("div"); body.className = "ed-body";
    const hl = document.createElement("pre"); hl.className = "ed-hl";
    const code = document.createElement("code"); hl.appendChild(code);
    const sug = document.createElement("div"); sug.className = "ed-suggest hidden";
    ta.parentNode.insertBefore(wrap, ta);
    wrap.appendChild(gutter); wrap.appendChild(body);
    body.appendChild(hl); body.appendChild(ta); body.appendChild(sug);
    ta.setAttribute("wrap", "off"); ta.spellcheck = false;
    const ed = { ta, items: [], sel: 0, tok: null };
    ed.refresh = () => {
      code.innerHTML = highlight(ta.value, lang());
      gutter.textContent = Array.from({ length: ta.value.split("\n").length }, (_, i) => i + 1).join("\n");
      hl.scrollTop = ta.scrollTop; hl.scrollLeft = ta.scrollLeft; gutter.scrollTop = ta.scrollTop;
    };
    const sync = () => { hl.scrollTop = ta.scrollTop; hl.scrollLeft = ta.scrollLeft; gutter.scrollTop = ta.scrollTop; };
    const curTok = () => {
      const pos = ta.selectionStart, head = ta.value.slice(0, pos);
      const m = head.match(/[<A-Za-z0-9_?$:@\/#.-]+$/);
      return m ? { token: m[0], start: pos - m[0].length } : null;
    };
    function hide() { sug.classList.add("hidden"); ed.items = []; }
    function update() {
      const t = curTok();
      const min = t && (t.token.startsWith("?") || t.token.startsWith("<")) ? 1 : 2;
      if (!t || t.token.length < min) return hide();
      const low = t.token.toLowerCase(), bare = low.replace(/^</, "");
      const items = [], seen = new Set();
      for (const it of completions()) {
        const l = String(it.text).toLowerCase(); if (seen.has(l)) continue;
        const hit = l.startsWith("<") ? (bare.length >= 2 && l.includes(bare)) : (l.startsWith(t.token.toLowerCase()) && l !== t.token.toLowerCase());
        if (hit) { seen.add(l); items.push(it); if (items.length >= 8) break; }
      }
      if (!items.length) return hide();
      ed.items = items; ed.tok = t; ed.sel = 0; render();
      sug.classList.remove("hidden");
    }
    function render() {
      sug.innerHTML = ed.items.map((it, i) =>
        `<div class="sg ${i === ed.sel ? "active" : ""}" data-i="${i}"><span>${esc(shorten(it.text, 44))}</span><span class="sg-k">${esc(it.kind || "")}</span></div>`).join("");
      sug.querySelectorAll("[data-i]").forEach((el) => el.onmousedown = (e) => { e.preventDefault(); accept(+el.dataset.i); });
    }
    function accept(i) {
      const it = ed.items[i]; if (!it || !ed.tok) return;
      ta.value = ta.value.slice(0, ed.tok.start) + it.text + ta.value.slice(ta.selectionStart);
      const c = ed.tok.start + it.text.length; ta.setSelectionRange(c, c);
      hide(); ed.refresh(); ta.focus();
    }
    ta.addEventListener("input", () => { ed.refresh(); update(); });
    ta.addEventListener("scroll", sync);
    ta.addEventListener("keydown", (e) => {
      if (sug.classList.contains("hidden")) return;
      if (e.key === "ArrowDown" || e.key === "ArrowUp") { e.preventDefault(); ed.sel = (ed.sel + (e.key === "ArrowDown" ? 1 : ed.items.length - 1)) % ed.items.length; render(); }
      else if (e.key === "Tab" || e.key === "Enter") { e.preventDefault(); accept(ed.sel); }
      else if (e.key === "Escape") hide();
    });
    ta.addEventListener("blur", () => setTimeout(hide, 120));
    ed.refresh();
    return ed;
  }

  // --- result renderers ---
  function renderTable(target, cols, rows, cap) {
    cap = cap || 300;
    if (!rows.length) { target.innerHTML = '<p class="microcopy" style="padding:8px">no rows</p>'; return; }
    const head = cols.map((c) => `<th>${esc(c)}</th>`).join("");
    const body = rows.slice(0, cap).map((r) =>
      "<tr>" + cols.map((c) => `<td title="${esc(r[c])}">${esc(shorten(r[c], 200))}</td>`).join("") + "</tr>").join("");
    const more = rows.length > cap ? `<p class="microcopy">showing ${cap} of ${rows.length}</p>` : "";
    target.innerHTML = more + `<div class="tbl"><table><thead><tr>${head}</tr></thead><tbody>${body}</tbody></table></div>`;
  }

  // Force-directed graph of [s,p,o] triples (for CONSTRUCT / 2+ col selects).
  function renderGraph(target, triples) {
    if (!triples || !triples.length) { target.innerHTML = '<p class="microcopy" style="padding:8px">graph view needs triples (CONSTRUCT, or a SELECT with ≥2 columns)</p>'; return; }
    const cap = 90, nodeMap = new Map(), nodes = [], edges = [];
    const add = (t) => { if (!nodeMap.has(t) && nodes.length < cap) { const i = nodes.length, a = i * 2.399963, r = 34 + 7 * Math.sqrt(i); nodeMap.set(t, i); nodes.push({ t, label: shorten(t, 26), x: 460 + Math.cos(a) * r, y: 260 + Math.sin(a) * r }); } return nodeMap.get(t); };
    triples.forEach((t) => { const s = add(String(t[0])), o = add(String(t[2])); if (s != null && o != null) edges.push({ s, o, p: String(t[1]) }); });
    for (let it = 0; it < 90; it++) {
      for (let i = 0; i < nodes.length; i++) for (let j = i + 1; j < nodes.length; j++) {
        const a = nodes[i], b = nodes[j], dx = a.x - b.x || 0.01, dy = a.y - b.y || 0.01, d2 = dx * dx + dy * dy, f = Math.min(260 / d2, 0.035);
        a.x += dx * f; a.y += dy * f; b.x -= dx * f; b.y -= dy * f;
      }
      edges.forEach((e) => { const a = nodes[e.s], b = nodes[e.o], dx = b.x - a.x, dy = b.y - a.y; a.x += dx * 0.012; a.y += dy * 0.012; b.x -= dx * 0.012; b.y -= dy * 0.012; });
      nodes.forEach((n) => { n.x += (460 - n.x) * 0.01; n.y += (260 - n.y) * 0.01; n.x = Math.max(28, Math.min(892, n.x)); n.y = Math.max(28, Math.min(492, n.y)); });
    }
    let svg = `<svg viewBox="0 0 920 520" class="pg-graph"><defs><marker id="ar" markerWidth="7" markerHeight="7" refX="6" refY="3.5" orient="auto"><path d="M0,0 L7,3.5 L0,7 z" fill="#9fb5ac"/></marker></defs>`;
    edges.forEach((e) => { const a = nodes[e.s], b = nodes[e.o]; svg += `<line class="gedge" x1="${a.x.toFixed(1)}" y1="${a.y.toFixed(1)}" x2="${b.x.toFixed(1)}" y2="${b.y.toFixed(1)}" marker-end="url(#ar)"/>`; });
    nodes.forEach((n) => { svg += `<circle class="gnode" cx="${n.x.toFixed(1)}" cy="${n.y.toFixed(1)}" r="7"><title>${esc(n.t)}</title></circle><text class="gnode-label" x="${(n.x + 10).toFixed(1)}" y="${(n.y + 4).toFixed(1)}">${esc(n.label)}</text>`; });
    target.innerHTML = `<p class="microcopy">${nodes.length} nodes · ${edges.length} edges</p><div class="tbl">${svg}</svg></div>`;
  }

  window.RetePG = { esc, shorten, highlight, enhanceEditor, renderTable, renderGraph };
})();
