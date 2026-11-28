/* rete-server home page. No build step, no dependencies.
   The shell is public; every fetch below carries the bearer token when there is one, so all data
   stays behind it. An open server (no token) shows no token UI at all.
   window.reteHome is what tools.js (WebMCP) and the tests drive. */
(() => {
  "use strict";
  const $ = (id) => document.getElementById(id);
  const TOKEN_KEY = "rete.home.token";
  const ROW_CAP = 200;
  const FIRST = 12;            // catalog graphs shown before "Show all"
  const FEATURED = 6;
  const SMALL = 1 << 20;       // "small and quick": up to 1 MB
  const HEARD_OF = ["a browser", "Python", "JavaScript", "R", "Java", "the command line", "Claude"];
  const GENERIC = [
    { title: "Sample triples", question: "A few statements, to see what is in here.", sparql: "SELECT ?s ?p ?o WHERE { ?s ?p ?o } LIMIT 20" },
    { title: "Classes", question: "The kinds of things, most common first.", sparql: "SELECT ?class (COUNT(?s) AS ?n) WHERE { ?s a ?class } GROUP BY ?class ORDER BY DESC(?n) LIMIT 20" },
    { title: "Predicates", question: "The properties used, most common first.", sparql: "SELECT ?p (COUNT(*) AS ?n) WHERE { ?s ?p ?o } GROUP BY ?p ORDER BY DESC(?n) LIMIT 20" },
  ];

  const S = {
    token: "", open: false, needsToken: false, datasets: [], filter: "", tag: "", showAll: false,
    selected: null, entry: null, schema: null, examples: [], uploads: null, uploadsKnown: false,
    health: null, last: null, mode: "header", busy: false,
  };

  // ---- small helpers ----------------------------------------------------------
  function h(tag, attrs, ...kids) {
    const el = document.createElement(tag);
    for (const [k, v] of Object.entries(attrs || {})) {
      if (v == null || v === false) continue;
      if (k === "class") el.className = v;
      else if (k.startsWith("on")) el.addEventListener(k.slice(2), v);
      else el.setAttribute(k, v === true ? "" : v);
    }
    for (const kid of kids.flat()) if (kid != null && kid !== false) el.append(kid.nodeType ? kid : document.createTextNode(String(kid)));
    return el;
  }
  const clip = (s, n) => { s = s == null ? "" : String(s); return s.length > n ? s.slice(0, n - 1) + "…" : s; };
  const enc = (key) => String(key).split("/").map(encodeURIComponent).join("/");
  const isIri = (v) => typeof v === "string" && /^https?:\/\/\S+$/.test(v);
  const num = (n) => (typeof n === "number" ? n.toLocaleString("en-US", { maximumFractionDigits: 4 }) : String(n));
  const reduced = () => { try { return matchMedia("(prefers-reduced-motion: reduce)").matches; } catch (_e) { return false; } };
  function bytes(n) {
    if (!(n >= 0)) return "";
    const u = ["B", "KB", "MB", "GB", "TB"]; let i = 0;
    while (n >= 1024 && i < u.length - 1) { n /= 1024; i++; }
    return (i ? n.toFixed(n < 10 ? 1 : 0) : n) + " " + u[i];
  }
  function sizeBytes(d) {
    if (d.size_bytes > 0) return d.size_bytes;
    const m = /^([\d.,]+)\s*(B|KB|MB|GB|TB)\b/i.exec(d.size || "");
    if (!m) return null;
    return parseFloat(m[1].replace(/,/g, "")) * Math.pow(1024, ["B", "KB", "MB", "GB", "TB"].indexOf(m[2].toUpperCase()));
  }
  // "1.33 M", "85.1 K", "5": a catalog's triple count as a number.
  function tripleCount(d) {
    const m = /^([\d.,]+)\s*([KMB])?/i.exec(String(d.triples ?? ""));
    if (!m) return 0;
    return parseFloat(m[1].replace(/,/g, "")) * ({ K: 1e3, M: 1e6, B: 1e9 }[(m[2] || "").toUpperCase()] || 1);
  }
  const isLocal = (d) => d.kind === "local" || d.kind === "generated" || /^(local|generated)\//.test(d.key || "");
  // Plain text from the markdown or HTML a card description may carry.
  function plain(s) {
    return String(s || "").replace(/<[^>]*>/g, " ").replace(/!?\[([^\]]*)\]\([^)]*\)/g, "$1")
      .replace(/\*\*|__|`/g, "").replace(/\*([^*\s][^*]*)\*/g, "$1")
      .replace(/&amp;/g, "&").replace(/&nbsp;/g, " ").replace(/&lt;/g, "<").replace(/&gt;/g, ">").replace(/\s+/g, " ").trim();
  }
  function firstSentence(s, n) {
    s = plain(s);
    // A sentence ends at . ! or ? before a capital, but not after an initial
    // ("Charles S. Peirce") or a common abbreviation ("e.g.", "St.").
    const m = /^(.+?(?<!\b[A-Z]|\b(?:e\.g|i\.e|St|Dr|Mr|Mrs|Ms|vs|cf|ca|No))[.!?])(\s+[A-Z0-9(\[]|$)/.exec(s);
    return clip(m ? m[1] : s, n || 170);
  }
  // "hugging-face.rete — the whole Hub (remote, lazy)" -> "The whole Hub".
  function cleanLabel(d) {
    let t = plain(d.label || "");
    const key = d.key || "";
    if (key && t.toLowerCase().startsWith(key.toLowerCase())) t = t.slice(key.length).replace(/^\.rete\b/i, "").replace(/^\s*[—–-]\s*/, "");
    t = t.replace(/\s*\((?:[^()]*,\s*)?(?:remote, lazy|embedded)\)\s*$/i, "").replace(/\s*\((?:remote, lazy|embedded)\)\s*$/i, "").trim();
    if (!t || t === key) return "";
    return t.charAt(0).toUpperCase() + t.slice(1);
  }
  function localName(iri) {
    const s = String(iri).replace(/[#\/]+$/, "");
    const m = /[^#\/]+$/.exec(s);
    let n = m ? m[0] : s;
    try { n = decodeURIComponent(n); } catch (_e) { /* keep raw */ }
    return n.replace(/_/g, " ") || iri;
  }
  let toastTimer = null;
  function toast(text, bad) {
    const el = $("toast");
    el.textContent = text; el.className = "toast" + (bad ? " bad" : ""); el.hidden = false;
    clearTimeout(toastTimer);
    toastTimer = setTimeout(() => { el.hidden = true; }, bad ? 9000 : 3500);
  }
  function goto(id) {
    const el = $(id);
    if (el) el.scrollIntoView({ block: "start", behavior: reduced() ? "auto" : "smooth" });
  }

  // ---- token ------------------------------------------------------------------
  function readToken() {
    let t = "";
    try {
      const p = new URLSearchParams(location.search);
      if (p.has("token")) {
        t = p.get("token") || "";
        p.delete("token");
        const q = p.toString();
        history.replaceState(null, "", location.pathname + (q ? "?" + q : "") + location.hash);
        if (t) { try { sessionStorage.setItem(TOKEN_KEY, t); } catch (_e) { /* private mode */ } }
      }
    } catch (_e) { /* no URL API */ }
    if (!t) { try { t = sessionStorage.getItem(TOKEN_KEY) || ""; } catch (_e) { /* none */ } }
    return t;
  }
  function saveToken(t) {
    S.token = t;
    try { if (t) sessionStorage.setItem(TOKEN_KEY, t); else sessionStorage.removeItem(TOKEN_KEY); } catch (_e) { /* ignore */ }
  }

  // ---- api --------------------------------------------------------------------
  class ApiError extends Error { constructor(status, msg) { super(msg); this.status = status; } }
  async function api(path, init) {
    init = init || {};
    const headers = Object.assign({}, init.headers || {});
    if (S.token) headers.Authorization = "Bearer " + S.token;
    let r;
    try { r = await fetch(path, Object.assign({}, init, { headers })); }
    catch (e) { throw new ApiError(0, "Cannot reach the server: " + (e.message || e)); }
    if (!r.ok) {
      let msg = r.statusText;
      try { const j = await r.json(); msg = typeof j.detail === "string" ? j.detail : JSON.stringify(j.detail || j); } catch (_e) { /* not JSON */ }
      if (r.status === 401) { S.open = false; S.needsToken = true; showGate(S.token ? "That token was not accepted." : ""); }
      throw new ApiError(r.status, msg || "HTTP " + r.status);
    }
    return r.json();
  }

  // ---- gate -------------------------------------------------------------------
  function showGate(msg) {
    $("app").hidden = true; $("gate").hidden = false;
    if (msg) $("gateMsg").textContent = msg;
    $("signOut").hidden = !S.token;
    if (!window.__homeBooting) $("tokenInput").focus();
  }
  async function start() {
    $("gate").hidden = true; $("app").hidden = false;
    try {
      const list = await api("/api/datasets");
      S.open = true; S.datasets = list;
    } catch (e) {
      if (e.status === 401) return;
      $("dlist").textContent = ""; $("dlist").append(h("li", { class: "empty" }, "Could not load the graphs. " + e.message));
      toast(e.message, true); return;
    }
    $("signOut").hidden = !S.token;
    S.uploadsKnown = false; renderDrop();
    api("/api/uploads").then((u) => { S.uploads = u; S.uploadsKnown = true; renderDrop(); })
      .catch(() => { S.uploads = null; S.uploadsKnown = true; renderDrop(); });
    renderStats(); renderList(); renderConnect();
    if (!S.selected && S.datasets.length === 1) select(S.datasets[0].key);
  }

  // ---- stats, list ------------------------------------------------------------
  function renderStats() {
    const v = S.health ? S.health.version : "";
    $("ver").textContent = v ? "v" + v : "";
    const box = $("stats"); box.textContent = "";
    if (!S.open) return;
    const cat = S.datasets.filter((d) => !isLocal(d)).length, loc = S.datasets.length - cat;
    const n = S.datasets.length;
    box.append(h("span", {}, num(n) + (n === 1 ? " graph" : " graphs")));
    if (cat && loc) box.append(h("span", {}, num(cat) + " in the catalog"));
    if (v) box.append(h("span", {}, "v" + v));
  }
  const words = (d) => ((d.key || "") + " " + (d.label || "") + " " + plain(d.description || "") + " " + (d.tags || []).join(" ")).toLowerCase();
  function filtered() {
    const w = S.filter.toLowerCase().split(/\s+/).filter(Boolean);
    return S.datasets.filter((d) => {
      if (S.tag && !(d.tags || []).includes(S.tag)) return false;
      const t = words(d); return w.every((x) => t.includes(x));
    });
  }
  function meta(d) {
    const out = [];
    if (d.triples != null && d.triples !== "") out.push((typeof d.triples === "number" ? num(d.triples) : String(d.triples)) + " triples");
    const sz = d.size_bytes ? bytes(d.size_bytes) : d.size;
    if (sz) out.push(sz);
    return out;
  }
  function pick(key) { select(key).catch(() => {}); goto("query"); }
  function item(d, local) {
    const sub = local ? "" : (firstSentence(d.description, 170) || cleanLabel(d));
    const tags = (d.tags || []).slice(0, 3);
    return h("li", { "data-key": d.key, class: d.key === S.selected ? "on" : "" },
      h("button", { type: "button", class: "ditem", "aria-pressed": String(d.key === S.selected), title: plain(d.label || d.key),
        onclick: () => pick(d.key) },
      h("span", { class: "t" }, local ? (d.label || d.key) : d.key),
      local ? h("span", { class: "k" }, clip(d.key, 70)) : null,
      sub ? h("span", { class: "sub" }, sub) : null,
      h("span", { class: "m" }, meta(d).map((m) => h("span", {}, m)), tags.map((t) => h("span", { class: "tg" }, t)))));
  }
  function featured() {
    const small = S.datasets.filter((d) => !isLocal(d) && sizeBytes(d) != null && sizeBytes(d) <= SMALL && (d.triples != null) && !d.shards);
    // No hand-picked list: of the graphs that open fast, the ones with the most in them.
    small.sort((a, b) => tripleCount(b) - tripleCount(a) || a.key.localeCompare(b.key));
    return small.slice(0, FEATURED);
  }
  function renderTags() {
    const counts = new Map();
    for (const d of S.datasets) if (!isLocal(d)) for (const t of d.tags || []) counts.set(t, (counts.get(t) || 0) + 1);
    let top = [...counts].filter((e) => e[1] >= 2).sort((a, b) => b[1] - a[1] || a[0].localeCompare(b[0])).slice(0, 14).map((e) => e[0]);
    if (S.tag && !top.includes(S.tag)) top = [S.tag].concat(top);
    const box = $("tagChips"); box.textContent = "";
    for (const t of top) box.append(h("button", { type: "button", class: "chip", "aria-pressed": String(S.tag === t),
      onclick: () => { S.tag = S.tag === t ? "" : t; renderList(); } }, t));
  }
  function renderList() {
    const list = filtered(), ul = $("dlist"), lu = $("llist");
    const cat = list.filter((d) => !isLocal(d)), loc = list.filter(isLocal);
    const active = !!(S.filter.trim() || S.tag);
    renderTags();

    // Small ones to try, only before anyone has searched.
    const feat = active ? [] : featured();
    const fb = $("featured"); fb.textContent = "";
    for (const d of feat) {
      fb.append(h("li", { class: "fcard", "data-key": d.key },
        h("h4", {}, d.key),
        h("p", {}, firstSentence(d.description, 120) || cleanLabel(d)),
        h("p", { class: "m" }, meta(d).map((m) => h("span", {}, m))),
        h("button", { type: "button", class: "btn btn-primary btn-sm", "aria-label": "Try " + d.key, onclick: () => pick(d.key) }, "Try it")));
    }
    $("featuredBox").hidden = !feat.length;

    ul.textContent = "";
    const skip = new Set(feat.map((d) => d.key));
    const rest = cat.filter((d) => !skip.has(d.key));
    const cap = S.showAll || active ? 500 : FIRST;
    if (!cat.length && !loc.length) ul.append(h("li", { class: "empty" }, S.datasets.length ? "Nothing matches. Try fewer words, or clear the tag." : "No graphs yet."));
    else if (!cat.length) ul.append(h("li", { class: "empty" }, S.datasets.some((d) => !isLocal(d)) ? "Nothing in the catalog matches." : "This server has no catalog, only the files below."));
    for (const d of rest.slice(0, cap)) ul.append(item(d, false));
    const more = rest.length - cap;
    $("moreBtn").hidden = !(more > 0);
    $("moreBtn").textContent = "Show all " + num(rest.length + feat.length) + " graphs";
    $("dcount").textContent = !S.datasets.length ? "" : active ? num(cat.length) + " in the catalog match" + (loc.length ? ", " + num(loc.length) + " files" : "") + "." : "";

    // Files that live on this server.
    const allLoc = S.datasets.filter(isLocal);
    $("localBox").hidden = !allLoc.length;
    $("localSum").textContent = "Files on this server (" + num(allLoc.length) + (active && loc.length !== allLoc.length ? ", " + num(loc.length) + " match" : "") + ")";
    lu.textContent = "";
    for (const d of loc.slice(0, 300)) lu.append(item(d, true));
    if (loc.length > 300) lu.append(h("li", { class: "empty" }, "Showing 300 of " + num(loc.length) + ". Search to narrow."));
    if (!loc.length && allLoc.length) lu.append(h("li", { class: "empty" }, "No file matches."));
    const noCatalog = !S.datasets.some((d) => !isLocal(d));
    if (noCatalog || (active && loc.length) || (S.selected && allLoc.some((d) => d.key === S.selected))) $("localBox").open = true;
  }

  // ---- card -------------------------------------------------------------------
  async function select(key, quiet) {
    S.selected = key; S.entry = null; S.schema = null; S.examples = [];
    $("current").textContent = key;
    $("qEmpty").hidden = true; $("qBody").hidden = false;
    $("result").textContent = ""; $("runMeta").textContent = "";
    $("q").value = "";
    renderList(); renderCard(); renderStarters();
    const q = (p) => api("/api/datasets/" + enc(key) + p);
    const [entry, schema, examples] = await Promise.allSettled([q(""), q("/schema"), q("/examples")]);
    if (S.selected !== key) return null;
    S.entry = entry.status === "fulfilled" ? entry.value : (S.datasets.find((d) => d.key === key) || { key });
    S.entryError = entry.status === "rejected" ? entry.reason.message : null;
    S.schema = schema.status === "fulfilled" ? schema.value : null;
    S.examples = examples.status === "fulfilled" && Array.isArray(examples.value) ? examples.value : [];
    renderCard(); renderStarters();
    if (!quiet) { $("cardPanel").classList.add("flash"); setTimeout(() => $("cardPanel").classList.remove("flash"), 1700); }
    return S.entry;
  }
  function renderCard() {
    const box = $("cardPanel"); box.textContent = "";
    if (!S.selected) return;
    const e = S.entry, card = (e && e.card) || {};
    const base = S.datasets.find((d) => d.key === S.selected) || { key: S.selected };
    const title = card.title || card.name || cleanLabel(e || base) || cleanLabel(base) || S.selected;
    box.append(h("h3", {}, clip(plain(title), 160)));
    if (!e) { box.append(h("p", { class: "hint loading" }, "Reading the card…")); return; }
    const desc = plain(card.description || e.description || "");
    if (desc && !/^A \.rete file in this server/.test(desc)) box.append(h("p", { class: "desc" }, clip(desc, 420)));
    const info = (S.schema && S.schema.info) || {};
    const lic = card.license || e.license;
    const facts = [
      ["triples", e.triples != null ? num(e.triples) : info.quads != null ? num(info.quads) : null],
      ["size", e.size_bytes ? bytes(e.size_bytes) : e.size || null],
      ["license", lic ? (typeof lic === "string" ? lic : JSON.stringify(lic)) : null],
      ["source", card.source || e.source || null],
    ].filter((f) => f[1]);
    if (facts.length) box.append(h("p", { class: "facts" }, facts.map(([k, v]) => h("span", {}, k + " ", h("b", {}, clip(plain(v), 90))))));
    const classes = (S.schema && S.schema.schema && S.schema.schema.classes) || [];
    if (classes.length) {
      box.append(h("details", { class: "classes-box" }, h("summary", {}, "What is in it"),
        h("div", { class: "classes" }, classes.slice(0, 14).map(([c, n]) => h("code", { title: c }, localName(c) + " · " + num(n))))));
    }
    const links = [];
    const sp = e.sparql_endpoint || (e.shards ? null : "/sparql/" + S.selected);
    if (sp) links.push(h("a", { href: sp }, "SPARQL endpoint"));
    if (e.url) links.push(h("a", { href: e.url }, ".rete file"));
    if (links.length) box.append(h("p", { class: "links" }, links));
    if (S.entryError) box.append(h("p", { class: "hint" }, S.entryError));
  }

  // ---- query ------------------------------------------------------------------
  function renderStarters() {
    const box = $("starters"); box.textContent = "";
    if (!S.selected) return;
    const list = [];
    for (const ex of S.examples.slice(0, 8)) {
      if (ex && ex.sparql) list.push({ title: ex.title || ex.question || "Example", question: ex.question || "", sparql: ex.sparql });
    }
    if (!list.length) for (const g of GENERIC) list.push(g);
    if (!S.entry) box.append(h("span", { class: "hint loading" }, "Looking for questions…"));
    for (const s of list) {
      const b = h("button", { class: "chip", type: "button", "aria-pressed": "false",
        title: s.question || s.title, onclick: () => { setQuery(s.sparql); run().catch(() => {}); } }, clip(s.title, 48));
      b._sparql = s.sparql;
      box.append(b);
    }
  }
  function setQuery(text) {
    $("q").value = text;
    for (const c of $("starters").children) c.setAttribute("aria-pressed", String(!!c._sparql && c._sparql.trim() === text.trim()));
  }
  function cellFromBinding(b) {
    if (!b) return h("td", { class: "none" }, "");
    if (b.type === "uri") return h("td", {}, h("a", { class: "iri", href: b.value, title: b.value, target: "_blank", rel: "noopener" }, clip(localName(b.value), 80)));
    if (b.type === "bnode") return h("td", { class: "bn" }, "_:" + b.value);
    const dt = b.datatype || "";
    if (/#(integer|decimal|double|float|long|int|short|nonNegativeInteger|positiveInteger|negativeInteger|unsignedLong|unsignedInt)$/.test(dt) && b.value !== "" && isFinite(Number(b.value))) {
      return h("td", { class: "n", title: b.value }, num(Number(b.value)));
    }
    const lang = b["xml:lang"];
    const long = b.value.length > 300;
    return h("td", { title: long ? b.value : dt || null }, long ? clip(b.value, 300) : b.value, lang ? h("span", { class: "lang" }, "@" + lang) : null);
  }
  function cellFromValue(v) {
    if (v == null) return h("td", { class: "none" }, "");
    if (typeof v === "number") return h("td", { class: "n" }, num(v));
    if (typeof v === "object") return h("td", {}, JSON.stringify(v));
    if (isIri(v)) return h("td", {}, h("a", { class: "iri", href: v, title: v, target: "_blank", rel: "noopener" }, clip(localName(v), 80)));
    return h("td", {}, clip(String(v), 300));
  }
  function warningsBox(res) {
    const w = (res.warnings || []).map((x) => (x && typeof x === "object" ? (x.message || JSON.stringify(x)) : String(x)));
    if (!w.length) return null;
    return h("div", { class: "warn" }, h("strong", {}, w.length === 1 ? "The engine noted one thing." : "The engine noted " + w.length + " things."),
      h("ul", {}, w.slice(0, 5).map((m) => h("li", {}, clip(m, 400)))));
  }
  function renderResult(res) {
    const box = $("result"); box.textContent = "";
    const warn = warningsBox(res);
    if (res.kind === "ask" || (res.boolean != null && !res.head)) {
      box.append(h("p", { class: "verdict" }, res.boolean ? "Yes" : "No", h("span", {}, res.boolean ? "  (true)" : "  (false)")));
      if (warn) box.append(warn);
      return;
    }
    const vars = (res.head && res.head.vars) || [];
    const rows = res.table || [];
    const binds = res.results && Array.isArray(res.results.bindings) && res.results.bindings.length === rows.length ? res.results.bindings : null;
    if (vars.length) {
      const total = rows.length;
      if (!total) {
        box.append(h("p", { class: "none-msg" }, h("strong", {}, "No rows."), " The query ran and nothing matched."));
      } else {
        const thead = h("thead", {}, h("tr", {}, vars.map((v) => h("th", { scope: "col" }, v))));
        const tbody = h("tbody", {}, rows.slice(0, ROW_CAP).map((r, i) => h("tr", {}, vars.map((v) => (binds ? cellFromBinding(binds[i][v]) : cellFromValue(r[v]))))));
        box.append(h("div", { class: "table-wrap", tabindex: "0", role: "region", "aria-label": "Query results" }, h("table", {}, thead, tbody)));
        box.append(h("p", { class: "table-foot" }, num(total) + (total === 1 ? " row" : " rows") +
          (total > ROW_CAP ? ", first " + ROW_CAP + " shown" : "") + (res.truncated ? ", the server cut the result short" : "") + "."));
      }
    } else if (Array.isArray(res.triples)) {
      if (!res.triples.length) box.append(h("p", { class: "none-msg" }, h("strong", {}, "No triples."), " The query ran and nothing matched."));
      else {
        const lines = res.triples.slice(0, ROW_CAP).map((t) => Array.isArray(t) ? t.join(" ") + " ." : typeof t === "string" ? t : JSON.stringify(t));
        box.append(h("pre", { class: "code" }, lines.join("\n")));
        box.append(h("p", { class: "table-foot" }, num(res.triples.length) + " triples."));
      }
    } else {
      box.append(h("pre", { class: "code" }, JSON.stringify(res, null, 2)));
    }
    if (warn) box.append(warn);
  }
  function renderError(e) {
    const box = $("result"); box.textContent = "";
    const msg = e && e.message ? e.message : String(e);
    box.append(h("div", { class: "err", role: "alert" },
      h("strong", {}, e && e.status === 0 ? "The server did not answer." : "The query did not run."),
      h("p", {}, msg),
      h("p", { class: "hint" }, "Check the SPARQL, or open \"Show the SPARQL\" and edit it.")));
  }
  async function run(text, dataset) {
    const q = (text != null ? text : $("q").value).trim();
    const key = dataset || S.selected;
    if (!key) { toast("Pick a graph first.", true); throw new Error("no dataset selected"); }
    if (!q) { toast("The query is empty.", true); throw new Error("query is empty"); }
    if (text != null) $("q").value = text;
    $("run").disabled = true; $("runMeta").textContent = "";
    const box = $("result"); box.textContent = ""; box.setAttribute("aria-busy", "true");
    box.append(h("p", { class: "loading" }, h("span", { class: "spin", "aria-hidden": "true" }), " Running the query…"));
    for (const b of $("starters").querySelectorAll("button")) b.disabled = true;
    try {
      const t0 = performance.now();
      const res = await api("/api/query", { method: "POST", headers: { "Content-Type": "application/json" },
        body: JSON.stringify({ dataset: key, query: q, limit: ROW_CAP }) });
      S.last = res;
      renderResult(res);
      const ms = Math.round(performance.now() - t0);
      const st = res.stats || {};
      $("runMeta").textContent = ms + " ms" + (st.bytes != null ? " · read " + bytes(st.bytes) + " in " + st.requests + (st.requests === 1 ? " request" : " requests") : "");
      box.scrollIntoView({ block: "nearest", behavior: reduced() ? "auto" : "smooth" });
      return res;
    } catch (e) {
      if (e.status !== 401) renderError(e);
      $("runMeta").textContent = "";
      throw e;
    } finally {
      box.removeAttribute("aria-busy");
      $("run").disabled = false;
      for (const b of $("starters").querySelectorAll("button")) b.disabled = false;
    }
  }

  // ---- uploads ----------------------------------------------------------------
  function renderDrop() {
    const u = S.uploads, known = S.uploadsKnown, on = !!(u && u.enabled);
    $("byoOn").hidden = !(known && on);
    $("byoOff").hidden = !(known && !on);
    $("add").hidden = !known;
    $("addTitle").textContent = on ? "Add your own graph" : "Bring your own graph";
    $("addLead").textContent = on
      ? "Turtle, N-Triples, N-Quads, TriG, JSON-LD, RDF/XML, Jelly or a ready .rete. The server builds it, keeps it as a graph and opens it for you."
      : "Your own .rete file does not have to be on this server. Open it in the browser, or run a server of your own.";
    if (on) $("dropHint").textContent = "Up to " + u.max_mb + " MB (RDF text up to " + u.rdf_text_max_mb + " MB).";
    renderConnect();
  }
  async function upload(file) {
    if (!S.open) { toast("Enter the token first.", true); throw new Error("no token"); }
    if (S.uploads && !S.uploads.enabled) { const m = S.uploads.note || "This server stores no uploads."; toast(m, true); throw new Error(m); }
    const drop = $("drop"); drop.classList.add("busy");
    $("dropTitle").textContent = "Building " + file.name + "…";
    try {
      const out = await api("/api/uploads?name=" + encodeURIComponent(file.name), {
        method: "POST", headers: { "Content-Type": "application/octet-stream" }, body: file });
      S.datasets = await api("/api/datasets");
      renderStats(); renderList();
      await select(out.key);
      toast("Added " + file.name + (out.statements != null ? " (" + num(out.statements) + " statements)" : ""));
      goto("query");
      setQuery(out.query); await run();
      return out;
    } catch (e) {
      toast(e.message, true);
      throw e;
    } finally {
      drop.classList.remove("busy"); $("dropTitle").textContent = "Drop a file, or click to choose one";
    }
  }
  function wireDrop() {
    let depth = 0;
    const live = () => !!(S.uploads && S.uploads.enabled && S.open);
    const hasFiles = (e) => e.dataTransfer && Array.prototype.indexOf.call(e.dataTransfer.types || [], "Files") >= 0;
    window.addEventListener("dragenter", (e) => { if (!hasFiles(e) || !live()) return; depth++; $("overlay").hidden = false; });
    window.addEventListener("dragleave", (e) => { if (!hasFiles(e) || !live()) return; depth = Math.max(0, depth - 1); if (!depth) $("overlay").hidden = true; });
    window.addEventListener("dragover", (e) => { if (hasFiles(e) && live()) e.preventDefault(); });
    window.addEventListener("drop", (e) => {
      if (!hasFiles(e) || !live()) return;
      e.preventDefault(); depth = 0; $("overlay").hidden = true;
      const f = e.dataTransfer.files && e.dataTransfer.files[0];
      if (f) upload(f).catch(() => {});
    });
    $("file").addEventListener("change", (e) => {
      const f = e.target.files && e.target.files[0];
      e.target.value = "";
      if (f) upload(f).catch(() => {});
    });
    $("drop").addEventListener("keydown", (e) => { if (e.key === "Enter" || e.key === " ") { e.preventDefault(); $("file").click(); } });
  }

  // ---- connect an agent -------------------------------------------------------
  function renderConnect() {
    const base = location.origin, need = !!S.token || S.needsToken;
    const shown = $("reveal").checked && S.token ? S.token : "<token>";
    const plainUrl = base + "/mcp";
    const urlTok = plainUrl + "?token=" + shown;
    $("tokenOpts").hidden = !need;
    $("queryTab").hidden = !need;
    const url = need && S.mode === "query" ? urlTok : plainUrl;
    $("mcpUrl").textContent = url;
    $("mcpCmd").textContent = "claude mcp add --transport http rete " + url +
      (need && S.mode === "header" ? ' --header "Authorization: Bearer ' + shown + '"' : "");
    // claude.ai and ChatGPT cannot set a header, so with a token they need it in the URL.
    const web = need ? urlTok : plainUrl;
    $("cUrl").textContent = web; $("gUrl").textContent = web;
    $("claudeAiNote").textContent = need ? "claude.ai cannot send a header, so the token goes in the URL. The server accepts that only when it was started with RETE_QUERY_TOKEN=1."
      : "No sign-in needed. Leave the OAuth fields empty.";
    $("chatgptNote").textContent = need ? "Same as claude.ai: the token goes in the URL, and the server needs RETE_QUERY_TOKEN=1."
      : "Choose \"No authentication\". Connectors in developer mode can call the tools right away.";
    $("pyCode").textContent = "import asyncio\nfrom fastmcp import Client\n\nasync def main():\n    async with Client(\"" + plainUrl + "\"" +
      (need ? ", auth=\"" + shown + "\"" : "") + ") as client:\n        tools = await client.list_tools()\n        print([t.name for t in tools])\n" +
      "        rows = await client.call_tool(\"sparql_query\", {\n            \"dataset\": \"" + (S.selected || (featured()[0] || S.datasets[0] || {}).key || "<dataset>") + "\",\n            \"query\": \"SELECT ?s ?p ?o WHERE { ?s ?p ?o } LIMIT 5\"})\n        print(rows.data)\n\nasyncio.run(main())\n";
    $("connectNote").textContent = S.mode === "query"
      ? "For clients that cannot set a header. The server accepts this only when started with RETE_QUERY_TOKEN=1. The token ends up in logs and history on the client side, so prefer the header."
      : "The header form works with any MCP client that can send headers.";
    const up = base + "/api/uploads?name=file.ttl";
    $("curlCode").textContent = "curl --data-binary @file.ttl \"" + up + "\"" + (need ? " \\\n  -H \"Authorization: Bearer $RETE_TOKEN\"" : "");
    const w = window.reteWebMCP;
    $("webmcpNote").textContent = w && w.supported
      ? "This page also offers " + w.tools.length + " WebMCP tools to the browser's own agent."
      : "In a browser with WebMCP this page offers its graphs and queries as tools to the browser's agent.";
  }
  function wireConnect() {
    const tabs = [...$("agentTabs").querySelectorAll("button")];
    const open = (b, focus) => {
      for (const o of tabs) {
        const on = o === b;
        o.setAttribute("aria-selected", String(on)); o.tabIndex = on ? 0 : -1;
        $("pane-" + o.dataset.pane).hidden = !on;
      }
      if (focus) b.focus();
    };
    for (const b of tabs) {
      b.addEventListener("click", () => open(b));
      b.addEventListener("keydown", (e) => {
        const i = tabs.indexOf(b);
        if (e.key === "ArrowRight") { e.preventDefault(); open(tabs[(i + 1) % tabs.length], true); }
        else if (e.key === "ArrowLeft") { e.preventDefault(); open(tabs[(i + tabs.length - 1) % tabs.length], true); }
        else if (e.key === "Home") { e.preventDefault(); open(tabs[0], true); }
        else if (e.key === "End") { e.preventDefault(); open(tabs[tabs.length - 1], true); }
      });
    }
    for (const b of $("connectTabs").querySelectorAll("button")) b.addEventListener("click", () => {
      S.mode = b.dataset.mode;
      for (const o of $("connectTabs").querySelectorAll("button")) o.setAttribute("aria-pressed", String(o === b));
      renderConnect();
    });
    $("reveal").addEventListener("change", renderConnect);
    for (const b of document.querySelectorAll("[data-copy]")) b.addEventListener("click", async () => {
      const text = $(b.dataset.copy).textContent;
      try { await navigator.clipboard.writeText(text); toast("Copied."); } catch (_e) { toast("Select the text and copy it.", true); }
    });
  }

  // ---- hero: "Query them from ..." --------------------------------------------
  function rotator() {
    const box = $("rot");
    if (!box) return;
    let i = 0, word = box.querySelector(".rot-word");
    const fit = (w) => { box.style.width = (w.getBoundingClientRect().width + 6) + "px"; };
    fit(word);
    if (reduced()) { box.classList.add("still"); return; }
    setInterval(() => {
      if (document.hidden) return;
      i = (i + 1) % HEARD_OF.length;
      const next = h("span", { class: "rot-word in" }, HEARD_OF[i]);
      next.style.position = "absolute"; next.style.left = "0"; next.style.top = "0";
      word.classList.add("out");
      box.append(next);
      fit(next);
      const old = word;
      setTimeout(() => { old.remove(); next.style.position = ""; next.classList.remove("in"); }, 460);
      word = next;
    }, 2200);
  }

  // ---- boot -------------------------------------------------------------------
  async function boot() {
    window.__homeBooting = true;
    S.token = readToken();
    $("gateForm").addEventListener("submit", (e) => {
      e.preventDefault();
      const t = $("tokenInput").value.trim();
      if (!t) return;
      saveToken(t); $("tokenInput").value = ""; start();
    });
    $("signOut").addEventListener("click", () => { saveToken(""); S.open = false; S.datasets = []; showGate("Token forgotten. Enter one to continue."); });
    $("search").addEventListener("input", (e) => { S.filter = e.target.value; renderList(); });
    $("moreBtn").addEventListener("click", () => { S.showAll = true; renderList(); });
    $("run").addEventListener("click", () => run().catch(() => {}));
    $("q").addEventListener("keydown", (e) => { if ((e.ctrlKey || e.metaKey) && e.key === "Enter") run().catch(() => {}); });
    $("q").addEventListener("input", () => { for (const c of $("starters").children) c.setAttribute("aria-pressed", "false"); });
    const top = $("top");
    const onScroll = () => top.classList.toggle("solid", scrollY > 20);
    addEventListener("scroll", onScroll, { passive: true }); onScroll();
    wireDrop(); wireConnect(); rotator(); renderConnect();
    try { S.health = await (await fetch("/health")).json(); } catch (_e) { S.health = null; }
    renderStats();
    await start();
    window.__homeBooting = false;
    window.dispatchEvent(new CustomEvent("rete:home-ready"));
  }

  window.reteHome = {
    state: S, api, select, run, setQuery, upload, h, clip, bytes, num, toast, renderConnect,
    reload: async () => { S.datasets = await api("/api/datasets"); renderStats(); renderList(); return S.datasets; },
    filtered, words,
  };
  boot();
})();
