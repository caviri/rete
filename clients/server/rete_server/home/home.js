/* rete-server home page. No build step, no dependencies.
   The shell is public; every fetch below carries the bearer token, so all data stays behind it.
   window.reteHome is what tools.js (WebMCP) and the tests drive. */
(() => {
  "use strict";
  const $ = (id) => document.getElementById(id);
  const TOKEN_KEY = "rete.home.token";
  const ROW_CAP = 200;
  const GENERIC = [
    { title: "Sample triples", sparql: "SELECT ?s ?p ?o WHERE { ?s ?p ?o } LIMIT 20" },
    { title: "Classes", sparql: "SELECT ?class (COUNT(?s) AS ?n) WHERE { ?s a ?class } GROUP BY ?class ORDER BY DESC(?n) LIMIT 20" },
    { title: "Predicates", sparql: "SELECT ?p (COUNT(*) AS ?n) WHERE { ?s ?p ?o } GROUP BY ?p ORDER BY DESC(?n) LIMIT 20" },
  ];

  const S = {
    token: "", open: false, datasets: [], filter: "", selected: null, entry: null, schema: null,
    examples: [], uploads: null, health: null, last: null, mode: "header", busy: false,
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
  const num = (n) => (typeof n === "number" ? n.toLocaleString("en-US") : String(n));
  function bytes(n) {
    if (!(n >= 0)) return "";
    const u = ["B", "KB", "MB", "GB", "TB"]; let i = 0;
    while (n >= 1024 && i < u.length - 1) { n /= 1024; i++; }
    return (i ? n.toFixed(n < 10 ? 1 : 0) : n) + " " + u[i];
  }
  let toastTimer = null;
  function toast(text, bad) {
    const el = $("toast");
    el.textContent = text; el.className = "toast" + (bad ? " bad" : ""); el.hidden = false;
    clearTimeout(toastTimer);
    toastTimer = setTimeout(() => { el.hidden = true; }, bad ? 9000 : 4500);
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
      if (r.status === 401) { S.open = false; showGate(S.token ? "That token was not accepted." : ""); }
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
    $("gate").hidden = true;
    try {
      const list = await api("/api/datasets");
      S.open = true; S.datasets = list;
    } catch (e) {
      if (e.status === 401) return;
      $("gate").hidden = true; $("app").hidden = false;
      toast(e.message, true); return;
    }
    $("app").hidden = false;
    $("signOut").hidden = !S.token;
    api("/api/uploads").then((u) => { S.uploads = u; renderDrop(); }).catch(() => { S.uploads = null; renderDrop(); });
    renderStats(); renderList(); renderConnect(); renderStarters();
    if (!S.selected && S.datasets.length === 1) select(S.datasets[0].key);
  }

  // ---- stats, list ------------------------------------------------------------
  function renderStats() {
    const v = S.health ? S.health.version : "";
    $("ver").textContent = v ? "v" + v : "";
    $("eyebrow").textContent = "rete-server" + (v ? " v" + v : "");
    const box = $("stats"); box.textContent = "";
    if (S.open) {
      box.append(h("span", {}, num(S.datasets.length) + (S.datasets.length === 1 ? " dataset" : " datasets")));
      if (S.uploads) box.append(h("span", {}, S.uploads.enabled ? "uploads up to " + S.uploads.max_mb + " MB" : "uploads off"));
      box.append(h("span", {}, S.token ? "token set" : "no token"));
    }
  }
  const words = (d) => ((d.key || "") + " " + (d.label || "") + " " + (d.description || "") + " " + (d.tags || []).join(" ")).toLowerCase();
  function filtered() {
    const w = S.filter.toLowerCase().split(/\s+/).filter(Boolean);
    return S.datasets.filter((d) => { const t = words(d); return w.every((x) => t.includes(x)); });
  }
  function renderList() {
    const list = filtered(), ul = $("dlist");
    ul.textContent = "";
    if (!list.length) ul.append(h("li", { class: "empty" }, S.datasets.length ? "Nothing matches." : "No datasets yet. Drop a file below."));
    for (const d of list.slice(0, 500)) {
      const meta = [d.triples != null ? num(d.triples) + " triples" : null, d.size_bytes ? bytes(d.size_bytes) : d.size, d.kind]
        .filter(Boolean);
      ul.append(h("li", { role: "option", "aria-selected": String(d.key === S.selected), "data-key": d.key, tabindex: "0",
        onclick: () => select(d.key), onkeydown: (e) => { if (e.key === "Enter" || e.key === " ") { e.preventDefault(); select(d.key); } } },
      h("span", { class: "t" }, d.label || d.key),
      h("span", { class: "k" }, h("span", {}, clip(d.key, 60)), meta.map((m) => h("span", {}, m)))));
    }
    $("dcount").textContent = list.length > 500 ? "Showing 500 of " + num(list.length) + ". Search to narrow." :
      num(list.length) + (list.length === S.datasets.length ? "" : " of " + num(S.datasets.length)) + " shown";
  }

  // ---- card -------------------------------------------------------------------
  async function select(key, quiet) {
    S.selected = key; S.entry = null; S.schema = null; S.examples = [];
    $("current").textContent = key;
    renderList(); renderCard(); renderStarters();
    const q = (p) => api("/api/datasets/" + enc(key) + p);
    const [entry, schema, examples] = await Promise.allSettled([q(""), q("/schema"), q("/examples")]);
    if (S.selected !== key) return null;
    S.entry = entry.status === "fulfilled" ? entry.value : (S.datasets.find((d) => d.key === key) || { key });
    S.entryError = entry.status === "rejected" ? entry.reason.message : null;
    S.schema = schema.status === "fulfilled" ? schema.value : null;
    S.examples = examples.status === "fulfilled" && Array.isArray(examples.value) ? examples.value : [];
    renderCard(); renderStarters();
    if (!quiet) $("cardPanel").classList.add("flash");
    setTimeout(() => $("cardPanel").classList.remove("flash"), 1700);
    return S.entry;
  }
  function renderCard() {
    const box = $("cardPanel"); box.textContent = "";
    if (!S.selected) { box.append(h("p", { class: "hint" }, "Pick a dataset to see its card.")); return; }
    const e = S.entry, card = (e && e.card) || {};
    const title = card.title || card.name || (e && e.label) || S.selected;
    const desc = card.description || (e && e.description) || "";
    box.append(h("h3", {}, title));
    if (!e) { box.append(h("p", { class: "hint" }, "Reading the card…")); return; }
    if (desc) box.append(h("p", { class: "desc" }, clip(desc, 1200)));
    const info = (S.schema && S.schema.info) || {};
    const lic = card.license || e.license;
    const facts = [
      ["triples", e.triples != null ? num(e.triples) : info.quads != null ? num(info.quads) : null],
      ["terms", info.terms != null ? num(info.terms) : null],
      ["size", e.size_bytes ? bytes(e.size_bytes) : e.size || null],
      ["license", lic ? (typeof lic === "string" ? lic : JSON.stringify(lic)) : null],
      ["source", card.source || e.source || null],
      ["kind", e.kind || null],
    ].filter((f) => f[1]);
    if (facts.length) box.append(h("p", { class: "facts" }, facts.map(([k, v]) => h("span", {}, k + " ", h("b", {}, clip(v, 90))))));
    const classes = (S.schema && S.schema.schema && S.schema.schema.classes) || [];
    if (classes.length) {
      box.append(h("h4", {}, "Classes"));
      box.append(h("div", { class: "classes" }, classes.slice(0, 14).map(([c, n]) =>
        h("code", { title: c }, c.replace(/^.*[#\/]/, "") + " · " + num(n)))));
    }
    const links = [];
    const sp = e.sparql_endpoint || (e.shards ? null : "/sparql/" + S.selected);
    if (sp) links.push(h("a", { href: sp.startsWith("/") ? sp : sp }, "SPARQL endpoint"));
    if (e.url) links.push(h("a", { href: e.url }, ".rete file"));
    if (links.length) box.append(h("p", { class: "links" }, links));
    if (S.entryError) box.append(h("p", { class: "hint" }, S.entryError));
  }

  // ---- query ------------------------------------------------------------------
  function renderStarters() {
    const box = $("starters"); box.textContent = "";
    const list = [];
    for (const ex of S.examples.slice(0, 8)) {
      if (ex && ex.sparql) list.push({ title: ex.title || ex.question || "Example", sparql: ex.sparql });
    }
    for (const g of GENERIC) list.push(g);
    for (const s of list) {
      box.append(h("button", { class: "chip", type: "button", role: "checkbox", "aria-checked": String($("q").value.trim() === s.sparql.trim()),
        onclick: () => { setQuery(s.sparql); run(); } }, clip(s.title, 40)));
    }
  }
  function setQuery(text) {
    $("q").value = text;
    for (const c of $("starters").children) c.setAttribute("aria-checked", "false");
  }
  function cell(v) {
    if (v == null) return h("td", {}, "");
    if (typeof v === "object") return h("td", {}, JSON.stringify(v));
    return h("td", { class: isIri(v) ? "iri" : "" }, String(v));
  }
  function renderResult(res) {
    const box = $("result"); box.textContent = "";
    if (res.kind === "ask" || res.boolean != null && !res.head) {
      box.append(h("p", { class: "verdict" }, res.boolean ? "true" : "false")); return;
    }
    const vars = (res.head && res.head.vars) || [];
    let rows = res.table || [];
    if (vars.length) {
      const total = rows.length;
      const thead = h("thead", {}, h("tr", {}, vars.map((v) => h("th", {}, v))));
      const tbody = h("tbody", {}, rows.slice(0, ROW_CAP).map((r) => h("tr", {}, vars.map((v) => cell(r[v])))));
      box.append(h("div", { class: "table-wrap" }, h("table", {}, thead, tbody)));
      box.append(h("p", { class: "table-foot" }, num(total) + (total === 1 ? " row" : " rows") +
        (total > ROW_CAP ? ", first " + ROW_CAP + " shown" : "") + (res.truncated ? ", the server cut the result" : "")));
    } else if (Array.isArray(res.triples)) {
      const lines = res.triples.slice(0, ROW_CAP).map((t) => Array.isArray(t) ? t.join(" ") + " ." : typeof t === "string" ? t : JSON.stringify(t));
      box.append(h("pre", { class: "code" }, lines.join("\n")));
      box.append(h("p", { class: "table-foot" }, num(res.triples.length) + " triples"));
    } else {
      box.append(h("pre", { class: "code" }, JSON.stringify(res, null, 2)));
    }
    if (res.warnings && res.warnings.length) {
      box.append(h("p", { class: "table-foot" }, "Warning: " + clip(JSON.stringify(res.warnings[0]), 300)));
    }
  }
  async function run(text, dataset) {
    const q = (text != null ? text : $("q").value).trim();
    const key = dataset || S.selected;
    if (!key) { toast("Pick a dataset first.", true); throw new Error("no dataset selected"); }
    if (!q) { toast("The query is empty.", true); throw new Error("query is empty"); }
    if (text != null) $("q").value = text;
    $("run").disabled = true; $("runMeta").textContent = "Running…";
    try {
      const t0 = performance.now();
      const res = await api("/api/query", { method: "POST", headers: { "Content-Type": "application/json" },
        body: JSON.stringify({ dataset: key, query: q, limit: ROW_CAP }) });
      S.last = res;
      renderResult(res);
      const ms = Math.round(performance.now() - t0);
      const st = res.stats || {};
      $("runMeta").textContent = ms + " ms" + (st.bytes != null ? " · " + bytes(st.bytes) + " read in " + st.requests + " requests" : "");
      return res;
    } catch (e) {
      $("result").textContent = ""; $("result").append(h("p", { class: "err" }, e.message));
      $("runMeta").textContent = "";
      throw e;
    } finally { $("run").disabled = false; }
  }

  // ---- uploads ----------------------------------------------------------------
  function renderDrop() {
    const u = S.uploads, off = u && !u.enabled;
    $("drop").classList.toggle("off", !!off);
    $("dropHint").textContent = !u ? "" : off ? u.note : "Up to " + u.max_mb + " MB (RDF text up to " + u.rdf_text_max_mb + " MB).";
    $("overlaySub").textContent = off ? u.note : "";
    renderConnect();
  }
  async function upload(file) {
    if (!S.open) { toast("Enter the token first.", true); throw new Error("no token"); }
    if (S.uploads && !S.uploads.enabled) { toast(S.uploads.note, true); throw new Error(S.uploads.note); }
    const drop = $("drop"); drop.classList.add("busy");
    $("dropTitle").textContent = "Building " + file.name + "…";
    try {
      const out = await api("/api/uploads?name=" + encodeURIComponent(file.name), {
        method: "POST", headers: { "Content-Type": "application/octet-stream" }, body: file });
      S.datasets = await api("/api/datasets");
      renderStats(); renderList();
      await select(out.key);
      toast("Added " + file.name + " as " + out.key + (out.statements != null ? " (" + num(out.statements) + " statements)" : ""));
      $("query").scrollIntoView({ block: "start" });
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
    const hasFiles = (e) => e.dataTransfer && Array.prototype.indexOf.call(e.dataTransfer.types || [], "Files") >= 0;
    window.addEventListener("dragenter", (e) => { if (!hasFiles(e)) return; depth++; $("overlay").hidden = false; });
    window.addEventListener("dragleave", (e) => { if (!hasFiles(e)) return; depth = Math.max(0, depth - 1); if (!depth) $("overlay").hidden = true; });
    window.addEventListener("dragover", (e) => { if (hasFiles(e)) e.preventDefault(); });
    window.addEventListener("drop", (e) => {
      if (!hasFiles(e)) return;
      e.preventDefault(); depth = 0; $("overlay").hidden = true;
      const f = e.dataTransfer.files && e.dataTransfer.files[0];
      if (f) upload(f).catch(() => {});
    });
    $("file").addEventListener("change", (e) => {
      const f = e.target.files && e.target.files[0];
      e.target.value = "";
      if (f) upload(f).catch(() => {});
    });
  }

  // ---- connect an agent -------------------------------------------------------
  function renderConnect() {
    const base = location.origin, need = !!S.token || !S.open;
    const shown = $("reveal").checked && S.token ? S.token : "<token>";
    const hdr = need ? ' --header "Authorization: Bearer ' + (S.token ? shown : "<token>") + '"' : "";
    const url = base + "/mcp" + (S.mode === "query" && need ? "?token=" + shown : "");
    $("mcpUrl").textContent = url;
    $("mcpCmd").textContent = "claude mcp add --transport http rete " + url + (S.mode === "header" ? hdr : "");
    $("queryTab").hidden = !need;
    $("connectNote").textContent = !need ? "This server has no token, so the URL is all an agent needs."
      : S.mode === "query" ? "For clients that cannot set a header. The server accepts this only when started with RETE_QUERY_TOKEN=1. The token ends up in logs and history on the client side, so prefer the header."
      : "The header form works with any MCP client that can send headers.";
    const up = base + "/api/uploads?name=file.ttl";
    $("curlCode").textContent = "curl --data-binary @file.ttl \"" + up + "\"" + (need ? " \\\n  -H \"Authorization: Bearer $RETE_TOKEN\"" : "");
    const w = window.reteWebMCP;
    $("webmcpNote").textContent = w && w.supported
      ? "This page also offers " + w.tools.length + " WebMCP tools to the browser's own agent."
      : "In a browser with WebMCP this page offers its datasets and queries as tools to the browser's agent.";
  }
  function wireConnect() {
    for (const b of $("connectTabs").querySelectorAll("button")) b.addEventListener("click", () => {
      S.mode = b.dataset.mode;
      for (const o of $("connectTabs").querySelectorAll("button")) o.setAttribute("aria-selected", String(o === b));
      renderConnect();
    });
    $("reveal").addEventListener("change", renderConnect);
    for (const b of document.querySelectorAll("[data-copy]")) b.addEventListener("click", async () => {
      const text = $(b.dataset.copy).textContent;
      try { await navigator.clipboard.writeText(text); toast("Copied."); } catch (_e) { toast("Select the text and copy it.", true); }
    });
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
    $("run").addEventListener("click", () => run().catch(() => {}));
    $("q").addEventListener("keydown", (e) => { if ((e.ctrlKey || e.metaKey) && e.key === "Enter") run().catch(() => {}); });
    $("q").addEventListener("input", () => { for (const c of $("starters").children) c.setAttribute("aria-checked", "false"); });
    const top = $("top");
    const onScroll = () => top.classList.toggle("solid", scrollY > 20);
    addEventListener("scroll", onScroll, { passive: true }); onScroll();
    wireDrop(); wireConnect();
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
