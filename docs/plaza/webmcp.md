# WebMCP on rete plaza

Plaza exposes a few tools to in-browser AI agents through
[WebMCP](https://github.com/webmachinelearning/webmcp). An agent can find a
dataset, read its card and run SPARQL against it, while you watch the same page.

The code is `js/webmcp.js`. It does nothing if the browser has no WebMCP.

## Tools

Gallery page (`index.html`):

- `search_datasets` - text, filters and sort, up to 50 summaries. Read only.
- `list_filters` - the filter groups and their values with counts. Read only.
- `get_dataset_card` - description, licence, source, counts, top classes and predicates, starter query titles. Read only.
- `show_datasets` - applies a query, filters and sort to the page you see.
- `open_dataset` - goes to a dataset page, which has its own tools.

Dataset page (`dataset.html?key=...`):

- `get_dataset_overview` - summary, schema digest, starter query titles. Read only.
- `get_example_queries` - the card's starter queries with their SPARQL. Read only.
- `run_sparql` - SELECT, ASK, CONSTRUCT or DESCRIBE; default 50 rows, at most 500. Read only.
- `find_entities` - label prefix search, returns IRIs. Read only.
- `show_query_in_page` - puts a query in the editor, optionally runs it.

Results are capped at about 18 KB and say when they were truncated. Errors from
the engine come back as tool errors with the message.

Page scripts can read `window.plazaWebMCP` (`{ supported, tools }`) or listen for
the `plaza:webmcp` event on `document`.

## Try it

WebMCP is an early preview, so the API may still change.

1. Use Chrome 149 or later. Open `chrome://flags/#enable-webmcp-testing`, enable it, relaunch.
2. Install the "Model Context Tool Inspector" extension from the Chrome Web Store.
3. Open the gallery or a dataset page. The inspector lists the tools; you can call
   them by hand or with a prompt.

Tools are registered with `document.modelContext.registerTool(tool, { signal })`
and removed on `pagehide` by aborting the signal. Older previews used
`navigator.modelContext` and `provideContext`; the script falls back to those.
