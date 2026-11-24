import { resolveSkin } from "./skins.js";

// Components that must be styled by one source: if one class of the group is missing, the whole
// group falls back to graphkit's own styling.
const GROUPS = [["button"], ["buttonPrimary"], ["select"], ["input"], ["panel"], ["segmented", "segmentedItem", "segmentedItemActive"], ["badge"]];

// CSS class names: ASCII word characters, escapes and any non-ASCII character. Written with escapes:
// U+FFFF is a Unicode noncharacter, and Chrome refuses an injected script that contains it raw.
const CLASS_RE = /\.((?:\\.|[A-Za-z0-9_\u00a0-\uffff-])+)/g;
const BS = String.fromCharCode(92);

function unescapeCss(s) { return s.split(BS).join(""); }

function collect(rules, into) {
  for (const r of rules) {
    if (r.selectorText) {
      let m;
      CLASS_RE.lastIndex = 0;
      while ((m = CLASS_RE.exec(r.selectorText))) into.add(unescapeCss(m[1]));
    }
    // @media, @supports, @layer, nested rules
    if (r.cssRules) { try { collect(r.cssRules, into); } catch (e) { /* skip */ } }
  }
}

// Every class name that appears in a selector of a readable stylesheet. `unreadable` counts the
// cross-origin sheets we cannot look into.
export function indexDocumentClasses(doc = document) {
  const set = new Set();
  let unreadable = 0;
  const sheets = [...doc.styleSheets, ...(doc.adoptedStyleSheets || [])];
  for (const sh of sheets) {
    let rules;
    try { rules = sh.cssRules; } catch (e) { unreadable++; continue; }
    if (!rules) { unreadable++; continue; }
    collect(rules, set);
  }
  return { set, unreadable };
}

function splitClasses(s) { return String(s || "").split(/\s+/).filter(Boolean); }

// Returns the classes of the skin's map that no loaded stylesheet has a rule for. Classes that are
// absent only while some stylesheet is unreadable (cross-origin) are "unknown", not "missing".
//   result: Array of missing class names; result.unknown: unknown names; result.byKey: {key: [missing]}
export function checkSkin(skin, overrides, doc) {
  const { classes } = resolveSkin(skin, overrides);
  const idx = indexDocumentClasses(doc || document);
  const missing = [], unknown = [], byKey = {};
  for (const key of Object.keys(classes)) {
    for (const c of splitClasses(classes[key])) {
      if (idx.set.has(c)) continue;
      if (idx.unreadable) { if (!unknown.includes(c)) unknown.push(c); continue; }
      if (!missing.includes(c)) missing.push(c);
      (byKey[key] = byKey[key] || []).push(c);
    }
  }
  missing.unknown = unknown;
  missing.byKey = byKey;
  missing.unreadableSheets = idx.unreadable;
  return missing;
}

// Drops every component whose classes are not all available, so it renders with the standalone look.
export function applyFallback(sk, doc) {
  if (sk.name === "standalone") return [];
  const res = checkSkin({ base: "standalone", classes: sk.classes }, null, doc);
  const dropped = [];
  for (const g of GROUPS) {
    if (!g.some((k) => res.byKey[k])) continue;
    for (const k of g) delete sk.classes[k];
    dropped.push({ component: g[0], missing: g.flatMap((k) => res.byKey[k] || []) });
  }
  return dropped;
}
