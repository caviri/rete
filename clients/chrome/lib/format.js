// Shared helpers: DOM builder (text-only by construction), URL safety, number/byte
// formatting, IRI shortening. Untrusted values go in through createTextNode /
// textContent only; the only markup ever parsed is static (icons, see ICON).
(function () {
  // h(tag, attrs, ...kids): string kids become text nodes. href/src attributes are
  // refused unless http(s) (use `safeUrl` first if you need a fallback).
  function h(tag, attrs, ...kids) {
    const el = document.createElement(tag);
    if (attrs) for (const [k, v] of Object.entries(attrs)) {
      if (v == null || v === false) continue;
      if (k === "class") el.className = v;
      else if (k.startsWith("on") && typeof v === "function") el.addEventListener(k.slice(2), v);
      else if ((k === "href" || k === "src") && !safeUrl(v)) continue;
      else el.setAttribute(k, v === true ? "" : v);
    }
    for (const k of kids.flat(Infinity)) if (k != null && k !== false) el.append(k.nodeType ? k : document.createTextNode(String(k)));
    if (tag === "a" && el.hasAttribute("href")) { el.target = "_blank"; el.rel = "noopener noreferrer"; }
    return el;
  }

  // http(s) only (also rejects javascript:, data:, chrome-extension:, relative junk).
  function safeUrl(u) {
    if (typeof u !== "string") return null;
    return /^https?:\/\/[^\s<>"']+$/i.test(u) ? u : null;
  }

  // same-origin path links the adapter builds itself (never from card data)
  function pathLink(href, ...kids) {
    const a = document.createElement("a");
    a.setAttribute("href", href);
    for (const k of kids.flat()) if (k != null) a.append(k.nodeType ? k : document.createTextNode(String(k)));
    return a;
  }

  // Static SVG icons (no data in them): the only innerHTML in the code base.
  const SVG = (body, vb = "0 0 24 24", cls = "w-4 h-4 flex-none") =>
    `<svg class="${cls}" xmlns="http://www.w3.org/2000/svg" viewBox="${vb}" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">${body}</svg>`;
  const ICONS = {
    table: SVG('<rect x="3" y="4" width="18" height="16" rx="2"/><path d="M3 10h18M3 15h18M9 4v16"/>', "0 0 24 24", "mr-1.5 text-gray-400 flex-none w-3 h-3"),
    tableSmall: SVG('<rect x="3" y="4" width="18" height="16" rx="2"/><path d="M3 10h18M3 15h18M9 4v16"/>', "0 0 24 24", "mr-2 w-4 h-4 flex-none"),
    cards: SVG('<rect x="3" y="3" width="7" height="7" rx="1"/><rect x="14" y="3" width="7" height="7" rx="1"/><rect x="3" y="14" width="7" height="7" rx="1"/><rect x="14" y="14" width="7" height="7" rx="1"/>', "0 0 24 24", "w-3.5 h-3.5 flex-none"),
    tableView: SVG('<rect x="3" y="4" width="18" height="16" rx="2"/><path d="M3 10h18M3 15h18M9 4v16"/>', "0 0 24 24", "w-3.5 h-3.5 flex-none"),
    graph: SVG('<circle cx="6" cy="6" r="2.5"/><circle cx="18" cy="8" r="2.5"/><circle cx="12" cy="18" r="2.5"/><path d="M8 7l8 1M7.5 8l3.5 8M16.5 10.5L13.5 16"/>', "0 0 24 24", "w-3.5 h-3.5 flex-none"),
    map: SVG('<path d="M9 4L3 6v14l6-2 6 2 6-2V4l-6 2-6-2z"/><path d="M9 4v14M15 6v14"/>', "0 0 24 24", "w-3.5 h-3.5 flex-none"),
    page: SVG('<path d="M7 3h7l5 5v13H7z"/><path d="M14 3v5h5M10 13h6M10 17h6"/>', "0 0 24 24", "w-4 h-4 flex-none"),
    graphChip: SVG('<circle cx="6" cy="6" r="2.5"/><circle cx="18" cy="8" r="2.5"/><circle cx="12" cy="18" r="2.5"/><path d="M8 7l8 1M7.5 8l3.5 8M16.5 10.5L13.5 16"/>', "0 0 24 24", "mr-1 w-3.5 h-3.5 flex-none text-gray-500"),
    license: SVG('<path d="M12 3v18M5 7h14M5 7l-2 8a3.5 3.5 0 0 0 7 0L8 7M19 7l-2 8a3.5 3.5 0 0 0 7 0l-2-8" transform="translate(-1.5 0)"/>', "0 0 24 24", "mr-1 w-3.5 h-3.5 flex-none text-gray-500"),
    size: SVG('<ellipse cx="12" cy="5" rx="8" ry="3"/><path d="M4 5v14c0 1.7 3.6 3 8 3s8-1.3 8-3V5M4 12c0 1.7 3.6 3 8 3s8-1.3 8-3"/>', "0 0 24 24", "mr-1 w-3.5 h-3.5 flex-none text-gray-500"),
    download: SVG('<path d="M12 3v12M7 10l5 5 5-5M4 19h16"/>', "0 0 24 24", "w-4 h-4 flex-none"),
    search: SVG('<circle cx="11" cy="11" r="7"/><path d="M21 21l-4.3-4.3"/>', "0 0 24 24", "w-4 h-4 flex-none"),
    chevron: SVG('<path d="M6 9l6 6 6-6"/>', "0 0 24 24", "ml-auto w-4 h-4 flex-none text-gray-500"),
    back: SVG('<path d="M19 12H5M12 19l-7-7 7-7"/>', "0 0 24 24", "w-4 h-4 flex-none"),
    external: SVG('<path d="M14 4h6v6M20 4l-9 9M18 14v5a1 1 0 0 1-1 1H5a1 1 0 0 1-1-1V7a1 1 0 0 1 1-1h5"/>', "0 0 24 24", "w-3 h-3 flex-none"),
    filter: SVG('<path d="M3 5h18l-7 8v6l-4 2v-8z"/>', "0 0 24 24", "w-4 h-4 flex-none"),
    sortAsc: SVG('<path d="M12 19V5M5 12l7-7 7 7"/>', "0 0 24 24", "w-4 h-4 flex-none"),
    sortDesc: SVG('<path d="M12 5v14M19 12l-7 7-7-7"/>', "0 0 24 24", "w-4 h-4 flex-none"),
    sortNone: SVG('<path d="M8 9l4-4 4 4M16 15l-4 4-4-4"/>', "0 0 24 24", "w-4 h-4 flex-none"),
    x: SVG('<path d="M18 6L6 18M6 6l12 12"/>', "0 0 24 24", "w-3 h-3 flex-none"),
    chevronUp: SVG('<path d="M6 15l6-6 6 6"/>', "0 0 24 24", "w-4 h-4 flex-none"),
    chevronRight: SVG('<path d="M9 6l6 6-6 6"/>', "0 0 24 24", "w-4 h-4 flex-none"),
    caret: SVG('<path d="M6 9l6 6 6-6"/>', "0 0 24 24", "ml-auto w-4 h-4 flex-none text-gray-500"),
    check: SVG('<path d="M5 12l5 5 9-10"/>', "0 0 24 24", "w-4 h-4 flex-none"),
    panel: '<div class="rounded-sm flex w-4 h-4 items-center justify-center border border-gray-400 bg-gray-100 hover:border-gray-600 dark:border-gray-600 dark:bg-gray-800"><div class="float-left h-full w-[65%]"></div><div class="float-right h-full w-[35%] bg-gray-400 dark:bg-gray-600"></div></div>',
  };
  // GitHub skin: Octicons (lib/octicons.js) instead of the HF-style glyphs
  const OCT = { table: "table", tableSmall: "table", tableView: "table", cards: "apps", graph: "workflow", graphChip: "workflow", license: "info", size: "file-binary", search: "search", chevron: "chevron-down", back: "arrow-left", download: "download", external: "link-external", copy: "copy", filter: "filter", sortAsc: "arrow-up", sortDesc: "arrow-down", sortNone: "triangle-down", x: "x", chevronUp: "chevron-up", chevronRight: "chevron-right", check: "check", caret: "triangle-down", kebab: "kebab-horizontal", play: "play", code: "code", info: "info" };
  let skin = "hf";
  function setSkin(s) { skin = s; }
  function icon(name) {
    const t = document.createElement("template");
    const oc = skin === "github" && window.RetelensOcticons && window.RetelensOcticons[OCT[name]];
    if (oc) {
      t.innerHTML = `<svg class="octicon flex-none" xmlns="http://www.w3.org/2000/svg" viewBox="0 0 16 16" width="16" height="16" fill="currentColor" aria-hidden="true">${oc.map((d) => `<path d="${d}"></path>`).join("")}</svg>`; // static path data
      return t.content.firstElementChild;
    }
    t.innerHTML = ICONS[name] || ICONS.info || ""; // static template, no data
    return t.content.firstElementChild;
  }

  const nf = new Intl.NumberFormat("en-US");
  const num = (n) => (typeof n === "number" ? nf.format(n) : String(n));
  function bytes(n) {
    if (n == null) return "?";
    const u = ["B", "kB", "MB", "GB", "TB"];
    let i = 0;
    while (n >= 1000 && i < u.length - 1) { n /= 1000; i++; }
    return (i ? n.toFixed(n < 10 ? 2 : 1) : n) + " " + u[i];
  }

  const PREFIXES = {
    rdf: "http://www.w3.org/1999/02/22-rdf-syntax-ns#", rdfs: "http://www.w3.org/2000/01/rdf-schema#",
    owl: "http://www.w3.org/2002/07/owl#", xsd: "http://www.w3.org/2001/XMLSchema#",
    skos: "http://www.w3.org/2004/02/skos/core#", foaf: "http://xmlns.com/foaf/0.1/",
    dct: "http://purl.org/dc/terms/", dc: "http://purl.org/dc/elements/1.1/",
    schema: "http://schema.org/", schemas: "https://schema.org/", void: "http://rdfs.org/ns/void#",
    prov: "http://www.w3.org/ns/prov#", geo: "http://www.opengis.net/ont/geosparql#",
    wgs: "http://www.w3.org/2003/01/geo/wgs84_pos#", time: "http://www.w3.org/2006/time#",
    dbo: "http://dbpedia.org/ontology/", dbr: "http://dbpedia.org/resource/", dbp: "http://dbpedia.org/property/",
    rel: "http://purl.org/vocab/relationship/", mo: "http://purl.org/ontology/mo/",
    wd: "http://www.wikidata.org/entity/", wdt: "http://www.wikidata.org/prop/direct/",
    obo: "http://purl.obolibrary.org/obo/", bibo: "http://purl.org/ontology/bibo/",
    cito: "http://purl.org/spar/cito/", fabio: "http://purl.org/spar/fabio/",
    oc: "https://w3id.org/oc/ontology/", dcat: "http://www.w3.org/ns/dcat#",
  };

  class Prefixes {
    constructor() { this.map = { ...PREFIXES }; }
    learn(text) {
      for (const m of String(text || "").matchAll(/PREFIX\s+([A-Za-z][\w-]*)?:\s*<([^>]*)>/gi)) {
        if (m[1] && !(m[1] in this.map)) this.map[m[1]] = m[2];
      }
    }
    shorten(iri) {
      let best = null;
      for (const [p, ns] of Object.entries(this.map)) {
        if (iri.startsWith(ns) && (!best || ns.length > best[1].length)) best = [p, ns];
      }
      if (best) {
        const local = iri.slice(best[1].length);
        if (/^[\w.\-~%]*$/.test(local)) return best[0] + ":" + local;
      }
      return iri;
    }
  }

  // a plain download link (no "downloads" permission): href must be http(s)
  function dlLink(href, cls, ...kids) {
    if (!safeUrl(href)) return null;
    const a = document.createElement("a");
    a.setAttribute("href", href);
    a.setAttribute("download", "");
    a.setAttribute("rel", "noopener noreferrer");
    a.className = cls || "";
    for (const k of kids.flat()) if (k != null) a.append(k.nodeType ? k : document.createTextNode(String(k)));
    return a;
  }

  window.RetelensFmt = { h, num, bytes, Prefixes, safeUrl, pathLink, icon, dlLink, setSkin };
})();
