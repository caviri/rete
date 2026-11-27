#!/usr/bin/env node
// semantic-site build: site.json + data -> a static site where every page is
// HTML content, JSON-LD, a static JSON API entry and a set of WebMCP tools.
//
//   node build.mjs <site.json> [--out <dir>] [--force]
//
// Node 20 or newer, no dependencies. The output is plain files for any static host.

import { readFileSync, writeFileSync, mkdirSync, existsSync, readdirSync, rmSync, cpSync, statSync } from 'node:fs';
import { dirname, resolve, join, relative, isAbsolute } from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';
import { createHash } from 'node:crypto';

import { buildModel, RESERVED, slugify } from './lib/model.mjs';
import { markdown } from './lib/markdown.mjs';
import * as R from './lib/render.mjs';
import * as E from './lib/emit.mjs';

const HERE = dirname(fileURLToPath(import.meta.url));
export const RUNTIME = resolve(HERE, '../../../runtime');
export const RUNTIME_FILES = ['boot.js', 'site.js', 'site-tools.js', 'webmcp-kit.js', 'query.js', 'site.css'];
const GENERATOR_VERSION = JSON.parse(readFileSync(resolve(HERE, '../../../.claude-plugin/plugin.json'), 'utf8')).version;
const MARKER = '.semantic-site.json';
const runtimeModule = (f) => pathToFileURL(join(RUNTIME, f)).href;

function fail(msg) {
  console.error(`semantic-site: ${msg}`);
  process.exit(1);
}

function readConfig(path) {
  let cfg;
  try { cfg = JSON.parse(readFileSync(path, 'utf8')); } catch (e) { throw new Error(`${path}: ${e.message}`); }
  if (!cfg.title) throw new Error(`${path}: "title" is required.`);
  if (cfg.baseUrl) {
    try { new URL(cfg.baseUrl); } catch (_) { throw new Error(`baseUrl "${cfg.baseUrl}" is not an absolute URL.`); }
    if (!cfg.baseUrl.endsWith('/')) cfg.baseUrl += '/';
  }
  if (typeof cfg.license === 'string') cfg.license = { name: cfg.license };
  cfg.pages = (cfg.pages || []).map((p) => {
    const path = slugify(p.path || p.title || '').toLowerCase();
    if (!path) throw new Error(`A page needs a path or title: ${JSON.stringify(p)}`);
    if (RESERVED.has(path)) throw new Error(`Page path "${path}" is reserved.`);
    return { ...p, path, title: p.title || path };
  });
  return cfg;
}

function readContent(baseDir, src) {
  const p = resolve(baseDir, src);
  const text = readFileSync(p, 'utf8');
  return /\.html?$/i.test(p) ? text : markdown(text);
}

// The page kinds' tool sets, documented on the agents page and in api/tools.json.
async function toolDocs(index) {
  const { siteTools } = await import(runtimeModule('site-tools.js'));
  const { describeTool } = await import(runtimeModule('webmcp-kit.js'));
  const first = index.collections[0];
  const env = (page, view) => ({
    site: index, page, view, url: (r) => r, location: '', navigate() {}, fetchJSON: async () => ({}),
  });
  const kinds = {
    home: env({ kind: 'home', title: index.site.title }, { search() {}, state() {} }),
    ...(first ? {
      collection: env({ kind: 'collection', collection: first.id, title: first.title }, { apply() {}, state() {} }),
      item: env({ kind: 'item', collection: first.id, id: 'x', title: 'x' }),
    } : {}),
    page: env({ kind: 'page', title: 'page' }),
    agents: env({ kind: 'agents', title: 'agents' }),
  };
  const defs = {};
  const byKind = {};
  for (const [k, e] of Object.entries(kinds)) {
    const ts = siteTools(e);
    byKind[k] = ts.map((t) => t.name);
    for (const t of ts) if (!defs[t.name]) defs[t.name] = describeTool(t);
  }
  const every = Object.values(byKind).reduce((a, b) => a.filter((n) => b.includes(n)));
  return { every, byKind, defs };
}

export async function build(configPath, { out, force = false, quiet = false } = {}) {
  const cfgPath = resolve(configPath);
  const baseDir = dirname(cfgPath);
  const cfg = readConfig(cfgPath);
  const outDir = resolve(out || join(baseDir, 'dist'));
  const inside = relative(outDir, cfgPath);
  if (!inside.startsWith('..') && !isAbsolute(inside)) {
    throw new Error(`The output directory ${outDir} contains the site sources; choose another --out.`);
  }

  // Refuse to wipe a directory this tool did not write.
  if (existsSync(outDir)) {
    const entries = readdirSync(outDir);
    if (entries.length && !entries.includes(MARKER) && !force) {
      throw new Error(`${outDir} is not empty and was not written by semantic-site. Choose another --out, or pass --force to replace it.`);
    }
    rmSync(outDir, { recursive: true, force: true });
  }

  const model = buildModel(cfg, baseDir);
  const S = {
    cfg,
    model,
    lang: model.lang,
    collections: model.collections,
    slug: slugify(cfg.title).toLowerCase() || 'site',
    generatorVersion: GENERATOR_VERSION,
  };

  // Default order of each collection, shared by pages, lists and search.
  const { sortItems } = await import(runtimeModule('query.js'));
  const sortedCache = new Map();
  const slimCache = new Map();
  S.slimSorted = (c) => {
    if (!slimCache.has(c)) {
      const slim = c.items.map((it) => E.slimItem(S, c, it));
      const bySlug = new Map(c.items.map((it) => [it.slug, it]));
      const sorted = sortItems(slim, c.sort);
      slimCache.set(c, sorted);
      sortedCache.set(c, sorted.map((s) => bySlug.get(s.id)));
    }
    return slimCache.get(c);
  };
  S.sorted = (c) => { S.slimSorted(c); return sortedCache.get(c); };

  const index = E.siteIndex(S);
  const runtimeText = RUNTIME_FILES.map((f) => readFileSync(join(RUNTIME, f), 'utf8')).join('\n');
  S.version = createHash('sha256').update(runtimeText).update(JSON.stringify(index)).update(String(model.collections.reduce((n, c) => n + c.items.length, 0))).digest('hex').slice(0, 10);
  index.site.version = S.version;
  S.toolDocs = await toolDocs(index);
  S.intro = cfg.intro ? readContent(baseDir, cfg.intro) : '';

  let files = 0;
  let bytes = 0;
  const write = (rel, content) => {
    const p = join(outDir, rel);
    mkdirSync(dirname(p), { recursive: true });
    const data = typeof content === 'string' ? content : JSON.stringify(content);
    writeFileSync(p, data);
    files++;
    bytes += Buffer.byteLength(data);
  };
  const pages = [];

  // Pages.
  const relHome = E.relFrom(S, '');
  write('index.html', R.homePage(S, [{ '@context': E.CONTEXT, '@graph': E.siteNodes(S, relHome) }]));
  pages.push('');
  for (const c of S.collections) {
    write(`${c.id}/index.html`, R.collectionPage(S, c, [E.collectionNode(S, c, E.relFrom(S, `${c.id}/`))]));
    pages.push(`${c.id}/`);
    for (const it of S.sorted(c)) {
      const rel = E.relFrom(S, `${c.id}/${it.slug}/`);
      write(`${c.id}/${it.slug}/index.html`, R.itemPage(S, c, it, [{ '@context': E.CONTEXT, ...E.itemNode(S, c, it, rel) }]));
      pages.push(`${c.id}/${it.slug}/`);
    }
  }
  for (const p of cfg.pages) {
    const html = p.source ? readContent(baseDir, p.source) : (p.html || '');
    const rel = E.relFrom(S, `${p.path}/`);
    write(`${p.path}/index.html`, R.contentPage(S, p, html, [{ '@context': E.CONTEXT, '@type': 'WebPage', name: p.title, url: rel(`${p.path}/`), isPartOf: { '@id': rel('') } }]));
    pages.push(`${p.path}/`);
  }
  {
    const rel = E.relFrom(S, 'agents/');
    write('agents/index.html', R.agentsPage(S, [{ '@context': E.CONTEXT, '@type': 'WebPage', name: 'Agents & API', url: rel('agents/'), isPartOf: { '@id': rel('') } }]));
    pages.push('agents/');
  }
  write('404.html', R.notFoundPage(S));

  // API.
  write('api/index.json', index);
  write('api/search.json', E.searchIndex(S));
  write('api/tools.json', {
    spec: { name: 'WebMCP', draft: '2026-11-24', url: 'https://webmachinelearning.github.io/webmcp/' },
    every_page: S.toolDocs.every,
    pages: S.toolDocs.byKind,
    tools: S.toolDocs.defs,
  });
  write('api/openapi.json', E.openapi(S));
  write('api/dataset.jsonld', E.datasetJsonLd(S));
  for (const c of S.collections) {
    write(`api/${c.id}/index.json`, E.collectionList(S, c));
    for (const it of c.items) write(`api/${c.id}/${it.slug}.json`, E.itemRecord(S, c, it));
  }

  // Discovery and hosting files.
  write('llms.txt', E.llmsTxt(S));
  write('robots.txt', E.robots(S));
  const sm = E.sitemap(S, pages);
  if (sm) write('sitemap.xml', sm);
  write('_headers', E.HEADERS);
  write('.nojekyll', '');

  // Runtime and the site's own static files.
  for (const f of RUNTIME_FILES) write(`assets/${f}`, readFileSync(join(RUNTIME, f), 'utf8'));
  if (cfg.static) {
    const src = resolve(baseDir, cfg.static);
    if (!existsSync(src) || !statSync(src).isDirectory()) throw new Error(`static: ${src} is not a directory.`);
    cpSync(src, outDir, { recursive: true, force: false, errorOnExist: false });
  }
  for (const s of (cfg.webmcp && cfg.webmcp.scripts) || []) {
    if (!existsSync(join(outDir, s))) model.warnings.push(`webmcp.scripts: ${s} is not in the output (put it in the "static" directory).`);
  }
  write(MARKER, { generator: `semantic-site ${GENERATOR_VERSION}`, version: S.version, config: relative(outDir, cfgPath).replace(/\\/g, '/') });

  const summary = {
    out: outDir,
    version: S.version,
    collections: S.collections.map((c) => ({ id: c.id, items: c.items.length, facets: c.facets, columns: c.columns })),
    pages: pages.length,
    files,
    bytes,
    tools: { every_page: S.toolDocs.every.length, by_kind: Object.fromEntries(Object.entries(S.toolDocs.byKind).map(([k, v]) => [k, v.length])) },
    warnings: model.warnings,
  };
  if (!quiet) {
    console.log(`Built ${outDir}`);
    for (const c of summary.collections) console.log(`  ${c.id}: ${c.items} items · facets ${c.facets.join(', ') || 'none'} · columns ${c.columns.join(', ') || 'none'}`);
    console.log(`  ${summary.pages} pages, ${files} files, ${(bytes / 1024).toFixed(0)} KiB`);
    console.log(`  WebMCP: ${summary.tools.every_page} tools on every page; ${Object.entries(summary.tools.by_kind).map(([k, n]) => `${k} ${n}`).join(', ')}`);
    for (const w of model.warnings) console.log(`  warning: ${w}`);
  }
  return summary;
}

function parseArgs(argv) {
  const a = { _: [] };
  for (let i = 0; i < argv.length; i++) {
    const x = argv[i];
    if (x === '--out' || x === '-o') a.out = argv[++i];
    else if (x === '--force') a.force = true;
    else if (x === '--json') a.json = true;
    else if (x === '-h' || x === '--help') a.help = true;
    else a._.push(x);
  }
  return a;
}

if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  const a = parseArgs(process.argv.slice(2));
  if (a.help || !a._[0]) {
    console.log('usage: node build.mjs <site.json> [--out <dir>] [--force] [--json]');
    process.exit(a.help ? 0 : 1);
  }
  try {
    const s = await build(a._[0], { out: a.out, force: a.force, quiet: a.json });
    if (a.json) console.log(JSON.stringify(s, null, 2));
  } catch (e) {
    fail(e.message);
  }
}

