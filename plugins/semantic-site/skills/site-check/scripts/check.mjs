#!/usr/bin/env node
// site-check: verify a static site is content + linked data + static API +
// WebMCP on every page.
//
//   node check.mjs <dir> [--json]
//
// Works on any static site; the API and tool-run checks apply when the site
// has api/index.json and the semantic-site runtime in assets/. Exit code 1
// when there is an error. Node 20 or newer, no dependencies.

import { readFileSync, readdirSync, statSync, existsSync, mkdtempSync, writeFileSync, rmSync } from 'node:fs';
import { join, resolve, dirname, relative, extname, sep } from 'node:path';
import { tmpdir } from 'node:os';
import { pathToFileURL, fileURLToPath } from 'node:url';

const RESULT_BUDGET = 18000;

function walk(dir, out = []) {
  for (const e of readdirSync(dir, { withFileTypes: true })) {
    const p = join(dir, e.name);
    if (e.isDirectory()) walk(p, out);
    else out.push(p);
  }
  return out;
}

const attr = (tag, name) => {
  const m = new RegExp(`\\s${name}\\s*=\\s*("([^"]*)"|'([^']*)'|([^\\s>]+))`, 'i').exec(tag);
  return m ? (m[2] ?? m[3] ?? m[4]) : null;
};
const decode = (s) => s.replace(/&amp;/g, '&').replace(/&quot;/g, '"').replace(/&#39;/g, "'").replace(/&lt;/g, '<').replace(/&gt;/g, '>');

export async function check(dirArg) {
  const root = resolve(dirArg);
  if (!existsSync(root) || !statSync(root).isDirectory()) throw new Error(`${root} is not a directory`);
  const issues = [];
  const notes = [];
  const err = (where, message) => issues.push({ level: 'error', where, message });
  const warn = (where, message) => issues.push({ level: 'warning', where, message });
  const rel = (p) => relative(root, p).split(sep).join('/');
  const files = walk(root);
  const html = files.filter((f) => /\.html?$/i.test(f));

  // A local URL -> the file it serves, or null when nothing would answer.
  function target(fromFile, url) {
    const clean = decode(url).split('#')[0].split('?')[0];
    if (!clean) return fromFile;
    let p;
    try { p = clean.startsWith('/') ? join(root, decodeURIComponent(clean)) : resolve(dirname(fromFile), decodeURIComponent(clean)); } catch (_) { return null; }
    if (p !== root && !p.startsWith(root + sep)) return null;
    if (existsSync(p) && statSync(p).isDirectory()) p = join(p, 'index.html');
    else if (!existsSync(p) && !extname(p) && existsSync(`${p}.html`)) p = `${p}.html`;
    return existsSync(p) ? p : null;
  }

  // Does a script (or what it imports) talk to WebMCP?
  const scriptCache = new Map();
  function scriptRegisters(file, depth = 0) {
    if (scriptCache.has(file)) return scriptCache.get(file);
    scriptCache.set(file, false);
    let text = '';
    try { text = readFileSync(file, 'utf8'); } catch (_) { return false; }
    let yes = /modelContext|registerTool/.test(text);
    if (!yes && depth < 4) {
      for (const m of text.matchAll(/(?:import\s[^'"]*?from\s*|import\s*\(\s*|import\s*)['"](\.{1,2}\/[^'"]+)['"]/g)) {
        const t = resolve(dirname(file), m[1]);
        if (existsSync(t) && scriptRegisters(t, depth + 1)) { yes = true; break; }
      }
    }
    scriptCache.set(file, yes);
    return yes;
  }

  // ---------------------------------------------------------------- pages
  let withWebmcp = 0;
  let jsonldBlocks = 0;
  let links = 0;
  const contexts = [];
  for (const f of html) {
    const where = rel(f);
    const text = readFileSync(f, 'utf8');
    if (!/<html[^>]*\slang\s*=/i.test(text)) warn(where, 'no lang on <html>');
    const title = /<title>([^<]*)<\/title>/i.exec(text);
    if (!title || !title[1].trim()) err(where, 'no <title>');
    if (!/<meta[^>]+name\s*=\s*["']description["']/i.test(text)) warn(where, 'no meta description');
    if (!/<meta[^>]+name\s*=\s*["']viewport["']/i.test(text)) warn(where, 'no meta viewport');

    for (const m of text.matchAll(/<script[^>]*type\s*=\s*["']application\/ld\+json["'][^>]*>([\s\S]*?)<\/script>/gi)) {
      jsonldBlocks++;
      try {
        const j = JSON.parse(m[1]);
        const nodes = Array.isArray(j) ? j : [j];
        if (!nodes.every((n) => n && n['@context'])) err(where, 'JSON-LD without @context');
      } catch (e) {
        err(where, `JSON-LD does not parse: ${e.message}`);
      }
    }
    const pc = /<script[^>]*id\s*=\s*["']page-context["'][^>]*>([\s\S]*?)<\/script>/i.exec(text);
    if (pc) {
      try { contexts.push({ file: f, ctx: JSON.parse(pc[1]) }); } catch (e) { err(where, `page-context does not parse: ${e.message}`); }
    }

    let registers = false;
    for (const m of text.matchAll(/<script\b([^>]*)>([\s\S]*?)<\/script>/gi)) {
      const src = attr(m[1], 'src');
      if (src && !/^[a-z]+:|^\/\//i.test(src)) {
        const t = target(f, src);
        if (t && scriptRegisters(t)) registers = true;
      } else if (!src && /modelContext|registerTool/.test(m[2])) registers = true;
    }
    if (registers) withWebmcp++;
    else if (!/^404\.html?$/i.test(where)) err(where, 'no WebMCP: the page registers no tools (load a script that calls document.modelContext.registerTool)');

    for (const m of text.matchAll(/<(a|link|script|img|source|iframe)\b[^>]*>/gi)) {
      const url = attr(m[0], m[1].toLowerCase() === 'a' || m[1].toLowerCase() === 'link' ? 'href' : 'src');
      if (!url || /^(#|[a-z][a-z0-9+.-]*:|\/\/)/i.test(url)) continue;
      if (/^404\.html?$/i.test(where) && url.startsWith('/')) continue; // a 404 page is served at any depth
      links++;
      if (!target(f, url)) err(where, `broken link: ${url}`);
    }
  }
  notes.push(`${html.length} pages: ${withWebmcp} register WebMCP tools, ${jsonldBlocks} JSON-LD blocks, ${links} local links checked`);

  // ---------------------------------------------------------------- static API
  const indexFile = join(root, 'api', 'index.json');
  let index = null;
  const readJSON = (p) => JSON.parse(readFileSync(p, 'utf8'));
  if (!existsSync(indexFile)) {
    warn('api/index.json', 'no static API: a semantic site describes itself at api/index.json');
  } else {
    for (const f of files.filter((x) => rel(x).startsWith('api/') && /\.(json|jsonld)$/.test(x))) {
      try { readJSON(f); } catch (e) { err(rel(f), `does not parse: ${e.message}`); }
    }
    try { index = readJSON(indexFile); } catch (_) { index = null; }
  }
  let itemCount = 0;
  if (index) {
    if (!index.site || !index.site.title) err('api/index.json', 'no site.title');
    if (!Array.isArray(index.collections)) err('api/index.json', 'no collections array');
    for (const k of ['search', 'tools', 'openapi', 'jsonld']) {
      if (!index.api || !index.api[k]) warn('api/index.json', `api.${k} is not declared`);
      else if (!existsSync(join(root, index.api[k]))) err('api/index.json', `api.${k} points to a missing file: ${index.api[k]}`);
    }
    const known = new Set();
    for (const c of index.collections || []) {
      const lf = join(root, c.list || '');
      if (!c.list || !existsSync(lf)) { err('api/index.json', `collection ${c.id}: list file missing`); continue; }
      let list;
      try { list = readJSON(lf); } catch (_) { continue; }
      if (list.count !== list.items.length) err(c.list, `count ${list.count} but ${list.items.length} items`);
      if (c.count !== list.count) err('api/index.json', `collection ${c.id}: count ${c.count}, list says ${list.count}`);
      if (!existsSync(join(root, c.page, 'index.html'))) err('api/index.json', `collection ${c.id}: page ${c.page} missing`);
      for (const it of list.items) {
        itemCount++;
        known.add(`${c.id}|${it.id}`);
        const rf = join(root, c.item.replace('{id}', it.id));
        if (!existsSync(rf)) { err(c.list, `item ${it.id}: ${c.item.replace('{id}', it.id)} missing`); continue; }
        if (!existsSync(join(root, it.url, 'index.html'))) err(c.list, `item ${it.id}: page ${it.url} missing`);
        for (const k of Object.keys(it.f || {})) if (!(c.filterable || []).includes(k)) warn(c.list, `item ${it.id}: field ${k} is not declared filterable`);
      }
      for (const f of c.facets || []) if (!list.facets || !list.facets[f]) err(c.list, `facet ${f} has no values`);
    }
    if (index.api && index.api.search && existsSync(join(root, index.api.search))) {
      try {
        const s = readJSON(join(root, index.api.search));
        for (const r of s.rows || []) if (!known.has(`${r[0]}|${r[1]}`)) err(index.api.search, `row for unknown item ${r[0]}/${r[1]}`);
      } catch (_) { /* reported above */ }
    }
    if (index.api && index.api.openapi && existsSync(join(root, index.api.openapi))) {
      try { if (!String(readJSON(join(root, index.api.openapi)).openapi || '').startsWith('3.')) err(index.api.openapi, 'not an OpenAPI 3 document'); } catch (_) { /* reported above */ }
    }
    if (!existsSync(join(root, 'llms.txt'))) warn('llms.txt', 'missing');
    notes.push(`API: ${(index.collections || []).length} collections, ${itemCount} items, every list, item and page file present`);
  }

  // ---------------------------------------------------------------- WebMCP tools, run
  const assets = join(root, 'assets');
  const runtime = ['webmcp-kit.js', 'site-tools.js', 'query.js'];
  if (index && runtime.every((f) => existsSync(join(assets, f)))) {
    const tmp = mkdtempSync(join(tmpdir(), 'site-check-'));
    try {
      for (const f of runtime) {
        const src = readFileSync(join(assets, f), 'utf8').replace(/from\s+'\.\/([\w-]+)\.js'/g, "from './$1.mjs'");
        writeFileSync(join(tmp, f.replace(/\.js$/, '.mjs')), src);
      }
      const kit = await import(pathToFileURL(join(tmp, 'webmcp-kit.mjs')).href);
      const { siteTools } = await import(pathToFileURL(join(tmp, 'site-tools.mjs')).href);
      const base = index.site.baseUrl || 'https://site.test/';
      const jsonCache = new Map();
      const fetchJSON = async (r) => {
        const p = join(root, decodeURIComponent(r.split('?')[0]));
        if (!jsonCache.has(p)) {
          if (!existsSync(p)) throw new Error(`${r}: HTTP 404`);
          jsonCache.set(p, readJSON(p));
        }
        return jsonCache.get(p);
      };
      const firstList = index.collections[0] ? await fetchJSON(index.collections[0].list) : null;
      const sampleWord = firstList && firstList.items[0] ? String(firstList.items[0].title).split(/\s+/)[0] : 'a';

      // One page of each kind.
      const picked = new Map();
      for (const { file, ctx } of contexts) {
        const key = ctx.kind === 'item' ? `item:${ctx.collection}` : ctx.kind;
        if (!picked.has(key)) picked.set(key, { file, ctx });
      }
      const perKind = {};
      for (const [key, { file, ctx }] of picked) {
        const where = rel(file);
        const calls = { navigate: [], apply: [], search: [] };
        const view = ctx.kind === 'collection'
          ? { apply: async (a) => { calls.apply.push(a); return { matched: 0, shown: 0 }; }, state: () => ({ query: '', where: {}, matched: 0 }) }
          : ctx.kind === 'home' ? { search: async (a) => { calls.search.push(a); return { matched: 0, shown: 0 }; }, state: () => ({}) } : null;
        const env = {
          site: index, page: ctx, fetchJSON, view,
          url: (r) => new URL(r, base).href,
          location: new URL(rel(dirname(file)) ? `${rel(dirname(file))}/` : '', base).href,
          navigate: (h) => calls.navigate.push(h),
          fetch: async () => { throw new Error('SPARQL is not called by the checker'); },
        };
        const registered = [];
        const ctxApi = {
          async registerTool(tool, opts) {
            if (!kit.NAME_RE.test(tool.name)) throw new Error(`bad name ${tool.name}`);
            if (!opts || !opts.signal) throw new Error('registered without an AbortSignal');
            registered.push(tool);
          },
        };
        let pageTools;
        try {
          pageTools = await kit.registerTools(siteTools(env), { label: 'check', ctx: ctxApi });
        } catch (e) {
          err(where, `tools failed to register: ${e.message}`);
          continue;
        }
        if (registered.length !== pageTools.tools.length) err(where, `${pageTools.tools.length} tools defined, ${registered.length} registered`);
        perKind[key] = registered.map((t) => t.name);
        for (const t of registered) {
          if (!t.description || t.description.length < 20) warn(where, `${t.name}: the description is too short to tell an agent when to use it`);
          if (!t.inputSchema || t.inputSchema.type !== 'object') err(where, `${t.name}: inputSchema must be an object schema`);
          const input = sampleInput(t, ctx, index, firstList, sampleWord);
          if (t.name === 'run_sparql') continue;
          try {
            const out = await t.execute(input, { signal: new AbortController().signal });
            const size = JSON.stringify(out).length;
            if (size > RESULT_BUDGET + 2000) err(where, `${t.name}: result is ${size} characters, over the ${RESULT_BUDGET} budget`);
          } catch (e) {
            err(where, `${t.name}(${JSON.stringify(input)}) threw: ${e.message}`);
          }
        }
        if (perKind[key].includes('open_page') && !calls.navigate.length) err(where, 'open_page did not navigate');
        if (perKind[key].includes('show_items') && !calls.apply.length) err(where, 'show_items did not reach the page');
        if (perKind[key].includes('show_search') && !calls.search.length) err(where, 'show_search did not reach the page');
        // Errors must be helpful, not crashes.
        const gi = registered.find((t) => t.name === 'get_item');
        if (gi) {
          try {
            await gi.execute({ collection: index.collections[0].id, id: '__missing__' }, {});
            err(where, 'get_item accepted an unknown id');
          } catch (e) {
            if (!/search|list_items/.test(e.message)) warn(where, `get_item's error does not say what to call instead: ${e.message}`);
          }
        }
      }
      const kinds = Object.entries(perKind).map(([k, v]) => `${k} ${v.length}`).join(', ');
      notes.push(`WebMCP tools registered and run on each page kind: ${kinds}`);
      const missing = html.length - withWebmcp - html.filter((f) => /^404\.html?$/i.test(rel(f))).length;
      if (!missing) notes.push('WebMCP on every page');
    } finally {
      rmSync(tmp, { recursive: true, force: true });
    }
  } else if (index) {
    notes.push('WebMCP tools not run: assets/ does not hold the semantic-site runtime');
  }

  return { root, ok: !issues.some((i) => i.level === 'error'), notes, issues };
}

function sampleInput(tool, ctx, index, firstList, word) {
  const props = (tool.inputSchema && tool.inputSchema.properties) || {};
  const req = (tool.inputSchema && tool.inputSchema.required) || [];
  const col = ctx.collection || (index.collections[0] && index.collections[0].id);
  const input = {};
  if (tool.name === 'open_page') return { target: 'home' };
  if (tool.name === 'show_items') return { query: word };
  if (tool.name === 'show_search') return { query: word };
  for (const k of req) {
    if (k === 'collection') input.collection = col;
    else if (k === 'id') input.id = ctx.kind === 'item' ? ctx.id : firstList.items[0].id;
    else if (k === 'query') input.query = word;
    else if (props[k] && props[k].enum) input[k] = props[k].enum[0];
  }
  if (tool.name === 'get_item' && input.collection !== (index.collections[0] && index.collections[0].id) && ctx.kind !== 'item') {
    input.collection = index.collections[0].id;
  }
  return input;
}

if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  const args = process.argv.slice(2);
  const json = args.includes('--json');
  const dir = args.find((a) => !a.startsWith('--'));
  if (!dir) {
    console.log('usage: node check.mjs <site-dir> [--json]');
    process.exit(1);
  }
  try {
    const r = await check(dir);
    if (json) console.log(JSON.stringify(r, null, 2));
    else {
      for (const n of r.notes) console.log(`  ok  ${n}`);
      for (const i of r.issues) console.log(`  ${i.level === 'error' ? 'ERR ' : 'warn'} ${i.where}: ${i.message}`);
      const e = r.issues.filter((i) => i.level === 'error').length;
      const w = r.issues.length - e;
      console.log(r.ok ? `site-check: passed${w ? ` with ${w} warnings` : ''}` : `site-check: ${e} errors, ${w} warnings`);
    }
    process.exit(r.ok ? 0 : 1);
  } catch (e) {
    console.error(`site-check: ${e.message}`);
    process.exit(1);
  }
}
