// Injected on demand into the active tab (chrome.scripting.executeScript, activeTab: the user chose "Read this
// page's data" from the context menu, the toolbar popup or a shortcut). Reads the structured data the page
// publishes and returns it; it changes nothing on the page and sends nothing anywhere. The result is the value of
// the last expression below.
//   jsonld     the text of every <script type="application/ld+json">
//   microdata  every top-level itemscope item, as JSON-LD with absolute IRIs (WHATWG microdata, item values as
//              the spec defines them: URLs from src/href/data, meta content, time datetime, otherwise the text)
//   meta       Open Graph, Twitter and Dublin Core <meta> tags, the description and the canonical link
//   rdfa       how many elements carry RDFa attributes (not read yet; reported so the viewer can say so)
(() => {
  const MAX_TEXT = 4 * 1048576;   // JSON-LD text kept, all blocks together
  const MAX_ITEMS = 400;          // microdata items
  const abs = (u) => { try { const x = new URL(String(u).trim(), document.baseURI); return /^https?:$/.test(x.protocol) ? x.href : null; } catch (_) { return null; } };
  const squash = (s) => String(s || "").replace(/\s+/g, " ").trim();

  // ---- JSON-LD
  const jsonld = [];
  let total = 0, truncated = false;
  for (const s of document.querySelectorAll('script[type="application/ld+json" i], script[type^="application/ld+json;" i]')) {
    const text = s.textContent || "";
    if (!text.trim()) continue;
    if (total + text.length > MAX_TEXT) { truncated = true; break; }
    total += text.length;
    jsonld.push(text);
  }

  // ---- microdata
  const MICRO = "http://www.w3.org/1999/xhtml/microdata#";
  const vocab = (types) => { const t = types[0]; if (!t) return MICRO; const m = /^(.*[/#])[^/#]*$/.exec(t); return m ? m[1] : MICRO; };
  // the properties of an item: elements with itemprop under it, not inside a nested item, plus itemref targets
  function propsOf(root) {
    const out = [];
    const queue = [...root.children];
    for (const id of (root.getAttribute("itemref") || "").split(/\s+/).filter(Boolean)) { const e = document.getElementById(id); if (e) queue.push(e); }
    let guard = 0;
    while (queue.length && guard++ < 20000) {
      const e = queue.shift();
      if (e.hasAttribute("itemprop")) out.push(e);
      if (!e.hasAttribute("itemscope")) queue.push(...e.children);
    }
    return out;
  }
  function value(el, seen) {
    if (el.hasAttribute("itemscope")) return item(el, seen);
    const tag = el.tagName.toLowerCase();
    const link = (attr) => { const u = abs(el.getAttribute(attr) || ""); return u ? { "@id": u } : squash(el.getAttribute(attr)); };
    if (tag === "meta") return el.getAttribute("content") || "";
    if (["audio", "embed", "iframe", "img", "source", "track", "video"].includes(tag)) return link("src");
    if (["a", "area", "link"].includes(tag)) return link("href");
    if (tag === "object") return link("data");
    if (tag === "data" || tag === "meter") return el.getAttribute("value") || "";
    if (tag === "time" && el.hasAttribute("datetime")) return el.getAttribute("datetime");
    return squash(el.textContent).slice(0, 20000);
  }
  function item(el, seen) {
    if (seen.has(el) || seen.size > 5000) return {};
    seen.add(el);
    const out = {};
    const types = (el.getAttribute("itemtype") || "").split(/\s+/).filter((t) => /^https?:\/\//i.test(t));
    if (types.length) out["@type"] = types.length === 1 ? types[0] : types;
    const id = abs(el.getAttribute("itemid") || "");
    if (id) out["@id"] = id;
    const base = vocab(types);
    for (const p of propsOf(el)) {
      const v = value(p, seen);
      if (v === "" || v == null) continue;
      for (const name of p.getAttribute("itemprop").split(/\s+/).filter(Boolean)) {
        const key = /^https?:\/\//i.test(name) ? name : base + name;
        (out[key] = out[key] || []).push(v);
      }
    }
    for (const k of Object.keys(out)) if (Array.isArray(out[k]) && out[k].length === 1) out[k] = out[k][0];
    return out;
  }
  const microdata = [];
  const seen = new Set();
  for (const el of document.querySelectorAll("[itemscope]:not([itemprop])")) {
    if (microdata.length >= MAX_ITEMS) { truncated = true; break; }
    const it = item(el, seen);
    if (Object.keys(it).length) microdata.push(it);
  }

  // ---- meta tags
  const meta = [];
  for (const m of document.querySelectorAll("meta[property], meta[name]")) {
    const k = squash(m.getAttribute("property") || m.getAttribute("name"));
    const v = m.getAttribute("content");
    if (!k || v == null || !v.trim()) continue;
    if (/^(og|article|book|profile|music|video|twitter|fb):/i.test(k) || /^(description|author|keywords|dc\.|dcterms\.)/i.test(k)) meta.push([k, v.trim().slice(0, 4000)]);
    if (meta.length >= 200) break;
  }
  const canonical = abs((document.querySelector('link[rel~="canonical" i]') || {}).href || "");

  return {
    url: location.href,
    canonical,
    title: squash(document.title).slice(0, 500),
    lang: document.documentElement.lang || "",
    jsonld,
    microdata,
    meta,
    rdfa: document.querySelectorAll("[typeof], [vocab], [property]:not(meta)").length,
    truncated,
  };
})();
