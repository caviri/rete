// A web page's structured data as a graph (the viewer's ?page=<id> mode; content/extract.js reads the page).
//   toDocs(extraction)  -> {nodes, sources, notes}: JSON-LD node objects ready for rete.build(..., "jsonld")
//   Reader(nodes, ...)  -> the page's things (a recipe, a product, an event...) without the page around them
//   EXAMPLES            -> starter queries for a page graph
// JSON-LD is read as the page wrote it, with three repairs: the schema.org context (remote, so never fetched) is
// replaced by its inline equivalent (the official context is @vocab http://schema.org/ with no type coercions),
// relative IRIs resolve against the page URL (@base), and a block without any context but with an @type is read
// with the schema.org context, as search engines read it. Every repair is listed in `notes`.
// The reader works on the JSON objects, not on the graph: RDF keeps no order, and an ingredient list or the steps
// of a recipe are read in order. Untrusted throughout: text nodes only, http(s) links only.
(function () {
  const { h, safeUrl } = window.RetelensFmt;
  const C = () => window.RetelensCells;

  // ---------------------------------------------------------------- JSON-LD normalisation
  const SCHEMA = "http://schema.org/";
  const SCHEMA_CTX = { "@vocab": SCHEMA, schema: SCHEMA };
  const isSchemaCtx = (s) => /^https?:\/\/(www\.)?schema\.org\/?(docs\/jsonldcontext\.json(ld)?)?$/i.test(String(s).trim());
  const vocabOf = (iri) => (/[/#]$/.test(iri) ? iri : /\/[^/]*\.[a-z0-9]+$/i.test(iri) ? iri.replace(/[^/]*$/, "") : iri + "/");

  function parseLenient(text) {
    const t = String(text).trim().replace(/^<!--/, "").replace(/-->$/, "").replace(/^\/\/\s*<!\[CDATA\[/, "").replace(/\/\/\s*\]\]>$/, "").replace(/^<!\[CDATA\[/, "").replace(/\]\]>$/, "").trim();
    try { return { json: JSON.parse(t) }; } catch (e1) {
      // common publishing slips: raw newlines or tabs inside strings, a trailing comma
      const u = t.replace(/[\u0000-\u001f]+/g, " ").replace(/,\s*([}\]])/g, "$1");
      try { return { json: JSON.parse(u), repaired: true }; } catch (_) { return { error: e1.message }; }
    }
  }
  function fixContext(ctx, notes) {
    if (typeof ctx === "string") {
      if (isSchemaCtx(ctx)) return { ...SCHEMA_CTX };
      const iri = ctx.trim();
      notes.add(`The remote context ${iri} is not fetched; its terms were read as ${vocabOf(iri)}<term>.`);
      return { "@vocab": vocabOf(iri) };
    }
    if (Array.isArray(ctx)) return ctx.map((c) => fixContext(c, notes));
    if (ctx && typeof ctx === "object") {
      const out = {};
      for (const [k, v] of Object.entries(ctx)) {
        if (k === "@import") { notes.add(`An @import of ${v} is not fetched.`); continue; }
        out[k] = v && typeof v === "object" && !Array.isArray(v) && v["@context"] !== undefined ? { ...v, "@context": fixContext(v["@context"], notes) } : v;
      }
      return out;
    }
    return ctx;
  }
  // every @context anywhere in a value (node-level contexts too)
  function fixAll(o, notes) {
    if (Array.isArray(o)) return o.map((x) => fixAll(x, notes));
    if (o && typeof o === "object") {
      const out = {};
      for (const [k, v] of Object.entries(o)) out[k] = k === "@context" ? fixContext(v, notes) : fixAll(v, notes);
      return out;
    }
    return o;
  }
  // a document -> its top-level node objects, each carrying its whole context. A {"@context", "@graph"} wrapper is
  // opened (inside an array it would otherwise make a blank-node named graph).
  function nodesOf(doc, ctx, out) {
    if (Array.isArray(doc)) { for (const d of doc) nodesOf(d, ctx, out); return out; }
    if (!doc || typeof doc !== "object") return out;
    const own = doc["@context"];
    const c = own === undefined ? ctx : ctx === undefined ? own : [].concat(ctx, own);
    const keys = Object.keys(doc).filter((k) => k !== "@context");
    if (keys.length === 1 && keys[0] === "@graph") return nodesOf(doc["@graph"], c, out);
    const n = { ...doc };
    delete n["@context"];
    out.push({ node: n, ctx: c });
    return out;
  }
  const hasRelType = (n) => [].concat(n["@type"] || []).some((t) => typeof t === "string" && !/^[a-z][a-z0-9+.-]*:/i.test(t));

  function toDocs(x) {
    const notes = new Set();
    const base = safeUrl(x.url) || "https://page.invalid/";
    const sources = [];
    const nodes = [];
    const withBase = (ctx) => [{ "@base": base }].concat(ctx === undefined ? [] : ctx);
    (x.jsonld || []).forEach((text, i) => {
      const label = `JSON-LD block ${i + 1}`;
      const p = parseLenient(text);
      if (p.error) { sources.push({ label, ok: false, error: "not valid JSON: " + p.error, bytes: text.length }); return; }
      if (p.repaired) notes.add(`${label} is not valid JSON as written (a raw line break or a trailing comma); it was read after repairing that.`);
      const fixed = fixAll(p.json, notes);
      const list = nodesOf(fixed, undefined, []);
      let n = 0;
      for (const { node, ctx } of list) {
        let c = ctx;
        if (c === undefined && (hasRelType(node) || Object.keys(node).some((k) => !k.startsWith("@") && !/^[a-z][a-z0-9+.-]*:/i.test(k)))) {
          c = { ...SCHEMA_CTX };
          notes.add(`${label} has no @context; it was read with schema.org's, as search engines read it.`);
        }
        nodes.push({ source: label, node, doc: { "@context": withBase(c), ...node } });
        n++;
      }
      sources.push({ label, ok: true, nodes: n, bytes: text.length });
    });
    if ((x.microdata || []).length) {
      for (const it of x.microdata) nodes.push({ source: "microdata", node: it, doc: { "@context": { "@base": base }, ...it } });
      sources.push({ label: "microdata", ok: true, nodes: x.microdata.length });
    }
    const metaNode = metaToNode(x, base);
    if (metaNode) { nodes.push({ source: "meta tags", node: metaNode.node, doc: metaNode.doc, meta: true }); sources.push({ label: "meta tags", ok: true, nodes: 1, tags: (x.meta || []).length }); }
    if (x.rdfa > 0) notes.add(`The page also has RDFa markup (${x.rdfa} elements), which is not read.`);
    if (x.truncated) notes.add("The page has more structured data than is read at once (4 MB of JSON-LD, 400 microdata items); the rest was left out.");
    return { nodes, sources, notes: [...notes] };
  }
  // Open Graph and the classic meta tags, as one node about the page (Open Graph's own namespaces; Twitter's tags
  // have none and repeat Open Graph's, so they are left out)
  const OG_NS = { og: "http://ogp.me/ns#", article: "http://ogp.me/ns/article#", book: "http://ogp.me/ns/book#", profile: "http://ogp.me/ns/profile#", music: "http://ogp.me/ns/music#", video: "http://ogp.me/ns/video#" };
  const DCT = "http://purl.org/dc/terms/";
  function metaToNode(x, base) {
    const node = {};
    const add = (k, v) => { (node[k] = node[k] || []).push(v); };
    if (x.title) add(DCT + "title", x.title);
    for (const [k, v] of x.meta || []) {
      const m = /^([a-z]+):(.+)$/i.exec(k);
      if (m && OG_NS[m[1].toLowerCase()]) {
        const key = OG_NS[m[1].toLowerCase()] + m[2].replace(/[^\w:.-]/g, "_");
        add(key, /^(image|url|video|audio)(:url|:secure_url)?$/i.test(m[2]) && safeUrl(v) ? { "@id": v } : v);
      } else if (/^description$/i.test(k)) add(DCT + "description", v);
      else if (/^author$/i.test(k)) add(DCT + "creator", v);
      else if (/^keywords$/i.test(k)) add(DCT + "subject", v);
      else if (/^(dc|dcterms)\.\w+$/i.test(k)) add(DCT + k.split(".")[1].toLowerCase(), v);
    }
    if (!Object.keys(node).length) return null;
    const id = safeUrl(x.canonical) || base;
    const doc = { "@context": { "@base": base }, "@id": id, ...node };
    return { node: { "@id": id, ...node }, doc };
  }

  // ---------------------------------------------------------------- reading values
  const local = (k) => String(k).replace(/^.*[/#:]/, "");
  const typesOf = (n) => [].concat((n && n["@type"]) || []).filter((t) => typeof t === "string").map(local);
  const plain = (s) => { s = String(s); if (!/[<&]/.test(s)) return s; try { return (new DOMParser().parseFromString(s, "text/html").body.textContent || "").trim(); } catch (_) { return s.replace(/<[^>]*>/g, ""); } };
  const words = (k) => local(k).replace(/([a-z0-9])([A-Z])/g, "$1 $2").replace(/[_-]+/g, " ").toLowerCase();
  function label(key, types, plural) {
    let w = words(key);
    for (const t of types) { const tl = t.toLowerCase(); if (w.replace(/ /g, "").startsWith(tl) && w.replace(/ /g, "").length > tl.length) { w = w.slice(w.toLowerCase().indexOf(" ") + 1); break; } }
    if (plural && !/s$/.test(w)) w = /[^aeiou]y$/.test(w) ? w.slice(0, -1) + "ies" : /(x|ch|sh)$/.test(w) ? w + "es" : w + "s";
    return w.charAt(0).toUpperCase() + w.slice(1);
  }
  const DUR = /^-?P(?:(\d+(?:\.\d+)?)Y)?(?:(\d+(?:\.\d+)?)M)?(?:(\d+(?:\.\d+)?)W)?(?:(\d+(?:\.\d+)?)D)?(?:T(?:(\d+(?:\.\d+)?)H)?(?:(\d+(?:\.\d+)?)M)?(?:(\d+(?:\.\d+)?)S)?)?$/;
  function duration(s) {
    const m = DUR.exec(s);
    if (!m || s === "P" || s === "PT") return null;
    const u = [["y", "yr"], ["mo", "mo"], ["w", "wk"], ["d", "d"], ["h", "h"], ["min", "min"], ["s", "s"]];
    const parts = [];
    m.slice(1).forEach((v, i) => { if (v && +v) parts.push(`${+v} ${u[i][1]}`); });
    return parts.length ? parts.join(" ") : "0 min";
  }
  const DATE = /^(\d{4})-(\d{2})-(\d{2})(?:[T ](\d{2}):(\d{2}))?/;
  function date(s) {
    const m = DATE.exec(s);
    if (!m) return null;
    const d = new Date(s.length === 10 ? s + "T00:00:00" : s);
    if (isNaN(d)) return null;
    return d.toLocaleDateString(undefined, { year: "numeric", month: "short", day: "numeric" });
  }
  const valOf = (v) => (v && typeof v === "object" && !Array.isArray(v) && "@value" in v ? v["@value"] : v);
  const arr = (v) => (v == null ? [] : Array.isArray(v) ? v : [v]);
  const first = (o, ...keys) => { for (const k of keys) for (const kk of Object.keys(o || {})) if (local(kk) === k && o[kk] != null && o[kk] !== "") return valOf(arr(o[kk])[0]); return null; };
  const isUrl = (s) => typeof s === "string" && /^https?:\/\/\S+$/i.test(s.trim());
  function imageUrls(v, out = []) {
    for (const x of arr(v)) {
      if (isUrl(valOf(x))) out.push(valOf(x).trim());
      else if (x && typeof x === "object") { const u = first(x, "contentUrl", "url", "@id"); if (isUrl(u)) out.push(u.trim()); else if (isUrl(x["@id"])) out.push(x["@id"]); }
    }
    return [...new Set(out)];
  }
  // a whole object as one line, when it has an obvious one (a rating, a price, a quantity, an address, a person)
  function oneLine(o, ctx) {
    if (!o || typeof o !== "object" || Array.isArray(o)) return null;
    const t = typesOf(o);
    const rv = first(o, "ratingValue");
    if (rv != null) {
      const best = first(o, "bestRating"), n = first(o, "ratingCount", "reviewCount");
      return `${rv}${best ? " / " + best : ""} ★${n != null ? ` · ${Number(n).toLocaleString()} ${t.includes("AggregateRating") ? "ratings" : "votes"}` : ""}`;
    }
    const price = first(o, "price", "lowPrice");
    if (price != null) {
      const cur = first(o, "priceCurrency") || "", hi = first(o, "highPrice"), av = first(o, "availability");
      return `${price}${hi != null ? "–" + hi : ""} ${cur}`.trim() + (av ? ` · ${local(av)}` : "");
    }
    const qv = first(o, "value");
    if (qv != null && (first(o, "unitText") || first(o, "unitCode") || first(o, "currency"))) return `${qv} ${first(o, "unitText") || first(o, "currency") || first(o, "unitCode")}`;
    if (t.includes("PostalAddress")) {
      const country = o.addressCountry && typeof o.addressCountry === "object" ? first(o.addressCountry, "name") : first(o, "addressCountry");
      return [first(o, "streetAddress"), [first(o, "postalCode"), first(o, "addressLocality")].filter(Boolean).join(" "), first(o, "addressRegion"), country].filter(Boolean).map(String).join(", ") || null;
    }
    const lat = first(o, "latitude"), lon = first(o, "longitude");
    if (lat != null && lon != null) return `${lat}, ${lon}`;
    const name = first(o, "name", "headline", "title");
    if (name != null && typeof name !== "object") return plain(name);
    const ref = o["@id"];
    if (typeof ref === "string" && Object.keys(o).length === 1) { const r = ctx.index.get(ref); return r ? oneLine(r, { ...ctx, depth: (ctx.depth || 0) + 1 }) : null; }
    return null;
  }
  function textNode(s, ctx) {
    s = plain(valOf(s));
    if (isUrl(s)) { let shown = s; try { const u = new URL(s); shown = u.host + (u.pathname.length > 1 ? decodeURIComponent(u.pathname).slice(0, 40) : ""); } catch (_) {} return h("a", { href: s, class: "rl-r-link", title: s }, shown); }
    const d = DUR.test(s) ? duration(s) : DATE.test(s) ? date(s) : null;
    return d ? h("span", { title: s }, d) : document.createTextNode(s);
  }
  // a scalar or a one-line object -> nodes; null when the value needs a block of its own
  function inline(v, ctx) {
    v = valOf(v);
    if (v == null) return null;
    if (typeof v === "boolean") return document.createTextNode(v ? "yes" : "no");
    if (typeof v === "number") return document.createTextNode(v.toLocaleString());
    if (typeof v === "string") return textNode(v, ctx);
    if (typeof v === "object" && !Array.isArray(v)) {
      const line = oneLine(v, ctx);
      if (line != null) { const u = first(v, "url", "sameAs"); return isUrl(u) && !first(v, "ratingValue", "price") ? h("a", { href: u.trim(), class: "rl-r-link" }, line) : document.createTextNode(line); }
      const ref = v["@id"];
      if (typeof ref === "string" && Object.keys(v).length === 1) return isUrl(ref) ? textNode(ref, ctx) : document.createTextNode(local(ref));
    }
    return null;
  }

  // ---------------------------------------------------------------- the reader
  const LEAD = new Set(["name", "headline", "alternativeHeadline", "title", "description", "abstract", "image", "thumbnailUrl", "logo", "photo", "@id", "@type"]);
  const LINKS = new Set(["url", "sameAs", "mainEntityOfPage", "isPartOf", "potentialAction"]);
  const FURNITURE = new Set(["WebSite", "WebPage", "BreadcrumbList", "SearchAction", "SiteNavigationElement", "WPHeader", "WPFooter", "WPSideBar", "ReadAction", "EntryPoint", "ImageObject", "CollectionPage", "ItemPage", "AboutPage", "ContactPage"]);
  function entity(n, ctx, level) {
    const types = typesOf(n);
    const box = h("article", { class: "rl-r-entity" + (level ? " rl-r-sub" : "") });
    const title = first(n, "name", "headline", "title", "alternativeHeadline");
    const head = h("header", { class: "rl-r-head" });
    // a nested thing sits under a heading that already says what it is: no type chips, and no title unless it has one
    if (types.length && !level) head.append(h("div", { class: "rl-r-types" }, types.map((t) => h("span", { class: "rl-r-type", title: t }, words(t)))));
    if (title != null || !level) head.append(h(level ? "h3" : "h2", { class: "rl-r-title" }, title != null ? plain(title) : typeof n["@id"] === "string" ? local(n["@id"]) : types.map(words).join(", ") || "Untitled"));
    else if (types.length && level && ctx.showType) head.append(h("div", { class: "rl-r-types" }, h("span", { class: "rl-r-type", title: types[0] }, words(types[0]))));
    if (!head.childNodes.length) head.hidden = true;
    box.append(head);
    const imgs = imageUrls(Object.keys(n).filter((k) => ["image", "thumbnailUrl", "logo", "photo"].includes(local(k))).flatMap((k) => arr(n[k])));
    if (imgs.length) {
      const gal = h("div", { class: "rl-r-gallery" + (imgs.length > 1 ? " rl-r-many" : "") });
      for (const u of imgs.slice(0, level ? 1 : 5)) {
        const b = h("button", { type: "button", class: "rl-imgbtn", "aria-label": "Enlarge image" }, C().img(C().thumbUrl(u, 900), { eager: true, cls: "rl-r-img" }));
        b.addEventListener("click", () => C().lightbox(u, title != null ? plain(title) : ""));
        gal.append(b);
      }
      box.append(gal);
    }
    const desc = first(n, "description", "abstract");
    if (desc != null && typeof desc !== "object") box.append(h("p", { class: "rl-r-desc" }, plain(desc)));
    const facts = h("dl", { class: "rl-r-facts" });
    const blocks = [];
    const links = [];
    for (const [k, raw] of Object.entries(n)) {
      const key = local(k);
      if (LEAD.has(key) || key.startsWith("@")) continue;
      const vals = arr(raw);
      if (!vals.length) continue;
      if (LINKS.has(key)) { for (const v of vals) { const u = typeof valOf(v) === "string" ? valOf(v) : v && v["@id"]; if (isUrl(u)) links.push([key, u.trim()]); } continue; }
      const inl = vals.map((v) => inline(v, ctx));
      if (inl.every(Boolean) && vals.length <= 3 && !(vals.length > 1 && vals.every((v) => typeof valOf(v) === "string" && String(valOf(v)).length > 40))) {
        const dd = h("dd");
        inl.forEach((x, i) => { if (i) dd.append(document.createTextNode(", ")); dd.append(x); });
        facts.append(h("div", { class: "rl-r-fact" }, h("dt", { title: k }, label(k, types, false)), dd));
        continue;
      }
      blocks.push([k, vals]);
    }
    if (facts.childNodes.length) box.append(facts);
    for (const [k, vals] of blocks) box.append(block(k, vals, types, ctx, level));
    if (links.length) box.append(h("div", { class: "rl-r-links" }, links.slice(0, 12).map(([k, u]) => h("a", { href: u, class: "rl-r-chip", title: u }, `${label(k, [], false)}: ${hostPath(u)}`))));
    return box;
  }
  const hostPath = (u) => { try { const x = new URL(u); return x.host + (x.pathname.length > 1 ? decodeURIComponent(x.pathname).slice(0, 32) : ""); } catch (_) { return u; } };
  // a property that needs room: a list of strings, a list of steps, nested things, a long list collapsed
  function block(k, vals, types, ctx, level) {
    const sec = h("section", { class: "rl-r-block" });
    const plural = vals.length > 1;
    const heading = h(level ? "h4" : "h3", { class: "rl-r-h", title: k }, label(k, types, plural) + (vals.length > 6 ? ` (${vals.length})` : ""));
    const resolved = vals.map((v) => { v = valOf(v); if (v && typeof v === "object" && typeof v["@id"] === "string" && Object.keys(v).length === 1 && ctx.index.has(v["@id"])) return ctx.index.get(v["@id"]); return v; });
    const allText = resolved.every((v) => typeof v !== "object" || v === null);
    const stepText = (v) => (v && typeof v === "object" ? first(v, "text") || (Object.keys(v).length <= 3 ? first(v, "name") : null) : null);
    let body;
    if (allText) body = h("ul", { class: "rl-r-list" }, resolved.map((v) => h("li", null, textNode(v, ctx))));
    else if (resolved.every((v) => v && typeof v === "object" && (stepText(v) != null || typesOf(v).includes("HowToSection")))) {
      body = h("div");
      let ol = null, n = 0;
      for (const v of resolved) {
        if (typesOf(v).includes("HowToSection") || (Array.isArray(v.itemListElement) && !stepText(v))) {
          ol = null;
          body.append(h("div", { class: "rl-r-subh" }, plain(first(v, "name") || "")));
          const inner = h("ol", { class: "rl-r-steps", start: String(n + 1) });
          for (const s of arr(v.itemListElement || v[Object.keys(v).find((x) => local(x) === "itemListElement")])) { n++; inner.append(stepLi(s, ctx)); }
          body.append(inner);
        } else {
          if (!ol) { ol = h("ol", { class: "rl-r-steps", start: String(n + 1) }); body.append(ol); }
          n++; ol.append(stepLi(v, ctx));
        }
      }
    } else {
      body = h("div", { class: "rl-r-nested" });
      const depth = (ctx.depth || 0) + 1;
      for (const v of resolved.slice(0, 50)) {
        if (v && typeof v === "object" && depth < 4) body.append(entity(v, { ...ctx, depth }, 1));
        else { const x = inline(v, ctx); if (x) body.append(h("div", null, x)); }
      }
      if (resolved.length > 50) body.append(h("div", { class: "rl-muted" }, `+${resolved.length - 50} more`));
    }
    const big = vals.length > 6 && !allText && !(body.tagName === "DIV" && body.querySelector("ol"));
    if (big) { const d = h("details", { class: "rl-r-more" }, h("summary", null, heading), body); sec.append(d); }
    else sec.append(heading, body);
    return sec;
  }
  function stepLi(s, ctx) {
    s = valOf(s);
    if (typeof s !== "object" || s === null) return h("li", null, textNode(s, ctx));
    const li = h("li");
    const text = first(s, "text") || first(s, "name") || "";
    li.append(document.createTextNode(plain(text)));
    const im = imageUrls(s.image || s[Object.keys(s).find((k) => local(k) === "image")]);
    if (im.length) li.append(h("div", null, C().img(C().thumbUrl(im[0], 400), { cls: "rl-r-stepimg", zoom: im[0] })));
    return li;
  }
  // which nodes are the page's subject: those named as mainEntity, else the typed nodes nobody points to that are
  // not page furniture (site, page, breadcrumbs), richest first
  function rank(items) {
    const index = new Map();
    for (const it of items) if (typeof it.node["@id"] === "string") index.set(it.node["@id"], it.node);
    const pointed = new Set();
    const mains = new Set();
    const visit = (o, key) => {
      if (Array.isArray(o)) return o.forEach((x) => visit(x, key));
      if (!o || typeof o !== "object") return;
      if (typeof o["@id"] === "string" && key) { pointed.add(o["@id"]); if (key === "mainEntity") mains.add(o["@id"]); }
      for (const [k, v] of Object.entries(o)) if (!k.startsWith("@")) visit(v, local(k));
    };
    for (const it of items) for (const [k, v] of Object.entries(it.node)) if (!k.startsWith("@")) visit(v, local(k));
    const score = (n) => Object.keys(n).length;
    const main = [], rest = [];
    for (const it of items) {
      const n = it.node, id = n["@id"], t = typesOf(n);
      if (it.meta) continue;
      const isMain = (typeof id === "string" && mains.has(id)) || (t.length && !t.every((x) => FURNITURE.has(x)) && !(typeof id === "string" && pointed.has(id)));
      (isMain ? main : rest).push(it);
    }
    main.sort((a, b) => score(b.node) - score(a.node));
    return { main, rest, index };
  }
  // the reader: header with what was read and built, the main things in full, the rest folded away
  function Reader(items, opts = {}) {
    const root = h("div", { class: "rl-reader" });
    const { main, rest, index } = rank(items);
    const ctx = { index, depth: 0 };
    if (!main.length && !rest.length) {
      const meta = items.find((it) => it.meta);
      root.append(h("div", { class: "rl-r-empty" }, h("p", null, "This page publishes no JSON-LD or microdata about what it shows."),
        meta ? h("p", { class: "rl-muted" }, "Only its meta tags were read (title, description, Open Graph); they are in the graph, see Explore.") : null));
      if (meta) root.append(entity({ "@type": ["Page"], name: first(meta.node, "title"), description: first(meta.node, "description"), image: arr(meta.node["http://ogp.me/ns#image"] || []).map((x) => (x && x["@id"]) || x) }, ctx, 0));
      return root;
    }
    for (const it of main) root.append(entity(it.node, ctx, 0));
    if (rest.length) {
      const d = h("details", { class: "rl-r-more rl-r-rest" }, h("summary", null, `About the page itself (${rest.length} ${rest.length === 1 ? "thing" : "things"}: ${[...new Set(rest.flatMap((it) => typesOf(it.node)))].slice(0, 6).join(", ") || "untyped"})`));
      for (const it of rest) d.append(entity(it.node, { ...ctx, showType: true }, 1));
      root.append(d);
    }
    return root;
  }

  // ---------------------------------------------------------------- starter queries for a page graph
  const PFX = "PREFIX schema: <http://schema.org/>\nPREFIX schemas: <https://schema.org/>\n";
  const EXAMPLES = [
    { title: "Things on this page", question: "What does the page describe, and what is each thing called?",
      sparql: PFX + "# the richest things first (most statements), as the reader orders them\nSELECT ?thing ?type ?name ?facts WHERE {\n  { SELECT ?thing (COUNT(*) AS ?facts) WHERE { ?thing ?p ?o } GROUP BY ?thing }\n  ?thing a ?type .\n  OPTIONAL { ?thing ?np ?name . VALUES ?np { schema:name schemas:name schema:headline schemas:headline } }\n} ORDER BY DESC(?facts)" },
    { title: "Every statement", question: "The whole graph built from the page, one statement per row.", sparql: "SELECT ?s ?p ?o WHERE { ?s ?p ?o } LIMIT 2000" },
    { title: "Pictures", question: "Every image the page's data names, with what it shows.",
      sparql: PFX + "SELECT ?thing ?image WHERE {\n  ?thing ?ip ?img . VALUES ?ip { schema:image schemas:image schema:thumbnailUrl schemas:thumbnailUrl schema:logo schemas:logo schema:photo schemas:photo }\n  OPTIONAL { ?img ?up ?u . VALUES ?up { schema:contentUrl schemas:contentUrl schema:url schemas:url } }\n  BIND(COALESCE(?u, ?img) AS ?image)\n  FILTER(!isBlank(?image))\n}" },
    { title: "Links to other pages", question: "Values that are web addresses: sources, profiles, related pages.",
      sparql: "SELECT ?thing ?property ?link WHERE {\n  ?thing ?property ?link .\n  FILTER(isIRI(?link) || (isLiteral(?link) && REGEX(STR(?link), \"^https?://\")))\n  FILTER(?property != <http://www.w3.org/1999/02/22-rdf-syntax-ns#type>)\n} LIMIT 500" },
  ];

  window.RetelensPage = { toDocs, Reader, EXAMPLES, parseLenient, duration, label };
})();
