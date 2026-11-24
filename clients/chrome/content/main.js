// Picks the adapter for this host and hands it a small toolbox. Libraries (lib/*.js)
// and adapters (content/adapters/*.js) load before this file; adapters register on
// self.__retelensAdapters. Add a host = one adapter file + a manifest entry.
//
// Turning the viewer off ("rete viewer" context menu / popup) is live and complete, with no reload:
// every change an adapter makes goes through the ctx registry (track / hide / replace / on / observe / every /
// later / onTeardown) and teardown reverts it in reverse order. A mount is one generation: its ctx dies on
// teardown, so late async continuations cannot leak nodes into the page. Turning on again mounts a fresh one.
(function () {
  const adapters = self.__retelensAdapters || [];
  const adapter = adapters.find((a) => a.matches(location));
  if (!adapter) return;
  if (self.__retelensMain) return; // once per page
  self.__retelensMain = true;

  const RULES = "offRules"; // ["page|<url>", "repo|<id>", "site|<host>"]
  const MARKERS = "[data-retelens],[data-retelens-tab],[data-retelens-back],[data-retelens-open],[data-retelens-bar],[data-retelens-engine]";

  // ------------------------------------------------------------ rule keys of this page
  const pageKey = () => location.origin + location.pathname.replace(/\/+$/, "");
  function keys() {
    const k = (adapter.keys && adapter.keys(location)) || {};
    return { page: pageKey(), repo: k.repo || null, repoNoun: k.noun || "repository", site: location.hostname };
  }
  const offOf = (rules, k) => ({ page: rules.includes("page|" + k.page), repo: !!k.repo && rules.includes("repo|" + k.repo), site: rules.includes("site|" + k.site) });

  // ------------------------------------------------------------ one mount generation
  let gen = null;
  function makeCtx() {
    const undo = [];
    const hidden = new Map(); // el -> original style attribute (null = none)
    let engine = null;
    const c = {
      dead: false,
      adapterId: adapter.id,
      // ONE engine per page: a hidden extension iframe owns the Worker; we hold one MessagePort to it.
      engine() { if (c.dead) throw new Error("retelens is off on this page"); return (engine = engine || RetelensEngine.viaIframe()); },
      isDark: () => document.documentElement.classList.contains("dark"),
      alive: () => !c.dead,

      // Resolve with the first element matching sel (observing DOM changes), or null after ms.
      waitFor(sel, ms = 8000, root = document) {
        return new Promise((resolve) => {
          const hit = root.querySelector(sel);
          if (hit) return resolve(hit);
          const mo = new MutationObserver(() => {
            const el = root.querySelector(sel);
            if (el) { mo.disconnect(); clearTimeout(t); resolve(el); }
          });
          mo.observe(document.documentElement, { childList: true, subtree: true });
          c.onTeardown(() => { mo.disconnect(); clearTimeout(t); resolve(null); });
          const t = setTimeout(() => { mo.disconnect(); resolve(root.querySelector(sel)); }, ms);
        });
      },

      // ---- the registry
      onTeardown(fn) { if (c.dead) { try { fn(); } catch (_) {} } else undo.push(fn); },
      track(node) { if (c.dead) { node.remove && node.remove(); return node; } undo.push(() => node.remove()); return node; },
      // hide el (display:none), remembering the exact style attribute for teardown
      hide(el) {
        if (c.dead || !el) return el;
        if (!hidden.has(el)) {
          hidden.set(el, el.getAttribute("style"));
          undo.push(() => { const s = hidden.get(el); if (s == null) el.removeAttribute("style"); else el.setAttribute("style", s); hidden.delete(el); });
        }
        el.style.display = "none";
        return el;
      },
      // undo hide() now; teardown still restores the original style attribute
      show(el) { if (!hidden.has(el)) return; const s = hidden.get(el); if (s == null) el.removeAttribute("style"); else el.setAttribute("style", s); },
      // put newEl where el is; teardown puts el back at the same place
      replace(el, newEl) {
        if (c.dead || !el || !el.parentNode) return newEl;
        const parent = el.parentNode, next = el.nextSibling;
        el.replaceWith(newEl);
        undo.push(() => { newEl.remove(); if (!el.isConnected) parent.insertBefore(el, next && next.parentNode === parent ? next : null); });
        return newEl;
      },
      on(target, type, fn, opts) { if (c.dead) return; target.addEventListener(type, fn, opts); undo.push(() => target.removeEventListener(type, fn, opts)); },
      observe(target, options, fn) { if (c.dead) return null; const mo = new MutationObserver(fn); mo.observe(target, options); undo.push(() => mo.disconnect()); return mo; },
      every(fn, ms) { if (c.dead) return; const t = setInterval(fn, ms); undo.push(() => clearInterval(t)); },
      later(fn, ms) { if (c.dead) return null; const t = setTimeout(fn, ms); undo.push(() => clearTimeout(t)); return t; },
      // run fn after each burst of mutations under root (debounced)
      watch(root, fn, ms = 150) {
        let t;
        undo.push(() => clearTimeout(t));
        return c.observe(root, { childList: true, subtree: true }, () => { clearTimeout(t); t = setTimeout(() => { if (!c.dead) fn(); }, ms); });
      },

      teardown() {
        if (c.dead) return;
        c.dead = true;
        for (const f of undo.splice(0).reverse()) { try { f(); } catch (e) { console.debug("[retelens] teardown step failed", e); } }
        if (engine) { try { if (engine.frame) engine.frame.remove(); if (engine.t && engine.t.close) engine.t.close(); } catch (_) {} engine = null; }
        for (const n of document.querySelectorAll(MARKERS)) n.remove(); // safety net: anything still carrying our marks
      },
    };
    return c;
  }

  function mount() {
    gen = makeCtx();
    Promise.resolve(adapter.mount(gen)).catch((e) => console.warn("[retelens] adapter failed", e));
  }
  function unmount() { if (gen) { gen.teardown(); gen = null; } }

  // ------------------------------------------------------------ rules, state, soft navigation
  let lastSig = "", rules = [];
  function evaluate() {
    const k = keys();
    const off = offOf(rules, k);
    const isOff = off.page || off.repo || off.site;
    if (isOff && gen) unmount();
    if (!isOff && !gen) mount();
    const sig = JSON.stringify([k, off, !!gen]);
    if (sig === lastSig) return;
    lastSig = sig;
    try { chrome.runtime.sendMessage({ retelens: "pageState", supported: true, adapter: adapter.id, host: location.hostname, keys: k, off, mounted: !!gen, url: location.href }).catch(() => {}); } catch (_) {}
  }
  async function loadRules() { try { const st = await chrome.storage.local.get(RULES); rules = Array.isArray(st[RULES]) ? st[RULES] : []; } catch (_) { rules = []; } }

  loadRules().then(evaluate);
  try { chrome.storage.onChanged.addListener((ch, area) => { if (area === "local" && RULES in ch) loadRules().then(evaluate); }); } catch (_) {}
  // soft navigations (GitHub): re-evaluate when the URL changes
  let lastHref = location.href;
  setInterval(() => { if (location.href !== lastHref) { lastHref = location.href; evaluate(); } }, 600);
  // the service worker asks again after a restart
  try { chrome.runtime.onMessage.addListener((msg, _s, send) => { if (msg && msg.retelens === "getPageState") { lastSig = ""; evaluate(); send({ ok: true }); } }); } catch (_) {}
})();
