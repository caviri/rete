// Small helpers shared by the graphkit modules. No DOM data sinks: everything that reaches the
// page goes through textContent / setAttribute with fixed names.

export const PALETTE = {
  light: ["#2a78d6", "#eb6834", "#1baf7a", "#eda100", "#e87ba4", "#008300", "#6250d6", "#e34948"],
  dark: ["#3987e5", "#d95926", "#199e70", "#c98500", "#d55181", "#0ca30c", "#9085e9", "#e66767"],
};
export const SEQ = ["#cde2fb", "#9ec5f4", "#6da7ec", "#3987e5", "#256abf", "#184f95", "#0d366b"];
// One fixed shape per colour slot; slot 8 is "Other".
export const SHAPES = ["ellipse", "round-rectangle", "diamond", "triangle", "hexagon", "star", "vee", "octagon", "rhomboid"];
export const OTHER = 8;
// Predicate (edge) slots use the same colours plus a fixed dash pattern as secondary encoding.
export const DASHES = ["solid", "dashed", "dotted", "solid", "dashed", "dotted", "solid", "dashed", "solid"];

export const TOKENS_LIGHT = { surface: "#ffffff", surface2: "#f6f7f9", text: "#1f2328", textMuted: "#656d76", border: "#d0d7de", accent: "#2a78d6", fontSans: "system-ui, -apple-system, 'Segoe UI', Roboto, sans-serif", fontMono: "ui-monospace, SFMono-Regular, Menlo, Consolas, monospace", radius: "8px" };
export const TOKENS_DARK = { surface: "#0b0f14", surface2: "#161b22", text: "#e6edf3", textMuted: "#8b949e", border: "#30363d", accent: "#3987e5", fontSans: TOKENS_LIGHT.fontSans, fontMono: TOKENS_LIGHT.fontMono, radius: "8px" };

export function isHttp(s) { return typeof s === "string" && /^https?:\/\/[^\s<>"]+$/i.test(s); }

export function localName(iri) {
  const s = String(iri);
  const m = s.match(/[^/#:]+(?=[/#]?$)/);
  return m ? decodeSafe(m[0]) : s;
}
function decodeSafe(s) { try { return decodeURIComponent(s); } catch { return s; } }
export function namespaceOf(iri) {
  const s = String(iri);
  const i = Math.max(s.lastIndexOf("#"), s.lastIndexOf("/"));
  return i > 8 ? s.slice(0, i + 1) : s;
}
export function shortIri(iri, max = 56) {
  let s = String(iri).replace(/^https?:\/\/(www\.)?/, "");
  if (s.length <= max) return s;
  return s.slice(0, Math.floor(max * 0.55)) + "…" + s.slice(-Math.floor(max * 0.4));
}
export function trunc(s, n) { s = String(s == null ? "" : s); return s.length > n ? s.slice(0, n - 1) + "…" : s; }

export function mulberry32(seed) {
  let a = seed >>> 0;
  return function () { a |= 0; a = (a + 0x6d2b79f5) | 0; let t = Math.imul(a ^ (a >>> 15), 1 | a); t = (t + Math.imul(t ^ (t >>> 7), 61 | t)) ^ t; return ((t ^ (t >>> 14)) >>> 0) / 4294967296; };
}

export function el(tag, cls, text, attrs) {
  const e = document.createElement(tag);
  if (cls) e.className = cls;
  if (text != null) e.textContent = text;
  if (attrs) for (const k in attrs) e.setAttribute(k, attrs[k]);
  return e;
}
const SVGNS = "http://www.w3.org/2000/svg";
export function svg(tag, attrs, text) {
  const e = document.createElementNS(SVGNS, tag);
  if (attrs) for (const k in attrs) e.setAttribute(k, attrs[k]);
  if (text != null) e.textContent = text;
  return e;
}
export function clear(n) { while (n.firstChild) n.removeChild(n.firstChild); }
export function fmt(n) { return Number(n).toLocaleString("en-US"); }
export function cx(...a) { return a.filter(Boolean).join(" "); }

// Glyph paths in a 0..10 box, one per shape name, for the legend.
export const GLYPHS = {
  "ellipse": "M5 1a4 4 0 1 0 .001 0z",
  "round-rectangle": "M3 1.2h4a1.8 1.8 0 0 1 1.8 1.8v4A1.8 1.8 0 0 1 7 8.8H3A1.8 1.8 0 0 1 1.2 7V3A1.8 1.8 0 0 1 3 1.2z",
  "diamond": "M5 .8L9.2 5 5 9.2 .8 5z",
  "triangle": "M5 1L9.3 8.8H.7z",
  "hexagon": "M2.6 1.2h4.8L9.4 5 7.4 8.8H2.6L.6 5z",
  "star": "M5 .6l1.3 2.9 3.2.3-2.4 2.1.7 3.1L5 7.4 2.2 9l.7-3.1L.5 3.8l3.2-.3z",
  "vee": "M.8 1.2L5 4l4.2-2.8L5 9z",
  "octagon": "M3.2 1h3.6L9 3.2v3.6L6.8 9H3.2L1 6.8V3.2z",
  "rhomboid": "M3 1.5h6.2L7 8.5H.8z",
};

export function nsName(ns) {
  let u = String(ns);
  const i = u.indexOf("://");
  if (i > 0 && i < 12) u = u.slice(i + 3);
  while (u.endsWith("/") || u.endsWith("#")) u = u.slice(0, -1);
  const parts = u.split("/");
  let host = parts[0];
  if (host.startsWith("www.") || host.startsWith("data.")) host = host.slice(host.indexOf(".") + 1);
  if (parts.length < 2) return host;
  const last = parts[parts.length - 1];
  return parts.length > 2 ? host + "/…/" + last : host + "/" + last;
}

// Labels in some datasets carry HTML entities (&#039;). Decode a few by hand; the result is still plain text.
export function decodeEntities(s) {
  s = String(s);
  if (s.indexOf("&") < 0) return s;
  const named = { amp: "&", lt: "<", gt: ">", quot: '"', apos: "'", nbsp: " " };
  let out = "", i = 0;
  while (i < s.length) {
    const c = s[i];
    if (c === "&") {
      const j = s.indexOf(";", i);
      if (j > i && j - i <= 9) {
        const body = s.slice(i + 1, j);
        let rep = null;
        if (body[0] === "#") {
          const code = body[1] === "x" || body[1] === "X" ? parseInt(body.slice(2), 16) : parseInt(body.slice(1), 10);
          if (code > 31 && code < 0x110000) rep = String.fromCodePoint(code);
        } else if (named[body] !== undefined) rep = named[body];
        if (rep !== null) { out += rep; i = j + 1; continue; }
      }
    }
    out += c; i++;
  }
  return out;
}
