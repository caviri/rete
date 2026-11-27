// The WebMCP tools every generated page registers. Pure: everything the tools
// touch comes in through `env`, so the same definitions run in the browser
// (boot.js), in the checker (Node, files on disk) and in the build (to document
// them on the agents page and in api/tools.json).
//
// env = {
//   site,                 // the parsed api/index.json
//   page,                 // { kind: home|collection|item|page|agents|notfound, collection?, id?, path?, title }
//   fetchJSON(rel),       // rel is relative to the site root, e.g. "api/index.json"
//   url(rel),             // absolute URL of a site-relative path
//   location,             // the current page URL
//   navigate(href),       // go to another page (after the tool has returned)
//   view,                 // optional page UI hooks: { apply(args), state(), search(args) }
//   fetch,                // optional fetch for SPARQL (defaults to globalThis.fetch)
// }

import { clip, fitList, RESULT_BUDGET } from './webmcp-kit.js';
import { filterItems, sortItems, searchRows, snippet, asList } from './query.js';

const MAX_SEARCH = 50;
const MAX_LIST = 100;
const MAX_ROWS = 500;

function collectionsOf(site) {
  return (site && site.collections) || [];
}

function findCollection(site, id) {
  const c = collectionsOf(site).find((x) => x.id === id);
  if (!c) {
    throw new Error(`No collection "${id}". Collections: ${collectionsOf(site).map((x) => x.id).join(', ')}.`);
  }
  return c;
}

const whereSchema = (site) => ({
  type: 'object',
  description:
    'Filters: field name -> value or list of values (alternatives). Fields combine with AND. ' +
    'Filterable fields per collection: ' +
    collectionsOf(site).map((c) => `${c.id}: ${(c.filterable || []).join(', ') || 'none'}`).join('; ') +
    '. Valid values: get_facets.',
  additionalProperties: {
    anyOf: [
      { type: 'string' },
      { type: 'number' },
      { type: 'array', items: { type: ['string', 'number'] }, maxItems: 50 },
    ],
  },
});

function collectionProp(site, required) {
  const cols = collectionsOf(site);
  const p = {
    type: 'string',
    description: 'Collection id: ' + cols.map((c) => `${c.id} (${c.title}, ${c.count} items)`).join('; ') + '.',
  };
  if (cols.length) p.enum = cols.map((c) => c.id);
  if (!required) p.description += ' Omit to use every collection.';
  return p;
}

function absItem(env, it) {
  const out = { id: it.id, title: it.title };
  if (it.summary) out.summary = clip(it.summary, 240);
  if (it.f && Object.keys(it.f).length) out.fields = it.f;
  out.url = env.url(it.url);
  return out;
}

export function siteInfo(env, toolNames) {
  const { site } = env;
  const info = {
    title: site.site.title,
    description: site.site.description,
    url: env.url(''),
    language: site.site.lang,
  };
  if (site.site.license) info.license = site.site.license;
  info.collections = collectionsOf(site).map((c) => ({
    id: c.id,
    title: c.title,
    description: clip(c.description, 300),
    count: c.count,
    type: c.type,
    fields: (c.fields || []).map((f) => ({ name: f.name, label: f.label, type: f.type, ...(f.link ? { links_to: f.link } : {}) })),
    filterable: c.filterable,
    facets: c.facets,
    page: env.url(c.page),
    api: { list: env.url(c.list), item: env.url(c.item) },
  }));
  info.pages = (site.pages || []).map((p) => ({ title: p.title, url: env.url(p.path) }));
  info.api = Object.fromEntries(Object.entries(site.api || {}).map(([k, v]) => [k, env.url(v)]));
  if (site.sparql) info.sparql = { endpoint: site.sparql.endpoint, examples: (site.sparql.examples || []).length };
  if (toolNames) info.tools_on_this_page = toolNames;
  return info;
}

async function listOf(env, collection) {
  const c = findCollection(env.site, collection);
  return { c, list: await env.fetchJSON(c.list) };
}

async function itemOf(env, collection, id) {
  const c = findCollection(env.site, collection);
  const rel = c.item.replace('{id}', encodeURIComponent(id));
  let rec;
  try {
    rec = await env.fetchJSON(rel);
  } catch (e) {
    throw new Error(`No item "${id}" in ${collection}. Use search or list_items to find ids.`);
  }
  return rec;
}

function slimRecord(env, rec) {
  const out = {
    collection: rec.collection,
    id: rec.id,
    title: rec.title,
    url: env.url(rec.url),
  };
  if (rec.iri) out.iri = rec.iri;
  if (rec.types && rec.types.length) out.types = rec.types;
  out.fields = rec.fields;
  if (rec.links && Object.keys(rec.links).length) {
    out.links = Object.fromEntries(Object.entries(rec.links).map(([k, v]) => [
      k, v.map((l) => ({ collection: l.collection, id: l.id, title: l.title })),
    ]));
  }
  if (rec.backlinks && rec.backlinks.length) {
    out.referenced_by = rec.backlinks.map((b) => ({ collection: b.collection, field: b.field, id: b.id, title: b.title }));
  }
  // Long values are clipped so one record cannot blow the budget.
  let budget = 1200;
  while (JSON.stringify(out).length > RESULT_BUDGET && budget > 80) {
    for (const [k, v] of Object.entries(out.fields || {})) {
      out.fields[k] = Array.isArray(v) ? v.slice(0, 20).map((x) => (typeof x === 'string' ? clip(x, budget) : x)) : (typeof v === 'string' ? clip(v, budget) : v);
    }
    if (out.referenced_by) out.referenced_by = out.referenced_by.slice(0, 50);
    out.truncated = true;
    budget = Math.floor(budget / 2);
  }
  return out;
}

export function siteTools(env) {
  const { site, page } = env;
  const untrusted = !(site.webmcp && site.webmcp.untrustedContent === false);
  const content = untrusted ? { readOnlyHint: true, untrustedContentHint: true } : { readOnlyHint: true };
  const tools = [];
  const names = () => tools.map((t) => t.name);

  tools.push({
    name: 'get_site_info',
    title: 'About this site',
    description:
      `What "${site.site.title}" contains: its collections (item counts, fields, filterable fields, facets), ` +
      'its content pages, licence, and the URLs of its static JSON API. Call this first.',
    inputSchema: { type: 'object', properties: {}, additionalProperties: false },
    annotations: { readOnlyHint: true },
    execute: async () => siteInfo(env, names()),
  });

  if (collectionsOf(site).length) {
    tools.push({
      name: 'search',
      title: 'Search the site',
      description:
        'Full-text search over every item of the site (titles weigh more than text). Every word must match. ' +
        'Returns collection, id, title, a short excerpt and the page URL; pass collection and id to get_item. ' +
        'Does not change what the user sees (use show_search on the home page for that).',
      inputSchema: {
        type: 'object',
        properties: {
          query: { type: 'string', description: 'Words to look for.', minLength: 1, maxLength: 200 },
          collection: collectionProp(site, false),
          limit: { type: 'integer', minimum: 1, maximum: MAX_SEARCH, default: 10, description: `Maximum results (default 10, max ${MAX_SEARCH}).` },
        },
        required: ['query'],
        additionalProperties: false,
      },
      annotations: content,
      async execute(a) {
        const idx = await env.fetchJSON(site.api.search);
        const hits = searchRows(idx, { query: a.query, collection: a.collection });
        const limit = Math.min(MAX_SEARCH, Math.max(1, a.limit || 10));
        const colById = Object.fromEntries(collectionsOf(site).map((c) => [c.id, c]));
        const rows = hits.slice(0, limit).map((h) => ({
          collection: h.c,
          id: h.id,
          title: h.title,
          excerpt: snippet(h.text, a.query),
          url: env.url(colById[h.c].itemPage.replace('{id}', encodeURIComponent(h.id))),
        }));
        const { items, truncated } = fitList(rows, (x) => ({ results: x }));
        return { query: a.query, total: hits.length, returned: items.length, truncated: truncated || hits.length > items.length, results: items };
      },
    });

    tools.push({
      name: 'list_items',
      title: 'List items of a collection',
      description:
        'List the items of one collection, optionally narrowed by words and field filters and sorted. ' +
        'Returns id, title, summary, the filterable fields and the page URL, plus the total that matched. ' +
        'Page through with offset. Does not change what the user sees (use show_items on a collection page).',
      inputSchema: {
        type: 'object',
        properties: {
          collection: collectionProp(site, true),
          query: { type: 'string', description: 'Words every returned item must contain.', maxLength: 200 },
          where: whereSchema(site),
          sort: { type: 'string', description: 'A filterable field, or "title"; prefix "-" for descending (e.g. "-year").', maxLength: 100 },
          offset: { type: 'integer', minimum: 0, default: 0, description: 'Items to skip.' },
          limit: { type: 'integer', minimum: 1, maximum: MAX_LIST, default: 20, description: `Maximum items (default 20, max ${MAX_LIST}).` },
        },
        required: ['collection'],
        additionalProperties: false,
      },
      annotations: content,
      async execute(a) {
        const { c, list } = await listOf(env, a.collection);
        checkWhere(c, a.where);
        const matched = sortItems(filterItems(list.items, { query: a.query, where: a.where }), a.sort || c.sort);
        const offset = a.offset || 0;
        const limit = Math.min(MAX_LIST, Math.max(1, a.limit || 20));
        const rows = matched.slice(offset, offset + limit).map((it) => absItem(env, it));
        const { items, truncated } = fitList(rows, (x) => ({ items: x }));
        return {
          collection: c.id,
          total: matched.length,
          offset,
          returned: items.length,
          truncated: truncated || offset + items.length < matched.length,
          items,
        };
      },
    });

    tools.push({
      name: 'get_item',
      title: 'Get one item',
      description:
        'Get every field of one item, its links to other items and the items that refer to it. ' +
        'Ids come from search, list_items or get_current_page.',
      inputSchema: {
        type: 'object',
        properties: {
          collection: collectionProp(site, true),
          id: { type: 'string', description: 'Item id.', minLength: 1, maxLength: 300 },
        },
        required: ['collection', 'id'],
        additionalProperties: false,
      },
      annotations: content,
      async execute(a) {
        return slimRecord(env, await itemOf(env, a.collection, a.id));
      },
    });

    tools.push({
      name: 'get_facets',
      title: 'Facet values of a collection',
      description:
        'The values of a collection\'s facet fields with how many items have each. Use them as list_items or show_items filters.',
      inputSchema: {
        type: 'object',
        properties: { collection: collectionProp(site, true) },
        required: ['collection'],
        additionalProperties: false,
      },
      annotations: { readOnlyHint: true },
      async execute(a) {
        const { c, list } = await listOf(env, a.collection);
        const facets = {};
        let cap = 100;
        for (const [k, v] of Object.entries(list.facets || {})) facets[k] = v.slice(0, cap);
        while (JSON.stringify(facets).length > RESULT_BUDGET && cap > 5) {
          cap = Math.floor(cap / 2);
          for (const k of Object.keys(facets)) facets[k] = facets[k].slice(0, cap);
        }
        return { collection: c.id, total: list.count, facets, truncated: cap < 100 };
      },
    });
  }

  tools.push({
    name: 'get_current_page',
    title: 'What the user is looking at',
    description:
      'Describe the page open in the browser: its kind (home, collection, item, page, agents), title and URL; ' +
      'on an item page the item itself, on a collection page the filters the user applied and how many items they see.',
    inputSchema: { type: 'object', properties: {}, additionalProperties: false },
    annotations: content,
    async execute() {
      const out = { kind: page.kind, title: page.title, url: env.location };
      if (page.collection) out.collection = page.collection;
      if (page.kind === 'item') out.item = slimRecord(env, await itemOf(env, page.collection, page.id));
      if (env.view && typeof env.view.state === 'function') out.view = env.view.state();
      return out;
    },
  });

  tools.push({
    name: 'open_page',
    title: 'Open a page',
    description:
      'Navigate the browser to the home page, a collection, an item, a content page or the agents page. ' +
      'The new page registers its own tools; the tools of this page go away.',
    inputSchema: {
      type: 'object',
      properties: {
        target: { type: 'string', enum: ['home', 'collection', 'item', 'page', 'agents'], description: 'Kind of page to open.' },
        collection: { ...collectionProp(site, false), description: 'For target collection or item.' },
        id: { type: 'string', description: 'For target item: the item id.', maxLength: 300 },
        path: { type: 'string', description: 'For target page: the page path from get_site_info.', maxLength: 300 },
      },
      required: ['target'],
      additionalProperties: false,
    },
    async execute(a) {
      let rel;
      if (a.target === 'home') rel = '';
      else if (a.target === 'agents') rel = site.agentsPage || 'agents/';
      else if (a.target === 'collection') rel = findCollection(site, a.collection).page;
      else if (a.target === 'item') {
        if (!a.id) throw new Error('target item needs an id.');
        const rec = await itemOf(env, a.collection, a.id);
        rel = rec.url;
      } else {
        const p = (site.pages || []).find((x) => x.path === a.path || x.path === `${a.path}/` || x.title === a.path);
        if (!p) throw new Error(`No page "${a.path}". Pages: ${(site.pages || []).map((x) => x.path).join(', ') || 'none'}.`);
        rel = p.path;
      }
      const href = env.url(rel);
      env.navigate(href);
      return { navigating_to: href };
    },
  });

  if (page.kind === 'collection' && env.view && typeof env.view.apply === 'function') {
    const c = collectionsOf(site).find((x) => x.id === page.collection);
    tools.push({
      name: 'show_items',
      title: 'Filter the list on screen',
      description:
        `Apply words, filters and a sort to the ${c ? c.title : ''} list the user is looking at, so they see the same items. ` +
        'Changes the visible list and the URL, not any data. Returns how many items match.',
      inputSchema: {
        type: 'object',
        properties: {
          query: { type: 'string', description: 'Words every shown item must contain. Empty clears.', maxLength: 200 },
          where: whereSchema(site),
          sort: { type: 'string', description: 'A filterable field, or "title"; prefix "-" for descending.', maxLength: 100 },
        },
        additionalProperties: false,
      },
      async execute(a) {
        if (c) checkWhere(c, a.where);
        return env.view.apply({ query: a.query || '', where: a.where || {}, sort: a.sort });
      },
    });
  }

  if (page.kind === 'home' && env.view && typeof env.view.search === 'function' && collectionsOf(site).length) {
    tools.push({
      name: 'show_search',
      title: 'Search on screen',
      description:
        'Type a search into the home page search box and show its results to the user. Returns how many items match.',
      inputSchema: {
        type: 'object',
        properties: {
          query: { type: 'string', description: 'Words to look for. Empty clears the results.', maxLength: 200 },
          collection: collectionProp(site, false),
        },
        additionalProperties: false,
      },
      async execute(a) {
        return env.view.search({ query: a.query || '', collection: a.collection });
      },
    });
  }

  if (site.sparql && site.sparql.endpoint) {
    tools.push({
      name: 'run_sparql',
      title: 'Run a SPARQL query',
      description:
        `Run a read-only SPARQL query (SELECT, ASK, CONSTRUCT or DESCRIBE) against ${site.sparql.endpoint}, the live graph behind this site. ` +
        `Returns up to ${MAX_ROWS} rows as {columns, rows}. Start from get_example_queries.`,
      inputSchema: {
        type: 'object',
        properties: {
          query: { type: 'string', description: 'The SPARQL query.', minLength: 1, maxLength: 20000 },
          limit: { type: 'integer', minimum: 1, maximum: MAX_ROWS, default: 50, description: `Maximum rows returned (default 50, max ${MAX_ROWS}).` },
        },
        required: ['query'],
        additionalProperties: false,
      },
      annotations: content,
      async execute(a, opts) {
        if (/^\s*(PREFIX[^\n]*\n\s*)*(INSERT|DELETE|LOAD|CLEAR|CREATE|DROP|COPY|MOVE|ADD)\b/i.test(a.query)) {
          throw new Error('Only read queries are allowed.');
        }
        return runSparql(env, site.sparql.endpoint, a.query, Math.min(MAX_ROWS, a.limit || 50), opts && opts.signal);
      },
    });
    tools.push({
      name: 'get_example_queries',
      title: 'Example SPARQL queries',
      description: 'The site\'s example SPARQL queries with titles; adapt one before writing your own.',
      inputSchema: { type: 'object', properties: {}, additionalProperties: false },
      annotations: { readOnlyHint: true },
      async execute() {
        return { endpoint: site.sparql.endpoint, examples: (site.sparql.examples || []).map((e) => ({ title: e.title, query: e.query })) };
      },
    });
  }

  return tools;
}

function checkWhere(c, where) {
  if (!where) return;
  const ok = new Set([...(c.filterable || []), 'title', 'id']);
  for (const k of Object.keys(where)) {
    if (!ok.has(k)) throw new Error(`"${k}" is not filterable in ${c.id}. Filterable: ${[...ok].join(', ')}.`);
  }
}

async function runSparql(env, endpoint, query, limit, signal) {
  const f = env.fetch || globalThis.fetch;
  const res = await f(endpoint, {
    method: 'POST',
    headers: {
      'Content-Type': 'application/x-www-form-urlencoded',
      Accept: 'application/sparql-results+json, application/n-triples;q=0.9, text/turtle;q=0.8',
    },
    body: new URLSearchParams({ query }).toString(),
    signal,
  });
  const type = res.headers.get('content-type') || '';
  const text = await res.text();
  if (!res.ok) throw new Error(`SPARQL endpoint answered ${res.status}: ${clip(text, 300)}`);
  if (type.includes('json')) {
    const j = JSON.parse(text);
    if (typeof j.boolean === 'boolean') return { boolean: j.boolean };
    const columns = (j.head && j.head.vars) || [];
    const all = (j.results && j.results.bindings) || [];
    const rows = all.slice(0, limit).map((b) => Object.fromEntries(columns.map((v) => [v, b[v] ? b[v].value : null])));
    const { items, truncated } = fitList(rows, (x) => ({ columns, rows: x }));
    return { columns, total: all.length, returned: items.length, truncated: truncated || all.length > items.length, rows: items };
  }
  const lines = text.split('\n').filter((l) => l.trim());
  const { items, truncated } = fitList(lines.slice(0, limit), (x) => ({ triples: x }));
  return { format: type.split(';')[0], total: lines.length, returned: items.length, truncated: truncated || lines.length > items.length, triples: items };
}

export { asList };
