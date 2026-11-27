// webmcp-kit — register a page's tools with an in-browser agent (WebMCP).
//
// Spec: WebMCP, W3C Web Machine Learning Community Group draft, 24 November 2026
// (https://webmachinelearning.github.io/webmcp/):
//
//   await document.modelContext.registerTool(tool, { signal })
//   tool = { name, title?, description, inputSchema?, outputSchema?,
//            execute(input, { signal }) -> Promise<any>, annotations? }
//   annotations = { readOnlyHint, untrustedContentHint, consequentialHint }
//   Unregister by aborting the signal. Secure contexts only (https or localhost).
//
// Older Chrome previews exposed navigator.modelContext and provideContext({ tools });
// the kit tries those after the spec route. Without any of them the page loses
// nothing: the tools stay callable from the page itself through
// window.pageTools.call(name, input), which is also how the checker tests them.
//
// No dependencies, no build step. Import it as an ES module:
//
//   import { registerTools } from './webmcp-kit.js';
//   await registerTools([{ name: 'get_status', description: '…', execute: async () => ({}) }]);

export const NAME_RE = /^[A-Za-z0-9_.-]{1,128}$/;
export const RESULT_BUDGET = 18000; // characters of JSON per result; agents pay for every one

export function modelContext() {
  try {
    if (typeof document !== 'undefined' && document.modelContext) return document.modelContext;
  } catch (_) { /* not exposed */ }
  try {
    if (typeof navigator !== 'undefined' && navigator.modelContext) return navigator.modelContext;
  } catch (_) { /* not exposed */ }
  return null;
}

export function clip(s, n) {
  s = s == null ? '' : String(s);
  return s.length > n ? s.slice(0, n - 1) + '…' : s;
}

export const jsonSize = (v) => JSON.stringify(v).length;

// Drop trailing items until wrap(items) fits the budget. Returns { items, truncated }.
export function fitList(items, wrap, budget = RESULT_BUDGET) {
  let n = items.length;
  while (n > 0 && jsonSize(wrap(items.slice(0, n))) > budget) {
    n = n < 5 ? n - 1 : Math.floor(n * 0.8);
  }
  return { items: items.slice(0, n), truncated: n < items.length };
}

const TYPES = {
  string: (v) => typeof v === 'string',
  integer: (v) => Number.isInteger(v),
  number: (v) => typeof v === 'number' && Number.isFinite(v),
  boolean: (v) => typeof v === 'boolean',
  array: (v) => Array.isArray(v),
  object: (v) => v !== null && typeof v === 'object' && !Array.isArray(v),
};

function typeOk(schema, v) {
  if (!schema || !schema.type) return true;
  const types = Array.isArray(schema.type) ? schema.type : [schema.type];
  return types.some((t) => (TYPES[t] ? TYPES[t](v) : true));
}

// A deliberately small check of the top-level input against the tool's schema:
// required members, types, enums, bounds. Agents read the message and retry.
export function checkInput(schema, input) {
  if (!schema || schema.type !== 'object') return;
  const props = schema.properties || {};
  for (const r of schema.required || []) {
    if (input[r] === undefined || input[r] === null || input[r] === '') {
      throw new Error(`Missing required input "${r}".`);
    }
  }
  if (schema.additionalProperties === false) {
    for (const k of Object.keys(input)) {
      if (!(k in props)) throw new Error(`Unknown input "${k}". Allowed: ${Object.keys(props).join(', ') || 'none'}.`);
    }
  }
  for (const [k, p] of Object.entries(props)) {
    const v = input[k];
    if (v === undefined) continue;
    if (!typeOk(p, v)) throw new Error(`Input "${k}" must be ${[].concat(p.type).join(' or ')}.`);
    if (p.enum && !p.enum.includes(v)) throw new Error(`Input "${k}" must be one of: ${p.enum.join(', ')}.`);
    if (typeof v === 'number') {
      if (p.minimum !== undefined && v < p.minimum) throw new Error(`Input "${k}" must be at least ${p.minimum}.`);
      if (p.maximum !== undefined && v > p.maximum) throw new Error(`Input "${k}" must be at most ${p.maximum}.`);
    }
    if (typeof v === 'string' && p.maxLength !== undefined && v.length > p.maxLength) {
      throw new Error(`Input "${k}" is longer than ${p.maxLength} characters.`);
    }
  }
}

function prepare(tool) {
  if (!tool || typeof tool !== 'object') throw new Error('A tool must be an object.');
  if (!NAME_RE.test(tool.name || '')) {
    throw new Error(`Tool name "${tool.name}" must be 1-128 ASCII letters, digits, "_", "-" or ".".`);
  }
  if (!tool.description || typeof tool.description !== 'string') {
    throw new Error(`Tool "${tool.name}" needs a description.`);
  }
  if (typeof tool.execute !== 'function') throw new Error(`Tool "${tool.name}" needs an execute function.`);
  const inputSchema = tool.inputSchema || { type: 'object', properties: {}, additionalProperties: false };
  const run = tool.execute;
  const def = {
    name: tool.name,
    description: tool.description,
    inputSchema,
    async execute(input, options) {
      let args = input;
      if (typeof args === 'string') {
        try { args = JSON.parse(args); } catch (_) { throw new Error('Input must be a JSON object.'); }
      }
      if (args == null) args = {};
      checkInput(inputSchema, args);
      const out = await run(args, options || {});
      return out === undefined ? { ok: true } : out;
    },
  };
  if (tool.title) def.title = tool.title;
  if (tool.outputSchema) def.outputSchema = tool.outputSchema;
  if (tool.annotations) def.annotations = { ...tool.annotations };
  return def;
}

// The JSON-serialisable description of a tool (everything but execute).
export function describeTool(t) {
  const d = { name: t.name, description: t.description, inputSchema: t.inputSchema };
  if (t.title) d.title = t.title;
  if (t.outputSchema) d.outputSchema = t.outputSchema;
  if (t.annotations) d.annotations = t.annotations;
  return d;
}

// Register tools with the agent (when the browser has WebMCP) and expose them to
// the page as window.pageTools. Calling it again replaces the previous set.
export async function registerTools(tools, { label = 'webmcp', ctx = modelContext() } = {}) {
  const defs = [];
  for (const t of tools) {
    try { defs.push(prepare(t)); } catch (e) { console.warn(`[${label}] skipped a tool:`, e.message); }
  }
  const byName = new Map(defs.map((d) => [d.name, d]));

  const state = (typeof window !== 'undefined' && window.pageTools && window.pageTools._kit) ? window.pageTools : null;
  if (state) state.unregister();

  let controllers = [];
  let api = null;
  const registered = [];

  async function register() {
    controllers = [];
    registered.length = 0;
    if (ctx && typeof ctx.registerTool === 'function') {
      api = 'registerTool';
      for (const d of defs) {
        const ac = typeof AbortController === 'function' ? new AbortController() : null;
        try {
          await ctx.registerTool(d, ac ? { signal: ac.signal } : undefined);
          controllers.push({ ac, name: d.name });
          registered.push(d.name);
        } catch (e) {
          console.warn(`[${label}] registerTool failed for ${d.name}:`, e && e.message);
        }
      }
    } else if (ctx && typeof ctx.provideContext === 'function') {
      api = 'provideContext';
      try {
        await ctx.provideContext({ tools: defs });
        registered.push(...defs.map((d) => d.name));
      } catch (e) {
        console.warn(`[${label}] provideContext failed:`, e && e.message);
      }
    }
  }

  function unregister() {
    for (const { ac, name } of controllers.splice(0)) {
      try {
        if (ac) ac.abort();
        else if (ctx && typeof ctx.unregisterTool === 'function') ctx.unregisterTool(name);
      } catch (_) { /* already gone */ }
    }
    if (api === 'provideContext' && ctx && typeof ctx.clearContext === 'function') {
      try { ctx.clearContext(); } catch (_) { /* ignore */ }
    }
    registered.length = 0;
  }

  await register();

  const pageTools = {
    _kit: true,
    supported: !!api,
    api,
    get tools() { return defs.map((d) => d.name); },
    get registered() { return registered.slice(); },
    describe: () => defs.map(describeTool),
    async call(name, input = {}) {
      const d = byName.get(name);
      if (!d) throw new Error(`No tool "${name}" on this page. Tools: ${[...byName.keys()].join(', ')}.`);
      return d.execute(input, { signal: typeof AbortController === 'function' ? new AbortController().signal : undefined });
    },
    unregister,
  };

  if (typeof window !== 'undefined') {
    window.pageTools = pageTools;
    if (!state) {
      // Leaving the page removes its tools; coming back from the back/forward cache restores them.
      window.addEventListener('pagehide', () => window.pageTools && window.pageTools.unregister());
      window.addEventListener('pageshow', (ev) => {
        if (ev.persisted && window.pageTools && window.pageTools._reregister) window.pageTools._reregister();
      });
    }
    pageTools._reregister = register;
    try {
      document.dispatchEvent(new CustomEvent('webmcp:ready', {
        detail: { supported: pageTools.supported, api, tools: pageTools.tools, registered: pageTools.registered },
      }));
    } catch (_) { /* no DOM events */ }
  }
  return pageTools;
}
