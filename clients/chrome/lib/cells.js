// Result cells shared by the Table and Cards views: images, IIIF manifests, WKT geometries, audio, video, PDF,
// links, Markdown and page previews, each column with a render type (Auto or forced). What the playground
// (web/playground-src/app.js) learnt about good cells, rebuilt as DOM for the extension: untrusted values only
// enter as text nodes or as http(s) URLs in src/href (format.js h()); the only parsed markup is static icons.
//
// Network: the inline cells read what a cell shows (thumbnails, IIIF manifests, media) and nothing else; a
// basemap is only fetched when a map is opened (lib/maps.js). On a host page whose CSP refuses an image
// (GitHub allows only its own image hosts) the bytes are fetched from the content script, which the page's
// connect-src does not bind, and shown from a blob: URL, which the page allows.
(function () {
  const { h, safeUrl } = window.RetelensFmt;
  const XSD = "http://www.w3.org/2001/XMLSchema#";
  const NUMERIC = /(integer|int|long|short|decimal|double|float|byte|nonNegativeInteger|positiveInteger|negativeInteger|nonPositiveInteger|unsigned\w*)$/;
  const EXT_PAGE = location.protocol === "chrome-extension:";
  const SVGNS = "http://www.w3.org/2000/svg";

  // ---------------------------------------------------------------- render types
  const TYPES = [
    ["auto", "Auto"], ["text", "Text"], ["link", "Link"], ["button", "Button"], ["image", "Image"], ["iiif", "IIIF"],
    ["geo", "Map"], ["audio", "Audio"], ["video", "Video"], ["pdf", "PDF"], ["page", "Page preview"], ["markdown", "Markdown"], ["number", "Number"],
  ];
  const TYPE_LABEL = Object.fromEntries(TYPES);

  // ---------------------------------------------------------------- detection
  const URL_RX = /^https?:\/\/[^\s<>"'`]+$/i;
  // the URL a term stands for: an http(s) IRI, or a plain string literal that is a whole URL (JSON-LD from web
  // pages writes most links as strings)
  function urlOf(t) {
    if (!t) return null;
    if (t.kind === "iri") return safeUrl(t.value);
    if (t.kind === "literal" && !t.lang && (!t.datatype || t.datatype === XSD + "string" || t.datatype === XSD + "anyURI")) {
      const v = String(t.value).trim();
      return URL_RX.test(v) ? v : null;
    }
    return null;
  }
  // a dereferenceable web URL: a real (dotted) host, so toy namespaces like http://ex/a are not linked
  const looksWebUrl = (v) => { try { const u = new URL(v); return /^https?:$/.test(u.protocol) && u.hostname.includes("."); } catch (_) { return false; } };
  // http media on an https page is mixed content: fetch over https (the link keeps the IRI as written)
  // (localhost is never mixed content and rarely has TLS)
  const up = (v) => (/^http:\/\/(localhost|127\.\d+\.\d+\.\d+|\[::1\])(:\d+)?\//i.test(String(v)) ? String(v) : String(v).replace(/^http:\/\//i, "https://"));
  const path = (v) => String(v).split(/[?#]/)[0];
  const IMG_EXT = /\.(jpe?g|png|gif|webp|avif|svg|bmp)$/i;
  // IIIF Image API: {base}/{region}/{size}/{rotation}/{quality}.{format}
  const IIIF_IMG = /\/(full|square|\d+,\d+,\d+,\d+|pct:[\d.,]+)\/(full|max|\d+,|,\d+|\d+,\d+|!\d+,\d+|pct:[\d.]+)\/!?[\d.]+\/(default|color|gray|grey|bitonal|native)\.(jpe?g|png|webp|gif|tif)$/i;
  const IIIF_INFO = /\/info\.json$/i;
  const looksImage = (v) => /commons\.wikimedia\.org\/wiki\/Special:FilePath\//i.test(v) || IMG_EXT.test(path(v)) || IIIF_IMG.test(path(v)) || (IIIF_INFO.test(path(v)) && /iiif/i.test(v));
  const looksIiif = (v) => !looksImage(v) && (/\/manifest(\.json(ld)?)?$/i.test(path(v)) || /\/iiif\//i.test(path(v)) || /iiif.*\/collection(\.json)?$/i.test(path(v)));
  const looksAudio = (v) => /\.(mp3|ogg|oga|opus|wav|flac|m4a|aac)$/i.test(path(v));
  const looksVideo = (v) => /\.(mp4|webm|ogv|mov|m4v)$/i.test(path(v));
  const looksPdf = (v) => /\.pdf$/i.test(path(v));
  const WKT_RE = /\b(POINT|LINESTRING|POLYGON|MULTIPOINT|MULTILINESTRING|MULTIPOLYGON|GEOMETRYCOLLECTION)\b\s*[ZM]*\s*\(/i;
  const isWkt = (t) => !!t && t.kind === "literal" && t.value.length < 4e6 && WKT_RE.test(t.value);
  const isNumber = (t) => !!t && t.kind === "literal" && !!t.datatype && NUMERIC.test(t.datatype);

  // thumbnails: Commons FilePath and IIIF Image API URLs are scaled by the server
  function thumbUrl(v, w) {
    if (/Special:FilePath\//i.test(v)) return up(v) + (v.includes("?") ? "&" : "?") + "width=" + w;
    const p = path(v);
    if (IIIF_INFO.test(p)) return up(p.replace(IIIF_INFO, "")) + `/full/!${w},${w}/0/default.jpg`;
    if (IIIF_IMG.test(p)) return up(p.replace(/\/(full|max|\d+,|,\d+|\d+,\d+|!\d+,\d+|pct:[\d.]+)\/(!?[\d.]+)\/(default|color|gray|grey|bitonal|native)\.(\w+)$/i, `/!${w},${w}/$2/$3.$4`));
    return up(v);
  }
  function fullUrl(v) {
    const p = path(v);
    if (IIIF_INFO.test(p)) return up(p.replace(IIIF_INFO, "")) + "/full/!2000,2000/0/default.jpg";
    if (/Special:FilePath\//i.test(v)) return thumbUrl(v, 1600);
    return up(v);
  }

  // the type Auto picks for a value
  function autoType(t) {
    if (!t) return "text";
    if (isWkt(t)) return "geo";
    const u = urlOf(t);
    if (u) {
      if (/.pmtiles$/i.test(path(u))) return "layer";
      if (looksImage(u)) return "image";
      if (looksIiif(u)) return "iiif";
      if (looksAudio(u)) return "audio";
      if (looksVideo(u)) return "video";
      if (looksPdf(u)) return "pdf";
      if (looksWebUrl(u)) return "link";
      return "iri";
    }
    if (isNumber(t)) return "number";
    return t.kind === "iri" ? "iri" : "text";
  }
  // a column's Auto type: the commonest non-text type among its first values (for the menu's label)
  function columnAuto(rows, v) {
    const n = {};
    for (const r of rows.slice(0, 50)) { const k = autoType(r[v]); n[k] = (n[k] || 0) + 1; }
    const best = Object.entries(n).sort((a, b) => b[1] - a[1])[0];
    return best ? best[0] : "text";
  }

  // ---------------------------------------------------------------- text
  const HL = (text, q) => (q ? window.RetelensResults.highlight(text, q) : [text]);
  // one term as inline nodes; `short` is the prefix shortener
  function termNodes(t, short, opts = {}) {
    if (!t) return [];
    if (t.kind === "iri") {
      const label = HL(short(t.value), opts.hl);
      const url = safeUrl(t.value);
      return [url ? h("a", { href: url, title: t.value, class: "iri text-blue-600 hover:underline dark:text-blue-400" }, label) : h("span", { title: t.value, class: "iri" }, label)];
    }
    if (t.kind === "bnode" || t.kind === "triple") return [h("span", { class: "text-gray-500" }, HL(t.value, opts.hl))];
    const out = HL(t.value, opts.hl);
    if (t.lang) out.push(h("span", { class: "ml-1 text-gray-400" }, "@" + t.lang));
    else if (t.datatype && t.datatype !== XSD + "string" && !NUMERIC.test(t.datatype) && !opts.noDatatype) out.push(h("span", { class: "ml-1 text-gray-400", title: t.datatype }, "^^" + short(t.datatype)));
    return out;
  }
  // a column name for people: camelCase and snake_case split, first letter up; the variable stays in the title
  const prettyLabel = (v) => String(v).replace(/([a-z0-9])([A-Z])/g, "$1 $2").replace(/[_-]+/g, " ").replace(/^./, (c) => c.toUpperCase());
  const shortText = (s, n) => (s.length > n ? s.slice(0, n - 1) + "…" : s);

  // ---------------------------------------------------------------- media pieces
  const SOURCE_LABEL = { image: "Open image", pdf: "Open PDF", audio: "Open audio", video: "Open video", iiif: "Open manifest", page: "Open page" };
  function sourceLink(url, kind) {
    return h("div", { class: "rl-mfoot" }, h("a", { href: safeUrl(url), class: "rl-msrc", title: url }, (SOURCE_LABEL[kind] || "Open source") + " ↗"));
  }
  function broken(el) {
    el.classList.add("rl-broken");
    el.alt = "image unavailable";
    el.removeAttribute("data-rl-zoom");
  }
  // A host page's CSP refusing outside pictures (GitHub) is seen as securitypolicyviolation events. When the fetch
  // route fails too (an image host without CORS), the explorer is told once ("retelens:media-blocked") and offers
  // the same file in the extension's own viewer, where every picture loads.
  let cspBlocked = false, blockedSaid = false;
  if (!EXT_PAGE) document.addEventListener("securitypolicyviolation", (e) => { if (/^(img|media)-src/.test(e.effectiveDirective || e.violatedDirective || "")) cspBlocked = true; }, true);
  const sayBlocked = () => setTimeout(() => { if (cspBlocked && !blockedSaid) { blockedSaid = true; document.dispatchEvent(new CustomEvent("retelens:media-blocked")); } }, 50);
  // CSP fallback: the bytes through fetch, shown from a blob: URL
  const blobs = new Map(); // url -> Promise<objectURL|null>
  function recover(el, url) {
    if (EXT_PAGE || !safeUrl(url)) return broken(el);
    if (!blobs.has(url)) {
      blobs.set(url, fetch(url, { credentials: "omit", referrerPolicy: "no-referrer" })
        .then((r) => (r.ok && /^image\//i.test(r.headers.get("content-type") || "") ? r.blob() : null))
        .then((b) => (b ? URL.createObjectURL(b) : null))
        .catch(() => null));
    }
    blobs.get(url).then((o) => {
      if (!o) { sayBlocked(); return broken(el); }
      el.addEventListener("error", () => broken(el), { once: true });
      el.src = o;
    });
  }
  function img(src, opts = {}) {
    const el = document.createElement("img");
    el.decoding = "async";
    el.loading = opts.eager ? "eager" : "lazy";
    el.referrerPolicy = "no-referrer";
    el.alt = opts.alt || "";
    el.className = opts.cls || "";
    if (opts.zoom && safeUrl(opts.zoom)) el.setAttribute("data-rl-zoom", opts.zoom);
    el.addEventListener("error", () => recover(el, src), { once: true });
    if (safeUrl(src)) el.src = src; else broken(el);
    return el;
  }

  // ---------------------------------------------------------------- modal shell
  // One dialog at a time per kind; Escape, the backdrop and the close button close it. Mounted on <body> under
  // its own [data-retelens] so the scoped host styles apply.
  function modal({ label, cls, onClose, onKey }) {
    const prevFocus = document.activeElement;
    const box = h("div", { class: "rl-modal-box rounded-lg border bg-white text-sm text-gray-900 shadow-lg dark:border-gray-800 dark:bg-gray-950 dark:text-gray-100 " + (cls || ""), role: "dialog", "aria-modal": "true", "aria-label": label, tabindex: "-1" });
    const closeBtn = h("button", { type: "button", class: "rl-modal-x", "aria-label": "Close", title: "Close (Esc)" }, "×");
    const root = h("div", { "data-retelens": "", class: "rl-modal" }, h("div", { class: "rl-modal-bg" }), box);
    box.append(closeBtn);
    const close = () => {
      document.removeEventListener("keydown", key, true);
      root.remove();
      onClose && onClose();
      if (prevFocus && prevFocus.isConnected && prevFocus.focus) prevFocus.focus();
    };
    const key = (e) => {
      if (e.key === "Escape") { e.preventDefault(); e.stopPropagation(); close(); return; }
      if (onKey && !/^(INPUT|TEXTAREA|SELECT)$/.test(e.target.tagName)) onKey(e);
    };
    root.firstChild.addEventListener("click", close);
    closeBtn.addEventListener("click", close);
    document.addEventListener("keydown", key, true);
    document.body.append(root);
    setTimeout(() => box.focus({ preventScroll: true }), 0);
    return { root, box, close };
  }

  // ---------------------------------------------------------------- image lightbox + hover zoom
  function lightbox(url, caption) {
    const m = modal({ label: "Image", cls: "rl-lightbox" });
    m.box.append(h("div", { class: "rl-lb-stage" }, img(fullUrl(url), { eager: true, cls: "rl-lb-img" })),
      h("div", { class: "rl-lb-bar" }, h("span", { class: "rl-lb-cap", title: caption || url }, caption || url), sourceLink(url, "image")));
  }
  // Desktop only (fine pointer with hover): a larger copy next to the thumbnail, kept inside the viewport.
  let zoomOn = false, zoomEl = null, zoomSrc = null, zoomTimer = 0;
  function installZoom() {
    if (zoomOn) return;
    zoomOn = true;
    if (!matchMedia("(hover: hover) and (pointer: fine)").matches) return;
    const hide = () => { clearTimeout(zoomTimer); zoomSrc = null; if (zoomEl) zoomEl.hidden = true; };
    document.addEventListener("mouseover", (e) => {
      const t = e.target;
      const im = t && t.closest ? t.closest("img[data-rl-zoom]") : null;
      if (!im || im.closest(".rl-modal")) { if (zoomSrc && (!t.closest || !t.closest(".rl-zoom"))) hide(); return; }
      if (zoomSrc === im) return;
      zoomSrc = im;
      clearTimeout(zoomTimer);
      zoomTimer = setTimeout(() => showZoom(im), 140);
    }, true);
    document.addEventListener("scroll", hide, true);
    document.addEventListener("mousedown", hide, true);
  }
  function showZoom(src) {
    if (zoomSrc !== src || !src.isConnected) return;
    const url = src.getAttribute("data-rl-zoom");
    if (!safeUrl(url)) return;
    if (!zoomEl) {
      zoomEl = h("div", { "data-retelens": "", class: "rl-zoom", "aria-hidden": "true" });
      document.body.append(zoomEl);
    }
    zoomEl.textContent = "";
    const big = img(thumbUrl(url, 800), { eager: true, cls: "rl-zoom-img" });
    // the thumbnail is already loaded: show it at once, swap for the large one when it arrives
    if (src.currentSrc) { const quick = img(src.currentSrc, { eager: true, cls: "rl-zoom-img" }); zoomEl.append(quick); big.addEventListener("load", () => { quick.remove(); zoomEl.append(big); }, { once: true }); }
    else zoomEl.append(big);
    zoomEl.hidden = false;
    const place = () => {
      if (zoomSrc !== src) return;
      const r = src.getBoundingClientRect(), vw = document.documentElement.clientWidth, vh = innerHeight, M = 8;
      const w = zoomEl.offsetWidth, hh = zoomEl.offsetHeight;
      const right = vw - r.right - M, left = r.left - M;
      let x = right >= w + 12 || right >= left ? r.right + 12 : r.left - 12 - w;
      x = Math.max(M, Math.min(x, vw - w - M));
      let y = r.top + r.height / 2 - hh / 2;
      y = Math.max(M, Math.min(y, vh - hh - M));
      zoomEl.style.left = x + "px"; zoomEl.style.top = y + "px";
    };
    place();
    for (const im of zoomEl.querySelectorAll("img")) im.addEventListener("load", place, { once: true });
    src.addEventListener("mouseleave", () => { if (zoomSrc === src) { zoomSrc = null; clearTimeout(zoomTimer); zoomEl.hidden = true; } }, { once: true });
  }

  // ---------------------------------------------------------------- lazy hydration
  // IIIF manifests, page previews: fetched when the cell comes near the viewport, never for the whole result
  let io = null;
  const hydrators = new WeakMap();
  function whenVisible(el, fn) {
    if (!("IntersectionObserver" in window)) { fn(); return; }
    if (!io) io = new IntersectionObserver((entries) => {
      for (const e of entries) {
        if (!e.isIntersecting) continue;
        io.unobserve(e.target);
        const f = hydrators.get(e.target);
        hydrators.delete(e.target);
        if (f) f();
      }
    }, { rootMargin: "300px" });
    hydrators.set(el, fn);
    io.observe(el);
  }

  // ---------------------------------------------------------------- IIIF (Presentation v2 and v3)
  // A manifest URL becomes a paged thumbnail; a click opens a large view with paging and the manifest's
  // metadata. Read cross-origin where the IIIF server sends CORS (the spec recommends it); on any failure the
  // link stays. IIIF text may hold HTML: it is reduced to text (DOMParser runs no scripts).
  const iiifDocs = new Map();
  const plain = (s) => { s = String(s); if (!/[<&]/.test(s)) return s; try { return new DOMParser().parseFromString(s, "text/html").body.textContent || ""; } catch (_) { return s.replace(/<[^>]*>/g, ""); } };
  function iiifText(v) {
    if (v == null) return "";
    if (typeof v === "string" || typeof v === "number") return plain(v).trim();
    if (Array.isArray(v)) return v.map(iiifText).filter(Boolean).join("; ");
    if (typeof v === "object") {
      if ("@value" in v) return iiifText(v["@value"]);
      const langs = Object.keys(v);
      const pick = langs.find((k) => k === "en") || langs.find((k) => k === "none") || langs[0];
      return pick ? iiifText(v[pick]) : "";
    }
    return "";
  }
  const idOf = (o) => (o && typeof o === "object" ? o.id || o["@id"] : typeof o === "string" ? o : null);
  function imageService(res) {
    const svc = res && (Array.isArray(res.service) ? res.service[0] : res.service);
    const base = idOf(svc);
    return base && safeUrl(base) ? base.replace(/\/info\.json$/i, "").replace(/\/$/, "") : null;
  }
  function iiifImage(res) {
    if (!res) return null;
    const base = imageService(res);
    if (base) return { thumb: up(base + "/full/!400,400/0/default.jpg"), zoom: up(base + "/full/!2000,2000/0/default.jpg") };
    const u = idOf(res);
    return u && safeUrl(u) ? { thumb: up(u), zoom: up(u) } : null;
  }
  function iiifCanvases(m) {
    const out = [];
    const push = (cv, res) => { const u = iiifImage(res); if (u) out.push({ ...u, label: iiifText(cv.label) }); };
    for (const seq of m.sequences || []) for (const cv of seq.canvases || []) push(cv, cv.images && cv.images[0] && cv.images[0].resource); // v2
    for (const cv of m.items || []) { // v3: Canvas > AnnotationPage > Annotation > body
      const ann = cv.items && cv.items[0] && cv.items[0].items && cv.items[0].items[0];
      const body = ann && (Array.isArray(ann.body) ? ann.body[0] : ann.body);
      push(cv, body);
    }
    if (!out.length) { const t = m.thumbnail && (Array.isArray(m.thumbnail) ? m.thumbnail[0] : m.thumbnail); const u = iiifImage(t); if (u) out.push({ ...u, label: "" }); }
    return out;
  }
  function iiifMeta(m) {
    const fields = [];
    for (const e of m.metadata || []) { const value = iiifText(e.value); if (value) fields.push([iiifText(e.label), value]); }
    const attr = iiifText(m.attribution) || (m.requiredStatement && iiifText(m.requiredStatement.value));
    if (attr) fields.push(["Attribution", attr]);
    const rights = iiifText(m.license) || iiifText(m.rights);
    if (rights) fields.push(["Rights", rights]);
    return { label: iiifText(m.label), summary: iiifText(m.description || m.summary), fields: fields.slice(0, 30) };
  }
  function fetchIiif(url) {
    if (iiifDocs.has(url)) return iiifDocs.get(url);
    const p = fetch(up(url), { credentials: "omit", referrerPolicy: "no-referrer", headers: { Accept: "application/ld+json, application/json" } })
      .then((r) => (r.ok ? r.json() : null))
      .then((m) => (m && typeof m === "object" ? { canvases: iiifCanvases(m), meta: iiifMeta(m) } : null))
      .catch(() => null);
    iiifDocs.set(url, p);
    return p;
  }
  function iiifModal(url, doc, start) {
    let i = start || 0;
    const n = doc.canvases.length;
    const stage = h("div", { class: "rl-lb-stage" });
    const count = h("span", { class: "rl-iiif-count" });
    const prev = h("button", { type: "button", class: "rl-navbtn", "aria-label": "Previous page" }, "‹");
    const next = h("button", { type: "button", class: "rl-navbtn", "aria-label": "Next page" }, "›");
    const show = (k) => {
      i = ((k % n) + n) % n;
      stage.textContent = "";
      stage.append(img(doc.canvases[i].zoom, { eager: true, cls: "rl-lb-img" }));
      count.textContent = `page ${i + 1} / ${n}` + (doc.canvases[i].label ? " · " + doc.canvases[i].label : "");
    };
    const m = modal({ label: "IIIF viewer", cls: "rl-lightbox rl-iiif-modal", onKey: (e) => { if (e.key === "ArrowLeft") show(i - 1); else if (e.key === "ArrowRight") show(i + 1); } });
    prev.addEventListener("click", () => show(i - 1));
    next.addEventListener("click", () => show(i + 1));
    if (n < 2) { prev.hidden = true; next.hidden = true; }
    const meta = h("div", { class: "rl-iiif-meta" });
    if (doc.meta.label) meta.append(h("div", { class: "rl-iiif-title" }, doc.meta.label));
    if (doc.meta.summary) meta.append(h("p", { class: "rl-iiif-sum" }, shortText(doc.meta.summary, 600)));
    if (doc.meta.fields.length) meta.append(h("dl", { class: "rl-dl" }, doc.meta.fields.flatMap(([k, v]) => [h("dt", null, k), h("dd", null, shortText(v, 400))])));
    m.box.append(h("div", { class: "rl-iiif-body" }, h("div", { class: "rl-iiif-main" }, stage, h("div", { class: "rl-lb-bar" }, prev, count, next, sourceLink(url, "iiif"))), meta));
    show(i);
  }
  function iiifCell(url, opts) {
    const el = h("div", { class: "rl-iiif" }, h("a", { href: safeUrl(url), class: "rl-iiif-pending", title: "IIIF manifest: " + url }, h("span", { class: "rl-spin" }), "IIIF"));
    whenVisible(el, () => fetchIiif(url).then((doc) => {
      el.textContent = "";
      if (!doc || !doc.canvases.length) {
        el.append(h("a", { href: safeUrl(url), class: "rl-iiif-fail", title: "Could not read this IIIF manifest (the server may not allow cross-site reads). Open it: " + url }, "IIIF manifest ↗"));
        return;
      }
      let i = 0;
      const cs = doc.canvases;
      const frame = h("button", { type: "button", class: "rl-iiif-frame", title: (doc.meta.label ? doc.meta.label + " · " : "") + "click to enlarge" });
      const count = h("span", { class: "rl-iiif-count" });
      const show = (k) => {
        i = ((k % cs.length) + cs.length) % cs.length;
        frame.textContent = "";
        frame.append(img(cs[i].thumb, { eager: opts.eager, cls: opts.card ? "rl-cimg" : "rl-thumb" }));
        count.textContent = `${i + 1} / ${cs.length}`;
      };
      frame.addEventListener("click", (e) => { e.stopPropagation(); iiifModal(url, doc, i); });
      const nav = h("div", { class: "rl-iiif-nav" });
      if (cs.length > 1) {
        nav.append(h("button", { type: "button", class: "rl-navbtn rl-navbtn-sm", "aria-label": "Previous page", onclick: (e) => { e.stopPropagation(); show(i - 1); } }, "‹"), count,
          h("button", { type: "button", class: "rl-navbtn rl-navbtn-sm", "aria-label": "Next page", onclick: (e) => { e.stopPropagation(); show(i + 1); } }, "›"));
      }
      el.append(frame, nav, sourceLink(url, "iiif"));
      show(0);
    }));
    return el;
  }

  // ---------------------------------------------------------------- WKT
  // WKT -> GeoJSON geometry (lon/lat, Z/M dropped); null when it does not parse. A CRS IRI in front is skipped.
  function parseWkt(s) {
    s = String(s).replace(/^\s*<[^>]*>\s*/, "");
    let i = 0;
    const ws = () => { while (i < s.length && /\s/.test(s[i])) i++; };
    const eat = (c) => { ws(); if (s[i] === c) { i++; return true; } return false; };
    const coord = () => { ws(); const m = /^([-+0-9.eE]+)\s+([-+0-9.eE]+)(?:\s+[-+0-9.eE]+){0,2}/.exec(s.slice(i)); if (!m) throw 0; i += m[0].length; const x = +m[1], y = +m[2]; if (!isFinite(x) || !isFinite(y)) throw 0; return [x, y]; };
    const list = (item) => { if (!eat("(")) throw 0; const out = [item()]; while (eat(",")) out.push(item()); if (!eat(")")) throw 0; return out; };
    const pointish = () => { ws(); if (s[i] === "(") { i++; const c = coord(); if (!eat(")")) throw 0; return c; } return coord(); };
    function geom() {
      ws();
      const m = /^(POINT|LINESTRING|POLYGON|MULTIPOINT|MULTILINESTRING|MULTIPOLYGON|GEOMETRYCOLLECTION)\s*(ZM|Z|M)?\s*/i.exec(s.slice(i));
      if (!m) throw 0;
      i += m[0].length;
      if (/^EMPTY/i.test(s.slice(i))) { i += 5; return null; }
      switch (m[1].toUpperCase()) {
        case "POINT": return { type: "Point", coordinates: list(coord)[0] };
        case "LINESTRING": return { type: "LineString", coordinates: list(coord) };
        case "POLYGON": return { type: "Polygon", coordinates: list(() => list(coord)) };
        case "MULTIPOINT": return { type: "MultiPoint", coordinates: list(pointish) };
        case "MULTILINESTRING": return { type: "MultiLineString", coordinates: list(() => list(coord)) };
        case "MULTIPOLYGON": return { type: "MultiPolygon", coordinates: list(() => list(() => list(coord))) };
        default: return { type: "GeometryCollection", geometries: list(geom).filter(Boolean) };
      }
    }
    try { return geom(); } catch (_) { return null; }
  }
  // every ring / line / point of a geometry as [[x,y],...] (for bounds and the mini map)
  function rings(g, out = []) {
    if (!g) return out;
    switch (g.type) {
      case "Point": out.push({ k: "pt", c: [g.coordinates] }); break;
      case "MultiPoint": for (const c of g.coordinates) out.push({ k: "pt", c: [c] }); break;
      case "LineString": out.push({ k: "ln", c: g.coordinates }); break;
      case "MultiLineString": for (const c of g.coordinates) out.push({ k: "ln", c }); break;
      case "Polygon": for (const c of g.coordinates) out.push({ k: "pg", c }); break;
      case "MultiPolygon": for (const p of g.coordinates) for (const c of p) out.push({ k: "pg", c }); break;
      case "GeometryCollection": for (const x of g.geometries) rings(x, out); break;
    }
    return out;
  }
  function bounds(parts) {
    let minX = Infinity, maxX = -Infinity, minY = Infinity, maxY = -Infinity;
    for (const p of parts) for (const [x, y] of p.c) { if (x < minX) minX = x; if (x > maxX) maxX = x; if (y < minY) minY = y; if (y > maxY) maxY = y; }
    return isFinite(minX) ? { minX, maxX, minY, maxY } : null;
  }
  const decimate = (r, cap) => { if (r.length <= cap) return r; const step = r.length / cap, out = []; for (let k = 0; k < r.length - 1; k += step) out.push(r[Math.floor(k)]); out.push(r[r.length - 1]); return out; };
  // round ticks at a step no smaller than span / want (so at most want + 1 of them), each printed with as many decimals
  // as the step needs: a 0.02° step over a small area reads 37.04°, 37.06°, not 37.0°, 37.1°, 37.1°
  function ticks(lo, hi, want) {
    const span = hi - lo; if (!(span > 0)) return Object.assign([lo], { dec: 0 });
    const raw = span / want, mag = Math.pow(10, Math.floor(Math.log10(raw))), nrm = raw / mag, step = (nrm > 5 ? 10 : nrm > 2 ? 5 : nrm > 1 ? 2 : 1) * mag;
    const out = []; for (let v = Math.ceil(lo / step - 1e-9) * step; v <= hi + 1e-9 && out.length < 8; v += step) out.push(Math.round(v / step) * step);
    return Object.assign(out, { dec: step >= 1 ? 0 : Math.min(6, Math.ceil(-Math.log10(step) - 1e-9)) });
  }
  const fmtDeg = (v, dec) => (Math.abs(v) < Math.pow(10, -(dec || 0) - 1) ? "0" : (v < 0 ? "−" : "") + Math.abs(v).toFixed(dec || 0)) + "°";
  const svg = (tag, attrs) => { const e = document.createElementNS(SVGNS, tag); for (const [k, v] of Object.entries(attrs || {})) e.setAttribute(k, String(v)); return e; };
  // A small locator: the shape fitted to its own box on a lat/lon graticule; a point sits in a 20° window with
  // its coordinates. Offline: no basemap is fetched until the map is opened.
  function geoCell(t, opts) {
    const g = parseWkt(t.value);
    let parts = rings(g);
    if (!parts.length) return h("span", { class: "rl-geo-bad", title: t.value }, shortText(t.value, 60));
    parts = parts.sort((a, b) => b.c.length - a.c.length).slice(0, 40).map((p) => ({ k: p.k, c: decimate(p.c, 120) }));
    const b = bounds(parts);
    const isPoint = parts.length === 1 && parts[0].k === "pt";
    const ext = Math.max(b.maxX - b.minX, b.maxY - b.minY);
    const half = isPoint || ext < 5e-4 ? 10 : ext * 0.58;
    const cx = (b.minX + b.maxX) / 2, cy = (b.minY + b.maxY) / 2;
    const lonT = ticks(cx - half, cx + half, 3), latT = ticks(cy - half, cy + half, 3);
    // the left margin fits the longest latitude label (7-unit text, about 3.9 units a character); the right one half a
    // longitude label
    const chars = (list) => Math.max(0, ...list.map((v) => fmtDeg(v, list.dec).length));
    const gutL = Math.max(18, 3 + chars(latT) * 3.9), padR = chars(lonT) * 2, gutB = 12, W = 96, H = 96, vbW = gutL + W + padR, vbH = H + gutB;
    const sx = (lon) => gutL + ((lon - (cx - half)) / (2 * half)) * W;
    const sy = (lat) => ((cy + half - lat) / (2 * half)) * H;
    const root = svg("svg", { viewBox: `0 0 ${vbW.toFixed(1)} ${vbH}`, class: "rl-geo-svg", role: "img" });
    const clipId = "rlgc" + Math.random().toString(36).slice(2, 9);
    const cp = svg("clipPath", { id: clipId }); cp.append(svg("rect", { x: gutL, y: 0, width: W, height: H })); root.append(cp);
    const gr = svg("g", { "clip-path": `url(#${clipId})` });
    for (const lon of lonT) gr.append(svg("line", { class: "rl-geo-grid", x1: sx(lon).toFixed(1), y1: 0, x2: sx(lon).toFixed(1), y2: H }));
    for (const lat of latT) gr.append(svg("line", { class: "rl-geo-grid", x1: gutL, y1: sy(lat).toFixed(1), x2: vbW, y2: sy(lat).toFixed(1) }));
    for (const p of parts) {
      if (p.k === "pt") { gr.append(svg("circle", { class: "rl-geo-pt", cx: sx(p.c[0][0]).toFixed(1), cy: sy(p.c[0][1]).toFixed(1), r: isPoint ? 3.2 : 2.4 })); continue; }
      const pts = p.c.map(([x, y]) => `${sx(x).toFixed(1)},${sy(y).toFixed(1)}`).join(" ");
      gr.append(svg(p.k === "pg" ? "polygon" : "polyline", { class: p.k === "pg" ? "rl-geo-poly" : "rl-geo-line", points: pts }));
    }
    root.append(gr, svg("rect", { class: "rl-geo-bd", x: gutL, y: 0, width: W, height: H }));
    for (const lon of lonT) { const x = sx(lon); if (x < gutL - 1 || x > vbW + 1) continue; const tx = svg("text", { class: "rl-geo-tick", x: Math.min(vbW - 1, Math.max(gutL + 1, x)).toFixed(1), y: vbH - 2, "text-anchor": "middle" }); tx.textContent = fmtDeg(lon, lonT.dec); root.append(tx); }
    for (const lat of latT) { const y = sy(lat); if (y < -1 || y > H + 1) continue; const tx = svg("text", { class: "rl-geo-tick", x: gutL - 2, y: Math.min(H - 1, Math.max(6, y + 2)).toFixed(1), "text-anchor": "end" }); tx.textContent = fmtDeg(lat, latT.dec); root.append(tx); }
    const label = isPoint ? `${b.minY.toFixed(4)}, ${b.minX.toFixed(4)}` : `${b.minY.toFixed(3)},${b.minX.toFixed(3)} … ${b.maxY.toFixed(3)},${b.maxX.toFixed(3)}`;
    const btn = h("button", { type: "button", class: "rl-geo" + (opts.card ? " rl-geo-card" : ""), title: (isPoint ? "Point " : "Box ") + label + " · click to open the map" }, root);
    if (isPoint) btn.append(h("span", { class: "rl-geo-xy" }, label));
    btn.addEventListener("click", (e) => { e.stopPropagation(); window.RetelensMaps && window.RetelensMaps.openGeometry(g, opts.caption || ""); });
    return btn;
  }

  // ---------------------------------------------------------------- Markdown (small, safe)
  // Escaping is by construction (text nodes); links only for http, https and mailto.
  function mdInline(s, out) {
    const rx = /(`[^`]+`)|\[([^\]]+)\]\(([^)\s]+)\)|(\*\*|__)(.+?)\4|(\*|_)([^*_]+?)\6|(https?:\/\/[^\s<>()]+)/g;
    let last = 0, m;
    while ((m = rx.exec(s))) {
      if (m.index > last) out.push(document.createTextNode(s.slice(last, m.index)));
      if (m[1]) out.push(h("code", { class: "rl-md-code" }, m[1].slice(1, -1)));
      else if (m[2]) { const href = /^(https?:|mailto:)/i.test(m[3]) ? m[3] : null; const a = document.createElement("a"); if (href) { a.setAttribute("href", href); a.target = "_blank"; a.rel = "noopener noreferrer"; } const kids = []; mdInline(m[2], kids); a.append(...kids); out.push(a); }
      else if (m[4]) { const kids = []; mdInline(m[5], kids); out.push(h("strong", null, kids)); }
      else if (m[6]) { const kids = []; mdInline(m[7], kids); out.push(h("em", null, kids)); }
      else if (m[8]) out.push(h("a", { href: safeUrl(m[8]) }, m[8]));
      last = rx.lastIndex;
    }
    if (last < s.length) out.push(document.createTextNode(s.slice(last)));
    return out;
  }
  function markdown(text) {
    const root = h("div", { class: "rl-md" });
    const lines = String(text).replace(/\r\n?/g, "\n").split("\n");
    let i = 0;
    const para = [];
    const flush = () => { if (para.length) { const kids = []; para.forEach((l, k) => { if (k) kids.push(h("br")); mdInline(l, kids); }); root.append(h("p", null, kids)); para.length = 0; } };
    while (i < lines.length) {
      const l = lines[i];
      let m;
      if (/^```/.test(l)) { flush(); const buf = []; i++; while (i < lines.length && !/^```/.test(lines[i])) buf.push(lines[i++]); i++; root.append(h("pre", { class: "rl-md-pre" }, h("code", null, buf.join("\n")))); continue; }
      if ((m = /^(#{1,6})\s+(.*)$/.exec(l))) { flush(); root.append(h("h" + Math.min(6, m[1].length + 2), { class: "rl-md-h" }, mdInline(m[2], []))); i++; continue; }
      if (/^\s*>/.test(l)) { flush(); const buf = []; while (i < lines.length && /^\s*>/.test(lines[i])) buf.push(lines[i++].replace(/^\s*>\s?/, "")); root.append(h("blockquote", { class: "rl-md-q" }, markdown(buf.join("\n")))); continue; }
      if (/^\s*([-*+]|\d+[.)])\s+/.test(l)) {
        flush();
        const ordered = /^\s*\d/.test(l);
        const list = h(ordered ? "ol" : "ul", { class: "rl-md-list" });
        while (i < lines.length && /^\s*([-*+]|\d+[.)])\s+/.test(lines[i])) list.append(h("li", null, mdInline(lines[i++].replace(/^\s*([-*+]|\d+[.)])\s+/, ""), [])));
        root.append(list);
        continue;
      }
      if (!l.trim()) { flush(); i++; continue; }
      para.push(l); i++;
    }
    flush();
    return root;
  }

  // ---------------------------------------------------------------- page preview (opt-in)
  // A sandboxed frame at desktop width, scaled down: scripts may run but it gets no same-origin access, no top
  // navigation, popups or downloads. Many sites refuse to be framed; the link below always works.
  function pagePreview(url) {
    const box = h("div", { class: "rl-page" }, h("div", { class: "rl-page-view" }, h("span", { class: "rl-page-note" }, "Loading preview… some sites refuse to be shown in a frame.")));
    whenVisible(box, () => {
      const view = box.firstChild;
      const f = document.createElement("iframe");
      f.setAttribute("sandbox", "allow-scripts");
      f.setAttribute("referrerpolicy", "no-referrer");
      f.setAttribute("loading", "lazy");
      f.setAttribute("tabindex", "-1");
      f.setAttribute("title", "Preview of " + url);
      f.className = "rl-page-frame";
      f.src = up(url);
      view.append(f);
    });
    let host = ""; try { host = new URL(url).host; } catch (_) {}
    box.append(h("div", { class: "rl-page-host" }, host), sourceLink(url, "page"));
    return box;
  }

  // ---------------------------------------------------------------- one value
  // render(t, type, ctx, opts) -> Node. opts: {hl, card, eager, caption}. `type` "auto" picks by value.
  function render(t, type, ctx, opts = {}) {
    if (!t) return document.createTextNode("");
    const kind = !type || type === "auto" ? autoType(t) : type;
    const url = urlOf(t);
    const textOf = () => h("span", { class: "rl-text" }, termNodes(t, ctx.short, { hl: opts.hl }));
    switch (kind) {
      case "image": {
        if (!url) return textOf();
        const cls = opts.card ? "rl-cimg" : "rl-thumb";
        const im = img(thumbUrl(url, opts.card ? 640 : 200), { eager: opts.eager, cls, zoom: opts.card ? null : url, alt: opts.caption || "" });
        const a = h("button", { type: "button", class: "rl-imgbtn", title: url, "aria-label": "Enlarge image" }, im);
        a.addEventListener("click", (e) => { e.stopPropagation(); lightbox(url, opts.caption); });
        return h("div", { class: "rl-media" }, a, opts.card ? null : sourceLink(url, "image"));
      }
      case "iiif": return url ? iiifCell(url, opts) : textOf();
      case "geo": return isWkt(t) ? geoCell(t, opts) : textOf();
      case "audio": {
        if (!url) return textOf();
        const el = h("audio", { controls: true, preload: "none", class: "rl-audio" }); el.src = up(url);
        return h("div", { class: "rl-media" }, el, sourceLink(url, "audio"));
      }
      case "video": {
        if (!url) return textOf();
        const el = h("video", { controls: true, preload: "metadata", playsinline: true, class: opts.card ? "rl-cvideo" : "rl-video" }); el.src = up(url);
        return h("div", { class: "rl-media" }, el, sourceLink(url, "video"));
      }
      case "layer": {
        // a PMTiles archive (a raster or vector layer): the Map view draws it; here a button opens it in the viewer
        if (!url) return textOf();
        const name = decodeURIComponent(path(url).split("/").pop());
        const b = h("button", { type: "button", class: "rl-btnlink", title: url }, "Map layer ↗");
        b.addEventListener("click", (e) => {
          e.stopPropagation();
          if (window.RetelensHost && window.RetelensHost.openLayer) window.RetelensHost.openLayer(url, name);
          else if (EXT_PAGE) window.open(chrome.runtime.getURL("viewer.html") + "?" + new URLSearchParams({ src: url, name, host: "url" }), "_blank", "noopener");
          else chrome.runtime.sendMessage({ retelens: "openViewer", src: url, name });
        });
        return h("div", { class: "rl-media" }, b, h("span", { class: "rl-host" }, name));
      }
      case "pdf": return url ? h("div", { class: "rl-media" }, h("a", { href: url, class: "rl-btnlink", title: url }, "PDF ↗"), h("span", { class: "rl-host" }, hostOf(url))) : textOf();
      case "page": return url ? pagePreview(url) : textOf();
      case "button": return url ? h("a", { href: url, class: "rl-btnlink", title: t.value }, "Open ↗") : textOf();
      case "link": return url ? h("a", { href: url, class: "iri rl-link text-blue-600 hover:underline dark:text-blue-400", title: t.value }, HL(shortText(t.kind === "iri" ? ctx.short(t.value) : t.value, 96), opts.hl)) : textOf();
      case "markdown": {
        const md = markdown(t.value);
        if (t.lang) md.append(h("span", { class: "rl-lang" }, "@" + t.lang));
        return md;
      }
      case "number": {
        const n = Number(t.value);
        return h("span", { class: "rl-num", title: t.datatype || "" }, Number.isFinite(n) ? new Intl.NumberFormat("en-US", { maximumFractionDigits: 6 }).format(n) : t.value);
      }
      case "text": return h("span", { class: "rl-text" }, HL(t.value, opts.hl), t.lang ? h("span", { class: "ml-1 text-gray-400" }, "@" + t.lang) : null);
      default: return textOf();
    }
  }
  const hostOf = (u) => { try { return new URL(u).host; } catch (_) { return ""; } };
  const isMediaType = (k) => k === "image" || k === "iiif" || k === "geo" || k === "video" || k === "page";

  installZoom();
  window.RetelensCells = { TYPES, TYPE_LABEL, render, autoType, columnAuto, termNodes, prettyLabel, urlOf, isWkt, parseWkt, rings, bounds, looksImage, thumbUrl, img, modal, lightbox, markdown, isMediaType, WKT_RE };
})();
