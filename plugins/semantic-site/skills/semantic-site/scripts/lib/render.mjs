// HTML for every page kind. Every page carries the same head: JSON-LD, a link
// to its own JSON, the OpenAPI description, its page context, and boot.js,
// which registers the page's WebMCP tools.

import { esc, markdown } from './markdown.mjs';
import { localName } from './read.mjs';

const json = (x) => JSON.stringify(x).replace(/</g, '\\u003c');
const isObj = (v) => v !== null && typeof v === 'object';
const LIST_LIMIT = 5000;

export function shortIri(iri) {
  const s = String(iri).replace(/^https?:\/\//, '');
  return s.length > 60 ? `${s.slice(0, 28)}…${s.slice(-28)}` : s;
}

function themeStyle(theme) {
  if (!theme) return '';
  const light = [];
  const dark = [];
  if (theme.accent) light.push(`--accent:${theme.accent};--accent-deep:${theme.accentDeep || theme.accent}`);
  if (theme.accent2) light.push(`--accent-2:${theme.accent2}`);
  if (theme.accentDark) dark.push(`--accent:${theme.accentDark};--accent-deep:${theme.accentDark}`);
  if (theme.accent2Dark) dark.push(`--accent-2:${theme.accent2Dark}`);
  const safe = (s) => s.replace(/[^#\w\s:;(),.%-]/g, '');
  let css = '';
  if (light.length) css += `:root{${safe(light.join(';'))}}`;
  if (dark.length) {
    css += `@media (prefers-color-scheme: dark){:root:not([data-theme="light"]){${safe(dark.join(';'))}}}`;
    css += `:root[data-theme="dark"]{${safe(dark.join(';'))}}`;
  }
  return css ? `<style>${css}</style>\n` : '';
}

export function shell(S, o) {
  const { cfg } = S;
  const root = o.root;
  const themeKey = `theme-${S.slug}`;
  const navLinks = [
    ...S.collections.map((c) => ({ href: `${c.id}/`, text: c.title })),
    ...(cfg.pages || []).map((p) => ({ href: `${p.path}/`, text: p.title })),
    { href: 'agents/', text: 'Agents & API' },
  ];
  const canonical = cfg.baseUrl && o.path !== undefined ? new URL(o.path, cfg.baseUrl).href : null;
  const head = [
    '<!doctype html>',
    `<html lang="${esc(S.lang)}">`,
    '<head>',
    '<meta charset="utf-8">',
    '<meta name="viewport" content="width=device-width, initial-scale=1">',
    `<title>${esc(o.title)}</title>`,
    `<meta name="description" content="${esc(o.description || cfg.description || '')}">`,
    `<meta name="site-root" content="${esc(root)}">`,
    `<meta name="theme-key" content="${esc(themeKey)}">`,
    '<meta name="generator" content="semantic-site">',
    canonical ? `<link rel="canonical" href="${esc(canonical)}">` : '',
    `<meta property="og:title" content="${esc(o.title)}">`,
    `<meta property="og:description" content="${esc(o.description || cfg.description || '')}">`,
    '<meta property="og:type" content="website">',
    canonical ? `<meta property="og:url" content="${esc(canonical)}">` : '',
    cfg.favicon ? `<link rel="icon" href="${esc(root + cfg.favicon)}">` : '<link rel="icon" href="data:image/svg+xml,%3Csvg xmlns=\'http://www.w3.org/2000/svg\' viewBox=\'0 0 32 32\'%3E%3Ccircle cx=\'16\' cy=\'16\' r=\'9\' fill=\'%23147d69\'/%3E%3C/svg%3E">',
    `<link rel="stylesheet" href="${root}assets/site.css?v=${S.version}">`,
    themeStyle(cfg.theme),
    `<script>try{var t=localStorage.getItem(${json(themeKey)});if(t)document.documentElement.setAttribute("data-theme",t)}catch(e){}</script>`,
    o.alternate ? `<link rel="alternate" type="application/json" href="${root}${o.alternate}" title="This page as JSON">` : '',
    `<link rel="service-desc" type="application/openapi+json" href="${root}api/openapi.json">`,
    `<link rel="alternate" type="application/ld+json" href="${root}api/dataset.jsonld" title="The whole site as JSON-LD">`,
    `<script type="application/json" id="page-context">${json(o.context)}</script>`,
    ...(o.jsonld || []).map((j) => `<script type="application/ld+json">${json(j)}</script>`),
    `<script type="module" src="${root}assets/boot.js?v=${S.version}"></script>`,
    '</head>',
  ].filter(Boolean);
  const current = o.navCurrent;
  const nav = navLinks.map((l) => `<a href="${root}${esc(l.href)}"${current === l.href ? ' aria-current="page"' : ''}>${esc(l.text)}</a>`).join('\n    ');
  const lic = cfg.license
    ? (cfg.license.url ? ` Data licence: <a href="${esc(cfg.license.url)}">${esc(cfg.license.name || cfg.license.url)}</a>.` : ` Data licence: ${esc(cfg.license.name || cfg.license)}.`)
    : '';
  const extra = (cfg.links || []).map((l) => `<a href="${esc(l.url)}">${esc(l.title)}</a>`).join('\n      ');
  return `${head.join('\n')}
<body>
<a class="skip" href="#main">Skip to content</a>
<header class="top">
  <div class="wrap">
    <a class="brand" href="${root}">${esc(cfg.title)}</a>
    <nav class="main" aria-label="Site">
    ${nav}
    </nav>
    <button class="theme-toggle" id="theme" type="button">Light / dark</button>
  </div>
</header>
<main id="main" class="wrap">
${o.body}
</main>
<footer>
  <div class="wrap">
    <div>
      <p>${esc(cfg.title)}.${lic}</p>
      <p>Every page is also static JSON, and registers WebMCP tools for in-browser agents.</p>
      <p><span id="webmcp-status">WebMCP tools load with JavaScript.</span></p>
    </div>
    <div class="links">
      <a href="${root}api/index.json">JSON API</a>
      <a href="${root}api/openapi.json">OpenAPI</a>
      <a href="${root}api/dataset.jsonld">JSON-LD</a>
      <a href="${root}llms.txt">llms.txt</a>
      <a href="${root}agents/">WebMCP tools</a>
      ${extra}
    </div>
  </div>
</footer>
</body>
</html>
`;
}

// ---------------------------------------------------------------- values

function valueHtml(S, v, field, root) {
  if (isObj(v) && v['@link']) {
    const t = S.model.byId.get(v['@link'].c);
    return `<a href="${root}${t.id}/${encodeURIComponent(v['@link'].slug)}/">${esc(S.model.label(v))}</a>`;
  }
  if (isObj(v) && v['@id']) {
    const iri = v['@id'];
    const text = field && field.name === 'type' ? localName(iri) : shortIri(iri);
    return /^https?:\/\//.test(iri) ? `<a href="${esc(iri)}" title="${esc(iri)}">${esc(text)}</a>` : `<code>${esc(iri)}</code>`;
  }
  if (isObj(v)) return esc(S.model.label(v));
  const s = String(v);
  if (/^https?:\/\/\S+$/.test(s)) return `<a href="${esc(s)}">${esc(shortIri(s))}</a>`;
  return esc(s).replace(/\n/g, '<br>');
}

function valuesHtml(S, vals, field, root) {
  if (vals.length === 1) return valueHtml(S, vals[0], field, root);
  const long = vals.some((v) => String(isObj(v) ? S.model.label(v) : v).length > 60);
  if (vals.length > 3 || long) return `<ul>${vals.map((v) => `<li>${valueHtml(S, v, field, root)}</li>`).join('')}</ul>`;
  return vals.map((v) => valueHtml(S, v, field, root)).join(', ');
}

// ---------------------------------------------------------------- pages

export function homePage(S, jsonld) {
  const { cfg } = S;
  const root = './';
  const cards = S.collections.map((c) => `
    <article class="card">
      <h3><a href="${c.id}/">${esc(c.title)}</a></h3>
      <span class="count">${c.items.length} ${c.items.length === 1 ? 'item' : 'items'}</span>
      <p>${esc(c.description)}</p>
    </article>`).join('');
  const opts = S.collections.map((c) => `<option value="${c.id}">${esc(c.title)}</option>`).join('');
  const body = `
<div class="hero">
  ${cfg.kicker ? `<p class="kicker">${esc(cfg.kicker)}</p>` : ''}
  <h1>${esc(cfg.title)}</h1>
  ${cfg.description ? `<p class="lede">${esc(cfg.description)}</p>` : ''}
</div>
${S.intro ? `<section class="prose">${S.intro}</section>` : ''}
${S.collections.length ? `<section id="find">
  <h2>Search</h2>
  <form id="search" class="controls" role="search">
    <label class="grow">Words <input type="search" name="q" autocomplete="off" placeholder="Search every item"></label>
    ${S.collections.length > 1 ? `<label>In <select name="collection"><option value="">everything</option>${opts}</select></label>` : ''}
    <button class="btn" type="submit">Search</button>
  </form>
  <ul class="results" id="search-results" aria-live="polite"></ul>
</section>
<section id="collections">
  <h2>Collections</h2>
  <div class="cards">${cards}
  </div>
</section>` : ''}
<section id="machines">
  <h2>For developers and agents</h2>
  <p class="sub">The same data, three ways besides these pages.</p>
  <div class="tablewrap"><table>
    <thead><tr><th>What</th><th>Where</th></tr></thead>
    <tbody>
      <tr><td class="name">Static JSON API</td><td><a href="api/index.json"><code>api/index.json</code></a>, described by <a href="api/openapi.json"><code>api/openapi.json</code></a></td></tr>
      <tr><td class="name">Linked data</td><td>JSON-LD in every page, and <a href="api/dataset.jsonld"><code>api/dataset.jsonld</code></a> for everything</td></tr>
      <tr><td class="name">WebMCP tools</td><td>${S.toolDocs.every.length} on every page, plus <code>show_items</code> on collection pages and <code>show_search</code> on this one: <a href="agents/">the list</a></td></tr>
      <tr><td class="name">For language models</td><td><a href="llms.txt"><code>llms.txt</code></a></td></tr>
      ${cfg.sparql && cfg.sparql.endpoint ? `<tr><td class="name">SPARQL</td><td><code>${esc(cfg.sparql.endpoint)}</code></td></tr>` : ''}
    </tbody>
  </table></div>
</section>`;
  return shell(S, {
    root, path: '', title: cfg.title, description: cfg.description, body, jsonld,
    alternate: 'api/index.json', context: { kind: 'home', title: cfg.title }, navCurrent: null,
  });
}

export function collectionPage(S, c, jsonld) {
  const root = '../';
  const fields = Object.fromEntries(c.fields.map((f) => [f.name, f]));
  const label = (n) => (fields[n] ? fields[n].label : n);
  const facetSel = c.facets.map((f) => `
    <label>${esc(label(f))} <select data-facet="${esc(f)}"><option value="">any</option>${c.facetValues[f].slice(0, 200).map((x) => `<option value="${esc(x.value)}">${esc(x.value)} (${x.count})</option>`).join('')}</select></label>`).join('');
  const sortOpts = [['', 'default'], ['title', 'name, A–Z'], ['-title', 'name, Z–A'],
    ...c.columns.flatMap((f) => [[f, `${label(f)}, ascending`], [`-${f}`, `${label(f)}, descending`]])]
    .map(([v, t]) => `<option value="${esc(v)}">${esc(t)}</option>`).join('');
  const shown = S.sorted(c).slice(0, LIST_LIMIT);
  const rows = shown.map((it) => `
      <tr><td class="name"><a href="${encodeURIComponent(it.slug)}/">${esc(it.label)}</a>${it.summary ? `<span class="sm">${esc(clip(it.summary, 160))}</span>` : ''}</td>${c.columns.map((f) => `<td>${(it.values[f] || []).map((v) => esc(S.model.facetLabel(v))).join(', ')}</td>`).join('')}</tr>`).join('');
  const body = `
<p class="crumbs"><a href="${root}">Home</a> /</p>
<div class="hero">
  <h1>${esc(c.title)}</h1>
  ${c.description ? `<p class="lede">${esc(c.description)}</p>` : ''}
  <p class="count">${c.items.length} ${c.items.length === 1 ? 'item' : 'items'}</p>
</div>
<form id="list-controls" class="controls" role="search">
  <label class="grow">Filter <input type="search" name="q" autocomplete="off" placeholder="Words in any field"></label>${facetSel}
  <label>Sort <select name="sort">${sortOpts}</select></label>
</form>
<div class="tablewrap"><table>
  <thead><tr><th>${esc(c.labelField ? label(c.labelField) : 'Name')}</th>${c.columns.map((f) => `<th>${esc(label(f))}</th>`).join('')}</tr></thead>
  <tbody id="list-body">${rows}
  </tbody>
</table></div>
<div class="listmeta"><span id="list-count">${shown.length} of ${c.items.length}</span><button id="list-more" class="btn ghost" type="button" hidden>Show more</button></div>
${c.items.length > LIST_LIMIT ? `<p class="note">The first ${LIST_LIMIT} are listed here; search or the API reach the rest.</p>` : ''}
<p class="note">This list as JSON: <a href="${root}api/${c.id}/index.json"><code>api/${c.id}/index.json</code></a></p>`;
  return shell(S, {
    root, path: `${c.id}/`, title: `${c.title} · ${S.cfg.title}`, description: c.description || `${c.title} in ${S.cfg.title}`,
    body, jsonld, alternate: `api/${c.id}/index.json`,
    context: { kind: 'collection', collection: c.id, title: c.title }, navCurrent: `${c.id}/`,
  });
}

function clip(s, n) {
  s = String(s || '');
  return s.length > n ? `${s.slice(0, n - 1)}…` : s;
}

export function itemPage(S, c, it, jsonld) {
  const root = '../../';
  const rows = c.fields
    .filter((f) => f.name !== c.labelField && f.name !== c.summaryField && it.values[f.name] && it.values[f.name].length)
    .map((f) => `
  <dt>${esc(f.label)}</dt>
  <dd>${valuesHtml(S, it.values[f.name], f, root)}</dd>`).join('');
  const groups = new Map();
  const seen = new Set();
  for (const b of it.backlinks) {
    const k = `${b.c}|${b.field}|${b.slug}`;
    if (seen.has(k)) continue;
    seen.add(k);
    const g = `${b.c}|${b.field}`;
    if (!groups.has(g)) groups.set(g, []);
    groups.get(g).push(b);
  }
  const back = [...groups.entries()].map(([g, list]) => {
    const [cid] = g.split('|');
    const src = S.model.byId.get(cid);
    return `
  <dt>${esc(list[0].fieldLabel)} of</dt>
  <dd>${list.length > 3 ? '<ul>' : ''}${list.slice(0, 200).map((b) => `${list.length > 3 ? '<li>' : ''}<a href="${root}${cid}/${encodeURIComponent(b.slug)}/">${esc(b.label)}</a>${list.length > 3 ? '</li>' : ''}`).join(list.length > 3 ? '' : ', ')}${list.length > 3 ? '</ul>' : ''}${list.length > 200 ? `<span class="sm">and ${list.length - 200} more in ${esc(src.title)}</span>` : ''}</dd>`;
  }).join('');
  const typeName = it.types.length ? localName(it.types[0]) : c.type ? (c.type.includes(':') ? localName(c.type) : c.type) : c.title;
  const body = `
<p class="crumbs"><a href="${root}">Home</a> / <a href="${root}${c.id}/">${esc(c.title)}</a> /</p>
<div class="hero">
  <p class="kicker">${esc(typeName)}</p>
  <h1>${esc(it.label)}</h1>
  ${it.summary ? `<p class="lede">${esc(it.summary)}</p>` : ''}
  ${it.iri && /^https?:\/\//.test(it.iri) ? `<p class="iri"><a href="${esc(it.iri)}">${esc(it.iri)}</a></p>` : it.iri ? `<p class="iri"><code>${esc(it.iri)}</code></p>` : ''}
</div>
${rows ? `<section><dl class="fields">${rows}
</dl></section>` : ''}
${back ? `<section><h2>Referenced by</h2><dl class="fields">${back}
</dl></section>` : ''}
<p class="note">This item as JSON: <a href="${root}api/${c.id}/${encodeURIComponent(it.slug)}.json"><code>api/${esc(c.id)}/${esc(it.slug)}.json</code></a></p>`;
  return shell(S, {
    root, path: `${c.id}/${it.slug}/`, title: `${it.label} · ${c.title} · ${S.cfg.title}`,
    description: it.summary ? clip(it.summary, 200) : `${it.label}, in ${c.title}.`,
    body, jsonld, alternate: `api/${c.id}/${it.slug}.json`,
    context: { kind: 'item', collection: c.id, id: it.slug, title: it.label }, navCurrent: `${c.id}/`,
  });
}

export function contentPage(S, p, html, jsonld) {
  const root = '../';
  const body = `
<p class="crumbs"><a href="${root}">Home</a> /</p>
<div class="hero"><h1>${esc(p.title)}</h1>${p.description ? `<p class="lede">${esc(p.description)}</p>` : ''}</div>
<section class="prose">
${html}
</section>`;
  return shell(S, {
    root, path: `${p.path}/`, title: `${p.title} · ${S.cfg.title}`, description: p.description || S.cfg.description,
    body, jsonld, context: { kind: 'page', path: `${p.path}/`, title: p.title }, navCurrent: `${p.path}/`,
  });
}

export function agentsPage(S, jsonld) {
  const root = '../';
  const { every, byKind, defs } = S.toolDocs;
  const kinds = { home: 'home page', collection: 'collection pages', item: 'item pages', page: 'content pages', agents: 'this page' };
  const toolHtml = Object.values(defs).map((d) => {
    const where = every.includes(d.name) ? 'every page' : Object.entries(byKind).filter(([, ns]) => ns.includes(d.name)).map(([k]) => kinds[k] || k).join(', ');
    const hints = [];
    if (d.annotations && d.annotations.readOnlyHint) hints.push('read-only');
    if (d.annotations && d.annotations.untrustedContentHint) hints.push('returns site data');
    if (!(d.annotations && d.annotations.readOnlyHint)) hints.push('changes the page');
    return `
<div class="tool" id="tool-${esc(d.name)}">
  <h3>${esc(d.name)}${hints.map((h) => `<span class="hint">${esc(h)}</span>`).join('')}</h3>
  <p>${esc(d.description)}</p>
  <p class="sm">On ${esc(where)}.</p>
  <details><summary>Input schema</summary><pre class="code"><code>${esc(JSON.stringify(d.inputSchema, null, 2))}</code></pre></details>
</div>`;
  }).join('');
  const first = S.collections[0];
  const sample = first && first.items[0] ? first.items[0].label.split(/\s+/)[0] : 'word';
  const body = `
<p class="crumbs"><a href="${root}">Home</a> /</p>
<div class="hero">
  <h1>Agents &amp; API</h1>
  <p class="lede">Every page of this site registers <a href="https://webmachinelearning.github.io/webmcp/">WebMCP</a> tools,
  so an agent in the browser can search, read and filter what you see, through the page itself.
  The same data is static JSON for any program.</p>
</div>
<section>
  <h2>Try the tools</h2>
  <ol>
    <li>In Chrome, enable <code>chrome://flags/#enable-webmcp-testing</code> and relaunch.</li>
    <li>Install the Model Context Tool Inspector extension and open any page of this site: it lists that page's tools and runs them.</li>
    <li>Without either, the tools still run from the console:
      <code>await pageTools.call('search', { query: ${esc(JSON.stringify(sample))} })</code>.</li>
  </ol>
  <div class="callout"><p>WebMCP is a W3C Community Group draft. The tools here follow the draft of 24 November 2026
  (<code>document.modelContext.registerTool</code>) and fall back to older browser previews.</p></div>
</section>
<section>
  <h2>The tools</h2>
  <p class="sub">${every.length} on every page; collection pages add <code>show_items</code>, the home page <code>show_search</code>.
  Machine-readable: <a href="${root}api/tools.json"><code>api/tools.json</code></a>.</p>
  ${toolHtml}
</section>
<section>
  <h2>The static API</h2>
  <p class="sub">Plain files, CORS-open, described in <a href="${root}api/openapi.json"><code>api/openapi.json</code></a>.</p>
  <div class="tablewrap"><table>
    <thead><tr><th>Path</th><th>What it holds</th></tr></thead>
    <tbody>
      <tr><td><code>api/index.json</code></td><td>The site: collections, their fields, facets and counts, pages, every endpoint</td></tr>
      <tr><td><code>api/search.json</code></td><td>The search index: collection, id, title and text of every item</td></tr>
      ${S.collections.map((c) => `<tr><td><code>api/${c.id}/index.json</code></td><td>${esc(c.title)}: every item with its list fields, and the facets</td></tr>
      <tr><td><code>api/${c.id}/{id}.json</code></td><td>One item of ${esc(c.title)}: every field, links, back-links and JSON-LD</td></tr>`).join('\n      ')}
      <tr><td><code>api/dataset.jsonld</code></td><td>Everything as one JSON-LD graph (schema.org)</td></tr>
      <tr><td><code>api/tools.json</code></td><td>The WebMCP tools of each page kind with their input schemas</td></tr>
      <tr><td><code>llms.txt</code></td><td>A short map of the site for language models</td></tr>
    </tbody>
  </table></div>
</section>`;
  return shell(S, {
    root, path: 'agents/', title: `Agents & API · ${S.cfg.title}`, description: `WebMCP tools and the static JSON API of ${S.cfg.title}.`,
    body, jsonld, alternate: 'api/tools.json', context: { kind: 'agents', title: 'Agents & API' }, navCurrent: 'agents/',
  });
}

export function notFoundPage(S) {
  let root = '/';
  if (S.cfg.baseUrl) root = new URL(S.cfg.baseUrl).pathname.replace(/\/?$/, '/');
  const body = `
<div class="hero"><h1>Not here</h1><p class="lede">Nothing lives at this address. Try the <a href="${root}">home page</a> or its search.</p></div>`;
  return shell(S, { root, title: `Not found · ${S.cfg.title}`, body, context: { kind: 'notfound', title: 'Not found' } });
}
