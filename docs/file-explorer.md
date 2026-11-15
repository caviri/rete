# File explorer — browse a `.rete` like an archive

Open a `.rete` the way an archive tool opens a `.zip`: folders, files, a preview
pane, a graph you can draw, and **extract**. It runs in a browser and ships as a
desktop app for macOS and Windows.

SPARQL is the wall most people hit; folders are the one interface everybody
already knows. Nothing here is a new engine feature — it projects what a `.rete`
already carries (its header and section table, the baked schema, the Dataset
Card, its permutation indexes) onto a metaphor a non-specialist can walk into.
It reads every format the engine reads, `0x05` to `0x08`.

> **Status: experiment.** It lives in `experiments/rete-file-explorer/` and is not
> part of the released surface. The desktop builds are unsigned.

---

## Run it in a browser

Served from the repo root, like the [plaza](plaza-guide.md):

```sh
python3 -m http.server 8901
# → http://127.0.0.1:8901/experiments/rete-file-explorer/index.html
```

It loads the WASM build from `web/pkg-nomodules/`, so run `scripts/build_wasm.sh`
first if that directory is stale.

Open a file from a catalog, paste any `.rete` link, or **drag a file onto the
window** — anywhere, at any time.

## Your files and your catalogs

The home has two shelves. **Your files** lists what you opened here, newest
first: a link reopens with a click, the desktop app reopens a path, and a browser
asks for a local file again (it cannot reopen one by itself). **Catalogs** lists
the files of every catalog you subscribe to, with a filter over names, tags and
licences.

A catalog is a JSON list of `.rete` links that anyone can publish next to their
graphs. Paste its link in the box at the top, or in the Catalogs shelf, to
subscribe; a link to a `.rete` file opens it instead. The explorer reads rete's
own catalog shape (`{"datasets": [{"key", "label", "url", "size", …}]}`, as in
`clients/relay/catalog.json`), the plaza's (`docs/plaza/plaza.json`, links
relative to the catalog), a list of entries, or a plain list of links. rete's
catalog is subscribed by default. Subscriptions and your files are kept in the
browser (or the desktop app's webview); nothing is sent anywhere.

Each picture is drawn from the file's own card — its classes as discs sized by
their instances, the links between them, coloured by vocabulary — the same
picture the plaza draws. The card is read with two small range requests, only for
files that scroll into view, and kept.

## Install the desktop app

Builds are attached to a GitHub Release by
[`.github/workflows/desktop-release.yml`](https://github.com/caviri/rete/actions/workflows/desktop-release.yml):
a universal macOS `.dmg` (Apple Silicon + Intel) and a Windows `.msi` /
`-setup.exe`.

The desktop build drives `rete-core` natively rather than WASM, which buys two
things the browser cannot give: no 4 GB heap ceiling, and real threads. Lazy
reads against a local file are no longer one of them — since issue #102 the
browser opens a dropped `.rete` through the same range reader it uses for a URL,
slicing the blob instead of issuing HTTP ranges, so a multi-gigabyte file on disk
faults in there too rather than being read whole.

**They are unsigned.** macOS refuses them on first open ("cannot be opened
because the developer cannot be verified", or "is damaged"); neither is true —
it is the quarantine attribute doing its job on an app with no Developer ID.
Once, after dragging to Applications:

```sh
xattr -dr com.apple.quarantine "/Applications/Rete File Explorer.app"
```

Or right-click the app → **Open** → **Open**. On Windows, SmartScreen shows
"Windows protected your PC" → **More info** → **Run anyway**.

## Quick Look on macOS (experimental)

The app bundles a **Quick Look preview extension**: select a `.rete` in Finder,
press space, and its Dataset Card appears — title, description, licence,
publisher, the section table, quad and term counts, content hash.

It is a good fit for a reason worth stating. Reading a card is **two positional
reads** — the 1 KB header, then the metadata section it points at. No dictionary,
no index, no query engine. So a 17 GB graph previews exactly as fast as a 200 KB
one, because the work does not scale with the file. Quick Look demands
sub-second previews and this clears it without trying.

> **It will not load from an unsigned build.** macOS refuses to register an app
> extension that carries no valid signature, and every release so far is
> unsigned. Everything else survives via the quarantine bypass; an extension
> does not. This is the first feature that genuinely needs an Apple Developer
> certificate rather than merely being nicer with one.

Source: `clients/tauri/quicklook/`. It is assembled without an Xcode project —
an app extension is a bundle whose binary links with `-e _NSExtensionMain`, so
`swiftc` builds one directly, which keeps it in the same shell step as the rest
rather than adding a second project file to keep in sync.

The app also declares `.rete` as an exported type
(`com.graphplaza.rete`), which is what the extension keys off and what gives
Finder double-click-to-open.

---

## Five views, because there is no single honest tree

RDF is a graph. It has no canonical folder hierarchy — but it has several
defensible ones, each true in its own way, so you switch between them.

| View | Folders are | What it costs |
|---|---|---|
| **Types** | classes; files are instances | the schema pyramid only — the triple index is never read |
| **Namespace** | the IRI's own path segments | a bounded sample per class |
| **Predicates** | one relation each; files are two-column tables | the schema pyramid only |
| **Graphs** | named graphs, as volumes | one `DISTINCT ?g` |
| **Sections** | the actual bytes on disk | the header and the section table: a few KB |

**Sections** is the one that makes the metaphor land. It renders the file's
section table with real offsets (the root block's table in `0x06`–`0x08`, the
1 KB directory in `0x05`), so you can see that the INDEX is 47% of a file and
the DICTIONARY 9%, and click a section to see its extent drawn against the whole
file. See [the format spec](SPEC.md) for what each section holds.

A "file" is a resource's Concise Bounded Description, previewable as a property
table, JSON-LD, or Turtle, with its incoming references underneath.

## Folders that do not bottom out

Expanding an instance lists the resources it points at, labelled by the predicate
that got you there, plus a `↩ referenced by` folder for the inbound side. So you
descend `Creature → A-Acererak the Archlich → printedIn → Adventures in the
Forgotten Realms → ↩ referenced by → …` for as long as the data keeps going.

That is not a limitation of the browser; it is the honest shape of RDF. Ancestors
travel with each node, so a link back to something already on your path is marked
`↺` rather than pretending to be new. Literals are deliberately *not* children —
they are the contents of the file, shown in the preview.

## Three layouts

- **Tree** — nested and expandable. The twisty expands; clicking the name opens
  the preview. They are separate targets on purpose: a resource is both a folder
  and a file, so clicking its name must not collapse it.
- **Icons** — one folder at a time as tiles with a breadcrumb, like a file
  manager. Tiles show a real thumbnail wherever the data has one.
- **Graph** — draws what is selected: an entity and its links, a sample of a
  class, a predicate or a named graph, or, with nothing selected, the schema as
  boxes of classes listing their properties. The **◉ Graph** button on a preview
  or on query results does the same, and double-clicking a row too.

## The graph

The Graph layout is the graph view of the [Claude Desktop extension](agents.md#the-desktop-extension-retemcpb):
the same page, which the explorer hosts the way Claude does (it is an
[MCP App](https://github.com/modelcontextprotocol/ext-apps)) and answers from the
open file instead of from the extension. Everything the view does in Claude works
here: double-click a node to bring in its links, search for a name, group nodes
into coloured layers, draw dots, boxes or a class diagram, change the layout,
hide, remove and aggregate nodes, switch to a table and save it as CSV. What
needs a conversation (sending a selection to the chat) is left out.

An entity adds to the picture, so walking from one thing to the next builds it
up; a schema, a sample or a query replaces it. The copy lives in
`experiments/rete-file-explorer/graph/`, generated from `clients/mcpb/src/view/`
by `clients/mcpb/build-explorer.mjs`; the extension's CI fails when it is out of
date.

## Naming things

IRIs make serviceable filenames and terrible reading. The **name by** control
lists every literal-valued predicate the schema knows about, with counts, and
lets you choose which supplies display names — because `rdfs:label` is not
universal. The default tries the usual set in order (`rdfs:label`,
`skos:prefLabel`, `schema:name`, `dcterms:title`, …).

Names are never on the critical path: listings paint local names immediately and
patch labels in as background chunks land.

## Search

Two indexes can answer a text search and neither is guaranteed, so the box tries
the better one first and **tells you which answered**:

| Index | Matches | Present when |
|---|---|---|
| `TEXT_INDEX` | whole words, `CONTAINS` | built with `rete build --text-index` |
| label prefix | the start of a label | the file has a pyramid |

That distinction matters. Silently returning nothing because a file has no text
index is indistinguishable from "no matches", and the difference decides whether
you rephrase the search or rebuild the file.

## SPARQL

The folder views can only answer questions someone anticipated. The **SPARQL**
tab is the escape hatch: an editor seeded from the file's own biggest class,
`Ctrl`/`Cmd`+`Enter` to run, results as a table whose IRIs are clickable — so you
can query your way to something and then keep walking the graph from it.
`CONSTRUCT` and `DESCRIBE` come back as Turtle. Results export as CSV or JSON.

See [SPARQL support](sparql.md) for what the engine accepts.

## Extract

Select a class, predicate, or named-graph folder and pull it out as **CSV**,
**JSON**, or **N-Triples**, with a row cap you control.

This is the half of the archive metaphor that RDF tooling never offers:
[`rete export`](cli.md) is all-or-nothing, and the alternative is writing SPARQL.
"Tick the `Person` folder, get a spreadsheet" is the actual product.

---

## What it costs to browse

The footer meter reads `RemoteGraph.stats()` — cumulative *physical* bytes and
requests, not a guess.

| Archive | Size | Open | Traffic to open |
|---|---|---|---|
| World Cup 2022 | 216 KB | instant | 216 KB / 2 req (small files just get read) |
| Lombardi | 1016 KB | instant | 760 KB / 6 req |
| Wikidata slice | 1.39 GB | ~1 s warm | 10.5 MB / 18 req — **0.74%** |
| data.bnf.fr | 7.16 GB, 673.5M quads | 5.7 s cold | 18.9 MB / 23 req — **0.26%** |

Listing a class of 1,067,797 Wikidata items — the first 200, with labels — took
the session to 109 MB. That cost is dominated by **dictionary chunk faults**
while decoding subject IDs, not by the `?s a <C>` scan, which is a contiguous POS
range.

## Files without a pyramid have no class list

`gharchive`, `orcid`, `dblp` and the `wikidata-xxl` shards were built without
one. Types and Predicates are empty for them, because deriving classes would mean
scanning the whole file — the page says so rather than appearing broken, and
Namespace and Sections still work.

Check before you wonder: the header says whether a file has one (the schema
presence bit in `0x06`–`0x08`, a nonzero `schemaMetaLen` in `0x05`).

---

## Shape, for reuse

The split is deliberate:

- **`js/rete-fs.js`** — the whole projection: header parsing, the five views,
  lazy listings, file bodies, search, query, extract. No DOM, no `fetch` of its
  own; it takes an `engine` (`query` / `prefix` / `text`) plus the file's
  self-description.
- **`js/fs-worker.js`** — owns one `Graph` / `RemoteGraph`. A classic worker,
  because remote reads use synchronous XHR, which browsers allow only there.
- **`js/library.js`** — your files and catalogs: what is kept, and how a catalog
  of any shape is read.
- **`js/graph-host.js`** — hosts the graph view and answers its tool calls with
  the view model shared with the extension (`clients/mcpb/src/view-model.mjs`).
- **`js/app.js`** — chrome only.

Exactly one line differs between the web and desktop builds:

```js
const w = isTauri() ? makeTauriWorkerShim() : new Worker("./js/fs-worker.js");
```

`tauri-bridge.js` makes the native commands speak the worker's message protocol,
so `rete-fs.js` is byte-identical in both. The engine is a seam; the desktop
build is the proof.
