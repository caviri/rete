// Text matching, filtering and sorting, shared by the page UI and the WebMCP
// tools: a person filtering a list and an agent calling list_items get the
// same rows in the same order.

export function norm(s) {
  return String(s ?? '')
    .normalize('NFD')
    .replace(/\p{M}/gu, '')
    .toLowerCase();
}

export function tokens(q) {
  return norm(q).split(/[^\p{L}\p{N}]+/u).filter(Boolean);
}

export const asList = (v) => (v == null || v === '' ? [] : Array.isArray(v) ? v : [v]);

function fieldValues(item, field) {
  if (field === 'title') return asList(item.title);
  if (field === 'id') return asList(item.id);
  return asList(item.f ? item.f[field] : undefined);
}

// where: { field: value | [values] } — values within a field are alternatives,
// fields are combined. Comparison ignores case and accents.
export function matchWhere(item, where) {
  if (!where || typeof where !== 'object') return true;
  for (const [field, want] of Object.entries(where)) {
    const wants = asList(want).map(norm);
    if (!wants.length) continue;
    const have = fieldValues(item, field).map(norm);
    if (!wants.some((w) => have.includes(w))) return false;
  }
  return true;
}

const textCache = new WeakMap();
function itemText(item) {
  let t = textCache.get(item);
  if (t === undefined) {
    const parts = [item.title, item.summary, item.id];
    for (const v of Object.values(item.f || {})) parts.push(...asList(v));
    t = norm(parts.join(' '));
    textCache.set(item, t);
  }
  return t;
}

export function filterItems(items, { query, where } = {}) {
  const toks = tokens(query || '');
  return items.filter(
    (it) => matchWhere(it, where) && toks.every((t) => itemText(it).includes(t)),
  );
}

const num = (v) => (typeof v === 'number' ? v : /^-?\d+(\.\d+)?$/.test(String(v)) ? Number(v) : NaN);

function compare(a, b) {
  const x = asList(a)[0];
  const y = asList(b)[0];
  if (x == null && y == null) return 0;
  if (x == null) return 1; // missing values last, whatever the order
  if (y == null) return -1;
  const nx = num(x);
  const ny = num(y);
  if (!Number.isNaN(nx) && !Number.isNaN(ny)) return nx - ny;
  return norm(x).localeCompare(norm(y));
}

// sort: "field" ascending, "-field" descending; "title" sorts by the label.
export function sortItems(items, sort) {
  if (!sort) return items.slice();
  const desc = sort.startsWith('-');
  const field = desc ? sort.slice(1) : sort;
  const pick = (it) => (field === 'title' ? it.title : field === 'id' ? it.id : it.f && it.f[field]);
  return items.slice().sort((a, b) => {
    const ma = pick(a) == null;
    const mb = pick(b) == null;
    if (ma !== mb) return ma ? 1 : -1;
    const c = compare(pick(a), pick(b));
    return desc ? -c : c;
  });
}

// The search index is { fields: ["c","id","t","s"], rows: [[collection, id, title, text], ...] }.
// Every query word must appear in the title or the text; title hits weigh more.
export function searchRows(index, { query, collection } = {}) {
  const toks = tokens(query || '');
  if (!toks.length) return [];
  const hits = [];
  for (const row of index.rows || []) {
    const [c, id, title, text] = row;
    if (collection && c !== collection) continue;
    const nt = norm(title);
    const nx = norm(text);
    let score = 0;
    let ok = true;
    for (const t of toks) {
      const inTitle = nt.includes(t);
      const inText = nx.includes(t);
      if (!inTitle && !inText) { ok = false; break; }
      score += (inTitle ? 3 : 0) + (inText ? 1 : 0) + (nt === t ? 5 : 0);
    }
    if (ok) hits.push({ c, id, title, text, score });
  }
  hits.sort((a, b) => b.score - a.score || norm(a.title).localeCompare(norm(b.title)));
  return hits;
}

// A short excerpt of text around the first query word.
export function snippet(text, query, n = 160) {
  const s = String(text || '');
  if (s.length <= n) return s;
  const toks = tokens(query || '');
  const lower = norm(s);
  let at = -1;
  for (const t of toks) {
    at = lower.indexOf(t);
    if (at >= 0) break;
  }
  const start = Math.max(0, at < 0 ? 0 : at - Math.floor(n / 3));
  const out = s.slice(start, start + n);
  return (start > 0 ? '…' : '') + out + (start + n < s.length ? '…' : '');
}
