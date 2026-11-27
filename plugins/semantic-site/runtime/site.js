// Page behaviour for generated sites: the theme toggle, the collection list
// (words, facet filters, sort, "show more") and the home page search. Pages
// work without it: every list is also rendered into the HTML.
//
// initView(env) returns the hooks the WebMCP tools drive (show_items,
// show_search, get_current_page), so the agent changes exactly what a person
// would change by hand.

import { filterItems, sortItems, searchRows, snippet, asList } from './query.js';

const PAGE = 100;

function el(tag, attrs, ...kids) {
  const e = document.createElement(tag);
  for (const [k, v] of Object.entries(attrs || {})) {
    if (v == null || v === false) continue;
    if (k === 'class') e.className = v;
    else e.setAttribute(k, v === true ? '' : v);
  }
  for (const k of kids.flat()) if (k != null) e.append(k instanceof Node ? k : String(k));
  return e;
}

export function initTheme(key = 'site-theme') {
  const root = document.documentElement;
  const btn = document.getElementById('theme');
  if (!btn) return;
  btn.addEventListener('click', () => {
    const dark = root.getAttribute('data-theme') === 'dark' ||
      (!root.getAttribute('data-theme') && matchMedia('(prefers-color-scheme: dark)').matches);
    const next = dark ? 'light' : 'dark';
    root.setAttribute('data-theme', next);
    try { localStorage.setItem(key, next); } catch (_) { /* private mode */ }
  });
}

function readParams(filterable) {
  const p = new URLSearchParams(location.search);
  const where = {};
  for (const f of filterable) {
    const v = p.getAll(`f.${f}`);
    if (v.length) where[f] = v;
  }
  return { query: p.get('q') || '', where, sort: p.get('sort') || undefined };
}

function writeParams({ query, where, sort }) {
  const p = new URLSearchParams();
  if (query) p.set('q', query);
  for (const [f, v] of Object.entries(where || {})) for (const x of asList(v)) p.append(`f.${f}`, x);
  if (sort) p.set('sort', sort);
  const qs = p.toString();
  history.replaceState(null, '', qs ? `?${qs}` : location.pathname);
}

function collectionView(env) {
  const form = document.getElementById('list-controls');
  const body = document.getElementById('list-body');
  const count = document.getElementById('list-count');
  const more = document.getElementById('list-more');
  if (!form || !body) return null;
  const c = env.site.collections.find((x) => x.id === env.page.collection);
  let list = null;
  let matched = [];
  let shown = 0;
  let current = { query: '', where: {}, sort: undefined };

  const cell = (v) => {
    const vals = asList(v);
    return el('td', {}, vals.map(String).join(', '));
  };

  function render(reset) {
    if (reset) { body.replaceChildren(); shown = 0; }
    const next = matched.slice(shown, shown + PAGE);
    for (const it of next) {
      const tr = el('tr', {},
        el('td', { class: 'name' }, el('a', { href: env.url(it.url) }, it.title),
          it.summary ? el('span', { class: 'sm' }, it.summary) : null),
        ...(list.columns || []).map((col) => cell(it.f && it.f[col.name])));
      body.append(tr);
    }
    shown += next.length;
    if (count) count.textContent = `${matched.length} of ${list.count}`;
    if (more) more.hidden = shown >= matched.length;
  }

  function syncControls() {
    form.elements.q.value = current.query || '';
    for (const sel of form.querySelectorAll('select[data-facet]')) {
      const v = asList(current.where[sel.dataset.facet])[0];
      sel.value = v == null ? '' : String(v);
    }
    if (form.elements.sort) form.elements.sort.value = current.sort || '';
  }

  function apply(args, { fromForm = false } = {}) {
    current = { query: args.query || '', where: args.where || {}, sort: args.sort || undefined };
    if (!fromForm) syncControls();
    matched = sortItems(filterItems(list.items, current), current.sort || c.sort);
    render(true);
    writeParams(current);
    return { shown: Math.min(PAGE, matched.length), matched: matched.length, total: list.count, url: location.href };
  }

  function fromForm() {
    const where = {};
    for (const sel of form.querySelectorAll('select[data-facet]')) if (sel.value) where[sel.dataset.facet] = [sel.value];
    return { query: form.elements.q.value.trim(), where, sort: form.elements.sort ? form.elements.sort.value || undefined : undefined };
  }

  const ready = env.fetchJSON(c.list).then((l) => {
    list = l;
    const initial = readParams(c.filterable || []);
    current = initial;
    syncControls();
    apply(initial, { fromForm: true });
    form.addEventListener('input', () => apply(fromForm(), { fromForm: true }));
    form.addEventListener('submit', (e) => { e.preventDefault(); apply(fromForm(), { fromForm: true }); });
    if (more) more.addEventListener('click', () => render(false));
  });

  return {
    ready,
    async apply(args) { await ready; return apply(args); },
    state() {
      return { query: current.query, where: current.where, sort: current.sort || null, matched: matched.length, shown, total: list ? list.count : null };
    },
  };
}

function homeView(env) {
  const form = document.getElementById('search');
  const out = document.getElementById('search-results');
  if (!form || !out) return null;
  const cols = Object.fromEntries(env.site.collections.map((c) => [c.id, c]));
  let index = null;
  let current = { query: '', collection: undefined, matched: 0 };

  async function search({ query, collection }) {
    if (!index) index = await env.fetchJSON(env.site.api.search);
    form.elements.q.value = query || '';
    if (form.elements.collection) form.elements.collection.value = collection || '';
    const hits = query ? searchRows(index, { query, collection: collection || undefined }) : [];
    out.replaceChildren();
    for (const h of hits.slice(0, 30)) {
      const c = cols[h.c];
      out.append(el('li', {},
        el('a', { href: env.url(c.itemPage.replace('{id}', encodeURIComponent(h.id))) }, h.title),
        el('span', { class: 'tag' }, c.title),
        h.text ? el('span', { class: 'sm' }, snippet(h.text, query)) : null));
    }
    if (query && !hits.length) out.append(el('li', { class: 'empty' }, 'Nothing matches every word.'));
    current = { query: query || '', collection: collection || undefined, matched: hits.length };
    return { matched: hits.length, shown: Math.min(30, hits.length) };
  }

  const run = () => search({ query: form.elements.q.value.trim(), collection: form.elements.collection ? form.elements.collection.value : undefined });
  form.addEventListener('submit', (e) => { e.preventDefault(); run(); });
  let t = null;
  form.addEventListener('input', () => { clearTimeout(t); t = setTimeout(run, 150); });
  const q = new URLSearchParams(location.search).get('q');
  if (q) search({ query: q });

  return { search, state: () => ({ ...current }) };
}

export function initView(env) {
  initTheme(env.themeKey);
  if (env.page.kind === 'collection') return collectionView(env);
  if (env.page.kind === 'home') return homeView(env);
  return null;
}

// "WebMCP: 8 tools" in the footer, with the tool names on hover.
export function showStatus(pageTools) {
  const s = document.getElementById('webmcp-status');
  if (!s) return;
  const n = pageTools.tools.length;
  s.textContent = pageTools.supported
    ? `WebMCP: ${n} tools registered`
    : `WebMCP: ${n} tools (this browser has no WebMCP; try pageTools.call() in the console)`;
  s.title = pageTools.tools.join(', ');
  s.dataset.supported = String(pageTools.supported);
}
