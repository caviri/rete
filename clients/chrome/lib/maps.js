// Maps: the Map view of a result (a WKT column, or latitude/longitude columns), the map a geometry cell opens, and
// PMTiles: a .pmtiles file at a URL, or a PMTiles archive embedded as a section of a .rete (read through the
// engine, so it shares its chunk cache and rate-limit handling). Leaflet + protomaps-leaflet + pmtiles live in
// lib/mapkit/mapkit.js, loaded on first use.
//
// Safety: map labels, popups and legends are DOM built with h() (text nodes). Leaflet sets attribution with
// innerHTML, so only static strings or escaped text from a file go there. Basemap tiles are fetched only when a
// map is open; on a host page the raster tiles come through fetch + blob: URLs (GitHub's CSP refuses outside
// images, not blob:).
(function () {
  const { h, num, bytes, safeUrl } = window.RetelensFmt;
  const C = () => window.RetelensCells;
  const EXT_PAGE = location.protocol === "chrome-extension:";

  // ---------------------------------------------------------------- loading the libraries
  let kitP = null;
  function ensure() {
    if (window.RetelensMapKit) return Promise.resolve(withCss(window.RetelensMapKit));
    kitP = kitP || new Promise((resolve, reject) => {
      const done = () => (window.RetelensMapKit ? resolve(withCss(window.RetelensMapKit)) : reject(new Error("the map library did not load")));
      if (EXT_PAGE) {
        const s = document.createElement("script");
        s.src = chrome.runtime.getURL("lib/mapkit/mapkit.js");
        s.onload = done;
        s.onerror = () => reject(new Error("the map library did not load"));
        document.head.append(s);
        return;
      }
      chrome.runtime.sendMessage({ retelens: "loadMapKit" }, (r) => (chrome.runtime.lastError || !r || !r.ok
        ? reject(new Error("the map library did not load: " + ((chrome.runtime.lastError && chrome.runtime.lastError.message) || (r && r.error) || "no answer")))
        : done()));
    });
    kitP.catch(() => { kitP = null; });
    return kitP;
  }
  function withCss(kit) {
    if (!document.querySelector("style[data-retelens-leaflet]")) {
      const st = document.createElement("style");
      st.setAttribute("data-retelens-leaflet", "");
      st.textContent = kit.css; // Leaflet's own stylesheet, bundled
      (document.head || document.documentElement).append(st);
    }
    return kit;
  }
  const escHtml = (s) => String(s).replace(/[&<>"']/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" })[c]);
  const isDark = (el) => { try { return window.RetelensTheme.tokens(el || document.body).dark; } catch (_) { return matchMedia("(prefers-color-scheme: dark)").matches; } };
  function accent(el) { try { return window.RetelensTheme.tokens(el || document.body).accent; } catch (_) { return "#2563eb"; } }

  // ---------------------------------------------------------------- basemap: an open PMTiles archive
  // The basemap is a PMTiles archive of open data, read by HTTP Range like any other file. By default: OpenStreetMap
  // as the Protomaps basemap (streets, water, land use, places), a world extract to zoom 12 of the 2026-11-24 build,
  // on data.graphplaza.com and in the rete-public bucket (osm-basemap/, with its README); drawn in the Protomaps light or
  // dark style, deeper zooms stretched from z12. The popup's Advanced section sets another URL; any vector archive gets
  // a quiet generic style (basemapLayer). Nothing is read before a map is open. (Keyless raster services, measured
  // 2026-11-24 from an extension page: Carto answers "API key required", tile.openstreetmap.org "Access blocked".)
  const DEFAULT_BASEMAP = "https://data.graphplaza.com/osm-basemap/osm-basemap-z12-2026-11-24.pmtiles";
  const DEFAULT_ATTR = '<a href="https://protomaps.com" target="_blank" rel="noopener noreferrer">Protomaps</a> &copy; <a href="https://www.openstreetmap.org/copyright" target="_blank" rel="noopener noreferrer">OpenStreetMap</a>';
  async function basemapUrl() {
    try { const st = await chrome.storage.local.get("basemapUrl"); return safeUrl(st.basemapUrl) || DEFAULT_BASEMAP; } catch (_) { return DEFAULT_BASEMAP; }
  }
  const baseArchives = new Map(); // url -> Promise<archive>
  function basemapArchive(kit) {
    return basemapUrl().then((url) => {
      if (!baseArchives.has(url)) {
        const p = openPmtiles(kit, new kit.pmtiles.FetchSource(url)).then((t) => ({ ...t, url }));
        p.catch(() => baseArchives.delete(url));
        baseArchives.set(url, p);
      }
      return baseArchives.get(url);
    });
  }
  const BASE_COLORS = {
    light: { water: "#e4ebf1", land: "#f8f7f4", border: ["#a59e93", "#cbc5bb", "#e0dbd3"], dot: "#6b6f76", text: "#4b5563", halo: "#ffffff" },
    dark: { water: "#0e151d", land: "#1b212a", border: ["#5a6474", "#3b4451", "#2c333d"], dot: "#a3acb7", text: "#c4cbd4", halo: "#0e151d" },
  };
  // A quiet style for any vector archive, so the data drawn on top stands out: polygon layers ordered by feature
  // count (the archive's tilestats), the coarsest filled as land from z0, the next ones as borders from z3, z6, z8;
  // line layers as thin lines; point layers as dots from z2 and labels from z3 (a name field; with a population-like
  // number field, the bigger places first and the small ones only zoomed in).
  function basemapLayer(kit, t, dark) {
    const { P, pmtiles } = kit;
    const attribution = t.url === DEFAULT_BASEMAP ? DEFAULT_ATTR : t.meta && t.meta.attribution ? escHtml(String(t.meta.attribution).replace(/<[^>]*>/g, "")) : "PMTiles";
    if (t.header.tileType !== pmtiles.TileType.Mvt) return pmtiles.leafletRasterLayer(t.pm, { attribution, maxNativeZoom: t.header.maxZoom, maxZoom: 20 });
    const layers = vectorLayers(t);
    const ids = layers.map((l) => l.id);
    if (ids.includes("earth") && ids.includes("water")) return P.leafletLayer({ url: t.pm, flavor: dark ? "dark" : "light", lang: "en", attribution, maxDataZoom: t.header.maxZoom });
    const c = BASE_COLORS[dark ? "dark" : "light"];
    const ts = new Map(((t.meta && t.meta.tilestats && t.meta.tilestats.layers) || []).map((l) => [l.layer, l]));
    const geom = (l) => String((ts.get(l.id) || {}).geometry || "");
    const count = (l) => Number((ts.get(l.id) || {}).count || 0);
    const polys = layers.filter((l) => /polygon/i.test(geom(l)) || !geom(l)).sort((a, b) => count(a) - count(b));
    const points = layers.filter((l) => /point/i.test(geom(l)) || !geom(l));
    const lines = layers.filter((l) => /line/i.test(geom(l)) || !geom(l));
    const MINZ = [0, 3, 6, 8];
    const paintRules = [];
    polys.forEach((l, i) => paintRules.push({ dataLayer: l.id, minzoom: MINZ[Math.min(i, 3)], filter: (z, f) => f.geomType === 3,
      symbolizer: new P.PolygonSymbolizer({ fill: c.land, opacity: i === 0 ? 1 : 0, stroke: c.border[Math.min(i, 2)], width: i === 0 ? 0.8 : 0.5 }) }));
    for (const l of lines) paintRules.push({ dataLayer: l.id, filter: (z, f) => f.geomType === 2, symbolizer: new P.LineSymbolizer({ color: c.border[1], width: 0.6 }) });
    const labelRules = [];
    for (const l of points) {
      const fields = Object.keys(l.fields);
      const nameF = fields.find((f) => /^name$/i.test(f)) || fields.find((f) => /name/i.test(f));
      const popF = fields.find((f) => /pop|rank/i.test(f) && /number/i.test(String(l.fields[f])));
      const big = (f, z) => !popF || Number(f.props[popF]) >= (z < 3 ? 5e6 : z < 5 ? 1e6 : z < 7 ? 2e5 : 0);
      paintRules.push({ dataLayer: l.id, minzoom: 2, filter: (z, f) => f.geomType === 1 && big(f, z), symbolizer: new P.CircleSymbolizer({ radius: 1.8, fill: c.dot, stroke: c.halo, width: 0.6 }) });
      if (nameF) labelRules.push({ dataLayer: l.id, minzoom: 3, filter: (z, f) => f.geomType === 1 && big(f, z),
        sort: popF ? (a, b) => Number(b[popF] || 0) - Number(a[popF] || 0) : undefined,
        symbolizer: new P.OffsetTextSymbolizer({ labelProps: [nameF], fill: c.text, stroke: c.halo, width: 2, font: "500 11px ui-sans-serif, system-ui, sans-serif", offsetX: 4, offsetY: 4 }) });
    }
    return P.leafletLayer({ url: t.pm, paintRules, labelRules, backgroundColor: c.water, attribution, maxDataZoom: t.header.maxZoom });
  }

  // ---------------------------------------------------------------- PMTiles
  // fixed categorical order; the same layer keeps its colour
  const PAL_LIGHT = ["#2a6fb0", "#c8641e", "#2f8a4c", "#8a4fb0", "#b8406e", "#6f7a1f", "#1f8a8a", "#8a6440"];
  const PAL_DARK = ["#6aa8e8", "#f09a55", "#6cc489", "#c08ae6", "#ec7ba4", "#b7c45a", "#5ccaca", "#d0a27a"];
  const TILE_TYPE = { 0: "unknown", 1: "vector (MVT)", 2: "PNG", 3: "JPEG", 4: "WebP", 5: "AVIF" };

  // A pmtiles Source over the engine: the archive is a section of the open .rete, `base` bytes in
  function engineSource(engine, key, base) {
    return {
      getKey: () => key,
      getBytes: async (offset, length) => ({ data: await engine.call("readRange", { offset: base + offset, length }) }),
    };
  }
  async function openPmtiles(kit, source) {
    const pm = new kit.pmtiles.PMTiles(source);
    const header = await pm.getHeader();
    let meta = {};
    try { meta = (await pm.getMetadata()) || {}; } catch (_) {}
    return { pm, header, meta };
  }
  // a layer for an archive: raster tiles as images; the Protomaps basemap schema with its own style; any other
  // vector archive with one colour per layer (polygons, lines, points), `hits` drawn on top in the accent colour
  function pmtilesLayer(kit, t, opts = {}) {
    const { P, pmtiles } = kit;
    const attribution = t.meta && t.meta.attribution ? escHtml(String(t.meta.attribution).replace(/<[^>]*>/g, "")) : "PMTiles";
    if (t.header.tileType !== pmtiles.TileType.Mvt) {
      return pmtiles.leafletRasterLayer(t.pm, { attribution, maxNativeZoom: t.header.maxZoom, maxZoom: Math.max(t.header.maxZoom + 3, 18) });
    }
    const layers = vectorLayers(t);
    const ids = layers.map((l) => l.id);
    if (ids.includes("earth") && ids.includes("water") && !opts.plain) {
      return P.leafletLayer({ url: t.pm, flavor: opts.dark ? "dark" : "light", lang: "en", attribution, maxDataZoom: t.header.maxZoom });
    }
    const pal = opts.dark ? PAL_DARK : PAL_LIGHT;
    const surface = opts.dark ? "#0b0f19" : "#ffffff";
    const hide = opts.hidden || new Set();
    const hits = opts.hits && opts.hits.size ? opts.hits : null;
    const hit = (f) => { for (const v of Object.values(f.props || {})) if ((typeof v === "string" || typeof v === "number") && hits.has(String(v))) return true; return false; };
    const paintRules = [];
    // Only the first shown layer is filled; the others are outlines. Nested polygon layers (countries, regions,
    // districts) filled on top of each other turn into one muddy colour.
    let filled = false;
    layers.forEach((l, i) => {
      if (hide.has(l.id)) return;
      const c = pal[i % pal.length];
      const fill = !filled; filled = true;
      paintRules.push(
        { dataLayer: l.id, symbolizer: new P.PolygonSymbolizer({ fill: c, opacity: fill ? (hits ? 0.1 : 0.2) : 0, stroke: c, width: (z) => (z < 4 ? 0.4 : 0.8) }), filter: (z, f) => f.geomType === 3 },
        { dataLayer: l.id, symbolizer: new P.LineSymbolizer({ color: c, width: 1.2, opacity: hits ? 0.5 : 0.9 }), filter: (z, f) => f.geomType === 2 },
        { dataLayer: l.id, symbolizer: new P.CircleSymbolizer({ radius: 2.6, fill: c, stroke: surface, width: 0.8, opacity: hits ? 0.5 : 1 }), filter: (z, f) => f.geomType === 1 });
      if (hits) {
        const a = opts.accent || "#2563eb";
        paintRules.push(
          { dataLayer: l.id, symbolizer: new P.PolygonSymbolizer({ fill: a, opacity: 0.5, stroke: a, width: 1.6 }), filter: (z, f) => f.geomType === 3 && hit(f) },
          { dataLayer: l.id, symbolizer: new P.LineSymbolizer({ color: a, width: 2.4 }), filter: (z, f) => f.geomType === 2 && hit(f) },
          { dataLayer: l.id, symbolizer: new P.CircleSymbolizer({ radius: 4.5, fill: a, stroke: surface, width: 1.2 }), filter: (z, f) => f.geomType === 1 && hit(f) });
      }
    });
    return P.leafletLayer({ url: t.pm, paintRules, labelRules: [], attribution, maxDataZoom: t.header.maxZoom, backgroundColor: opts.background || undefined });
  }
  function vectorLayers(t) {
    const vl = t.meta && Array.isArray(t.meta.vector_layers) ? t.meta.vector_layers : [];
    return vl.filter((l) => l && typeof l.id === "string").map((l) => ({ id: l.id, fields: l.fields && typeof l.fields === "object" ? l.fields : {}, minzoom: l.minzoom, maxzoom: l.maxzoom, description: typeof l.description === "string" ? l.description : "" }));
  }
  const headerBounds = (hd) => (hd && isFinite(hd.minLat) && hd.maxLat > hd.minLat ? [[hd.minLat, hd.minLon], [hd.maxLat, hd.maxLon]] : null);
  // what is under a click on a vector layer, as a popup of its properties
  function pickPopup(kit, map, layer, ev) {
    if (!layer || !layer.queryTileFeaturesDebug) return;
    let picked;
    try { picked = layer.queryTileFeaturesDebug(ev.latlng.lng, ev.latlng.lat, 8); } catch (_) { return; }
    const list = [];
    for (const arr of picked.values()) for (const p of arr) list.push(p);
    if (!list.length) return;
    const box = h("div", { class: "rl-popup" });
    for (const p of list.slice(0, 4)) {
      const props = Object.entries((p.feature && p.feature.props) || {}).slice(0, 24);
      box.append(h("div", { class: "rl-popup-layer" }, p.layerName || "feature"),
        props.length ? h("dl", { class: "rl-dl" }, props.flatMap(([k, v]) => [h("dt", null, k), h("dd", null, String(v).slice(0, 300))])) : h("div", { class: "rl-muted" }, "no properties"));
    }
    if (list.length > 4) box.append(h("div", { class: "rl-muted" }, `+${list.length - 4} more here`));
    kit.L.popup({ maxWidth: 360 }).setLatLng(ev.latlng).setContent(box).openOn(map);
  }

  // ---------------------------------------------------------------- features from result rows
  const isNumTerm = (t) => !!t && t.kind === "literal" && Number.isFinite(Number(t.value)) && String(t.value).trim() !== "";
  // which columns carry geometry: a WKT column, else a latitude + longitude pair
  function geoColumns(vars, rows) {
    let wkt = null, best = 0;
    for (const v of vars) { let n = 0; for (const r of rows.slice(0, 200)) if (C().isWkt(r[v])) n++; if (n > best) { best = n; wkt = v; } }
    if (wkt) return { wkt };
    const lat = vars.find((v) => /(^|[^a-z])lat(itude)?$/i.test(v) || /latitude$/i.test(v));
    const lon = vars.find((v) => /(^|[^a-z])(lon|lng|long)$/i.test(v) || /longitude$/i.test(v));
    if (lat && lon && rows.slice(0, 200).some((r) => isNumTerm(r[lat]) && isNumTerm(r[lon]) && Math.abs(+r[lat].value) <= 90 && Math.abs(+r[lon].value) <= 180)) return { lat, lon };
    return null;
  }
  function labelVar(vars, geo) {
    const skip = new Set([geo.wkt, geo.lat, geo.lon]);
    const rest = vars.filter((v) => !skip.has(v));
    return rest.find((v) => window.RetelensViews && window.RetelensViews.nameLike(v)) || rest[0] || null;
  }
  const termText = (t, short) => (!t ? "" : t.kind === "iri" ? short(t.value) : String(t.value));
  function toFeatures(rows, vars, geo, short) {
    const lab = labelVar(vars, geo);
    const feats = [];
    for (const r of rows) {
      let g = null;
      if (geo.wkt) { if (C().isWkt(r[geo.wkt])) g = C().parseWkt(r[geo.wkt].value); }
      else if (isNumTerm(r[geo.lat]) && isNumTerm(r[geo.lon])) g = { type: "Point", coordinates: [+r[geo.lon].value, +r[geo.lat].value] };
      if (!g) continue;
      feats.push({ type: "Feature", geometry: g, properties: { label: lab ? termText(r[lab], short) : "", row: r } });
    }
    return feats;
  }
  // the literal values of a result: features of embedded tiles whose properties hold one of them are drawn on top
  function resultValues(rows, vars, geo) {
    const out = new Set();
    for (const r of rows) for (const v of vars) {
      if (geo && (v === geo.wkt || v === geo.lat || v === geo.lon)) continue;
      const t = r[v];
      if (t && t.kind === "literal" && t.value.length < 200 && !C().isWkt(t)) out.add(String(t.value));
    }
    return out;
  }
  function rowPopup(row, vars, ctx, geo) {
    const box = h("div", { class: "rl-popup" });
    const dl = h("dl", { class: "rl-dl" });
    for (const v of vars) {
      if (!row[v] || (geo && v === geo.wkt)) continue;
      dl.append(h("dt", { title: "?" + v }, C().prettyLabel(v)), h("dd", null, C().render(row[v], "auto", ctx, { card: false })));
    }
    box.append(dl);
    return box;
  }

  // ---------------------------------------------------------------- one map (shared by the Map view, the
  // geometry modal and the PMTiles viewer): basemap choice, features, optional tiles
  function buildMap(kit, el, o) {
    const L = kit.L;
    const dark = isDark(el);
    const map = L.map(el, { worldCopyJump: true, zoomControl: true, attributionControl: true, preferCanvas: true });
    map.attributionControl.setPrefix('<a href="https://leafletjs.com" target="_blank" rel="noopener noreferrer">Leaflet</a>');
    let base = null, baseId = null, tilesLayer = null, dead = false;
    // "map": the open PMTiles basemap; "tiles": the archive in the file is the basemap; "none"
    const setBase = async (id) => {
      baseId = id;
      if (base) { map.removeLayer(base); base = null; }
      if (id !== "map") return;
      let t;
      try { t = await basemapArchive(kit); }
      catch (e) { if (o.onBaseError) o.onBaseError(e); return; }
      if (dead || baseId !== "map" || base) return;
      base = basemapLayer(kit, t, dark);
      base.addTo(map);
      if (base.bringToBack) base.bringToBack();
    };
    if (o.tiles) {
      tilesLayer = pmtilesLayer(kit, o.tiles, { dark, hits: o.hits, accent: accent(el), hidden: o.hiddenLayers });
      tilesLayer.addTo(map);
      map.on("click", (ev) => pickPopup(kit, map, tilesLayer, ev));
    }
    setBase(o.basemap || (o.tiles ? "tiles" : "map"));
    let fl = null;
    if (o.features && o.features.length) {
      const col = accent(el);
      fl = L.geoJSON({ type: "FeatureCollection", features: o.features }, {
        style: () => ({ color: col, weight: 1.6, opacity: 0.9, fillColor: col, fillOpacity: 0.22 }),
        pointToLayer: (f, ll) => L.circleMarker(ll, { radius: 5, color: dark ? "#0b0f19" : "#ffffff", weight: 1.5, fillColor: col, fillOpacity: 0.9 }),
        onEachFeature: (f, layer) => {
          if (f.properties.label) layer.bindTooltip(h("span", null, f.properties.label), { sticky: true, direction: "top" });
          if (o.popup) layer.on("click", (e) => { L.DomEvent.stopPropagation(e); L.popup({ maxWidth: 380 }).setLatLng(e.latlng).setContent(o.popup(f)).openOn(map); });
        },
      }).addTo(map);
    }
    const fit = () => {
      let b = null;
      try { if (fl) b = fl.getBounds(); } catch (_) {}
      if (b && b.isValid()) map.fitBounds(b, { padding: [24, 24], maxZoom: 12 });
      else if (o.tiles && headerBounds(o.tiles.header)) map.fitBounds(headerBounds(o.tiles.header), { padding: [8, 8] });
      else map.setView([20, 0], 2);
    };
    fit();
    // a map built while hidden (or inside a dialog that is still opening) has no size yet
    const ro = new ResizeObserver(() => map.invalidateSize());
    ro.observe(el);
    return { map, setBase, fit, tilesLayer, destroy: () => { dead = true; ro.disconnect(); map.remove(); } };
  }
  function baseSelect(current, hasTiles, onChange) {
    const sel = h("select", { class: "rl-select", "aria-label": "Basemap" });
    const opts = [...(hasTiles ? [{ id: "tiles", label: "Tiles in this file" }] : []), { id: "map", label: "Open basemap (PMTiles)" }, { id: "none", label: "No basemap" }];
    for (const b of opts) { const op = h("option", { value: b.id }, b.label); if (b.id === current) op.selected = true; sel.append(op); }
    sel.addEventListener("change", () => onChange(sel.value));
    return h("label", { class: "rl-mapctl" }, h("span", null, "Basemap"), sel);
  }

  // ---------------------------------------------------------------- the Map view of a result
  // opts: {tiles: () => Promise<tilesInfo|null>, height}
  function MapView(rows, vars, ctx, opts = {}) {
    const geo = geoColumns(vars, rows);
    const el = h("div", { "data-rl": "mapbox", class: "rl-mapview" });
    const bar = h("div", { class: "rl-mapbar" });
    const mapEl = h("div", { class: "rl-map" });
    if (opts.height) mapEl.style.height = opts.height;
    const note = h("span", { class: "rl-muted" }, "Loading the map…");
    bar.append(note);
    el.append(bar, mapEl);
    let m = null, dead = false;
    (async () => {
      const kit = await ensure();
      let tiles = null;
      try { tiles = opts.tiles ? await opts.tiles() : null; } catch (_) { tiles = null; }
      if (dead) return;
      const feats = geo ? toFeatures(rows, vars, geo, ctx.short) : [];
      const hits = tiles ? resultValues(rows, vars, geo) : null;
      m = buildMap(kit, mapEl, { features: feats, tiles, hits, popup: (f) => rowPopup(f.properties.row, vars, ctx, geo),
        onBaseError: (e) => bar.append(h("span", { class: "rl-err" }, "The basemap could not be read (" + String(e.message || e).slice(0, 120) + ").")) });
      bar.textContent = "";
      bar.append(baseSelect(tiles ? "tiles" : "map", !!tiles, (id) => m.setBase(id)));
      const parts = [];
      if (geo) parts.push(feats.length === rows.length ? `${num(feats.length)} ${feats.length === 1 ? "place" : "places"}` : `${num(feats.length)} of ${num(rows.length)} rows have a ${geo.wkt ? "geometry (" + geo.wkt + ")" : "position"}`);
      if (tiles) parts.push(`vector tiles from a PMTiles section inside this file (${bytes(tiles.length)})${hits && hits.size ? ", features named in the result drawn on top" : ""}`);
      if (!geo && !tiles) parts.push("No geometry in this result: the Map view needs a WKT column or latitude and longitude columns.");
      bar.append(h("span", { class: "rl-muted" }, parts.join(" · ")));
    })().catch((e) => { if (!dead) { bar.textContent = ""; bar.append(h("span", { class: "rl-err" }, String(e.message || e))); } });
    return { el, destroy: () => { dead = true; if (m) m.destroy(); } };
  }

  // ---------------------------------------------------------------- a geometry cell's map
  function openGeometry(g, caption) {
    if (!g) return;
    const md = C().modal({ label: "Map", cls: "rl-mapmodal" });
    const mapEl = h("div", { class: "rl-map rl-map-modal" });
    const bar = h("div", { class: "rl-mapbar" }, h("span", { class: "rl-muted" }, "Loading the map…"));
    md.box.append(bar, mapEl);
    ensure().then((kit) => {
      if (!md.root.isConnected) return;
      const m = buildMap(kit, mapEl, { features: [{ type: "Feature", geometry: g, properties: { label: caption || "" } }] });
      bar.textContent = "";
      bar.append(baseSelect("map", false, (id) => m.setBase(id)));
      const parts = C().rings(g), b = C().bounds(parts);
      if (b) bar.append(h("span", { class: "rl-muted" }, g.type + " · " + (b.minX === b.maxX && b.minY === b.maxY ? `${b.minY.toFixed(5)}, ${b.minX.toFixed(5)}` : `${b.minY.toFixed(4)}, ${b.minX.toFixed(4)} … ${b.maxY.toFixed(4)}, ${b.maxX.toFixed(4)}`)));
      const obs = new MutationObserver(() => { if (!md.root.isConnected) { obs.disconnect(); m.destroy(); } });
      obs.observe(document.body, { childList: true });
    }).catch((e) => { bar.textContent = ""; bar.append(h("span", { class: "rl-err" }, String(e.message || e))); });
  }

  // ---------------------------------------------------------------- PMTiles viewer (a .pmtiles URL)
  // The archive's header and metadata next to the map: layers with their fields (one colour each, each can be
  // hidden), zooms, bounds, tile type, attribution. A click lists the features under the pointer.
  function PmtilesViewer(url, opts = {}) {
    const el = h("div", { class: "rl-pmviewer" });
    const mapEl = h("div", { class: "rl-map rl-map-full" });
    const side = h("aside", { class: "rl-pmside" }, h("div", { class: "rl-muted" }, "Reading the archive header…"));
    const bar = h("div", { class: "rl-mapbar" });
    el.append(h("div", { class: "rl-pmmain" }, bar, mapEl), side);
    let m = null;
    (async () => {
      const kit = await ensure();
      const src = typeof opts.source === "function" ? opts.source(kit) : opts.source || new kit.pmtiles.FetchSource(url);
      const t = await openPmtiles(kit, src);
      const hidden = new Set();
      const dark = isDark(el);
      const layers = vectorLayers(t);
      m = buildMap(kit, mapEl, { tiles: t, hiddenLayers: hidden, basemap: "none" });
      let base = "none";
      const redraw = () => {
        if (m.tilesLayer) m.map.removeLayer(m.tilesLayer);
        m.tilesLayer = pmtilesLayer(kit, t, { dark, hidden, plain: plain.checked });
        m.tilesLayer.addTo(m.map);
        m.setBase(base);
      };
      m.map.off("click");
      m.map.on("click", (ev) => pickPopup(kit, m.map, m.tilesLayer, ev));
      bar.append(baseSelect("none", false, (id) => { base = id; m.setBase(id); }));
      const plain = h("input", { type: "checkbox" });
      const isBasemap = layers.some((l) => l.id === "earth") && layers.some((l) => l.id === "water");
      if (isBasemap) { plain.addEventListener("change", redraw); bar.append(h("label", { class: "rl-mapctl" }, plain, h("span", null, "One colour per layer"))); }
      bar.append(h("span", { class: "rl-muted" }, "Click the map to see the features under the pointer."));
      // side panel
      side.textContent = "";
      const md = t.meta || {};
      const hd = t.header;
      if (md.name) side.append(h("h2", { class: "rl-pm-title" }, String(md.name)));
      if (md.description) side.append(h("p", { class: "rl-pm-desc" }, String(md.description).slice(0, 800)));
      const facts = [
        ["Tiles", TILE_TYPE[hd.tileType] || String(hd.tileType)],
        ["Zoom", `${hd.minZoom} – ${hd.maxZoom}`],
        ["Bounds", `${hd.minLat.toFixed(3)}, ${hd.minLon.toFixed(3)} … ${hd.maxLat.toFixed(3)}, ${hd.maxLon.toFixed(3)}`],
        ["Center", `${hd.centerLat.toFixed(3)}, ${hd.centerLon.toFixed(3)} (z${hd.centerZoom})`],
        hd.numAddressedTiles ? ["Addressed tiles", num(hd.numAddressedTiles)] : null,
        hd.numTileContents ? ["Distinct tiles", num(hd.numTileContents)] : null,
        opts.size ? ["File", bytes(opts.size)] : null,
        ["PMTiles", "v" + hd.specVersion],
        md.attribution ? ["Attribution", String(md.attribution).replace(/<[^>]*>/g, "")] : null,
        md.generator ? ["Made with", String(md.generator)] : null,
      ].filter(Boolean);
      side.append(h("dl", { class: "rl-dl" }, facts.flatMap(([k, v]) => [h("dt", null, k), h("dd", null, v)])));
      if (layers.length) {
        const pal = dark ? PAL_DARK : PAL_LIGHT;
        const list = h("ul", { class: "rl-pm-layers" });
        layers.forEach((l, i) => {
          const cb = h("input", { type: "checkbox", checked: true, "aria-label": "Show layer " + l.id });
          cb.addEventListener("change", () => { if (cb.checked) hidden.delete(l.id); else hidden.add(l.id); redraw(); });
          const fields = Object.keys(l.fields);
          list.append(h("li", null,
            h("label", { class: "rl-pm-layer" }, cb, h("span", { class: "rl-swatch", style: `background:${pal[i % pal.length]}` }), h("b", null, l.id),
              l.minzoom != null ? h("span", { class: "rl-muted" }, ` z${l.minzoom}–${l.maxzoom}`) : null),
            fields.length ? h("div", { class: "rl-pm-fields" }, fields.slice(0, 30).join(", ") + (fields.length > 30 ? ", …" : "")) : null));
        });
        side.append(h("h3", { class: "rl-pm-h" }, `Layers (${layers.length})`), list);
        if (isBasemap) side.append(h("p", { class: "rl-muted" }, "This archive uses the Protomaps basemap layers, so it is drawn with the Protomaps style; tick \"One colour per layer\" to see the layers themselves."));
      }
      opts.onReady && opts.onReady(t);
    })().catch((e) => { side.textContent = ""; side.append(h("div", { class: "rl-err" }, "Could not read this PMTiles archive: " + String(e.message || e))); });
    return { el, destroy: () => m && m.destroy() };
  }

  window.RetelensMaps = { ensure, MapView, geoColumns, openGeometry, PmtilesViewer, engineSource, openPmtiles, pmtilesLayer };
})();
