// The machine side of a site: JSON-LD nodes, the static JSON API, OpenAPI,
// llms.txt, sitemap, robots and host headers. All derived from the same model
// as the pages.

import { localName } from './read.mjs';

export const SCHEMA = 'https://schema.org/';
export const CONTEXT = { '@vocab': SCHEMA };

// CSV/JSON field names that already mean a schema.org property.
const WELL_KNOWN = {
  name: 'name', title: 'name', label: 'name', headline: 'headline', description: 'description', abstract: 'abstract',
  summary: 'description', url: 'url', homepage: 'url', website: 'url', image: 'image', author: 'author',
  creator: 'creator', contributor: 'contributor', keywords: 'keywords', license: 'license', identifier: 'identifier',
  doi: 'identifier', sameAs: 'sameAs', dateCreated: 'dateCreated', datePublished: 'datePublished',
  dateModified: 'dateModified', publisher: 'publisher', inLanguage: 'inLanguage', language: 'inLanguage',
  email: 'email', telephone: 'telephone', latitude: 'latitude', longitude: 'longitude', startDate: 'startDate',
  endDate: 'endDate', location: 'location', birthDate: 'birthDate', deathDate: 'deathDate', genre: 'genre',
  citation: 'citation', funder: 'funder', affiliation: 'affiliation', givenName: 'givenName', familyName: 'familyName',
};

const isObj = (v) => v !== null && typeof v === 'object';
const one = (a) => (a.length === 1 ? a[0] : a);

// https://schema.org/name -> name: the context's @vocab is schema.org.
function compact(iri) {
  if (!iri) return iri;
  if (iri.startsWith(SCHEMA) && /^[A-Za-z][A-Za-z0-9]*$/.test(iri.slice(SCHEMA.length))) return iri.slice(SCHEMA.length);
  return iri;
}

// IRIs relative to the document that holds them, or absolute when baseUrl is set.
export function relFrom(S, fromDir) {
  const depth = fromDir.split('/').filter(Boolean).length;
  return (path) => {
    if (S.cfg.baseUrl) return new URL(path, S.cfg.baseUrl).href;
    const r = '../'.repeat(depth) + path;
    return r || './';
  };
}

export function itemNode(S, c, it, rel) {
  let types;
  if (c.type) {
    const same = new Set([c.type, SCHEMA + c.type, `http://schema.org/${c.type}`]);
    types = [c.type, ...it.types.filter((t) => !same.has(t))];
  } else {
    types = it.types.length ? it.types.map(compact) : ['Thing'];
  }
  const page = rel(`${c.id}/${it.slug}/`);
  const node = { '@id': it.iri || page, '@type': types.length === 1 ? types[0] : types, url: page, name: it.label };
  const extra = [];
  const value = (v) => {
    if (isObj(v) && v['@link']) {
      const t = S.model.byId.get(v['@link'].c);
      const target = t.bySlug.get(v['@link'].slug.toLowerCase());
      return { '@id': (target && target.iri) || rel(`${t.id}/${v['@link'].slug}/`) };
    }
    if (isObj(v) && v['@id']) return { '@id': v['@id'] };
    if (isObj(v) && v['@nested']) {
      const o = {};
      for (const [k, vs] of Object.entries(v['@nested'])) {
        if (k === 'type') { o['@type'] = one(vs.map((x) => (isObj(x) && x['@id'] ? compact(x['@id']) : String(x)))); continue; }
        const pk = c.predicates && c.predicates[k] ? compact(c.predicates[k]) : WELL_KNOWN[k] || k;
        o[pk] = one(vs.map(value));
      }
      return o;
    }
    return v;
  };
  for (const f of c.fields) {
    if (f.name === 'type') continue;
    const vals = it.values[f.name];
    if (!vals || !vals.length) continue;
    const key = compact(f.property || f.iri || WELL_KNOWN[f.name]);
    if (key === 'name' && f.name === c.labelField) continue;
    if (key) {
      const jv = vals.map(value);
      node[key] = jv.length === 1 ? jv[0] : jv;
    } else {
      for (const v of vals) extra.push({ '@type': 'PropertyValue', name: f.label, value: isObj(v) ? S.model.label(v) : v });
    }
  }
  if (extra.length) node.additionalProperty = extra;
  return node;
}

export function siteNodes(S, rel) {
  const { cfg } = S;
  const website = { '@type': 'WebSite', '@id': rel(''), name: cfg.title, url: rel(''), inLanguage: S.lang };
  if (cfg.description) website.description = cfg.description;
  const dataset = {
    '@type': 'Dataset',
    '@id': `${rel('')}#dataset`,
    name: cfg.title,
    description: cfg.description || cfg.title,
    url: rel(''),
    isAccessibleForFree: true,
    distribution: [
      { '@type': 'DataDownload', encodingFormat: 'application/ld+json', contentUrl: rel('api/dataset.jsonld') },
      { '@type': 'DataDownload', encodingFormat: 'application/json', contentUrl: rel('api/index.json') },
    ],
  };
  if (cfg.license) dataset.license = cfg.license.url || cfg.license.name || cfg.license;
  if (cfg.creator) dataset.creator = cfg.creator;
  return [website, dataset];
}

export function collectionNode(S, c, rel) {
  return {
    '@context': CONTEXT,
    '@type': 'CollectionPage',
    name: c.title,
    description: c.description || undefined,
    url: rel(`${c.id}/`),
    isPartOf: { '@id': rel('') },
    mainEntity: {
      '@type': 'ItemList',
      numberOfItems: c.items.length,
      itemListElement: S.sorted(c).slice(0, 100).map((it, i) => ({
        '@type': 'ListItem', position: i + 1, url: rel(`${c.id}/${it.slug}/`), name: it.label,
      })),
    },
  };
}

// ---------------------------------------------------------------- JSON API

export function slimItem(S, c, it) {
  const f = {};
  for (const name of c.filterable) {
    const vals = (it.values[name] || []).map(S.model.facetLabel);
    if (vals.length) f[name] = one(vals);
  }
  const out = { id: it.slug, title: it.label };
  if (it.summary) out.summary = it.summary.length > 240 ? `${it.summary.slice(0, 239)}…` : it.summary;
  out.url = `${c.id}/${it.slug}/`;
  if (Object.keys(f).length) out.f = f;
  return out;
}

export function siteIndex(S) {
  const { cfg } = S;
  const out = {
    site: {
      title: cfg.title,
      description: cfg.description || '',
      lang: S.lang,
      ...(cfg.license ? { license: cfg.license } : {}),
      ...(cfg.baseUrl ? { baseUrl: cfg.baseUrl } : {}),
      generator: `semantic-site ${S.generatorVersion}`,
      version: S.version,
    },
    collections: S.collections.map((c) => ({
      id: c.id,
      title: c.title,
      description: c.description,
      type: c.type || 'Thing',
      count: c.items.length,
      fields: c.fields.map((f) => ({ name: f.name, label: f.label, type: f.type, ...(f.iri ? { iri: f.iri } : {}), ...(f.link ? { link: f.link } : {}) })),
      filterable: c.filterable,
      facets: c.facets,
      sort: c.sort,
      page: `${c.id}/`,
      list: `api/${c.id}/index.json`,
      item: `api/${c.id}/{id}.json`,
      itemPage: `${c.id}/{id}/`,
    })),
    pages: (cfg.pages || []).map((p) => ({ path: `${p.path}/`, title: p.title })),
    api: {
      index: 'api/index.json',
      search: 'api/search.json',
      tools: 'api/tools.json',
      openapi: 'api/openapi.json',
      jsonld: 'api/dataset.jsonld',
      llms: 'llms.txt',
    },
    agentsPage: 'agents/',
    webmcp: {
      untrustedContent: !(cfg.webmcp && cfg.webmcp.untrustedContent === false),
      scripts: (cfg.webmcp && cfg.webmcp.scripts) || [],
    },
  };
  if (cfg.sparql && cfg.sparql.endpoint) out.sparql = { endpoint: cfg.sparql.endpoint, examples: cfg.sparql.examples || [] };
  return out;
}

export function collectionList(S, c) {
  return {
    collection: c.id,
    title: c.title,
    count: c.items.length,
    columns: c.columns.map((n) => ({ name: n, label: (c.fields.find((f) => f.name === n) || {}).label || n })),
    filterable: c.filterable,
    facets: c.facetValues,
    sort: c.sort,
    items: S.slimSorted(c),
  };
}

export function itemRecord(S, c, it) {
  const rel = relFrom(S, `api/${c.id}/`);
  const display = (v) => {
    if (isObj(v) && v['@nested']) return Object.fromEntries(Object.entries(v['@nested']).map(([k, vs]) => [k, one(vs.map(display))]));
    return isObj(v) ? S.model.label(v) : v;
  };
  const fields = {};
  const links = {};
  for (const f of c.fields) {
    const vals = it.values[f.name];
    if (!vals || !vals.length) continue;
    fields[f.name] = one(vals.map(display));
    const ls = vals.filter((v) => isObj(v) && v['@link']).map((v) => ({
      collection: v['@link'].c, id: v['@link'].slug, title: S.model.label(v), url: `${v['@link'].c}/${v['@link'].slug}/`,
    }));
    if (ls.length) links[f.name] = ls;
  }
  const seen = new Set();
  const backlinks = [];
  for (const b of it.backlinks) {
    const k = `${b.c}|${b.field}|${b.slug}`;
    if (seen.has(k)) continue;
    seen.add(k);
    backlinks.push({ collection: b.c, field: b.field, id: b.slug, title: b.label, url: `${b.c}/${b.slug}/` });
  }
  const rec = { collection: c.id, id: it.slug, title: it.label, url: `${c.id}/${it.slug}/` };
  if (it.iri) rec.iri = it.iri;
  if (it.types.length) rec.types = it.types;
  if (it.summary) rec.summary = it.summary;
  rec.fields = fields;
  rec.links = links;
  rec.backlinks = backlinks;
  rec.jsonld = { '@context': CONTEXT, ...itemNode(S, c, it, rel) };
  return rec;
}

export function searchIndex(S) {
  const rows = [];
  for (const c of S.collections) {
    for (const it of S.sorted(c)) {
      const parts = [];
      if (it.summary) parts.push(it.summary);
      for (const f of c.fields) {
        if (f.name === c.labelField || f.name === c.summaryField) continue;
        for (const v of it.values[f.name] || []) {
          const s = isObj(v) ? (v['@id'] && !v['@link'] ? localName(v['@id']) : S.model.label(v)) : String(v);
          if (s && s.length < 200) parts.push(s);
        }
      }
      let text = parts.join(' · ');
      if (text.length > 400) text = `${text.slice(0, 399)}…`;
      rows.push([c.id, it.slug, it.label, text]);
    }
  }
  return { fields: ['c', 'id', 't', 's'], rows };
}

export function datasetJsonLd(S) {
  const rel = relFrom(S, 'api/');
  const graph = [...siteNodes(S, rel)];
  for (const c of S.collections) for (const it of S.sorted(c)) graph.push(itemNode(S, c, it, rel));
  return { '@context': CONTEXT, '@graph': graph };
}

export function openapi(S) {
  const { cfg } = S;
  const ids = S.collections.map((c) => c.id);
  const ok = (description, type = 'application/json', ref) => ({
    200: { description, content: { [type]: ref ? { schema: { $ref: `#/components/schemas/${ref}` } } : {} } },
  });
  const colParam = { name: 'collection', in: 'path', required: true, schema: { type: 'string', ...(ids.length ? { enum: ids } : {}) } };
  return {
    openapi: '3.1.0',
    info: {
      title: `${cfg.title} — static API`,
      version: S.version,
      description: 'Read-only JSON files on static hosting: GET only, no authentication, CORS open. The same data as the HTML pages.',
      ...(cfg.license && cfg.license.url ? { license: { name: cfg.license.name || cfg.license.url, url: cfg.license.url } } : {}),
    },
    servers: [{ url: cfg.baseUrl ? cfg.baseUrl.replace(/\/$/, '') : '..' }],
    paths: {
      '/api/index.json': { get: { operationId: 'getSite', summary: 'The site: collections, fields, facets, counts and endpoints', responses: ok('Site index', 'application/json', 'Site') } },
      '/api/search.json': { get: { operationId: 'getSearchIndex', summary: 'Search index: [collection, id, title, text] rows', responses: ok('Search index', 'application/json', 'SearchIndex') } },
      '/api/{collection}/index.json': { get: { operationId: 'listItems', summary: 'Every item of a collection with its list fields, and its facets', parameters: [colParam], responses: ok('Collection list', 'application/json', 'ItemList') } },
      '/api/{collection}/{id}.json': { get: { operationId: 'getItem', summary: 'One item: fields, links, back-links, JSON-LD', parameters: [colParam, { name: 'id', in: 'path', required: true, schema: { type: 'string' } }], responses: { ...ok('Item', 'application/json', 'Item'), 404: { description: 'No such item' } } } },
      '/api/dataset.jsonld': { get: { operationId: 'getDataset', summary: 'Everything as one JSON-LD graph (schema.org)', responses: ok('JSON-LD', 'application/ld+json') } },
      '/api/tools.json': { get: { operationId: 'getTools', summary: 'The WebMCP tools each page kind registers', responses: ok('WebMCP tools') } },
    },
    components: {
      schemas: {
        Site: { type: 'object', required: ['site', 'collections', 'api'], properties: { site: { type: 'object' }, collections: { type: 'array', items: { type: 'object' } }, pages: { type: 'array' }, api: { type: 'object' } } },
        SearchIndex: { type: 'object', properties: { fields: { type: 'array', items: { type: 'string' } }, rows: { type: 'array', items: { type: 'array', items: { type: 'string' } } } } },
        ItemList: { type: 'object', properties: { collection: { type: 'string' }, count: { type: 'integer' }, facets: { type: 'object' }, items: { type: 'array', items: { $ref: '#/components/schemas/ItemSummary' } } } },
        ItemSummary: { type: 'object', required: ['id', 'title', 'url'], properties: { id: { type: 'string' }, title: { type: 'string' }, summary: { type: 'string' }, url: { type: 'string' }, f: { type: 'object' } } },
        Item: { type: 'object', required: ['collection', 'id', 'title', 'fields'], properties: { collection: { type: 'string' }, id: { type: 'string' }, title: { type: 'string' }, iri: { type: 'string' }, url: { type: 'string' }, fields: { type: 'object' }, links: { type: 'object' }, backlinks: { type: 'array' }, jsonld: { type: 'object' } } },
      },
    },
  };
}

export function llmsTxt(S) {
  const { cfg } = S;
  const u = (p) => (cfg.baseUrl ? new URL(p, cfg.baseUrl).href : p || './');
  const lines = [`# ${cfg.title}`, ''];
  if (cfg.description) lines.push(`> ${cfg.description.replace(/\s+/g, ' ')}`, '');
  lines.push(
    'Every page has JSON-LD, links its own JSON file, and registers WebMCP tools for in-browser agents.',
    '',
    '## API',
    '',
    `- [Site index](${u('api/index.json')}): collections, fields, facets, counts, endpoints`,
    `- [Search index](${u('api/search.json')}): collection, id, title and text of every item`,
    `- [OpenAPI description](${u('api/openapi.json')})`,
    `- [Everything as JSON-LD](${u('api/dataset.jsonld')})`,
    `- [WebMCP tools](${u('api/tools.json')}): the tools each page registers, with input schemas`,
  );
  if (cfg.sparql && cfg.sparql.endpoint) lines.push(`- [SPARQL endpoint](${cfg.sparql.endpoint})`);
  if (S.collections.length) {
    lines.push('', '## Collections', '');
    for (const c of S.collections) {
      lines.push(`- [${c.title}](${u(`${c.id}/`)}): ${c.items.length} items${c.description ? `. ${c.description.replace(/\s+/g, ' ')}` : ''} JSON: ${u(`api/${c.id}/index.json`)}, one item: ${u(`api/${c.id}/{id}.json`)}`);
    }
  }
  if ((cfg.pages || []).length) {
    lines.push('', '## Pages', '');
    for (const p of cfg.pages) lines.push(`- [${p.title}](${u(`${p.path}/`)})${p.description ? `: ${p.description}` : ''}`);
  }
  lines.push('', '## Optional', '', `- [Agents & API](${u('agents/')}): how to use the WebMCP tools`, '');
  return lines.join('\n');
}

export function sitemap(S, paths) {
  if (!S.cfg.baseUrl) return null;
  const esc = (s) => s.replace(/&/g, '&amp;').replace(/</g, '&lt;');
  return `<?xml version="1.0" encoding="UTF-8"?>
<urlset xmlns="http://www.sitemaps.org/schemas/sitemap/0.9">
${paths.map((p) => `  <url><loc>${esc(new URL(p, S.cfg.baseUrl).href)}</loc></url>`).join('\n')}
</urlset>
`;
}

export function robots(S) {
  return `User-agent: *\nAllow: /\n${S.cfg.baseUrl ? `Sitemap: ${new URL('sitemap.xml', S.cfg.baseUrl).href}\n` : ''}`;
}

export const HEADERS = `# Response headers for Cloudflare Pages / Workers static assets and Netlify.
# GitHub Pages ignores this file and already sends Access-Control-Allow-Origin: *.
# Cloudflare applies every rule that matches and joins the values, so no header
# name appears in two rules that can match the same path.
/*
  X-Content-Type-Options: nosniff
  Referrer-Policy: strict-origin-when-cross-origin

/api/*
  Access-Control-Allow-Origin: *
  Cache-Control: public, max-age=300

/api/*.jsonld
  Content-Type: application/ld+json; charset=utf-8

/llms.txt
  Access-Control-Allow-Origin: *
  Content-Type: text/plain; charset=utf-8

/assets/*
  Cache-Control: public, max-age=0, must-revalidate
`;
