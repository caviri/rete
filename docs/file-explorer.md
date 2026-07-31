# File explorer — browse a `.rete` like an archive

Open a `.rete` the way an archive tool opens a `.zip`: folders, files, a preview
pane, and **extract**. It runs in a browser and ships as a desktop app for macOS
and Windows.

SPARQL is the wall most people hit; folders are the one interface everybody
already knows. Nothing here is a new engine feature — it projects what a `.rete`
already carries (the 1 KB section directory, the baked schema pyramid, the
Dataset Card, six permutation indexes) onto a metaphor a non-specialist can walk
into.

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

Open one of the catalogued datasets, paste any `.rete` URL, or **drag a file onto
the window** — anywhere, at any time.

## Install the desktop app

Builds are attached to a GitHub Release by
[`.github/workflows/desktop-release.yml`](https://github.com/caviri/rete/actions/workflows/desktop-release.yml):
a universal macOS `.dmg` (Apple Silicon + Intel) and a Windows `.msi` /
`-setup.exe`.

The desktop build drives `rete-core` natively rather than WASM, which buys three
things the browser cannot give: no 4 GB heap ceiling, real threads, and
positional reads against a local file — so a multi-gigabyte `.rete` on disk
faults in rather than being read whole.

**They are unsigned.** macOS refuses them on first open ("cannot be opened
because the developer cannot be verified", or "is damaged"); neither is true —
it is the quarantine attribute doing its job on an app with no Developer ID.
Once, after dragging to Applications:

```sh
xattr -dr com.apple.quarantine "/Applications/Rete File Explorer.app"
```

Or right-click the app → **Open** → **Open**. On Windows, SmartScreen shows
"Windows protected your PC" → **More info** → **Run anyway**.

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
| **Sections** | the actual bytes on disk | nothing — it *is* the first 1 KB |

**Sections** is the one that makes the metaphor land. It renders the header's
typed section directory with real offsets, so you can see that the INDEX is 47%
of a file and the DICTIONARY 9%, and click a section to see its extent drawn
against the whole file. See [the format spec](SPEC.md) for what each section holds.

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

## Two layouts

- **Tree** — nested and expandable. The twisty expands; clicking the name opens
  the preview. They are separate targets on purpose: a resource is both a folder
  and a file, so clicking its name must not collapse it.
- **Icons** — one folder at a time as tiles with a breadcrumb, like a file
  manager. Tiles show a real thumbnail wherever the data has one.

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

`gharchive`, `orcid`, `dblp` and the `wikidata-xxl` shards carry
`schemaMetaLen = 0`. Types and Predicates are empty for them, because deriving
classes would mean scanning the whole file — the page says so rather than
appearing broken, and Namespace and Sections still work.

Check before you wonder: a nonzero `schemaMetaLen` in the header means the views
will populate.

---

## Shape, for reuse

The split is deliberate:

- **`js/rete-fs.js`** — the whole projection: header parsing, the five views,
  lazy listings, file bodies, search, query, extract. No DOM, no `fetch` of its
  own; it takes an `engine` (`query` / `prefix` / `text`) plus the file's
  self-description.
- **`js/fs-worker.js`** — owns one `Graph` / `RemoteGraph`. A classic worker,
  because remote reads use synchronous XHR, which browsers allow only there.
- **`js/app.js`** — chrome only.

Exactly one line differs between the web and desktop builds:

```js
const w = isTauri() ? makeTauriWorkerShim() : new Worker("./js/fs-worker.js");
```

`tauri-bridge.js` makes the native commands speak the worker's message protocol,
so `rete-fs.js` is byte-identical in both. The engine is a seam; the desktop
build is the proof.
