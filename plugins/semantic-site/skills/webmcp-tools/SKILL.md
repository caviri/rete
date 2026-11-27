---
name: webmcp-tools
description: Add WebMCP tools to any web page or web app, so an agent in the browser can read and act on what the user sees through the page itself (document.modelContext.registerTool). Use when making a page, site, dashboard or app "WebMCP ready" or "agent ready", when writing, reviewing or debugging WebMCP tools, or when adding custom tools to a site built with the semantic-site skill.
---

# Make a page WebMCP ready

[WebMCP](https://webmachinelearning.github.io/webmcp/) lets a page hand tools to
an agent running in the browser. The page keeps its session and its UI; the
agent calls functions the page defines instead of guessing at the DOM.

The current spec is the W3C Web Machine Learning CG draft of **24 November 2026**
([reference/spec.md](reference/spec.md) has the IDL that matters):

```js
await document.modelContext.registerTool(
  { name, title, description, inputSchema, execute(input, { signal }), annotations },
  { signal },                      // abort it to unregister
);
```

## Use the kit

`runtime/webmcp-kit.js` at the plugin root is a dependency-free ES module that
does the registration right, so pages only define tools. Copy it next to the
page's scripts.

```html
<script type="module">
  import { registerTools, fitList } from './webmcp-kit.js';

  await registerTools([
    {
      name: 'list_orders',
      title: 'List orders',
      description: 'List the signed-in user\'s orders, newest first, with id, date, total and status. ' +
        'Use get_order for one order\'s lines. Does not change anything.',
      inputSchema: {
        type: 'object',
        properties: {
          status: { type: 'string', enum: ['open', 'shipped', 'cancelled'], description: 'Only orders with this status.' },
          limit: { type: 'integer', minimum: 1, maximum: 50, default: 10 },
        },
        additionalProperties: false,
      },
      annotations: { readOnlyHint: true, untrustedContentHint: true },
      async execute({ status, limit = 10 }) {
        const rows = (await app.orders({ status })).slice(0, limit);
        const { items, truncated } = fitList(rows, (x) => ({ orders: x }));
        return { total: rows.length, truncated, orders: items };
      },
    },
  ]);
</script>
```

What the kit does for you:

- **Feature detection**: `document.modelContext` (spec), then `navigator.modelContext` and `provideContext` (older Chrome previews); a no-op without any.
- **Lifecycle**: one `AbortController` per tool; tools are removed on `pagehide` and restored when the page returns from the back/forward cache.
- **Validation**: tool names must be 1–128 of `A–Z a–z 0–9 _ - .`; every tool needs a description and `execute`; the input is checked against the top level of `inputSchema` (required, types, enums, bounds, unknown keys) with messages an agent can act on.
- **Testing without WebMCP**: `window.pageTools` exposes `tools`, `registered`, `describe()` and `call(name, input)` in every browser, and a `webmcp:ready` event fires on `document`.
- **Budgets**: `fitList` and `clip` keep results under `RESULT_BUDGET` (18,000 characters).

## Design rules

1. **One tool per intent, named as a verb**: `search_products`, `get_order`, `show_results`. Prefer 5–10 tools a page really needs over a mirror of every button.
2. **Descriptions are for choosing**: say what it does, what it returns, which tool comes before or after it, and what it does not do. "Does not change what the user sees" is useful information.
3. **Separate reading from acting**. Read tools get `readOnlyHint: true`. Tools that change the visible page (`show_*`, `open_*`) say so. Tools with real consequences (buy, send, delete, publish) get `consequentialHint: true` and the page asks the user to confirm in its own UI.
4. **Mark third-party data**: tools that return content the site owner did not write (user posts, harvested records, search results) get `untrustedContentHint: true`, so the agent treats it as data, not instructions.
5. **Tight schemas**: `type: "object"`, `additionalProperties: false`, `enum` from real values, `minimum`/`maximum` on counts, `maxLength` on strings, `required` only for what is truly needed.
6. **Small, linked results**: return JSON with ids that feed the next tool, totals, and a `truncated` flag. Never return HTML or whole documents.
7. **Errors that teach**: throw `Error` with what to do next ("No item "x". Use search to find ids."), not a stack trace.
8. **Mirror the UI**: a `show_*` tool changes the screen the same way the user's own controls do, and a `get_current_page` tool says what the user is looking at, so the agent and the person stay on the same page.
9. **Navigation ends the page's tools**: a tool that navigates returns first (navigate on a timer) and says the next page brings its own tools.
10. **Register when ready**: register after the data the tools need has loaded, and only in a secure context (https, or localhost). Inside an iframe the embedder must allow it: `<iframe allow="tools">`.

## On a semantic-site site

Add tools to every page without touching the generator: put a module in the
site's `static` folder and list it in `site.json`:

```json
"webmcp": { "scripts": ["assets/my-tools.js"] }
```

```js
// static/assets/my-tools.js — env is what the built-in tools use:
// env.site (api/index.json), env.page, env.fetchJSON(rel), env.url(rel), env.navigate(href), env.view
export default function tools(env) {
  if (env.page.kind !== 'item') return [];
  return [{
    name: 'cite_item',
    description: 'A citation for the item on screen, in APA style.',
    annotations: { readOnlyHint: true },
    async execute() {
      const rec = await env.fetchJSON(`api/${env.page.collection}/${env.page.id}.json`);
      return { citation: `${rec.title}. ${env.site.site.title}. ${env.url(rec.url)}` };
    },
  }];
}
```

## Test

1. `await pageTools.call('tool_name', { … })` in the console, in any browser.
2. Chrome: enable `chrome://flags/#enable-webmcp-testing`, relaunch, install the **Model Context Tool Inspector** extension; it lists the page's tools and runs them. In that Chrome, `await document.modelContext.getTools()` and `document.modelContext.executeTool(tool, input)` are the agent's view.
3. `node <plugin-dir>/skills/site-check/scripts/check.mjs <site-dir>` fails any page that registers no tools, and on semantic-site sites runs every read tool.
