// Standalone viewer: viewer.html?src=<url>&name=<label>&host=hf|zenodo|url|github
//   ?page=<id>  a web page's structured data (background.js readPage -> chrome.storage.session), as a reader view and
//               a .rete built in this tab (lib/page-data.js)
//   a .pmtiles URL or file opens the PMTiles map viewer (lib/maps.js)
//   &theme=dark|light  &q=<example index>  &view=table|cards|graph  &pick=1 (landing page)
// No src: a landing page with a URL field, a file picker / drag-and-drop for local files (never uploaded)
// and the recently opened URLs. Same components as the host-page integrations (lib/explorer.js).
(function () {
  const { h, safeUrl, icon, bytes } = window.RetelensFmt;
  const q = new URLSearchParams(location.search);
  const theme = q.get("theme");
  const dark = theme ? theme === "dark" : matchMedia("(prefers-color-scheme: dark)").matches;
  document.documentElement.classList.toggle("dark", dark);
  const app = document.getElementById("app");
  const REPO = "https://github.com/caviri/rete";
  const { Explorer, Session } = window.RetelensExplorer;

  // ---------------------------------------------------------------- source description
  // where a URL lives, where to download it, and a page to open it on
  function describe(src) {
    let m;
    const clean = src.split("?")[0];
    if ((m = clean.match(/^https:\/\/huggingface\.co\/datasets\/([^/]+)\/([^/]+)\/resolve\/([^/]+)\/(.+)$/)))
      return { where: { label: `${m[1]}/${m[2]}`, href: `https://huggingface.co/datasets/${m[1]}/${m[2]}` }, download: clean + "?download=true", page: { label: "Open on Hugging Face", href: `https://huggingface.co/datasets/${m[1]}/${m[2]}/blob/${m[3]}/${m[4]}` } };
    if ((m = clean.match(/^https:\/\/huggingface\.co\/buckets\/([^/]+)\/([^/]+)\/resolve\/(.+)$/)))
      return { where: { label: `${m[1]}/${m[2]}`, href: `https://huggingface.co/buckets/${m[1]}/${m[2]}` }, download: clean + "?download=true", page: { label: "Open on Hugging Face", href: `https://huggingface.co/buckets/${m[1]}/${m[2]}/tree/${m[3]}` } };
    if ((m = clean.match(/^https:\/\/raw\.githubusercontent\.com\/([^/]+)\/([^/]+)\/(.+)$/))) {
      const rest = m[3].split("/");
      const ref = rest[0] === "refs" ? rest[2] : rest[0];
      return { where: { label: `${m[1]}/${m[2]} @ ${ref}`, href: `https://github.com/${m[1]}/${m[2]}/tree/${rest[0] === "refs" ? rest.slice(0, 3).join("/") : rest[0]}` }, download: `https://github.com/${m[1]}/${m[2]}/raw/${m[3]}`, page: { label: "Open on GitHub", href: `https://github.com/${m[1]}/${m[2]}/blob/${m[3]}` } };
    }
    if ((m = clean.match(/^https:\/\/github\.com\/([^/]+)\/([^/]+)\/raw\/(.+)$/)))
      return describe(`https://raw.githubusercontent.com/${m[1]}/${m[2]}/${m[3]}`);
    if ((m = clean.match(/^https:\/\/zenodo\.org\/records\/(\d+)\/files\/(.+)$/)))
      return { where: { label: `Zenodo record ${m[1]}`, href: `https://zenodo.org/records/${m[1]}` }, download: clean + "?download=1", page: { label: "Open on Zenodo", href: `https://zenodo.org/records/${m[1]}` } };
    try { const u = new URL(src); return { where: { label: u.host, href: u.origin }, download: src, page: null }; } catch (_) { return { where: null, download: src, page: null }; }
  }

  // ---------------------------------------------------------------- top bar
  const badge = (t, hidden) => h("span", { class: "rounded-full border px-2 text-xs text-gray-500 dark:border-gray-700", hidden: !!hidden }, t);
  function downloadLink(href, label) {
    const a = document.createElement("a");
    a.setAttribute("href", href); // validated http(s) by the caller
    a.setAttribute("download", "");
    a.setAttribute("rel", "noopener noreferrer");
    a.className = "flex items-center gap-1.5 rounded-md border border-gray-800 bg-gray-900 px-2.5 py-1 text-sm font-medium text-white hover:bg-gray-700 dark:border-gray-200 dark:bg-gray-100 dark:text-gray-900 dark:hover:bg-gray-300";
    a.append(icon("download"), document.createTextNode(label));
    // a real download even when a redirect rule would turn the URL into a preview: ask the service worker
    a.addEventListener("click", (e) => {
      if (!(window.chrome && chrome.runtime && chrome.runtime.sendMessage)) return;
      e.preventDefault();
      chrome.runtime.sendMessage({ retelens: "download", url: href });
    });
    return a;
  }
  function topBar(name, desc, local) {
    const fmt = badge("", true), size = badge("", true), hash = badge("", true);
    const mark = h("a", { href: REPO, class: "mr-1 flex items-center gap-1 rounded-md border px-1.5 text-sm font-semibold hover:no-underline dark:border-gray-700", title: "rete on GitHub" }, icon("graphChip"), "rete");
    const left = h("div", { class: "flex min-w-0 flex-wrap items-center gap-2" }, mark, h("span", { class: "break-all font-mono text-base font-semibold" }, name),
      local ? badge("local file") : desc.where ? h("a", { href: desc.where.href, class: "text-sm text-gray-500 hover:text-blue-600" }, desc.where.label) : null, fmt, size, hash);
    const right = h("div", { class: "ml-auto flex flex-wrap items-center gap-2" });
    if (!local && safeUrl(desc.download)) {
      right.append(downloadLink(desc.download, "Download"));
      const copy = h("button", { type: "button", class: "btn rounded-md border px-2 py-1 text-sm" }, "Copy link");
      copy.addEventListener("click", async () => { try { await navigator.clipboard.writeText(desc.download); copy.textContent = "Copied"; setTimeout(() => (copy.textContent = "Copy link"), 1200); } catch (_) {} });
      right.append(copy);
      if (desc.page) right.append(h("a", { href: desc.page.href, class: "btn rounded-md border px-2 py-1 text-sm hover:no-underline" }, desc.page.label));
    }
    const el = h("header", { class: "flex flex-wrap items-center gap-x-4 gap-y-2 border-b border-gray-100 bg-gradient-to-t from-gray-50 to-white px-4 py-2 dark:border-gray-800 dark:from-gray-950 dark:to-gray-950", "data-retelens-topbar": "" }, left, right);
    return {
      el,
      update(meta) {
        const f = meta?.card?.format_version ?? meta?.format;
        if (f != null) { fmt.textContent = "format 0x0" + Number(f).toString(16); fmt.hidden = false; }
        if (meta?.contentHash) { hash.textContent = "hash " + String(meta.contentHash).slice(0, 8); hash.title = "blake3-16 " + meta.contentHash; hash.hidden = false; }
        const len = meta?.stats?.fileLength;
        if (len) { size.textContent = bytes(len); size.hidden = false; }
      },
    };
  }

  // ---------------------------------------------------------------- recents (title + URL only)
  async function saveRecent(title, url) {
    try {
      const { recents } = await chrome.storage.local.get("recents");
      const list = [{ title, url }, ...(Array.isArray(recents) ? recents : []).filter((r) => r.url !== url)].slice(0, 10);
      await chrome.storage.local.set({ recents: list });
    } catch (_) {}
  }

  // ---------------------------------------------------------------- landing
  async function landing() {
    document.title = "Retelens";
    app.textContent = "";
    const input = h("input", { type: "url", placeholder: "Paste a .rete URL", class: "w-full rounded-md border bg-transparent px-2 py-1.5 font-mono text-sm dark:border-gray-700", "aria-label": "URL of a .rete file" });
    const open = (v) => {
      v = v.trim().replace("/blob/", "/resolve/");
      if (!safeUrl(v)) { input.setCustomValidity("Needs an http(s) URL"); input.reportValidity(); return; }
      const name = decodeURIComponent(new URL(v).pathname.split("/").pop() || "graph.rete");
      location.search = "?" + new URLSearchParams({ src: v, name, host: "url" });
    };
    input.addEventListener("input", () => input.setCustomValidity(""));
    input.addEventListener("keydown", (e) => { if (e.key === "Enter") open(input.value); });
    const file = h("input", { type: "file", class: "hidden", id: "rl-file", "aria-label": "Choose a .rete file" });
    file.addEventListener("change", () => file.files[0] && startLocal(file.files[0]));
    const choose = h("button", { type: "button", class: "btn rounded-md border px-3 py-1.5 text-sm font-medium", onclick: () => file.click() }, "Choose a file…");
    const drop = h("div", { class: "mt-5 flex flex-col items-center gap-2 rounded-lg border border-dashed px-4 py-10 text-center text-sm text-gray-500 dark:border-gray-700", id: "rl-drop" },
      h("div", null, "Drop a .rete or .pmtiles file here"), h("div", { class: "text-xs" }, "It is read in place, in this tab. Nothing is uploaded."), choose, file);
    const recents = h("ul", { class: "mt-5 divide-y rounded-lg border dark:divide-gray-800 dark:border-gray-800", hidden: true });
    app.append(h("div", { class: "mx-auto w-full max-w-2xl px-4 py-10" },
      h("div", { class: "mb-4 flex items-center gap-2" }, h("a", { href: REPO, class: "flex items-center gap-1 rounded-md border px-1.5 text-sm font-semibold hover:no-underline dark:border-gray-700" }, icon("graphChip"), "rete"), h("h1", { class: "text-xl font-semibold" }, "Preview a .rete file")),
      h("div", { class: "flex gap-2" }, input, h("button", { type: "button", class: "btn rounded-md border px-3 py-1.5 text-sm font-medium", onclick: () => open(input.value) }, "Open")),
      drop, h("h2", { class: "mt-6 text-xs font-semibold uppercase tracking-wide text-gray-400" }, "Recent"), recents));
    try {
      const { recents: list } = await chrome.storage.local.get("recents");
      for (const r of (Array.isArray(list) ? list : []).slice(0, 8)) {
        if (!safeUrl(r.url)) continue;
        recents.hidden = false;
        recents.append(h("li", null, h("a", { class: "block px-3 py-2 hover:bg-gray-50 hover:no-underline dark:hover:bg-gray-900", href: location.pathname + "?" + new URLSearchParams({ src: r.url, name: r.title || "", host: "url" }) },
          h("div", { class: "text-sm" }, r.title || r.url), h("div", { class: "truncate font-mono text-xs text-gray-500" }, r.url))));
      }
    } catch (_) {}
    if (q.get("pick") === "1") choose.focus();
  }

  // ---------------------------------------------------------------- a file (URL or local)
  let engine = null;
  function show(name, desc, local, files) {
    document.title = name + " - Retelens";
    app.textContent = "";
    if (engine && engine.t && engine.t.terminate) engine.t.terminate();
    engine = new RetelensEngine();
    const bar = topBar(name, desc, local);
    const body = h("div", { class: "flex min-h-0 flex-1 flex-col md:h-[calc(100vh-49px)]" });
    app.append(bar.el, body);
    const session = new Session(engine, files);
    session.onChange(() => bar.update(session.meta));
    const ex = new Explorer({ session, mode: "studio", urlKeys: local ? [] : ["q", "view"], topbar: true });
    body.append(ex.el);
    window.__retelens = { ex, session, engine, ready: false };
    ex.start().then(() => { window.__retelens.ready = true; }, (e) => { window.__retelens.ready = true; window.__retelens.error = String(e); });
  }
  function startLocal(file) {
    if (/\.pmtiles$/i.test(file.name)) return startPmtiles(file.name, { where: null, download: null }, true, (kit) => new kit.pmtiles.FileSource(file));
    show(file.name, { where: null, download: null }, true, [{ path: file.name, local: file, label: file.name, lazy: q.get("lazy") === "1" }]);
  }
  function startUrl(src) {
    const name = q.get("name") || decodeURIComponent(src.split("?")[0].split("/").pop() || "graph.rete");
    if (!safeUrl(src)) { app.append(h("div", { class: "m-4 rounded-lg border border-red-200 bg-red-50 p-3 text-sm text-red-800" }, "src must be an http(s) URL.")); return; }
    saveRecent(name, src);
    const desc = describe(src);
    if (/\.pmtiles$/i.test(src.split(/[?#]/)[0])) return startPmtiles(name, desc, false, null, src);
    show(name, desc, false, [{ path: name, src, label: name, download: desc.download }]);
  }

  // drag and drop anywhere on the page
  addEventListener("dragover", (e) => { if (e.dataTransfer && [...e.dataTransfer.types].includes("Files")) e.preventDefault(); });
  addEventListener("drop", (e) => { const f = e.dataTransfer && e.dataTransfer.files[0]; if (f) { e.preventDefault(); startLocal(f); } });

  // ---------------------------------------------------------------- a PMTiles archive (URL or local file)
  function startPmtiles(name, desc, local, source, src) {
    document.title = name + " - Retelens";
    app.textContent = "";
    const bar = topBar(name, desc, local);
    window.__retelens = { pmtiles: null, ready: false };
    const v = window.RetelensMaps.PmtilesViewer(src || null, { source, onReady: (t) => { window.__retelens = { pmtiles: t.header, layers: (t.meta && t.meta.vector_layers || []).map((l) => l.id), ready: true }; } });
    app.append(bar.el, v.el);
  }

  // ---------------------------------------------------------------- a page's structured data
  const pageTab = q.get("tab") === "explore" ? "explore" : "reader";
  async function startPage(id) {
    document.title = "Page data - Retelens";
    app.textContent = "";
    const fail = (title, ...lines) => { app.append(h("div", { class: "mx-auto w-full max-w-2xl px-4 py-10" }, h("h1", { class: "mb-3 text-xl font-semibold" }, title), ...lines.map((l) => h("p", { class: "mb-2 text-sm text-gray-600 dark:text-gray-400" }, l)))); window.__retelens = { page: null, ready: true, error: title }; };
    let x = null;
    try { const st = await chrome.storage.session.get("page:" + id); x = st["page:" + id] || null; } catch (_) {}
    if (!x) return fail("This page's data is gone", "Retelens keeps what it read from a page only while the browser is open, and only the last few pages. Read the page again: right-click it and choose Read this page's data.");
    if (x.error) return fail("Retelens could not read this page", (x.url ? x.url + ": " : "") + x.error, "Browser pages (chrome://, brave://), the Chrome Web Store and files such as PDFs cannot be read by extensions.");
    const P = window.RetelensPage;
    const { nodes, sources, notes } = P.toDocs(x);
    const pageUrl = safeUrl(x.url);
    let host = ""; try { host = new URL(x.url).host; } catch (_) {}
    document.title = (x.title || host || "Page") + " - data - Retelens";

    // build: everything at once; when the engine refuses that, find the documents it refuses and keep the rest
    engine = new RetelensEngine();
    let built = null, refused = 0;
    const docsOf = (list) => JSON.stringify(list.map((it) => it.doc));
    if (nodes.length) {
      try { built = await engine.buildJsonld(docsOf(nodes)); }
      catch (_) {
        const ok = [];
        for (const it of nodes) { try { await engine.buildJsonld(JSON.stringify([it.doc])); ok.push(it); } catch (e) { it.error = String(e.message || e); refused++; } }
        if (ok.length) { try { built = await engine.buildJsonld(docsOf(ok)); } catch (e) { built = null; } }
      }
    }
    const good = nodes.filter((it) => !it.error);
    const slug = (host + "-" + (x.title || "page")).toLowerCase().replace(/[^a-z0-9.]+/g, "-").replace(/^-+|-+$/g, "").slice(0, 80) || "page";
    const blobLink = (data, type, file, label, primary) => {
      const a = document.createElement("a");
      a.href = URL.createObjectURL(new Blob([data], { type }));
      a.download = file;
      a.className = primary ? "flex items-center gap-1.5 rounded-md border border-gray-800 bg-gray-900 px-2.5 py-1 text-sm font-medium text-white hover:bg-gray-700 dark:border-gray-200 dark:bg-gray-100 dark:text-gray-900 dark:hover:bg-gray-300" : "btn rounded-md border px-2 py-1 text-sm hover:no-underline";
      a.setAttribute("data-rl", primary ? "download-rete" : "download-jsonld");
      a.append(icon("download"), document.createTextNode(label));
      return a;
    };

    // top bar
    const mark = h("a", { href: REPO, class: "mr-1 flex items-center gap-1 rounded-md border px-1.5 text-sm font-semibold hover:no-underline dark:border-gray-700", title: "rete on GitHub" }, icon("graphChip"), "rete");
    const right = h("div", { class: "ml-auto flex flex-wrap items-center gap-2" });
    if (built) right.append(blobLink(built.bytes, "application/octet-stream", slug + ".rete", "Download .rete", true));
    if (good.length) right.append(blobLink(JSON.stringify(good.map((it) => it.doc), null, 2), "application/ld+json", slug + ".jsonld", "JSON-LD", false));
    if (pageUrl) right.append(h("a", { href: pageUrl, class: "btn rounded-md border px-2 py-1 text-sm hover:no-underline" }, "Open the page"));
    const count = badge("", true);
    const top = h("header", { class: "flex flex-wrap items-center gap-x-4 gap-y-2 border-b border-gray-100 bg-gradient-to-t from-gray-50 to-white px-4 py-2 dark:border-gray-800 dark:from-gray-950 dark:to-gray-950", "data-retelens-topbar": "" },
      h("div", { class: "flex min-w-0 flex-wrap items-center gap-2" }, mark, h("span", { class: "font-semibold" }, x.title || host || "Page"), pageUrl ? h("a", { href: pageUrl, class: "text-sm text-gray-500 hover:text-blue-600", title: pageUrl }, host) : null, built ? badge(".rete " + bytes(built.size)) : null, count), right);

    // tabs: the reader, and the explorer over the .rete built from the page
    const readerPane = h("div", { class: "rl-page-pane", role: "tabpanel" });
    const explorePane = h("div", { class: "flex min-h-0 flex-1 flex-col md:h-[calc(100vh-90px)]", role: "tabpanel", hidden: true });
    let ex = null;
    const tabBtn = (id, label) => h("button", { type: "button", class: "rl-tab", "data-tab": id, "aria-selected": "false", role: "tab", onclick: () => setTab(id) }, label);
    const tabs = h("nav", { class: "rl-tabs", role: "tablist" }, tabBtn("reader", "Reader"), tabBtn("explore", "Explore the graph"));
    const setTab = (id) => {
      for (const b of tabs.children) b.setAttribute("aria-selected", String(b.dataset.tab === id));
      readerPane.hidden = id !== "reader"; explorePane.hidden = id !== "explore";
      const u = new URL(location.href); if (id === "explore") u.searchParams.set("tab", "explore"); else u.searchParams.delete("tab"); history.replaceState(null, "", u);
      if (id === "explore" && !ex && built) {
        const session = new Session(engine, [{ path: slug + ".rete", bytes: built.bytes, label: slug + ".rete", examples: P.EXAMPLES }]);
        ex = new Explorer({ session, mode: "studio", urlKeys: [], topbar: true });
        explorePane.append(ex.el);
        window.__retelens.ex = ex; window.__retelens.session = session;
        ex.start().then(() => { window.__retelens.exploreReady = true; }, (e) => { window.__retelens.exploreReady = true; window.__retelens.error = String(e); });
      }
      if (id === "explore" && !built && !explorePane.childNodes.length) explorePane.append(h("p", { class: "p-6 text-sm text-gray-500" }, "Nothing on this page could be turned into statements."));
    };

    // what was read and built
    const report = h("section", { class: "rl-r-report" });
    const srcList = h("ul", { class: "rl-r-sources" }, sources.map((sx) => h("li", null, h("b", null, sx.label), ": ",
      sx.ok ? (sx.tags != null ? sx.tags + " tags" : sx.nodes + " " + (sx.nodes === 1 ? "thing" : "things")) : h("span", { class: "rl-err" }, sx.error))));
    report.append(h("h2", { class: "rl-r-h" }, "What was read"),
      sources.length ? srcList : h("p", null, "No JSON-LD, microdata or meta tags on this page."),
      built ? h("p", null, "Built a .rete of " + bytes(built.size) + " in this tab from " + good.length + " " + (good.length === 1 ? "document" : "documents") + (refused ? " (" + refused + " refused by the engine)" : "") + ". Nothing was sent anywhere: the file stays in this tab until you download it.") : null,
      ...notes.map((n) => h("p", { class: "rl-muted" }, n)),
      ...nodes.filter((it) => it.error).slice(0, 5).map((it) => h("p", { class: "rl-err" }, it.source + ": " + it.error)));
    readerPane.append(P.Reader(nodes), report);
    app.append(top, tabs, readerPane, explorePane);
    window.__retelens = { page: { nodes: nodes.length, sources, built: built ? built.size : 0, good: good.length }, ready: true };
    if (built) engine.openBytes(built.bytes).then(() => engine.run("SELECT (COUNT(*) AS ?n) WHERE { ?s ?p ?o }")).then((r) => engine.page(r.handle, 0, 1)).then((pg) => {
      const n = pg.rows[0] && pg.rows[0].n ? Number(pg.rows[0].n.value) : null;
      if (n != null) { count.textContent = n.toLocaleString("en-US") + " statements"; count.hidden = false; window.__retelens.page.statements = n; }
    }).catch(() => {});
    setTab(pageTab);
  }

  const src = q.get("src");
  const pageId = q.get("page");
  if (pageId) startPage(pageId); else if (src) startUrl(src); else landing();
})();
