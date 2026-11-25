// Theme bridge between a host page and components that cannot read CSS variables or Tailwind classes
// (the graph view draws on canvas): resolve the host's colours and fonts into concrete values, notice
// theme changes, and check which classes of a skin the host stylesheets really define.
//   RetelensTheme.tokens(el, skin) -> {surface, surface2, text, textMuted, border, accent, fontSans, fontMono, radius, dark}
//   RetelensTheme.watch(fn)        -> unsubscribe; fn() runs after the host switched light/dark
//   RetelensTheme.classSet()       -> Set of class names the page's stylesheets define (cached)
//   RetelensTheme.checkSkin(map)   -> {ok: {key: classes}, missing: {key: [classes]}}
(function () {
  // Probes: elements carrying the host's OWN classes (HF, standalone) or Primer variables (GitHub),
  // read back through getComputedStyle. They live inside `el` so scoped stylesheets apply.
  const TW = {
    surface: ["bg-white dark:bg-gray-950", "backgroundColor"],
    surface2: ["bg-gray-50 dark:bg-gray-900", "backgroundColor"],
    text: ["text-gray-900 dark:text-gray-100", "color"],
    textMuted: ["text-gray-500 dark:text-gray-400", "color"],
    border: ["border border-gray-200 dark:border-gray-800", "borderTopColor"],
    accent: ["text-blue-600 dark:text-blue-400", "color"],
  };
  const PRIMER = {
    surface: ["background-color", "var(--bgColor-default)", "backgroundColor"],
    surface2: ["background-color", "var(--bgColor-muted)", "backgroundColor"],
    text: ["color", "var(--fgColor-default)", "color"],
    textMuted: ["color", "var(--fgColor-muted)", "color"],
    border: ["border-top-color", "var(--borderColor-default)", "borderTopColor"],
    accent: ["color", "var(--fgColor-accent)", "color"],
  };
  const FALLBACK = {
    light: { surface: "#ffffff", surface2: "#f9fafb", text: "#111827", textMuted: "#6b7280", border: "#e5e7eb", accent: "#2563eb" },
    dark: { surface: "#0b0f19", surface2: "#111827", text: "#f3f4f6", textMuted: "#9ca3af", border: "#1f2937", accent: "#60a5fa" },
  };
  const lum = (c) => {
    const m = String(c).match(/rgba?\(\s*(\d+)[ ,]+(\d+)[ ,]+(\d+)/);
    if (!m) return null;
    return (0.2126 * +m[1] + 0.7152 * +m[2] + 0.0722 * +m[3]) / 255;
  };
  // Any CSS colour as rgb()/rgba(). Hugging Face moved to Tailwind v4, whose colours compute to oklch(...); a canvas
  // keeps such a colour as written in fillStyle, and cytoscape (the graph view) cannot read it and draws black, so the
  // colour is painted on one pixel and read back. rgb() and #hex pass through.
  let pix = null;
  function toRgb(c) {
    const s = String(c || "").trim();
    if (!s || /^(rgba?\(|#[0-9a-f]{3,8}$)/i.test(s) || s === "transparent") return s;
    try {
      if (!pix) { const cv = document.createElement("canvas"); cv.width = cv.height = 1; pix = cv.getContext("2d", { willReadFrequently: true }); }
      pix.clearRect(0, 0, 1, 1);
      pix.fillStyle = "#000"; pix.fillStyle = s;
      pix.fillRect(0, 0, 1, 1);
      const d = pix.getImageData(0, 0, 1, 1).data;
      return d[3] === 255 ? `rgb(${d[0]}, ${d[1]}, ${d[2]})` : `rgba(${d[0]}, ${d[1]}, ${d[2]}, ${+(d[3] / 255).toFixed(3)})`;
    } catch (_) { return s; }
  }
  const opaque = (c) => c && c !== "transparent" && !/rgba\([^)]*,\s*0\)$/.test(c) && !/\/\s*0\s*\)$/.test(c);

  function tokens(el, skin) {
    const github = skin === "github";
    const probe = document.createElement("div");
    probe.style.cssText = "position:absolute;visibility:hidden;pointer-events:none;width:0;height:0;overflow:hidden";
    const kids = {};
    for (const k of Object.keys(TW)) {
      const c = document.createElement("div");
      if (github) c.style.setProperty(PRIMER[k][0], PRIMER[k][1]);
      else c.className = TW[k][0];
      if (k === "border" && github) c.style.borderTopStyle = "solid";
      probe.append(c); kids[k] = c;
    }
    const mono = document.createElement("div"); mono.className = "font-mono"; probe.append(mono);
    const round = document.createElement("div"); round.className = "rounded-lg"; probe.append(round);
    (el.isConnected ? el : document.body).append(probe);
    const out = {};
    const cs = (n) => getComputedStyle(n);
    for (const k of Object.keys(TW)) out[k] = toRgb(cs(kids[k])[(github ? PRIMER : TW)[k][github ? 2 : 1]]);
    // pick the fallback set from the page's own luminance when the page did not give a surface colour
    const dark0 = lum(toRgb(getComputedStyle(document.body).backgroundColor));
    const fb = FALLBACK[dark0 != null && dark0 < 0.4 ? "dark" : "light"];
    for (const k of Object.keys(TW)) if (!opaque(out[k])) out[k] = fb[k];
    out.fontSans = cs(el.isConnected ? el : document.body).fontFamily || "system-ui, sans-serif";
    const gm = github ? getComputedStyle(document.documentElement).getPropertyValue("--fontStack-monospace").trim() : "";
    out.fontMono = gm || cs(mono).fontFamily || "ui-monospace, monospace";
    out.radius = github ? "6px" : (cs(round).borderTopLeftRadius || "8px");
    probe.remove();
    const l = lum(out.surface);
    out.dark = l != null ? l < 0.4 : false;
    return out;
  }

  // Host theme changes: HF/standalone toggle `dark` on <html>, GitHub switches data-color-mode / data-*-theme,
  // and "system" themes follow prefers-color-scheme.
  function watch(fn) {
    let t = null;
    const fire = () => { clearTimeout(t); t = setTimeout(fn, 60); };
    const mo = new MutationObserver(fire);
    mo.observe(document.documentElement, { attributes: true, attributeFilter: ["class", "data-color-mode", "data-dark-theme", "data-light-theme"] });
    let mq = null;
    try { mq = matchMedia("(prefers-color-scheme: dark)"); mq.addEventListener("change", fire); } catch (_) {}
    return () => { mo.disconnect(); clearTimeout(t); try { mq && mq.removeEventListener("change", fire); } catch (_) {} };
  }

  // ---- class vocabulary of the page
  let cached = null;
  function classSet() {
    if (cached) return cached;
    const set = new Set();
    const rx = /\.((?:\\.|[\w-])+)/g;
    const walk = (rules) => {
      for (const r of rules) {
        if (r.selectorText) { for (const m of r.selectorText.matchAll(rx)) set.add(m[1].replace(/\\(.)/g, "$1")); }
        if (r.cssRules) walk(r.cssRules);
      }
    };
    const sheets = [...document.styleSheets, ...(document.adoptedStyleSheets || [])];
    for (const ss of sheets) { try { walk(ss.cssRules); } catch (_) { /* cross-origin sheet */ } }
    cached = set;
    return set;
  }
  // classes of a skin map the host really defines (a key with any undefined class is reported missing)
  function checkSkin(map, extra) {
    const have = classSet();
    const ok = {}, missing = {};
    for (const [k, v] of Object.entries(map || {})) {
      const toks = String(v).split(/\s+/).filter(Boolean);
      const lacking = toks.filter((t) => !have.has(t) && !(extra && extra.has(t)));
      if (lacking.length) missing[k] = lacking; else ok[k] = v;
    }
    return { ok, missing };
  }

  window.RetelensTheme = { tokens, toRgb, watch, classSet, checkSkin, resetClassCache: () => { cached = null; } };
})();
