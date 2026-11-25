// GitHub adapter. GitHub is client-side routed (React app + Turbo), so unlike HF the
// adapter re-runs on soft navigations and cleans up what it injected when the route changes.
// Styling: Primer CSS variables (content/github.css, generated, scoped under [data-retelens]).
//
// Raw bytes: the blob page's own Raw link (github.com/<o>/<r>/raw/<ref-and-path>) 302s to
// raw.githubusercontent.com/<o>/<r>/<ref-and-path>, which does Range with CORS *. We take the
// Raw link's tail as is (branch names may contain "/") and only swap the host, so the extension
// needs no github.com host permission for blobs. Release assets (no CORS) need host permissions.
//
// Untrusted data (card, IRIs, literals) only reaches the DOM through lib/format.js h() (text nodes,
// http(s)-only hrefs). Same-origin links here are built from location + fixed query keys.
(function () {
  const list = (self.__retelensAdapters = self.__retelensAdapters || []);
  const { h, pathLink, icon, safeUrl, dlLink } = window.RetelensFmt;
  const { Explorer, Session } = window.RetelensExplorer;

  const RE_BLOB = /^\/([^/]+)\/([^/]+)\/blob\/(.+\.rete)$/;
  const RE_BLOB_PM = /^\/([^/]+)\/([^/]+)\/blob\/(.+\.pmtiles)$/i;
  const RE_TREE = /^\/([^/]+)\/([^/]+)\/tree\/.+/;
  const RE_RELEASES = /^\/([^/]+)\/([^/]+)\/releases(\/|$)/;
  const isRete = (p) => /\.rete$/i.test(p.split("?")[0]);

  // github.com/<o>/<r>/raw/<tail>  ->  raw.githubusercontent.com/<o>/<r>/<tail>
  function rawSource(href) {
    let u; try { u = new URL(href, location.href); } catch (_) { return null; }
    const m = u.pathname.match(/^\/([^/]+)\/([^/]+)\/raw\/(.+)$/);
    if (u.hostname === "github.com" && m) return `https://raw.githubusercontent.com/${m[1]}/${m[2]}/${m[3]}`;
    if (u.hostname === "raw.githubusercontent.com" || u.hostname === "media.githubusercontent.com") return u.href;
    return null;
  }

  function wrap(el, cls) {
    const outer = document.createElement("div");
    outer.setAttribute("data-retelens", "");
    outer.setAttribute("data-retelens-gh", "");
    const inner = document.createElement("div");
    if (cls) inner.className = cls;
    inner.append(el);
    outer.append(inner);
    return outer;
  }

  const NOT_REPO = new Set(["settings", "orgs", "marketplace", "sponsors", "notifications", "explore", "topics", "search", "login", "signup", "features", "pricing", "about", "new", "issues", "pulls", "codespaces", "organizations", "apps", "users", "enterprise", "collections", "trending", "readme", "site", "customer-stories"]);

  // .rete links and filenames the precise hooks did not cover (README text, tables). Names resolve against the repo:
  // github.com/<o>/<r>/raw/<ref>/<path>, ref from a blob/tree URL, HEAD on the repo home.
  function startScanner(ctx) {
    const rm = location.pathname.match(/^\/([^/]+)\/([^/]+)(?:\/(?:blob|tree)\/([^/]+))?/);
    const repo = rm && !NOT_REPO.has(rm[1]) ? { o: rm[1], r: rm[2], ref: rm[3] || "HEAD" } : null;
    const viewerHref = (src, name) => chrome.runtime.getURL("viewer.html") + "?" + new URLSearchParams({ src, name, host: "github" });
    const exists = new Map(); // raw URL -> Promise<boolean>
    window.RetelensScanner.start({
      ctx,
      textRoots: "article.markdown-body, .markdown-body",
      linkRoots: ".markdown-body, [data-testid='issue-body'], .comment-body, .js-comment-body",
      // the file this blob page shows is covered by the precise hook (raw/download/permalink links included)
      handledHit: (hit) => { const m = location.pathname.match(/^\/[^/]+\/[^/]+\/blob\/(.+)$/); return !!(m && hit.url.split("?")[0].endsWith("/" + decodeURIComponent(m[1]))); },
      nameLink({ url, name }) {
        const a = document.createElement("a");
        a.setAttribute("href", viewerHref(url, name)); a.setAttribute("target", "_blank"); a.setAttribute("rel", "noopener noreferrer");
        a.setAttribute("title", "Preview with rete");
        a.style.cssText = "text-decoration:underline dotted";
        return a;
      },
      handled: (a) => { const c = a.closest("li, tr, div.react-directory-row, [role='row']"); return !!(c && c.querySelector("[data-retelens-open]")); },
      linkUrl(a) {
        let u; try { u = new URL(a.getAttribute("href"), location.href); } catch (_) { return null; }
        if (u.origin === location.origin && u.pathname === location.pathname) return null; // the page's own header/breadcrumb
        const name = decodeURIComponent(u.pathname.split("/").pop());
        const src = rawSource(u.href.replace("/blob/", "/raw/"));
        return { url: src || u.href, name };
      },
      async nameUrl(tn, name) {
        const p = name.replace(/^\.?\/+/, "");
        // "data.example.org/dir/x.rete" is an address, not a path in the repo
        if (/^[\w-]+(\.[\w-]+)+\/./.test(p) && !/\.(md|json|ttl|nt|txt)\//i.test(p)) return { url: "https://" + p, name: p.split("/").pop(), unverified: true };
        if (!repo) return null;
        const url = `https://raw.githubusercontent.com/${repo.o}/${repo.r}/${repo.ref}/${p}`;
        if (!exists.has(url)) exists.set(url, fetch(url, { method: "HEAD" }).then((r) => r.status !== 404 && r.status !== 403, () => true)); // only a real 404 drops the name
        return (await exists.get(url)) ? { url, name: p.split("/").pop() } : null;
      },
      affordance(hit) {
        const a = document.createElement("a");
        a.className = "Label Label--secondary ml-2";
        a.setAttribute("href", viewerHref(hit.url, hit.name));
        a.setAttribute("target", "_blank"); a.setAttribute("rel", "noopener noreferrer");
        a.setAttribute("title", "Preview " + hit.name + " with rete");
        a.style.verticalAlign = "middle";
        a.textContent = "Preview";
        return a;
      },
    });
  }

  list.push({
    id: "github",
    matches: (loc) => loc.hostname === "github.com",

    // rule keys for "turn off on this repository"
    keys(loc) {
      const m = loc.pathname.match(/^\/([^/]+)\/([^/]+)/);
      return m && !NOT_REPO.has(m[1]) ? { repo: `github.com/${m[1]}/${m[2]}`, noun: "repository" } : {};
    },

    links(doc) {
      const out = [];
      for (const a of doc.querySelectorAll('a[href$=".rete"]')) {
        const src = rawSource(a.getAttribute("href").replace("/blob/", "/raw/"));
        if (src) out.push({ url: src, name: decodeURIComponent(src.split("/").pop()) });
      }
      return out;
    },

    mount(ctx) {
      let cleanups = [];
      let current = null; // {key, studio}
      let timer = null;
      const addCleanup = (f) => cleanups.push(f);
      const cleanup = () => { for (const f of cleanups.splice(0)) { try { f(); } catch (_) {} } };
      const inStudio = () => new URLSearchParams(location.search).get("studio") === "1";

      // ------------------------------------------------------ a .pmtiles blob page
      // the map viewer (lib/maps.js PmtilesViewer) as the file's body, where GitHub says it cannot show a binary file.
      // A file kept with Git LFS is read from its media.githubusercontent.com address (the Raw link's redirect target,
      // which answers Range with CORS); raw.githubusercontent.com would serve the LFS pointer instead.
      async function blobPmtiles(m) {
        const raw = await ctx.waitFor('a[data-testid="raw-button"]', 10000);
        if (!raw || location.pathname !== "/" + m[1] + "/" + m[2] + "/blob/" + m[3]) return;
        const lfs = !!document.querySelector('[data-testid="lfs-pointer"], [aria-label*="Git LFS"]') || /Stored with Git LFS/i.test(document.querySelector('section[aria-labelledby*="file-name-id"]')?.textContent || "");
        let src = rawSource(raw.getAttribute("href"));
        if (src && lfs && /^https:\/\/raw\.githubusercontent\.com\//.test(src)) src = src.replace("https://raw.githubusercontent.com/", "https://media.githubusercontent.com/media/");
        if (!src) return;
        let v = null, el = null;
        const mount = () => {
          const section = document.querySelector('section[aria-labelledby*="file-name-id"]');
          if (!section || !section.parentElement) return;
          if (el && el.isConnected && section.style.display === "none") return;
          ctx.hide(section);
          if (!v) { v = window.RetelensMaps.PmtilesViewer(src, {}); v.el.classList.add("rl-pm-inline"); el = wrap(v.el, ""); }
          el.firstElementChild.style.marginTop = getComputedStyle(section).marginTop;
          if (!el.isConnected) section.after(el);
        };
        mount();
        const mo = ctx.observe(document.body, { childList: true, subtree: true }, () => { if (!(el && el.isConnected)) mount(); });
        addCleanup(() => { if (mo) mo.disconnect(); if (v) v.destroy(); el && el.remove(); const sec = document.querySelector('section[aria-labelledby*="file-name-id"]'); if (sec) ctx.show(sec); });
      }

      // ------------------------------------------------------------ blob page
      async function blob(m) {
        const raw = await ctx.waitFor('a[data-testid="raw-button"]', 10000);
        if (!raw || location.pathname !== "/" + m[1] + "/" + m[2] + "/blob/" + m[3]) return;
        const src = rawSource(raw.getAttribute("href"));
        if (!src) return;
        const filePath = decodeURIComponent(m[3]);
        const label = filePath.split("/").pop();
        const session = new Session(ctx.engine(), [{ path: filePath, src, label, download: new URL(raw.getAttribute("href"), location.href).href }]);
        const fileUrl = location.pathname;

        let boxEl = null, boxEx = null, boxStarted = false, studio = null;

        const mountBox = () => {
          const section = document.querySelector('section[aria-labelledby*="file-name-id"]');
          if (!section || !section.parentElement) return false;
          boxEx = boxEx || new Explorer({ session, mode: "box", urlKeys: ["view"], onStudio: goStudio, skin: "github", toolbar: true });
          // Data Studio goes into GitHub's own blob toolbar, in front of the Raw / copy / download group
          const grp = document.querySelector('[data-testid="raw-button"]')?.closest('[data-component="ButtonGroup"]');
          if (grp && boxEx.toolbarEl && !boxEx.toolbarEl.isConnected) grp.parentElement.insertBefore(ctx.track(boxEx.toolbarEl), grp);
          if (boxEl && boxEl.isConnected && section.style.display === "none") return true;
          ctx.hide(section);
          boxEl = boxEl || wrap(boxEx.el, ""); // sits inside GitHub's own file Box, as the file body
          // GitHub keeps its sticky file header clear of the content with a top margin: copy it
          boxEl.firstElementChild.style.marginTop = getComputedStyle(section).marginTop;
          if (!boxEl.isConnected) section.after(boxEl);
          if (!boxStarted && !inStudio()) { boxStarted = true; boxEx.start(); }
          return true;
        };
        const mountStudio = () => {
          const app = document.querySelector("react-app") || document.querySelector("main");
          if (!app) return;
          if (!studio) {
            const ex = new Explorer({ session, mode: "studio", urlKeys: ["q", "view"], skin: "github", topbar: true });
            // blob-header style bar: back arrow, owner/repo @ ref, path breadcrumb; Download as btn-primary
            const parts = filePath.split("/"), ref = parts[0], rest = parts.slice(1), base = `/${m[1]}/${m[2]}`;
            const back = pathLink(fileUrl, icon("back"));
            back.className = "btn btn-sm btn-octicon"; back.setAttribute("data-retelens-back", ""); back.setAttribute("aria-label", "Back to file"); back.setAttribute("title", "Back to file");
            back.addEventListener("click", (e) => { e.preventDefault(); history.pushState(null, "", fileUrl); apply(); });
            const crumbs = [pathLink(base, `${m[1]}/${m[2]}`), h("span", { class: "color-fg-muted" }, "@ " + ref)];
            let acc = "";
            rest.forEach((seg, i) => { acc += (i ? "/" : "") + seg; crumbs.push(h("span", { class: "color-fg-muted" }, "/"), i < rest.length - 1 ? pathLink(`${base}/tree/${ref}/${acc}`, decodeURIComponent(seg)) : h("strong", null, decodeURIComponent(seg))); });
            const right = h("div", { class: "ml-auto flex flex-wrap items-center gap-2" }, h("a", { href: "https://github.com/caviri/rete", class: "f6 flex items-center gap-1", title: "rete on GitHub" }, icon("graphChip"), "rete"));
            const dl = session.file && dlLink(session.file.download, "btn btn-sm btn-primary", icon("download"), "Download");
            if (dl) right.append(dl);
            const bar = h("div", { class: "flex flex-wrap items-center gap-2 border-b px-4 py-2 text-sm", "data-retelens-bar": "" }, back, ...crumbs, right);
            const body = h("div", { class: "flex flex-col md:h-[calc(100vh-9rem)]" }, ex.el);
            studio = { ex, el: wrap(h("div", null, bar, body), ""), started: false };
          }
          ctx.hide(app);
          if (!studio.el.isConnected) app.after(studio.el);
          if (!studio.started) { studio.started = true; studio.ex.start(); }
        };
        const unmountStudio = () => {
          if (!studio) return;
          studio.el.remove();
          const app = document.querySelector("react-app") || document.querySelector("main");
          if (app) ctx.show(app);
        };
        function goStudio(st) { const u = new URL(location.href); u.searchParams.set("studio", "1"); if (st && st.q) u.searchParams.set("q", String(st.q)); if (st && st.console) u.searchParams.set("console", "1"); history.pushState(null, "", u); apply(); }
        function apply() {
          if (inStudio()) { mountStudio(); } else { unmountStudio(); mountBox(); if (boxEx && !boxStarted) { boxStarted = true; boxEx.start(); } }
        }
        apply();
        // React re-renders may drop our nodes: put them back
        const mo = ctx.observe(document.body, { childList: true, subtree: true }, () => { if (!inStudio() && (!(boxEl && boxEl.isConnected) || (boxEx && boxEx.toolbarEl && !boxEx.toolbarEl.isConnected))) mountBox(); else if (inStudio() && studio && !studio.el.isConnected) mountStudio(); });
        addCleanup(() => { if (mo) mo.disconnect(); boxEl && boxEl.remove(); boxEx && boxEx.toolbarEl && boxEx.toolbarEl.remove(); unmountStudio(); const sec = document.querySelector('section[aria-labelledby*="file-name-id"]'); if (sec) ctx.show(sec); });
        const pop = () => apply();
        ctx.on(window, "popstate", pop);
      }

      // ------------------------------------------------- tree + release rows
      function rows(anchorSel, hrefFor) {
        const scan = () => {
          for (const a of document.querySelectorAll(anchorSel)) {
            const href = a.getAttribute("href") || "";
            if (!isRete(href)) continue;
            const cell = a.parentElement;
            if (!cell || cell.querySelector("[data-retelens-open]")) continue;
            const to = hrefFor(href);
            if (!to) continue;
            const link = pathLink(to, "Open in viewer");
            link.className = "ml-3 whitespace-nowrap text-xs";
            link.setAttribute("data-retelens-open", "");
            link.setAttribute("data-retelens", "");
            link.style.color = "var(--fgColor-accent)";
            a.after(link);
          }
        };
        scan();
        const mo = ctx.watch(document.body, scan, 150);
        addCleanup(() => { if (mo) mo.disconnect(); document.querySelectorAll("[data-retelens-open]").forEach((n) => n.remove()); });
      }

      // ---------------------------------------------------------------- route
      function route() {
        const path = location.pathname;
        let m;
        if ((m = path.match(RE_BLOB))) { blob(m); return; }
        if ((m = path.match(RE_BLOB_PM))) { blobPmtiles(m); return; }
        if (RE_TREE.test(path)) {
          rows('a[href*="/blob/"][href$=".rete"]', (href) => href.split("?")[0] + "?studio=1");
          return;
        }
        if (RE_RELEASES.test(path)) {
          // release assets are not blob pages: open the standalone viewer in a new tab
          rows('a[href*="/releases/download/"][href$=".rete"]', (href) => {
            const abs = new URL(href, location.href).href;
            return safeUrl(abs) ? chrome.runtime.getURL("viewer.html") + "?" + new URLSearchParams({ src: abs, name: decodeURIComponent(abs.split("/").pop()), host: "github" }) : null;
          });
        }
      }

      function maybeRoute() {
        if (!ctx.alive()) return;
        const key = location.pathname;
        if (current && current.key === key) return; // same page (e.g. ?studio toggles are handled inside blob())
        cleanup();
        current = { key };
        route();
      }
      const schedule = () => { clearTimeout(timer); timer = ctx.later(maybeRoute, 150); };

      ctx.onTeardown(cleanup); // turned off: run the current route's cleanup, then the registry reverts the rest
      startScanner(ctx);
      maybeRoute();
      // soft navigations: Navigation API, Turbo events, history events, and a URL-poll fallback
      if (self.navigation && self.navigation.addEventListener) ctx.on(self.navigation, "navigatesuccess", schedule);
      for (const ev of ["turbo:load", "turbo:render", "turbo:frame-render", "popstate", "pjax:end"]) ctx.on(document, ev, schedule);
      ctx.on(window, "popstate", schedule);
      let last = location.href;
      ctx.every(() => { if (location.href !== last) { last = location.href; schedule(); } }, 500);
    },
  });
})();
