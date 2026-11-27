// WebMCP tools for rete plaza.
// Registers tools through document.modelContext (current spec; navigator.modelContext
// and provideContext are tried as fallbacks for older previews). No-op without the API.
// Tool results are plain JSON-serialisable values (the spec serialises whatever
// execute() resolves with); failures are thrown Errors with a readable message.

const MAX_BYTES = 18000; // keep each result well under ~20 KB
const WAIT_MS = 10000;

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

async function waitForAny(names) {
  const end = Date.now() + WAIT_MS;
  while (Date.now() < end) {
    for (const n of names) if (window[n]) return n;
    await sleep(100);
  }
  return null;
}

function getContext() {
  try {
    if (typeof document !== 'undefined' && document.modelContext) return document.modelContext;
  } catch (_) { /* ignore */ }
  try {
    if (typeof navigator !== 'undefined' && navigator.modelContext) return navigator.modelContext;
  } catch (_) { /* ignore */ }
  return null;
}

const size = (v) => JSON.stringify(v).length;

function clip(s, n) {
  s = s == null ? '' : String(s);
  return s.length > n ? s.slice(0, n - 1) + '…' : s;
}

// Drop trailing items until the JSON fits; returns { items, truncated }.
function fitList(items, wrap, budget = MAX_BYTES) {
  let n = items.length;
  while (n > 0 && size(wrap(items.slice(0, n))) > budget) n = Math.max(0, Math.floor(n * 0.8) - (n < 5 ? 1 : 0));
  return { items: items.slice(0, n), truncated: n < items.length };
}

function slim(s) {
  return {
    key: s.key,
    title: s.title,
    description: clip(s.description, 240),
    category: s.category,
    tags: (s.tags || []).slice(0, 8),
    licence: s.licence,
    triples: s.triples,
    terms: s.terms,
    bytes: s.bytes,
    vocabularies: (s.vocabularies || []).slice(0, 8),
    features: (s.features || []).slice(0, 8),
    delivery: s.delivery,
    page: s.page,
  };
}

const FILTER_GROUPS = ['type', 'kind', 'feature', 'vocab', 'topic', 'licence', 'size', 'provider'];
const SORTS = ['relevance', 'name', 'triples', 'size', 'vocabs'];

const filtersSchema = {
  type: 'object',
  description:
    'Facet filters: group id -> list of values to match. Get valid values from list_filters. Values within a group are alternatives; groups are combined.',
  properties: Object.fromEntries(
    FILTER_GROUPS.map((g) => [g, { type: 'array', items: { type: 'string' }, maxItems: 20 }]),
  ),
  additionalProperties: false,
};

function cleanFilters(f) {
  const out = {};
  if (!f || typeof f !== 'object') return out;
  for (const g of FILTER_GROUPS) {
    const v = f[g];
    if (Array.isArray(v) && v.length) out[g] = v.map(String).slice(0, 20);
    else if (typeof v === 'string' && v) out[g] = [v];
  }
  return out;
}

function cleanSort(s) {
  return SORTS.includes(s) ? s : undefined;
}

function viewArgs(a) {
  return {
    query: typeof a.query === 'string' ? a.query : '',
    filters: cleanFilters(a.filters),
    sort: cleanSort(a.sort),
  };
}

const gallerySchemaProps = {
  query: { type: 'string', description: 'Free-text query over title, description, tags, vocabularies. Empty for no text constraint.', maxLength: 300 },
  filters: filtersSchema,
  sort: { type: 'string', enum: SORTS, description: 'Sort order. relevance only makes sense with a query.' },
};

// ---- tool definitions ----------------------------------------------------

function galleryTools(plaza) {
  return [
    {
      name: 'search_datasets',
      description:
        'Search the rete plaza gallery of knowledge-graph datasets. Use first when the user asks what data exists on a topic. Returns compact summaries (key, title, short description, licence, triple count, size in bytes, vocabularies, features, page URL). The key identifies a dataset in get_dataset_card and open_dataset. Does not change what the user sees; use show_datasets for that.',
      inputSchema: {
        type: 'object',
        properties: {
          ...gallerySchemaProps,
          limit: { type: 'integer', minimum: 1, maximum: 50, default: 20, description: 'Maximum results (default 20, max 50).' },
        },
        additionalProperties: false,
      },
      annotations: { readOnlyHint: true },
      async execute(a = {}) {
        const limit = Math.min(50, Math.max(1, Number(a.limit) || 20));
        const v = viewArgs(a);
        const all = (await plaza.search({ ...v, limit })) || [];
        const rows = all.slice(0, limit).map(slim);
        const { items, truncated } = fitList(rows, (x) => ({ results: x }));
        return {
          total: all.length,
          returned: items.length,
          truncated: truncated || all.length > rows.length,
          results: items,
        };
      },
    },
    {
      name: 'list_filters',
      description:
        'List the filter groups of the gallery (type, kind, feature, vocab, topic, licence, size, provider) with their values and dataset counts. Call before search_datasets or show_datasets when you need valid filter values.',
      inputSchema: { type: 'object', properties: {}, additionalProperties: false },
      annotations: { readOnlyHint: true },
      async execute() {
        const f = plaza.facets() || {};
        const out = {};
        let truncated = false;
        for (const [id, g] of Object.entries(f)) {
          const vals = (g.values || []).map((x) => ({ value: x.value, count: x.count }));
          out[id] = { label: g.label, values: vals };
        }
        // shrink the largest groups until it fits
        let cap = 60;
        while (size(out) > MAX_BYTES && cap > 5) {
          cap = Math.floor(cap / 2);
          for (const g of Object.values(out)) {
            if (g.values.length > cap) { g.values = g.values.slice(0, cap); truncated = true; }
          }
        }
        return { groups: out, truncated };
      },
    },
    {
      name: 'get_dataset_card',
      description:
        'Get the details of one dataset by key: description, licence, source, counts, main classes and predicates, vocabularies, and the titles of its starter queries. Use after search_datasets to decide which dataset to open. To query the data, call open_dataset, then use the query tools the dataset page exposes.',
      inputSchema: {
        type: 'object',
        properties: { key: { type: 'string', description: 'Dataset key from search_datasets.', minLength: 1, maxLength: 200 } },
        required: ['key'],
        additionalProperties: false,
      },
      annotations: { readOnlyHint: true },
      async execute(a = {}) {
        const key = String(a.key || '');
        const hit = (plaza.list() || []).find((s) => s.key === key);
        const card = await plaza.card(key);
        if (!card && !hit) throw new Error(`No dataset with key "${key}". Use search_datasets to find keys.`);
        return digestCard(hit, card);
      },
    },
    {
      name: 'show_datasets',
      description:
        'Apply a query, filters and sort to the gallery page the user is looking at, so they see the same list. Changes the visible page and URL but not any data. Returns how many datasets match. Use when the user asks to "show" or "filter" datasets; use search_datasets when you only need the results yourself.',
      inputSchema: { type: 'object', properties: gallerySchemaProps, additionalProperties: false },
      async execute(a = {}) {
        const r = await plaza.applyView(viewArgs(a));
        return { count: r && r.count, applied: viewArgs(a) };
      },
    },
    {
      name: 'open_dataset',
      description:
        'Navigate the page to one dataset (by key). The dataset page then exposes query tools (get_dataset_overview, get_example_queries, run_sparql, find_entities, show_query_in_page); the tool list changes after navigation. Use when the user wants to explore or query a specific dataset.',
      inputSchema: {
        type: 'object',
        properties: { key: { type: 'string', description: 'Dataset key from search_datasets.', minLength: 1, maxLength: 200 } },
        required: ['key'],
        additionalProperties: false,
      },
      async execute(a = {}) {
        const key = String(a.key || '');
        if (!key) throw new Error('key is required');
        plaza.open(key);
        return { navigating: true, key, note: 'The dataset page is loading; its query tools will be available there.' };
      },
    },
  ];
}

function digestCard(summary, card) {
  const out = {};
  if (summary) Object.assign(out, slim(summary), { description: clip(summary.description, 600) });
  if (card && typeof card === 'object') {
    const pick = (...names) => { for (const n of names) if (card[n] != null) return card[n]; return undefined; };
    const desc = pick('description', 'abstract', 'summary');
    if (desc) out.description = clip(typeof desc === 'string' ? desc : JSON.stringify(desc), 1200);
    for (const [k, names] of Object.entries({
      licence: ['licence', 'license'],
      source: ['source', 'sources', 'provenance', 'homepage'],
      publisher: ['publisher'],
      version: ['version', 'sourceDate', 'source_date'],
    })) {
      const v = pick(...names);
      if (v != null) out[k] = clipDeep(v, 400);
    }
    // The card keeps its counts as flat fields.
    const counts = {};
    for (const [k, name] of Object.entries({
      triples: 'triple_count', quads: 'quad_count', namedGraphs: 'named_graph_count', terms: 'term_count',
    })) if (typeof card[name] === 'number') counts[k] = card[name];
    if (Object.keys(counts).length) out.counts = counts;
    const classes = card.classes || (card.schema && card.schema.classes);
    const preds = card.predicates || (card.schema && card.schema.predicates);
    if (Array.isArray(classes)) out.topClasses = classes.slice(0, 15).map(brief);
    if (Array.isArray(preds)) out.topPredicates = preds.slice(0, 15).map(brief);
    const vocabs = pick('vocabularies', 'vocabs');
    if (vocabs) out.vocabularies = clipDeep(vocabs, 200, 20);
    // `example_queries` are SPARQL strings whose first comment line is the
    // title; `queries` is the generated starter library ({ title, sparql, … }).
    const titleOf = (e) => {
      if (typeof e === 'string') { const m = /^\s*#\s*(.+)/.exec(e); return m ? m[1] : e.split('\n')[0]; }
      return (e && (e.title || e.label || e.name)) || '';
    };
    const ex = [].concat(Array.isArray(card.example_queries) ? card.example_queries : [], Array.isArray(card.queries) ? card.queries : []);
    if (ex.length) out.starterQueryTitles = ex.slice(0, 20).map((e) => clip(titleOf(e), 120)).filter(Boolean);
  }
  // final guard
  while (size(out) > MAX_BYTES) {
    const k = ['topPredicates', 'topClasses', 'starterQueryTitles', 'vocabularies', 'source', 'counts'].find((x) => out[x] !== undefined);
    if (!k) { out.description = clip(out.description, 300); break; }
    delete out[k];
    out.truncated = true;
  }
  return out;
}

function brief(x) {
  // Cards store classes and predicates as [iri, count] pairs.
  if (Array.isArray(x)) return { iri: String(x[0] || '').replace(/^<|>$/g, ''), count: x[1] };
  if (x && typeof x === 'object') {
    return { iri: x.iri || x.id, label: x.label, count: x.count };
  }
  return x;
}

function clipDeep(v, n, maxItems = 10) {
  if (typeof v === 'string') return clip(v, n);
  if (Array.isArray(v)) return v.slice(0, maxItems).map((x) => clipDeep(x, n, maxItems));
  if (v && typeof v === 'object') {
    const o = {};
    for (const [k, x] of Object.entries(v).slice(0, 20)) o[k] = clipDeep(x, n, maxItems);
    return o;
  }
  return v;
}

function datasetTools(pd) {
  const digest = (sch) => ({
    classes: ((sch && sch.classes) || []).slice(0, 25).map(brief),
    predicates: ((sch && sch.predicates) || []).slice(0, 25).map(brief),
    prefixes: sch && sch.prefixes ? Object.fromEntries(Object.entries(sch.prefixes).slice(0, 25)) : {},
  });
  const exampleList = (ex, withSparql) =>
    (ex || []).slice(0, 20).map((e, i) => ({
      index: i,
      title: clip(e.title, 150),
      ...(withSparql ? { sparql: clip(e.sparql, 1500) } : {}),
    }));

  return [
    {
      name: 'get_dataset_overview',
      description:
        'Overview of the dataset on this page: summary (title, licence, size, counts), a digest of its schema (top classes and predicates with counts, namespace prefixes) and the titles of its starter queries. Call this first, before writing SPARQL, so queries use real class and predicate IRIs.',
      inputSchema: { type: 'object', properties: {}, additionalProperties: false },
      annotations: { readOnlyHint: true },
      async execute() {
        await pd.ready;
        const out = { summary: slim(pd.summary() || {}) };
        try { out.schema = digest(pd.schema()); } catch (e) { out.schemaError = String(e && e.message || e); }
        out.exampleQueries = exampleList(pd.examples(), false);
        while (size(out) > MAX_BYTES && out.schema.predicates.length > 5) {
          out.schema.predicates.length = Math.floor(out.schema.predicates.length / 2);
          out.schema.classes.length = Math.max(5, Math.floor(out.schema.classes.length * 0.75));
          out.truncated = true;
        }
        return out;
      },
    },
    {
      name: 'get_example_queries',
      description:
        'The dataset card’s starter SPARQL queries (title and query text). Use them as templates for run_sparql.',
      inputSchema: { type: 'object', properties: {}, additionalProperties: false },
      annotations: { readOnlyHint: true },
      async execute() {
        await pd.ready;
        const all = pd.examples() || [];
        const { items, truncated } = fitList(exampleList(all, true), (x) => ({ examples: x }));
        return { total: all.length, examples: items, truncated: truncated || all.length > 20 };
      },
    },
    {
      name: 'run_sparql',
      description:
        'Run a read-only SPARQL query (SELECT, ASK, CONSTRUCT, DESCRIBE) against the dataset on this page. The data is read remotely in ranges, so selective queries are fast; add LIMIT and avoid unbounded scans. Returns columns, rows (arrays of strings, IRIs in full), truncated flag and stats (bytes read, requests, ms). Errors are returned as tool errors with the engine message; fix the query and retry. Call get_dataset_overview first for class and predicate IRIs.',
      inputSchema: {
        type: 'object',
        properties: {
          query: { type: 'string', description: 'SPARQL 1.1 query text. Include PREFIX declarations.', minLength: 1, maxLength: 8000 },
          limit: { type: 'integer', minimum: 1, maximum: 500, default: 50, description: 'Maximum rows returned (default 50, max 500).' },
        },
        required: ['query'],
        additionalProperties: false,
      },
      annotations: { readOnlyHint: true, untrustedContentHint: true },
      async execute(a = {}) {
        await pd.ready;
        const q = String(a.query || '').trim();
        if (!q) throw new Error('query is required');
        // Blank the IRIs first: a "#" inside <…> is not a comment.
        const body = q.replace(/<[^<>\s]*>/g, '<>').replace(/#[^\n]*/g, '').replace(/^\s*(?:(?:PREFIX\s+[^\s:]*:\s*<>|BASE\s*<>)\s*)*/i, '');
        if (!/^(SELECT|ASK|CONSTRUCT|DESCRIBE)\b/i.test(body)) {
          throw new Error('Only read-only queries (SELECT, ASK, CONSTRUCT, DESCRIBE) are supported.');
        }
        const limit = Math.min(500, Math.max(1, Number(a.limit) || 50));
        let r;
        try {
          r = await pd.query(q, { limit, timeoutMs: 60000 });
        } catch (e) {
          throw new Error('SPARQL error: ' + (e && e.message ? e.message : String(e)));
        }
        const columns = (r.columns || []).map(String);
        let rows = (r.rows || []).slice(0, limit).map((row) => row.map((c) => clip(c, 300)));
        let truncated = !!r.truncated || (r.rows || []).length > rows.length;
        const wrap = (x) => ({ columns, rows: x });
        const fit = fitList(rows, wrap);
        if (fit.truncated) truncated = true;
        return {
          columns,
          rows: fit.items,
          rowCount: fit.items.length,
          truncated,
          ...(truncated ? { note: 'Result truncated; add a tighter LIMIT/FILTER or aggregate to see the rest.' } : {}),
          stats: r.stats || {},
        };
      },
    },
    {
      name: 'find_entities',
      description:
        'Find entities in this dataset by label text (prefix search). Returns IRIs with labels, to use in SPARQL. Use when the user names a thing (a person, place, concept) and you need its IRI.',
      inputSchema: {
        type: 'object',
        properties: {
          text: { type: 'string', description: 'Label prefix to look for.', minLength: 1, maxLength: 200 },
          limit: { type: 'integer', minimum: 1, maximum: 50, default: 10, description: 'Maximum results (default 10, max 50).' },
        },
        required: ['text'],
        additionalProperties: false,
      },
      annotations: { readOnlyHint: true, untrustedContentHint: true },
      async execute(a = {}) {
        await pd.ready;
        const text = String(a.text || '').trim();
        if (!text) throw new Error('text is required');
        const limit = Math.min(50, Math.max(1, Number(a.limit) || 10));
        const res = (await pd.suggest(text, { limit })) || [];
        const rows = res.slice(0, limit).map((x) => ({ iri: x.iri, label: clip(x.label, 200) }));
        const { items, truncated } = fitList(rows, (x) => ({ results: x }));
        return { results: items, truncated };
      },
    },
    {
      name: 'show_query_in_page',
      description:
        'Put a SPARQL query into the visible query editor of this page and optionally run it, so the human sees what you did and can edit it. Use to hand a query to the user ("show me", "put it in the editor"). Does not return rows; use run_sparql when you need the results.',
      inputSchema: {
        type: 'object',
        properties: {
          query: { type: 'string', description: 'SPARQL query text.', minLength: 1, maxLength: 8000 },
          run: { type: 'boolean', default: true, description: 'Also run it (default true).' },
        },
        required: ['query'],
        additionalProperties: false,
      },
      async execute(a = {}) {
        await pd.ready;
        const q = String(a.query || '').trim();
        if (!q) throw new Error('query is required');
        const run = a.run !== false;
        await pd.showQuery(q, { run });
        return { shown: true, ran: run };
      },
    },
  ];
}

// ---- registration --------------------------------------------------------

const controllers = [];

async function registerAll(ctx, tools) {
  const names = [];
  if (typeof ctx.registerTool === 'function') {
    for (const t of tools) {
      const ac = typeof AbortController === 'function' ? new AbortController() : null;
      try {
        // Current spec: registerTool(tool, { signal }) -> Promise<undefined>; abort the signal to unregister.
        await ctx.registerTool(t, ac ? { signal: ac.signal } : undefined);
        controllers.push({ ac, name: t.name });
        names.push(t.name);
      } catch (e) {
        console.debug('[plaza webmcp] registerTool failed for', t.name, e && e.message);
      }
    }
  } else if (typeof ctx.provideContext === 'function') {
    // Early-preview shape.
    try { await ctx.provideContext({ tools }); names.push(...tools.map((t) => t.name)); }
    catch (e) { console.debug('[plaza webmcp] provideContext failed', e && e.message); }
  }
  return names;
}

function unregisterAll(ctx) {
  for (const { ac, name } of controllers.splice(0)) {
    try {
      if (ac) ac.abort();
      else if (ctx && typeof ctx.unregisterTool === 'function') ctx.unregisterTool(name);
    } catch (_) { /* ignore */ }
  }
}

function announce(detail) {
  window.plazaWebMCP = detail;
  try { document.dispatchEvent(new CustomEvent('plaza:webmcp', { detail })); } catch (_) { /* ignore */ }
}

async function main() {
  const ctx = getContext();
  if (!ctx || (typeof ctx.registerTool !== 'function' && typeof ctx.provideContext !== 'function')) {
    console.debug('[plaza webmcp] navigator.modelContext not available; agent tools off');
    announce({ supported: false, tools: [] });
    return;
  }
  const which = await waitForAny(['plaza', 'plazaDataset']);
  const plaza = which === 'plaza' ? window.plaza : null;
  const pd = which === 'plazaDataset' ? window.plazaDataset : null;
  let tools = [];
  if (plaza) {
    try { await plaza.ready; tools = galleryTools(plaza); } catch (e) { console.debug('[plaza webmcp] plaza not ready', e && e.message); }
  } else if (pd) {
    try { await pd.ready; tools = datasetTools(pd); } catch (e) { console.debug('[plaza webmcp] dataset not ready', e && e.message); }
  }
  if (!tools.length) { announce({ supported: true, tools: [] }); return; }
  const names = await registerAll(ctx, tools);
  addEventListener('pagehide', () => unregisterAll(ctx));
  console.debug('[plaza webmcp] registered', names.join(', '));
  announce({ supported: true, tools: names });
}

main().catch((e) => {
  console.debug('[plaza webmcp] failed', e && e.message);
});
