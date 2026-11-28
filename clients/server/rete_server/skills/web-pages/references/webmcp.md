# WebMCP tools for a page backed by a .rete file

WebMCP (W3C Web Machine Learning CG draft, 24 November 2026) lets a page hand
tools to an agent in the browser. The page keeps its session and its UI; the
agent calls functions the page defines.

```js
await document.modelContext.registerTool(
  { name, title, description, inputSchema, outputSchema, execute(input, { signal }), annotations },
  { signal }            // abort the signal to unregister
);
```

Older Chrome previews exposed `navigator.modelContext` (with `provideContext({ tools })`).
Secure contexts only: https or localhost. Without WebMCP the page loses nothing;
keep the tools reachable as `window.pageTools.call(name, input)` so you can test
them and so other scripts can use them.

`page.html` next to this file does all of that in about twenty lines. For a
fuller kit (validation, lifecycle on `pagehide`, result budgets), the rete
`semantic-site` plugin ships `runtime/webmcp-kit.js`, a dependency-free module.

## Which tools a graph-backed page should offer

Few and high-level first, a raw query tool last.

| Tool | Does | Notes |
|---|---|---|
| `get_dataset_info` | the card: title, description, license, counts, source | from `graph.card()`; two small range reads |
| `search_<things>` | find entities by name | `graph.prefixSearch(text, 20)` if the file has a label index, else a `CONTAINS` query with `LIMIT` |
| `get_<thing>` | one entity with its properties and links | `VALUES ?s { <iri> }` or a bound IRI; small |
| `list_<things>` | filtered, sorted, paged list | `where` and `sort` inputs, `limit` max 50 |
| `run_sparql` | any read-only query | SELECT and ASK only, `LIMIT` enforced, rows capped |
| `get_example_queries` | the file's own starter queries | `graph.examples()`; they are known to run |
| `show_<things>` | change what the page shows | say in the description that it changes the page |

## Writing a tool

- **Name**: 1 to 128 characters, letters, digits, `_`, `-`, `.`. Verb first: `search_papers`.
- **Description**: what it does, when to use it, what it returns, and what it does not change. Two sentences. Agents pick tools from this text.
- **inputSchema**: JSON Schema with `type: "object"`, per-property descriptions, `enum`s and `minimum`/`maximum` where they apply, `additionalProperties: false`.
- **Result**: small JSON, a few KB (the kit budgets 18,000 characters). Cut long lists and return `{ truncated: true, total }`. Agents pay for every character.
- **Errors**: throw an `Error` with a message that says how to fix the call. The agent reads it and retries.
- **Annotations**: `readOnlyHint: true` for reads; `untrustedContentHint: true` when the result is data someone else wrote (all graph content is); `consequentialHint: true` for anything with real effects, and then ask the user to confirm in the page's own UI.

## `run_sparql` safely

```js
{
  name: "run_sparql",
  description: "Run a read-only SPARQL SELECT or ASK against this page's graph and return up to 200 rows. Add a LIMIT.",
  inputSchema: { type: "object", properties: { query: { type: "string", maxLength: 4000 } }, required: ["query"], additionalProperties: false },
  annotations: { readOnlyHint: true, untrustedContentHint: true },
  execute: async ({ query }) => {
    if (!/^\s*(PREFIX[^\n]*\n\s*)*(SELECT|ASK)\b/i.test(query)) throw new Error("Only SELECT and ASK are allowed.");
    return { rows: (await call("query", { query })).slice(0, 200) };
  },
}
```

The graph is read-only, so a query cannot change data, but an expensive one can
freeze the worker. Cap rows, reject `SERVICE` unless you mean to allow it, and
keep the worker separate from the page so the page stays responsive.

## Testing

- Without a browser that has WebMCP: `window.pageTools.call("search_papers", { text: "orbit" })` in the console.
- With a stub, in a test: `document.modelContext = { registerTool: async (t) => seen.push(t.name) }` before the page script runs; the template registers into it.
- Check that every tool returns valid JSON and stays small, including for an empty result and for an error.
