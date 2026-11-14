// search.js — text matching and the search combobox.
//
// Two halves, both free of page state:
//
//  1. A small matching engine. A document is a set of weighted fields (title,
//     key, tags, vocabularies, licence, class and predicate names, description).
//     A query is split into words; every word must match somewhere (AND). A word
//     matches a field word exactly, as a prefix, as a substring (3+ letters) or
//     with one or two typos (4+ letters). Text is case- and diacritic-folded.
//     Title and key hits outrank everything else because their weights are
//     higher.
//
//  2. mountSearch(): the combobox. Suggestions are grouped by what they ARE
//     (a dataset opens; a topic, vocabulary or licence becomes a filter), the
//     list is a real ARIA listbox driven with aria-activedescendant, and the
//     shortcuts are `/` and Ctrl/Cmd+K to focus, Esc to clear.

// ── folding and tokens ──────────────────────────────────────────────────────

/** Lower-case and strip diacritics one character at a time, so the folded
 *  string has the same length as the original and match offsets carry over. */
export function fold(s) {
  let out = "";
  for (const ch of String(s == null ? "" : s)) {
    if (ch.length > 1) { out += ch; continue; }
    const base = ch.normalize("NFD")[0] || ch;
    out += base.toLowerCase()[0] || base;
  }
  return out;
}
const foldPlain = (s) => fold(s);

/** Split into words. camelCase and snake_case are split so "birthDate" is
 *  found by "birth"; the joined word stays too, so "birthdate" matches as well. */
export function tokenize(s) {
  const raw = String(s == null ? "" : s)
    .replace(/([a-z\d])([A-Z])/g, "$1 $2")
    .replace(/([A-Z]+)([A-Z][a-z])/g, "$1 $2");
  const words = fold(raw).split(/[^\p{L}\p{N}]+/u).filter(Boolean);
  const joined = fold(String(s == null ? "" : s)).replace(/[^\p{L}\p{N}]+/gu, "");
  if (words.length > 1 && joined.length <= 40) words.push(joined);
  return words;
}

/** Query words: folded, no camel splitting (people type what they mean). */
export function queryTokens(q) {
  return fold(q).split(/[^\p{L}\p{N}]+/u).filter(Boolean);
}

// ── documents ───────────────────────────────────────────────────────────────

export const FIELD_WEIGHT = {
  title: 10, key: 9, tags: 6, vocab: 5, licence: 4,
  classes: 3, predicates: 3, other: 2.5, text: 2,
};
export const FIELD_LABEL = {
  tags: "topic", vocab: "vocabulary", licence: "licence",
  classes: "class", predicates: "property", other: "", text: "description",
};

/** @param {Record<string, string[]|string>} fields  field name -> strings */
export function buildDoc(fields) {
  const doc = { fields: {}, title: fold([].concat(fields.title || []).join(" ")) };
  for (const [name, val] of Object.entries(fields)) {
    const strings = [].concat(val || []).filter(Boolean);
    const set = new Set();
    for (const s of strings) for (const t of tokenize(s)) set.add(t);
    doc.fields[name] = { w: FIELD_WEIGHT[name] || 1, tokens: [...set], strings };
  }
  return doc;
}

// ── scoring ─────────────────────────────────────────────────────────────────

function withinEdits(a, b, max) {
  const la = a.length, lb = b.length;
  if (Math.abs(la - lb) > max) return false;
  let prev = Array.from({ length: lb + 1 }, (_, j) => j);
  for (let i = 1; i <= la; i++) {
    const cur = [i];
    let rowMin = i;
    for (let j = 1; j <= lb; j++) {
      const v = Math.min(prev[j] + 1, cur[j - 1] + 1, prev[j - 1] + (a[i - 1] === b[j - 1] ? 0 : 1));
      cur.push(v);
      if (v < rowMin) rowMin = v;
    }
    if (rowMin > max) return false;
    prev = cur;
  }
  return prev[lb] <= max;
}

/** Quality of one query word against one field word: 0 for no match. */
function exactQuality(qt, ft, sub) {
  if (ft === qt) return 1;
  if (ft.startsWith(qt)) return 0.85;
  if (sub && qt.length >= 3 && ft.includes(qt)) return 0.5; // only where names are compound
  return 0;
}
function typoQuality(qt, ft) {
  if (qt.length < 4) return 0;
  const max = qt.length >= 8 ? 2 : 1;
  if (withinEdits(qt, ft, max)) return 0.4;
  if (ft.length > qt.length && withinEdits(qt, ft.slice(0, qt.length), max)) return 0.35;
  return 0;
}

/**
 * Score a document against query words.
 * @returns {{score:number, via:string}|null}  null when a word matches nothing
 *   (all words must match). `via` is the strongest non-title field that matched.
 */
export function scoreDoc(doc, qtokens, rawFolded = "") {
  if (!qtokens.length) return { score: 0, via: "" };
  let total = 0;
  let via = "", viaScore = 0;
  for (const qt of qtokens) {
    let best = 0, bestField = "";
    for (const pass of [0, 1]) {
      for (const [name, f] of Object.entries(doc.fields)) {
        for (const ft of f.tokens) {
          const q = pass === 0 ? exactQuality(qt, ft, name === "title" || name === "key") : typoQuality(qt, ft);
          const s = q * f.w;
          if (s > best) { best = s; bestField = name; }
        }
      }
      if (best > 0) break; // typos are only tried when nothing matched plainly
    }
    if (best === 0) return null;
    total += best;
    if (bestField !== "title" && bestField !== "key" && best > viaScore) { viaScore = best; via = bestField; }
  }
  if (rawFolded && qtokens.length > 1) {
    if (doc.title.includes(rawFolded)) total += 8;
  }
  if (doc.title.startsWith(qtokens[0])) total += 4;
  return { score: total, via };
}

// ── highlighting ────────────────────────────────────────────────────────────

const esc = (s) =>
  String(s == null ? "" : s).replace(/[&<>"]/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;" }[c]));
export const escapeHtml = esc;

/** Escape `text` and wrap the parts the query words matched in <mark>. */
export function highlight(text, qtokens) {
  text = String(text == null ? "" : text);
  if (!qtokens || !qtokens.length) return esc(text);
  const f = foldPlain(text);
  if (f.length !== text.length) return esc(text); // exotic folding: skip rather than mis-mark
  const ranges = [];
  for (const qt of qtokens) {
    if (!qt) continue;
    // Prefer word starts; fall back to anywhere for 3+ letter words.
    const re = new RegExp("(?<![\\p{L}\\p{N}])" + qt.replace(/[.*+?^${}()|[\]\\]/g, "\\$&"), "gu");
    let m;
    while ((m = re.exec(f))) { ranges.push([m.index, m.index + qt.length]); if (re.lastIndex === m.index) re.lastIndex++; }
  }
  if (!ranges.length) return esc(text);
  ranges.sort((a, b) => a[0] - b[0]);
  const merged = [];
  for (const r of ranges) {
    const last = merged[merged.length - 1];
    if (last && r[0] <= last[1]) last[1] = Math.max(last[1], r[1]); else merged.push([...r]);
  }
  let out = "", at = 0;
  for (const [a, b] of merged) { out += esc(text.slice(at, a)) + "<mark>" + esc(text.slice(a, b)) + "</mark>"; at = b; }
  return out + esc(text.slice(at));
}

// ── the combobox ────────────────────────────────────────────────────────────

const PER_GROUP = 5;
const GROUP_ORDER = ["dataset", "tag", "vocabulary", "licence", "provider", "feature"];
const GROUP_LABEL = {
  dataset: "Datasets", tag: "Topics", vocabulary: "Vocabularies",
  licence: "Licences", provider: "Connected to", feature: "Features",
};
const ACTION = { dataset: "open", tag: "filter", vocabulary: "filter", licence: "filter", provider: "filter", feature: "filter" };

/**
 * @param {object} o
 * @param {HTMLInputElement} o.input   role=combobox is set here
 * @param {HTMLElement} o.panel        the listbox
 * @param {HTMLElement} [o.clearBtn]
 * @param {(q: string) => Array<{type:string,label:string,meta?:string,pick:Function}>} o.suggest
 * @param {(q: string) => void} o.onQuery   free text changed (every keystroke)
 * @param {() => void} [o.onCommit]         Enter with nothing highlighted
 */
export function mountSearch({ input, panel, clearBtn, suggest, onQuery, onCommit }) {
  let items = [];
  let cursor = -1;
  let lastHtml = "";
  const live = document.createElement("div");
  live.className = "pz-sr";
  live.setAttribute("aria-live", "polite");
  panel.after(live);

  input.setAttribute("role", "combobox");
  input.setAttribute("aria-autocomplete", "list");
  input.setAttribute("aria-haspopup", "listbox");
  input.setAttribute("aria-controls", panel.id);
  input.setAttribute("aria-expanded", "false");
  panel.setAttribute("role", "listbox");

  const syncClear = () => { if (clearBtn) clearBtn.hidden = !input.value; };

  const close = () => {
    panel.hidden = true;
    input.setAttribute("aria-expanded", "false");
    input.removeAttribute("aria-activedescendant");
    cursor = -1;
  };

  function open() {
    const raw = input.value.trim();
    if (!raw) return close();
    const qt = queryTokens(raw);
    const byGroup = new Map();
    for (const it of suggest(raw)) {
      const b = byGroup.get(it.type) || [];
      if (b.length < PER_GROUP) b.push(it);
      byGroup.set(it.type, b);
    }
    items = [];
    let html = "";
    for (const g of GROUP_ORDER) {
      const b = byGroup.get(g);
      if (!b || !b.length) continue;
      const gid = `ac-g-${g}`;
      html += `<div role="group" aria-labelledby="${gid}"><div class="pz-ac-group" id="${gid}">${GROUP_LABEL[g]}</div>`;
      for (const it of b) {
        const id = `ac-o-${items.length}`;
        it._id = id;
        html +=
          `<div class="pz-ac-item" role="option" id="${id}" aria-selected="false" data-i="${items.length}">` +
          `<span class="pz-ac-lab">${highlight(it.label, qt)}</span>` +
          `<span class="pz-ac-meta">${esc(it.meta || "")}</span>` +
          `<span class="pz-ac-act">${ACTION[g] || ""}</span></div>`;
        items.push(it);
      }
      html += `</div>`;
    }
    const full = html
      ? html + `<div class="pz-ac-foot">Enter on no suggestion keeps the text as a search</div>`
      : `<div class="pz-ac-empty">No suggestions for “${esc(raw)}”. The list below still searches descriptions, classes and properties.</div>`;
    if (!panel.hidden && full === lastHtml) return; // nothing changed: keep the cursor
    lastHtml = full;
    panel.innerHTML = full;
    panel.hidden = false;
    input.setAttribute("aria-expanded", "true");
    input.removeAttribute("aria-activedescendant");
    cursor = -1;
    live.textContent = items.length ? `${items.length} suggestions` : "No suggestions";
  }

  function setCursor(i) {
    const els = panel.querySelectorAll(".pz-ac-item");
    cursor = i;
    els.forEach((el, k) => {
      const on = k === i;
      el.classList.toggle("on", on);
      el.setAttribute("aria-selected", String(on));
    });
    if (i >= 0 && els[i]) {
      input.setAttribute("aria-activedescendant", els[i].id);
      els[i].scrollIntoView({ block: "nearest" });
    } else input.removeAttribute("aria-activedescendant");
  }

  function choose(i) {
    const it = items[i];
    if (!it) return;
    close();
    if (it.type !== "dataset") { input.value = ""; syncClear(); onQuery(""); }
    it.pick();
  }

  function clear() {
    input.value = "";
    syncClear();
    onQuery("");
    close();
  }

  panel.addEventListener("mousedown", (e) => {
    const el = e.target.closest(".pz-ac-item");
    if (!el) return;
    e.preventDefault(); // keep focus (and the panel) until we have chosen
    choose(Number(el.dataset.i));
  });
  panel.addEventListener("mousemove", (e) => {
    const el = e.target.closest(".pz-ac-item");
    if (el && Number(el.dataset.i) !== cursor) setCursor(Number(el.dataset.i));
  });

  input.addEventListener("input", () => { syncClear(); onQuery(input.value); open(); });
  input.addEventListener("focus", () => { if (input.value.trim()) open(); });
  input.addEventListener("blur", () => setTimeout(close, 120));
  clearBtn?.addEventListener("click", () => { clear(); input.focus(); });
  input.addEventListener("keydown", (e) => {
    const n = items.length;
    if (e.key === "ArrowDown") {
      e.preventDefault();
      if (panel.hidden) open();
      else setCursor(cursor + 1 >= n ? -1 : cursor + 1);
    } else if (e.key === "ArrowUp") {
      e.preventDefault();
      if (panel.hidden) open();
      else setCursor(cursor < 0 ? n - 1 : cursor - 1);
    } else if (e.key === "Enter") {
      if (!panel.hidden && cursor >= 0) { e.preventDefault(); choose(cursor); }
      else { close(); onCommit?.(); }
    } else if (e.key === "Escape") {
      e.preventDefault();
      if (input.value) clear(); else { close(); input.blur(); }
    } else if (e.key === "Tab") close();
  });

  // "/" and Ctrl/Cmd+K focus the field from anywhere.
  const focusSearch = () => {
    input.scrollIntoView({ block: "center" });
    input.focus({ preventScroll: true });
    input.select();
  };
  document.addEventListener("keydown", (e) => {
    const k = e.key;
    const typing = (() => {
      const t = e.target;
      return t && (t.tagName === "INPUT" || t.tagName === "TEXTAREA" || t.tagName === "SELECT" || t.isContentEditable);
    })();
    if (k === "/" && !e.metaKey && !e.ctrlKey && !e.altKey && !typing) { e.preventDefault(); focusSearch(); }
    else if ((k === "k" || k === "K") && (e.metaKey || e.ctrlKey) && !e.altKey) { e.preventDefault(); focusSearch(); }
  });

  syncClear();
  return { refresh: () => { if (!panel.hidden) open(); }, syncClear, close, focus: focusSearch };
}
