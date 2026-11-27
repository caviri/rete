// Entry point of every generated page: wire the page UI, then register the
// site's WebMCP tools (plus any the site adds through webmcp.scripts).

import { registerTools } from './webmcp-kit.js';
import { siteTools } from './site-tools.js';
import { initView, showStatus } from './site.js';

const meta = (n) => {
  const m = document.querySelector(`meta[name="${n}"]`);
  return m ? m.content : null;
};

const root = new URL(meta('site-root') || './', location.href);
const ctxEl = document.getElementById('page-context');
const page = ctxEl ? JSON.parse(ctxEl.textContent) : { kind: 'page', title: document.title };

const cache = new Map();
function fetchJSON(rel) {
  const u = new URL(rel, root).href;
  if (!cache.has(u)) {
    cache.set(u, fetch(u).then((r) => {
      if (!r.ok) throw new Error(`${rel}: HTTP ${r.status}`);
      return r.json();
    }));
  }
  return cache.get(u);
}

async function main() {
  const site = await fetchJSON('api/index.json');
  const env = {
    site,
    page,
    root,
    fetchJSON,
    url: (rel) => new URL(rel, root).href,
    get location() { return location.href; }, // show_items rewrites the query string
    navigate: (href) => setTimeout(() => location.assign(href), 50),
    themeKey: meta('theme-key') || 'site-theme',
  };
  env.view = initView(env);

  const tools = siteTools(env);
  for (const src of (site.webmcp && site.webmcp.scripts) || []) {
    try {
      const mod = await import(new URL(src, root).href);
      const extra = typeof mod.default === 'function' ? await mod.default(env) : [];
      tools.push(...(extra || []));
    } catch (e) {
      console.warn('[site webmcp] could not load', src, e && e.message);
    }
  }
  const pageTools = await registerTools(tools, { label: 'site webmcp' });
  showStatus(pageTools);
}

main().catch((e) => console.warn('[site webmcp]', e && e.message));
