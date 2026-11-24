// .rete link and filename scanner: a safety net next to each adapter's precise hooks.
//   - every a[href] whose path ends in .rete
//   - every text node under `textRoots` that mentions a name like  dir/file.rete  (the text node is split)
// Each hit gets a small "Preview" affordance made by the host adapter (native look per host) or, on any other website,
// by scan-any.js (a neutral badge in a closed shadow root). Processed nodes carry data-retelens-scanned and are
// restored exactly on teardown. Everything resolved here is a URL string checked with safeUrl; names are only
// ever inserted as text nodes.
(function () {
  const NAME = /[\w.\-/%~+]*[\w\-%~+]\.rete\b/g; // dir/file.rete, file.rete (no scheme: full URLs are links)
  const SKIP = "a,button,script,style,textarea,input,select,svg,[contenteditable],[data-retelens],[data-retelens-scanned]";

  // running text: paragraphs, headings, quotes, description blocks (not table cells or list items)
  const isProse = (n) => { const el = n.parentElement; if (!el || el.closest("td, th, li")) return false; return !!el.closest("p, blockquote, h1, h2, h3, h4, h5, h6, #description, .rich-input-content"); };
  // the name follows a command word / redirection on the same line: `rete convert in.rete -o out.rete`, `curl -O x.rete`, `> x.rete`
  const isCommand = (text, idx) => /(?:^|[\s;|&])(?:rete|curl|wget|cat|cp|mv|scp|rsync|aws|hf|git)\s[^\n]*$|(?:^|\s)-{1,2}[\w-]+(?:=|\s+)$|[>|<]\s*$/.test(text.slice(0, idx));
  const isReteHref = (href, base) => { try { const u = new URL(href, base || location.href); return /^https?:$/.test(u.protocol) && /\.rete$/i.test(decodeURIComponent(u.pathname)); } catch (_) { return false; } };

  // opts:
  //   ctx            the registry (track/onTeardown/watch); dead ctx = no-op
  //   root           where to look for links (default document.body)
  //   textRoots      selector for containers whose text nodes are scanned ("" = none)
  //   handled(a)     true when a precise hook already covers this link/row
  //   linkUrl(a)     -> {url, name} | null   for a link
  //   nameUrl(tn, name) -> {url, name} | null | Promise   for a filename in text
  //   affordance({url,name}) -> Node         the host's native "Preview" control
  function start(opts) {
    const ctx = opts.ctx;
    const done = new WeakSet();
    const root = () => opts.root || document.body;
    let scanning = false, again = false;

    function mark(el) {
      el.setAttribute("data-retelens-scanned", "");
      ctx.onTeardown(() => el.removeAttribute("data-retelens-scanned"));
    }
    function place(after, hit) {
      const node = opts.affordance(hit);
      if (!node) return null;
      node.setAttribute("data-retelens", node.getAttribute("data-retelens") || "");
      after.after(node);
      ctx.track(node);
      return node;
    }

    function links() {
      for (const a of root().querySelectorAll("a[href]")) {
        if (done.has(a) || a.hasAttribute("data-retelens-scanned") || a.closest("[data-retelens]")) continue;
        if (opts.linkRoots && !a.closest(opts.linkRoots)) continue; // host file chrome (headers, toolbars, trees, commit rows) is never decorated
        const href = a.getAttribute("href") || "";
        if (!isReteHref(href)) continue;
        done.add(a);
        if (opts.handled && opts.handled(a)) continue;
        const hit = opts.linkUrl(a);
        if (!hit || !hit.url) continue;
        if (opts.handledHit && opts.handledHit(hit)) continue;
        if (place(a, hit)) mark(a);
      }
    }

    async function texts() {
      if (!opts.textRoots || !opts.nameUrl) return;
      const found = [];
      for (const container of root().querySelectorAll(opts.textRoots)) {
        if (container.closest("[data-retelens]")) continue;
        const w = document.createTreeWalker(container, NodeFilter.SHOW_TEXT, {
          acceptNode: (n) => (done.has(n) || !n.nodeValue || n.nodeValue.indexOf(".rete") < 0 || (n.parentElement && n.parentElement.closest(SKIP)) ? NodeFilter.FILTER_REJECT : NodeFilter.FILTER_ACCEPT),
        });
        for (let n = w.nextNode(); n; n = w.nextNode()) found.push(n);
      }
      for (const tn of found) {
        if (ctx.dead || done.has(tn) || !tn.isConnected) continue;
        done.add(tn);
        const text = tn.nodeValue;
        const matches = [...text.matchAll(NAME)].filter((m) => !/^https?:/i.test(text.slice(Math.max(0, m.index - 8), m.index)));
        if (!matches.length) continue;
        const parts = [], added = [];
        let last = 0;
        for (const m of matches) {
          let hit = null;
          try { hit = await opts.nameUrl(tn, m[0]); } catch (_) {}
          if (ctx.dead || !tn.isConnected) return;
          if (!hit || !hit.url) continue;
          if (isCommand(text, m.index)) continue;                    // command lines: nothing
          if (opts.handledHit && opts.handledHit(hit)) continue;     // the precise hook already covers this file
          const prose = isProse(tn);
          if (prose && (!opts.nameLink || hit.unverified)) continue; // running text: never a button, a link only for a real file
          parts.push({ m, hit, prose });
        }
        if (!parts.length) continue;
        const frag = document.createDocumentFragment();
        for (const { m, hit, prose } of parts) {
          if (m.index > last) { const t = document.createTextNode(text.slice(last, m.index)); frag.append(t); added.push(t); }
          const span = prose ? opts.nameLink(hit) : document.createElement("span"); // the match itself: marks the processed spot
          span.setAttribute("data-retelens-scanned", "");
          if (prose) span.setAttribute("data-retelens", "");
          span.textContent = m[0];
          frag.append(span); added.push(span);
          const aff = prose ? null : opts.affordance(hit);
          if (aff) { aff.setAttribute("data-retelens", aff.getAttribute("data-retelens") || ""); frag.append(aff); added.push(aff); }
          last = m.index + m[0].length;
        }
        if (last < text.length) { const t = document.createTextNode(text.slice(last)); frag.append(t); added.push(t); }
        tn.replaceWith(frag);
        ctx.onTeardown(() => { const first = added.find((n) => n.parentNode); if (first) first.before(tn); for (const n of added) n.remove(); });
      }
    }

    async function scan() {
      if (ctx.dead) return;
      if (scanning) { again = true; return; }
      scanning = true;
      try { links(); await texts(); } catch (e) { console.debug("[retelens] scan failed", e); }
      scanning = false;
      if (again && !ctx.dead) { again = false; scan(); }
    }
    scan();
    ctx.watch(document.body, scan, 250);
    return { scan };
  }

  window.RetelensScanner = { start, isReteHref, NAME };
})();
