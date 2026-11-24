// Hugging Face adapter. Contract: wacz-eval/REPORT.md + hf-dom.json (+ live checks).
//
// Everything is rendered natively in HF's DOM with HF's Tailwind classes (see lib/*.js);
// the only extension frame is the hidden engine iframe that owns the Worker.
// HF is not an SPA: mount() runs once per page. Islands (div.SVELTE_HYDRATER) may
// rehydrate, so we insert siblings of island wrappers; the dataset /viewer and the
// bucket studio have to edit inside their header island and re-apply on mutation.
//
// Untrusted values (card, IRIs, literals) never reach innerHTML or hrefs: h() in
// lib/format.js builds text nodes and refuses non-http(s) URLs. Same-origin links
// are built here from a fixed prefix + encodeURIComponent.
(function () {
  const list = (self.__retelensAdapters = self.__retelensAdapters || []);
  const { h, safeUrl, pathLink, icon } = window.RetelensFmt;
  const { Explorer, Session } = window.RetelensExplorer;

  const ORIGIN = "https://huggingface.co";
  const RE_DATASET_HOME = /^\/datasets\/([^/]+)\/([^/]+)\/?$/;
  const RE_DATASET_VIEWER = /^\/datasets\/([^/]+)\/([^/]+)\/viewer\/?$/;
  const RE_DATASET_TREE = /^\/datasets\/([^/]+)\/([^/]+)\/tree\/([^/]+)(?:\/(.*))?$/;
  const RE_DATASET_BLOB = /^\/datasets\/([^/]+)\/([^/]+)\/blob\/([^/]+)\/(.+\.rete)$/;
  const RE_BUCKET_ROOT = /^\/buckets\/([^/]+)\/([^/]+)\/?$/;
  const RE_BUCKET_TREE = /^\/buckets\/([^/]+)\/([^/]+)\/tree\/(.+)$/;

  const isRete = (p) => /\.rete$/i.test(p.split("?")[0]);
  const encPath = (p) => p.split("/").map(encodeURIComponent).join("/");
  const resolveDataset = (ns, name, rev, path) => `${ORIGIN}/datasets/${ns}/${name}/resolve/${encodeURIComponent(rev)}/${encPath(path)}`;
  const resolveBucket = (ns, name, path) => `${ORIGIN}/buckets/${ns}/${name}/resolve/${encPath(path)}`;

  // Our generated CSS is scoped as `[data-retelens] .cls`, so the marked element must not itself carry
  // utility classes: the marker goes on a plain outer div, the classes on an inner one.
  function wrap(el, cls) {
    const outer = document.createElement("div");
    outer.setAttribute("data-retelens", "");
    const inner = document.createElement("div");
    if (cls) inner.className = cls;
    inner.append(el);
    outer.append(inner);
    return outer;
  }
  function tpl(html) { const t = document.createElement("template"); t.innerHTML = html.trim(); return t.content.firstElementChild; } // static templates only

  async function getJson(url) {
    const r = await fetch(url, { credentials: "same-origin" });
    if (!r.ok) throw new Error(url + " -> " + r.status);
    return r.json();
  }

  // newest 0x08-looking file first: subfolder files (the converted ones) before root files, later names first
  function rank(files) {
    return [...files].sort((a, b) => {
      const da = a.includes("/") ? 1 : 0, db = b.includes("/") ? 1 : 0;
      return db - da || (a < b ? 1 : a > b ? -1 : 0);
    });
  }
  const datasetFiles = (ns, name, paths) => rank(paths).map((p) => { const src = resolveDataset(ns, name, "main", p); return { path: p, src, label: p, download: src + "?download=true" }; });

  // HF's viewer "works" only if it renders a real data table (never decide from its message or status kind)
  const hfShowsTable = async (ctx, root, ms = 2500) => !!(await ctx.waitFor("table, tr[data-row-idx]", ms, root));

  // ----------------------------------------------------------------- chips
  // HF may already show a group with the same label (the repo README can carry its own tags and license).
  // Rule: never add a second group. Tags/Formats: append into HF's group (deduped, case-insensitive);
  // License/Size: if HF shows them, skip ours (the per-file license is in the sidebar).
  const GROUP = "div.mr-1.flex.flex-wrap.items-center";
  function findGroup(row, label) {
    return [...row.querySelectorAll(GROUP)].find((g) => !g.hasAttribute("data-retelens") && g.firstElementChild && g.firstElementChild.textContent.trim() === label) || null;
  }
  function chipEl(it) {
    const a = tpl('<a class="mb-1 mr-1 md:mb-1.5 md:mr-1.5 rounded-lg" data-retelens="chips"><div class="tag tag-white"><span></span></div></a>');
    const tag = a.firstElementChild;
    if (it.icon) tag.prepend(icon(it.icon));
    tag.querySelector("span").textContent = it.text; // text only: card values are untrusted
    return a;
  }
  function chipGroup(label, items) {
    const g = tpl('<div class="mr-1 flex flex-wrap items-center" data-retelens="chips"><span class="mb-1 mr-1 p-1 text-sm leading-tight text-gray-400 md:mb-1.5"></span></div>');
    g.firstElementChild.textContent = label;
    for (const it of items) g.append(chipEl(it));
    return g;
  }
  function addChips(meta) {
    const row = document.querySelector('[data-target="DatasetHeader"] h1 + div.mb-3.flex.flex-wrap');
    if (!row || !meta) return;
    row.querySelectorAll('[data-retelens="chips"]').forEach((n) => n.remove());
    const c = meta.card || {};
    const put = (label, items, how) => {
      const g = findGroup(row, label);
      if (g) { if (how === "append") for (const it of items) g.append(chipEl(it)); return; } // "skip": HF's own group stays
      row.append(chipGroup(label, items));
    };
    put("Formats:", [{ icon: "graphChip", text: c.format_version != null ? `rete (0x0${Number(c.format_version).toString(16)})` : "rete" }], "append");
    const quads = meta.info?.quads ?? c.quad_count;
    if (quads != null) put("Size:", [{ icon: "size", text: new Intl.NumberFormat("en-US").format(quads) + " quads" }], "skip");
    if (c.license) put("License:", [{ icon: "license", text: typeof c.license === "string" ? c.license : String(c.license.name || c.license.id || "see card") }], "skip");
    const kw = Array.isArray(c.keywords) ? c.keywords.map(String) : [];
    if (kw.length) {
      const g = findGroup(row, "Tags:");
      const have = new Set(g ? [...g.querySelectorAll("a span")].map((x) => x.textContent.trim().toLowerCase()) : []);
      const fresh = kw.filter((k) => { const l = k.trim().toLowerCase(); if (!l || have.has(l)) return false; have.add(l); return true; });
      const items = [...fresh.slice(0, 5), ...(fresh.length > 5 ? ["+" + (fresh.length - 5)] : [])].map((t) => ({ text: t }));
      if (items.length) put("Tags:", items, "append");
    }
  }

  // --------------------------------------------------------------- sidebar
  function installSidebar(session) {
    const section = document.querySelector("section.md\\:w-64.lg\\:w-80.xl\\:w-96");
    if (!section) return;
    const body = h("div");
    const holder = wrap(h("div", null, h("h2", { class: "text-smd mb-5 flex items-baseline font-semibold text-gray-800 dark:text-gray-200" }, "Graph card"), body, h("div", { class: "divider-column-vertical" })), "mt-4");
    // after HF's own stat tiles ("Total file size", filled client-side), so HF's order stays intact
    const tiles = [...section.children].find((c) => c.querySelector && c.querySelector("a.pointer-events-none"));
    const dl = [...section.querySelectorAll("dl")].find((d) => /Downloads last month/i.test(d.textContent));
    holder.firstElementChild.firstElementChild.before(h("div", { class: "divider-column-vertical" }));
    if (tiles) tiles.after(holder);
    else {
      let anchor = dl;
      while (anchor && anchor.nextElementSibling && !anchor.classList.contains("divider-column-vertical")) anchor = anchor.nextElementSibling;
      if (anchor && anchor.parentElement === section) anchor.after(holder); else section.append(holder);
    }
    session.onChange(() => window.RetelensCardSidebar.render(body, session.cardData()));
  }

  // --------------------------------------------------- README-style summary
  function summaryNode(session, exampleHref) {
    if (!session.meta) return null;
    const c = session.meta.card;
    const root = h("div", { class: "prose hf-sanitized mb-6 max-w-none rounded-lg border border-gray-100 p-5 dark:border-gray-800" });
    if (!c) {
      const i = session.meta.info || {};
      root.append(h("h1", { class: "relative group flex items-baseline" }, String(session.file?.label || "Graph")));
      root.append(h("p", null, `This .rete file carries no embedded Dataset Card. It holds ${new Intl.NumberFormat("en-US").format(i.quads ?? 0)} quads and ${new Intl.NumberFormat("en-US").format(i.terms ?? 0)} terms.`));
      return root;
    }
    root.append(h("p", { class: "!mt-0 text-sm text-gray-400" }, "Card embedded in " + (session.file?.label || "the .rete file")));
    root.append(h("h1", { class: "relative group flex items-baseline" }, String(c.title || session.file?.label || "Graph")));
    for (const p of String(c.description || "").split(/\n{2,}/).filter(Boolean)) root.append(h("p", null, p));
    const bits = [];
    if (c.source) bits.push(h("span", null, h("strong", null, "Source: "), safeUrl(c.source) ? h("a", { href: c.source }, String(c.source).replace(/^https?:\/\//, "")) : String(c.source)));
    if (c.license) bits.push(h("span", null, h("strong", null, "License: "), window.RetelensCardSidebar.licenseNode(c.license)));
    if (c.source_date) bits.push(h("span", null, h("strong", null, "Source date: "), String(c.source_date)));
    if (bits.length) root.append(h("p", null, bits.flatMap((b, i) => (i ? [" · ", b] : [b]))));
    const ex = session.examples.filter((e) => e.curated).slice(0, 12);
    const all = ex.length ? ex : session.examples.slice(0, 8);
    if (all.length) {
      root.append(h("h2", { class: "relative group flex items-baseline" }, "Example queries"));
      root.append(h("ul", null, all.map((e) => h("li", null, pathLink(exampleHref(session.examples.indexOf(e)), e.title), e.question ? " — " + e.question : ""))));
    }
    return root;
  }

  // ----------------------------------------------------------- studio shell
  // Full-width studio under an HF header. `hideEl` is the page body we replace.
  function studioShell(ctx, headerEl, hideEl) {
    const outer = h("div", { "data-retelens": "" });
    const wrapEl = h("div", { class: "flex flex-col md:overflow-hidden md:h-[var(--rl-h)]" });
    outer.append(wrapEl);
    const setH = () => wrapEl.style.setProperty("--rl-h", Math.max(480, innerHeight - Math.round(headerEl.getBoundingClientRect().height)) + "px");
    setH();
    ctx.on(window, "resize", setH);
    hideEl.after(outer);
    return { wrap: wrapEl, outer, setH };
  }
  function backButton(href, onclick) {
    const a = pathLink(href, icon("back"));
    a.className = "btn mr-2 rounded-md border px-1.5 py-1 text-gray-500 hover:text-gray-700 dark:text-gray-300";
    a.setAttribute("title", "Back");
    a.setAttribute("aria-label", "Back");
    a.setAttribute("data-retelens-back", "");
    if (onclick) a.addEventListener("click", (e) => { e.preventDefault(); onclick(); });
    return a;
  }
  // top bar of a full-width studio: back arrow + breadcrumb (bucket / folders / file)
  function studioBar(base, relPath, backHref, onBack) {
    const parts = relPath.split("/").filter(Boolean);
    const bar = h("div", { class: "flex flex-wrap items-center gap-1.5 border-b px-3 py-2 text-sm dark:border-gray-800", "data-retelens-bar": "" });
    bar.append(backButton(backHref, onBack));
    const crumb = (text, href) => (href ? pathLink(href, text) : h("span", { class: "font-semibold" }, text));
    const sep = () => h("span", { class: "text-gray-300" }, "/");
    bar.append(crumb(base.split("/").slice(-2).join("/"), base));
    let acc = "";
    parts.forEach((seg, i) => {
      acc += (i ? "/" : "") + seg;
      bar.append(sep(), crumb(decodeURIComponent(seg), i < parts.length - 1 ? `${base}/tree/${encPath(decodeURIComponent(acc))}` : null));
    });
    const tag = h("span", { class: "ml-2 rounded-full border px-2 text-xs text-gray-500 dark:border-gray-700" }, "Data Studio");
    bar.append(tag);
    return bar;
  }
  function studioTab(text) {
    const a = document.createElement("a");
    a.className = "tab-alternate active";
    a.setAttribute("data-retelens-tab", "");
    a.append(icon("table"), document.createTextNode(text));
    return a;
  }

  // ----------------------------------------------------------- dataset home
  async function datasetHome(ctx, ns, name) {
    const info = await getJson(`/api/datasets/${ns}/${name}`);
    const paths = (info.siblings || []).map((s) => s.rfilename).filter(isRete);
    if (!paths.length) return; // not a rete dataset: leave the page alone
    const files = datasetFiles(ns, name, paths);
    const session = new Session(ctx.engine(), files);
    const base = `/datasets/${ns}/${name}`;
    const viewerHref = (st) => {
      const q = new URLSearchParams();
      if (st && st.file) q.set("file", st.file);
      if (st && st.q) q.set("q", String(st.q));
      if (st && st.view) q.set("view", st.view);
      if (st && st.console) q.set("console", "1");
      return base + "/viewer" + (q.toString() ? "?" + q : "");
    };

    const island = document.querySelector('[data-target="DatasetViewer"]');
    const noHfViewer = !!island && !(await hfShowsTable(ctx, island));

    installSidebar(session);
    session.onChange(() => {
      addChips(session.meta);
      const box = document.querySelector('[data-retelens="summary-slot"]');
      if (box) { box.textContent = ""; const n = summaryNode(session, (i) => viewerHref({ file: session.file.path, q: i })); if (n) box.append(n); }
    });

    // "No dataset card yet" box -> a README-style summary of the embedded card
    const none = [...document.querySelectorAll("div.border-dotted")].find((d) => /No dataset card yet/i.test(d.textContent));
    if (none) none.before(wrap(h("div"), "")), none.previousElementSibling.setAttribute("data-retelens", "summary-slot");

    if (island && noHfViewer) {
      const ex = new Explorer({ session, mode: "box", urlKeys: ["view"], studioHref: viewerHref });
      ctx.hide(island);
      island.after(wrap(ex.el, "mb-10"));
      // tab bar: Data Studio between Dataset card and Files
      const first = document.querySelector('a.tab-alternate[href$="/tree/main"]')?.parentElement?.querySelector("a.tab-alternate");
      if (first) {
        const tab = pathLink(viewerHref(), icon("table"), document.createTextNode("Data Studio"));
        tab.className = "tab-alternate";
        tab.setAttribute("data-retelens-tab", "");
        first.after(tab);
      }
      await ex.start();
    } else {
      // HF shows its own table (parquet next to .rete): leave it, add a Graph tab that opens our studio
      const first = document.querySelector('a.tab-alternate[href$="/tree/main"]')?.parentElement?.querySelector("a.tab-alternate");
      if (first) {
        const q = new URLSearchParams({ src: files[0].src, name: files[0].label, host: "hf" });
        const tab = tpl("<a class=\"tab-alternate\" target=\"_blank\" rel=\"noopener\" data-retelens-tab></a>");
        tab.setAttribute("href", chrome.runtime.getURL("viewer.html") + "?" + q);
        tab.append(icon("table"), document.createTextNode("Graph"));
        first.after(tab);
      }
      await session.open(0);
    }
  }

  // ------------------------------------------------- dataset /viewer takeover
  async function datasetViewerPage(ctx, ns, name) {
    const dbg = (...a) => console.debug("[retelens] /viewer:", ...a);
    const island = await ctx.waitFor('[data-target="DatasetViewerContent"]');
    if (!island) return dbg("no DatasetViewerContent island");
    if (await hfShowsTable(ctx, island)) return dbg("HF renders a data table; leaving it");
    const info = await getJson(`/api/datasets/${ns}/${name}`);
    const paths = (info.siblings || []).map((s) => s.rfilename).filter(isRete);
    if (!paths.length) return dbg("no .rete files in the repo");
    const header = island.querySelector("header");
    if (!header) return dbg("no header in island");
    const body = header.parentElement.nextElementSibling; // the "not available" body
    if (!body) return dbg("no body after header");
    dbg("taking over, files", paths.length);
    const base = `/datasets/${ns}/${name}`;

    const session = new Session(ctx.engine(), datasetFiles(ns, name, paths));
    ctx.hide(body);
    const shell = studioShell(ctx, header, body);
    const ex = new Explorer({ session, mode: "studio", urlKeys: ["file", "q", "view"] });
    shell.wrap.append(ex.el);

    // header: back control before "Datasets:", Data Studio tab (active) after Dataset card
    const ensure = () => {
      const h1 = header.querySelector("h1");
      if (h1 && !h1.querySelector("[data-retelens-back]")) h1.prepend(backButton(base));
      const bar = header.querySelector("div.-mb-px.flex");
      if (bar && !bar.querySelector("[data-retelens-tab]")) {
        const first = bar.querySelector("a.tab-alternate");
        if (first) first.after(studioTab("Data Studio"));
      }
      ctx.hide(body);
      shell.setH();
    };
    ensure();
    ctx.watch(island, ensure, 100);
    await ex.start();
  }

  // ------------------------------------------------------------------ rows
  function addOpenLink(a, href) {
    const row = a.closest("li, tr");
    if (!row || row.querySelector("[data-retelens-open]")) return;
    const link = pathLink(href, "Open in viewer");
    link.className = "ml-3 whitespace-nowrap text-xs text-blue-600 hover:underline dark:text-blue-400";
    link.setAttribute("data-retelens-open", "");
    link.setAttribute("data-retelens", "");
    a.after(link);
  }
  async function listLike(ctx, listSel, rowSel, hrefFor, onScan) {
    const root = await ctx.waitFor(listSel);
    if (!root) return null;
    const scan = () => {
      const found = [];
      root.querySelectorAll(rowSel).forEach((a) => {
        const href = a.getAttribute("href") || "";
        if (!isRete(href)) return;
        found.push(href);
        const to = hrefFor(href);
        if (to) addOpenLink(a, to);
      });
      onScan && onScan(found, root);
    };
    scan();
    ctx.watch(root, scan, 150);
    return root;
  }
  const datasetRowHref = (ns, name) => (href) => {
    const m = href.match(/^\/datasets\/[^/]+\/[^/]+\/(?:blob|resolve)\/[^/]+\/(.+)$/);
    return m ? `/datasets/${ns}/${name}/viewer?file=${encodeURIComponent(decodeURIComponent(m[1]))}` : null;
  };
  const bucketRowHref = (href) => (/^\/buckets\/[^/]+\/[^/]+\/tree\/.+/.test(href) ? href.split("?")[0] + "?studio=1" : null);

  // -------------------------------------------------------- dataset blob page
  async function datasetBlob(ctx, m) {
    const menu = await ctx.waitFor('[data-target="BlobDownloadMenu"]');
    if (!menu) return;
    let src;
    try {
      const p = JSON.parse(menu.getAttribute("data-props") || "{}");
      src = new URL(p.resolvePath || p.downloadUrl, ORIGIN).href.replace(/\?download=true$/, "");
    } catch (_) {}
    src = src || resolveDataset(m[1], m[2], m[3], m[4]);
    const region = menu.parentElement.nextElementSibling;
    const target = region && (region.querySelector("div.p-4.py-8.text-center") || region.firstElementChild);
    if (!target) return;
    const session = new Session(ctx.engine(), [{ path: m[4], src, label: m[4].split("/").pop(), download: src.split("?")[0] + "?download=true" }]);
    const ex = new Explorer({ session, mode: "box", urlKeys: ["view"], studioHref: (st) => `/datasets/${m[1]}/${m[2]}/viewer?file=${encodeURIComponent(m[4])}` + (st.q ? `&q=${st.q}` : "") + (st.view ? `&view=${st.view}` : "") + (st.console ? "&console=1" : "") });
    ctx.hide(target);
    target.before(wrap(ex.el, "m-3"));
    await ex.start();
  }

  // ----------------------------------------------------- bucket file + studio
  async function bucketFile(ctx, m) {
    const dl = await ctx.waitFor('a[download][href*="/buckets/"][href*="/resolve/"]');
    if (!dl) return false;
    const src = new URL(dl.getAttribute("href"), ORIGIN).href.replace(/\?download=true$/, "");
    const filePath = decodeURIComponent(m[3]);
    // HF's "no preview" body: "This file contains binary data..." and "File too large to display" share one container
    // (div.relative.flex.min-h-[100px].flex-col.overflow-clip.rounded-b-lg.border.border-t-0 with a p.text-center).
    // Match the container, not the text; fall back to the container itself so a markup tweak cannot lose the box.
    const region = await ctx.waitFor('div[class*="min-h-[100px]"][class*="rounded-b-lg"]', 6000);
    const target = region && (region.querySelector("p.text-center")?.parentElement || region.firstElementChild || region);
    const session = new Session(ctx.engine(), [{ path: filePath, src, label: filePath.split("/").pop(), download: src + "?download=true" }]);

    const content = document.querySelector("main > div.container");
    const header = document.querySelector('[data-target="BucketHeader"] header');
    const fileUrl = location.pathname;
    let studio = null;
    const inStudio = () => new URLSearchParams(location.search).get("studio") === "1";
    const apply = () => {
      const on = inStudio();
      if (on && !studio && content && header) {
        const shell = studioShell(ctx, header, content);
        const ex = new Explorer({ session, mode: "studio", urlKeys: ["q", "view"] });
        const bar = studioBar(`/buckets/${m[1]}/${m[2]}`, filePath, fileUrl, () => { history.pushState(null, "", fileUrl); apply(); });
        shell.wrap.append(bar, ex.el);
        const h1 = header.querySelector("h1");
        const back = backButton(fileUrl, () => { history.pushState(null, "", fileUrl); apply(); });
        studio = { shell, ex, back, tab: studioTab("Data Studio"), started: false, h1 };
        // HF may re-render the header island: always look the live header up, never keep the first node
        const liveHeader = () => document.querySelector('[data-target="BucketHeader"] header') || header;
        const ensureHeader = () => {
          const hd = liveHeader();
          const h1n = hd.querySelector("h1");
          if (h1n && !h1n.querySelector("[data-retelens-back]")) h1n.prepend(back);
          const bar = hd.querySelector("div.-mb-px.flex");
          if (bar && !bar.querySelector("[data-retelens-tab]")) bar.append(studio.tab);
        };
        ensureHeader();
        ctx.watch(document.querySelector('[data-target="BucketHeader"]') || header.parentElement || document.body, ensureHeader, 100);
      }
      if (studio) {
        studio.shell.outer.style.display = on ? "" : "none";
        studio.back.style.display = on ? "" : "none";
        studio.tab.style.display = on ? "" : "none";
        if (on) ctx.hide(content); else ctx.show(content);
        if (on) { studio.shell.setH(); if (!studio.started) { studio.started = true; studio.ex.start(); } }
      }
      if (!on && boxEx && !boxStarted) { boxStarted = true; boxEx.start(); }
    };
    let boxEx = null, boxStarted = false;
    if (target) {
      const goStudio = (st) => { const u = new URL(location.href); u.searchParams.set("studio", "1"); if (st && st.q) u.searchParams.set("q", String(st.q)); if (st && st.console) u.searchParams.set("console", "1"); history.pushState(null, "", u); apply(); };
      const ex = new Explorer({ session, mode: "box", urlKeys: ["view"], onStudio: goStudio });
      ctx.hide(target);
      target.before(wrap(ex.el, "w-full"));
      boxEx = ex; // started by apply() when not in studio
    }
    ctx.on(window, "popstate", apply);
    apply();
    return true;
  }

  // ----------------------------------------- bucket folder summary (1 .rete)
  async function bucketFolder(ctx, m, onlyIfOne) {
    let done = false;
    return listLike(ctx, '[data-target="BucketFileList"]', "tbody tr.h-10 td.max-w-0 a[href*='/tree/']", bucketRowHref, (found, root) => {
      if (done || !onlyIfOne) return;
      const uniq = [...new Set(found)];
      if (uniq.length !== 1) return;
      done = true;
      const href = uniq[0].split("?")[0];
      const filePath = decodeURIComponent(href.replace(/^\/buckets\/[^/]+\/[^/]+\/tree\//, ""));
      const session = new Session(ctx.engine(), [{ path: filePath, src: resolveBucket(m[1], m[2], filePath), label: filePath.split("/").pop(), download: resolveBucket(m[1], m[2], filePath) + "?download=true" }]);
      const slot = wrap(h("div"), "mb-4");
      root.before(slot);
      session.onChange(() => {
        slot.firstElementChild.textContent = "";
        const n = summaryNode(session, (i) => `${href}?studio=1&q=${i}`);
        if (!n) return;
        const btn = pathLink(href + "?studio=1", icon("tableView"), document.createTextNode("Open in Data Studio"));
        btn.className = "btn shadow-xs mb-3 inline-flex items-center gap-1.5 rounded-full border bg-gradient-to-b from-white to-red-100/90 px-3 py-1 text-sm font-medium text-[#2D3648] hover:shadow-inner dark:from-gray-900 dark:to-red-800/30 dark:text-gray-100";
        slot.firstElementChild.append(n);
        n.prepend(btn);
      });
      session.open(0).catch(() => { slot.remove(); });
    });
  }

  // ---------------------------------------------------------------- scanner (safety net)
  // .rete links and filenames the precise hooks above did not cover. A name like linked-jazz/x.rete in a README resolves
  // to .../resolve/main/<path> when it is one of the repo's files (bucket names are taken as given).
  function startScanner(ctx) {
    const m = location.pathname.match(/^\/(datasets|buckets)\/([^/]+)\/([^/]+)/);
    let siblings = null;
    const listFiles = async () => {
      if (siblings) return siblings;
      try { siblings = (await getJson(`/api/datasets/${m[2]}/${m[3]}`)).siblings.map((s) => s.rfilename); } catch (_) { siblings = []; }
      return siblings;
    };
    const tail = (p) => p.replace(/^\.?\/+/, "");
    const viewerHref = (src, name) => chrome.runtime.getURL("viewer.html") + "?" + new URLSearchParams({ src, name, host: "hf" });
    window.RetelensScanner.start({
      ctx,
      textRoots: ".prose, [data-target='DatasetViewer'] .prose, article",
      linkRoots: ".prose, article",
      nameLink({ url, name }) {
        const a = document.createElement("a");
        a.className = "text-blue-600 dark:text-blue-400";
        a.setAttribute("href", viewerHref(url, name)); a.setAttribute("target", "_blank"); a.setAttribute("rel", "noopener noreferrer");
        a.setAttribute("title", "Preview with rete");
        a.style.cssText = "text-decoration:underline dotted";
        return a;
      },
      handled: (a) => !!(a.closest("li, tr") && a.closest("li, tr").querySelector("[data-retelens-open]")),
      linkUrl(a) {
        let u; try { u = new URL(a.getAttribute("href"), location.href); } catch (_) { return null; }
        if (u.origin === location.origin && u.pathname === location.pathname) return null; // the page's own header/breadcrumb
        const name = decodeURIComponent(u.pathname.split("/").pop());
        const mm = u.pathname.match(/^\/(datasets|buckets)\/([^/]+)\/([^/]+)\/(?:blob|resolve)\/(?:[^/]+\/)?(.+)$/);
        if (u.origin === location.origin && mm) { const src = (mm[1] === "buckets" ? resolveBucket(mm[2], mm[3], decodeURIComponent(mm[4])) : resolveDataset(mm[2], mm[3], "main", decodeURIComponent(mm[4]))); return { url: src, name, path: decodeURIComponent(mm[4]) }; }
        return { url: u.href, name };
      },
      async nameUrl(tn, name) {
        if (!m) return null;
        const p = tail(name);
        if (m[1] === "buckets") return { url: resolveBucket(m[2], m[3], p), name: p.split("/").pop(), path: p };
        const files = await listFiles();
        const hit = files.includes(p) ? p : files.filter((f) => f === p || f.endsWith("/" + p)).sort((a, b) => a.length - b.length)[0];
        return hit ? { url: resolveDataset(m[2], m[3], "main", hit), name: hit.split("/").pop(), path: hit } : null;
      },
      affordance(hit) {
        const a = document.createElement("a");
        a.className = "ml-2 whitespace-nowrap text-xs text-blue-600 hover:underline dark:text-blue-400";
        a.setAttribute("href", viewerHref(hit.url, hit.name)); // the resolved file URL, in the standalone viewer tab
        a.setAttribute("target", "_blank"); a.setAttribute("rel", "noopener noreferrer");
        a.setAttribute("title", "Preview " + hit.name + " with rete");
        a.textContent = "Preview with rete";
        return a;
      },
    });
  }

  // ---------------------------------------------------------------- adapter
  list.push({
    id: "hf",
    matches: (loc) => loc.hostname === "huggingface.co",

    links(doc) {
      const out = [], seen = new Set();
      for (const a of doc.querySelectorAll('a[href*=".rete"]')) {
        let u; try { u = new URL(a.getAttribute("href"), location.href); } catch (_) { continue; }
        if (!isRete(u.pathname)) continue;
        const href = u.href.replace("/blob/", "/resolve/");
        if (!seen.has(href)) { seen.add(href); out.push({ url: href, name: decodeURIComponent(u.pathname.split("/").pop()) }); }
      }
      return out;
    },

    // rule keys for "turn off on this dataset / bucket"
    keys(loc) {
      const m = loc.pathname.match(/^\/(datasets|buckets)\/([^/]+)\/([^/]+)/);
      return m ? { repo: `huggingface.co/${m[1]}/${m[2]}/${m[3]}`, noun: m[1] === "buckets" ? "bucket" : "dataset" } : {};
    },

    async mount(ctx) {
      const path = location.pathname;
      let m;
      startScanner(ctx);
      if ((m = path.match(RE_DATASET_VIEWER))) return datasetViewerPage(ctx, m[1], m[2]);
      if ((m = path.match(RE_DATASET_HOME))) return datasetHome(ctx, m[1], m[2]);
      if ((m = path.match(RE_DATASET_BLOB))) return datasetBlob(ctx, m);
      if ((m = path.match(RE_DATASET_TREE))) return listLike(ctx, '[data-target="ViewerIndexTreeList"]', "li a[href$='.rete']", datasetRowHref(m[1], m[2]));
      if ((m = path.match(RE_BUCKET_ROOT))) return bucketFolder(ctx, m, false);
      if ((m = path.match(RE_BUCKET_TREE))) {
        if (isRete(path) && (await bucketFile(ctx, m))) return;
        return bucketFolder(ctx, m, true);
      }
    },
  });
})();
