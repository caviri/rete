// .rete scanner for any website the user granted (every website, or a host through "Allow reading files from <host>").
// Registered as a dynamic content script by background.js (not injected at install). Loads after content/scanner.js.
// The affordance is a minimal neutral badge in a closed shadow root, so it cannot inherit or leak page styles.
// Clicking it asks the service worker to open the viewer tab. Honours the "site" off rule from the popup.
(function () {
  if (self.__retelensScanAny || !window.RetelensScanner) return;
  self.__retelensScanAny = true;
  const RULES = "offRules";
  const SITE = "site|" + location.hostname;
  let undo = [], dead = true;

  const ctx = {
    get dead() { return dead; },
    onTeardown(f) { undo.push(f); },
    track(n) { undo.push(() => n.remove()); return n; },
    watch(root, fn, ms = 150) {
      let t;
      const mo = new MutationObserver(() => { clearTimeout(t); t = setTimeout(() => { if (!dead) fn(); }, ms); });
      mo.observe(root, { childList: true, subtree: true });
      undo.push(() => { clearTimeout(t); mo.disconnect(); });
      return mo;
    },
  };

  function badge({ url, name }) {
    const host = document.createElement("span");
    host.setAttribute("data-retelens", "badge");
    host.style.cssText = "display:inline-block;margin-left:.4em;vertical-align:baseline";
    const root = host.attachShadow({ mode: "closed" });
    const a = document.createElement("a");
    a.setAttribute("href", "#");
    a.setAttribute("role", "button");
    a.setAttribute("title", "Preview " + name + " with rete");
    a.textContent = "rete ▸";
    a.style.cssText = "all:initial;cursor:pointer;font:11px/1.2 system-ui,sans-serif;color:#555;background:#f3f3f3;border:1px solid #c8c8c8;border-radius:3px;padding:1px 5px;white-space:nowrap";
    a.addEventListener("mouseenter", () => { a.style.background = "#e6e6e6"; });
    a.addEventListener("mouseleave", () => { a.style.background = "#f3f3f3"; });
    a.addEventListener("click", (e) => { e.preventDefault(); e.stopPropagation(); try { chrome.runtime.sendMessage({ retelens: "openViewer", src: url, name }); } catch (_) {} });
    root.append(a);
    return host;
  }

  function begin() {
    if (!dead) return;
    dead = false;
    window.RetelensScanner.start({
      ctx,
      textRoots: "td, li, p, code, dd, dt",
      linkUrl(a) {
        let u; try { u = new URL(a.getAttribute("href"), location.href); } catch (_) { return null; }
        return { url: u.href, name: decodeURIComponent(u.pathname.split("/").pop()) };
      },
      nameUrl(tn, name) {
        // relative to the nearest link in the same row/item when there is one, else to this page
        const row = tn.parentElement && tn.parentElement.closest("tr, li");
        const near = row && row.querySelector("a[href]");
        let base = location.href;
        try { if (near) base = new URL(near.getAttribute("href"), location.href).href; } catch (_) {}
        try { const u = new URL(name, base); return /^https?:$/.test(u.protocol) ? { url: u.href, name: name.split("/").pop() } : null; } catch (_) { return null; }
      },
      affordance: badge,
    });
  }
  function end() {
    if (dead) return;
    dead = true;
    for (const f of undo.splice(0).reverse()) { try { f(); } catch (_) {} }
    for (const n of document.querySelectorAll("[data-retelens-scanned],[data-retelens]")) { if (n.hasAttribute("data-retelens-scanned") && !n.hasAttribute("data-retelens")) n.removeAttribute("data-retelens-scanned"); else if (n.getAttribute("data-retelens") === "badge") n.remove(); }
  }
  const apply = async () => {
    let rules = [];
    try { const st = await chrome.storage.local.get(RULES); rules = Array.isArray(st[RULES]) ? st[RULES] : []; } catch (_) {}
    if (rules.includes(SITE)) end(); else begin();
  };
  apply();
  try { chrome.storage.onChanged.addListener((ch, area) => { if (area === "local" && RULES in ch) apply(); }); } catch (_) {}
})();
