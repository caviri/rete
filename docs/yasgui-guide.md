# SPARQL IDE — yasgui·wasm

**[▸ Launch the IDE](yasgui.html)** — one static HTML file, no server, no account.

A [Yasgui](https://github.com/TriplyDB/Yasgui)-style SPARQL IDE with one twist: the *endpoint* is a
**`.rete` file**. Where the original sends every query to a SPARQL server, here the engine is the rete
WASM build inlined into the page — so the endpoint field takes:

- **a URL** — any `.rete` served with CORS + HTTP Range. The file is opened *lazily*: the engine
  range-fetches only the bytes your query touches. The stats line shows exactly how much — the default
  example answers "who did Rembrandt teach?" over Getty ULAN by fetching **~512 KB of a 4.1 MB file in
  3 range requests**; running it again fetches 0 bytes (session cache).
- **a local file** — the **⬆ open file** button or drag-and-drop. The file is read in browser memory
  and never uploaded anywhere.

The **datasets ▾** menu offers a curated slice of the [published datasets](playground-guide.md)
(Getty ULAN lineage, Spanish law, Herculaneum scrolls, Magic cards, a 100 MB Wikidata bite, …), each
served straight from object storage.

## What it does

Everything you'd expect from Yasgui, plus a few things a server endpoint can't offer:

- **Tabs** — each with its own endpoint, query, and results; rename by double-click, reorder by drag,
  persisted in `localStorage` across visits.
- **Editor** (CodeMirror 6, SPARQL mode, classic YASQE colors) — `Ctrl+Enter` runs, `Ctrl+Space`
  completes. Autocompletion merges three sources:
  1. SPARQL keywords, functions, and the query's own `?variables`;
  2. **prefixes** — a curated table merged with [prefix.cc](https://prefix.cc)'s popular list, and,
     YASQE-style, typing `foaf:` in the body auto-inserts the `PREFIX` declaration at the top;
  3. **entities from the dataset itself** — suggestions come from the open file's label prefix-index
     (type `Rembr`, get *Rembrandt van Rijn* with its IRI), served lazily over HTTP range like
     everything else.
- **Result views**, per result kind: **Table | Pivot | Response** for `SELECT` (the pivot is a
  row × column count/sum matrix), **Table | Turtle | Response** for `CONSTRUCT`, a boolean panel for
  `ASK`. The table filters, sorts, paginates; CSV/JSON download included.
- **Share links** — 🔗 copies a URL that reopens your query + endpoint in a fresh tab, like Yasgui's.
- **🧠 reason** — runs the query with [OWL 2 QL entailment](reasoning.md) rewritten against the file's
  ontology; try it on the `boe` dataset.
- **A per-query traffic line** — elapsed time plus bytes / range requests actually fetched, measured by
  the engine. In-memory files report *in-memory*; cached re-runs report *0 bytes fetched*.

## Hosting your own

Point the endpoint field at any `.rete` you [host yourself](hosting.md): the server must send
`Access-Control-Allow-Origin` and honor `Range` requests (status 206) — R2, S3-compatible stores,
GitHub releases and Zenodo all qualify. Build the file with [`rete build`](cli.md), from RDF or
[anything else](data-engineering.md).

## How it's built

`scripts/build_yasgui.py` inlines the CodeMirror bundle, the wasm-bindgen no-modules glue and the
base64-encoded engine into `docs/yasgui.html` from `web/yasgui.template.html` +
`web/yasgui-src/{app,worker}.js`. Queries run in a Blob-spawned Web Worker — the lazy remote reader
uses synchronous XHR, which browsers only allow off the main thread. The end-to-end regression lives
at `tests/gate/checks/check_yasgui.mjs` (run it manually after touching the sources; it is not part
of the gate matrix).

UI after [Yasgui](https://yasgui.triply.cc/) by Triply — the tab model, the round query button, the
YASR view plugins and the autocompleters are all loving imitations.
