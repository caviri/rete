# rete filesystem — browse a `.rete` like an archive

An experiment: open a `.rete` graph the way an archive tool opens a `.zip`.
Folders, files, a preview pane, and — the verb that matters — **extract**.

The premise is that SPARQL is the wall most people hit, and folders are the one
interface everybody already knows. Nothing here is a new engine feature; it is a
projection of what a `.rete` already carries (a 1 KB section directory, a baked
schema pyramid, a Dataset Card, six permutation indexes) onto a metaphor a
non-specialist can walk into.

## Run it

Served from the repo root, like `experiments/plaza`:

```sh
python3 -m http.server 8901
# → http://127.0.0.1:8901/experiments/filesystem/index.html
```

It loads the WASM build from `../../../web/pkg-nomodules/`, so run
`scripts/build_wasm.sh` (in Docker) first if that directory is stale.

Open one of the catalogued R2 archives, paste any `.rete` URL, or drop a local
file on the page.

## Five views, because there is no single honest tree

RDF is a graph. A graph has no canonical folder hierarchy — but it has several
defensible ones, each true in its own way, so you switch between them:

| View | Folders are | Cost |
|---|---|---|
| **Types** | classes; files are instances | schema pyramid only — no index read |
| **Namespace** | the IRI's own path segments | a bounded sample per class |
| **Predicates** | one relation each; files are 2-column tables | schema pyramid only |
| **Graphs** | named graphs, as volumes | one `DISTINCT ?g` |
| **Sections** | the actual bytes on disk | zero queries — it *is* the first 1 KB |

`Sections` is the one that makes the metaphor land: it renders the header's
typed section directory with real offsets, so you can see that the INDEX is 47%
of a file and the DICTIONARY 9%, and click a section to see its extent drawn
against the whole file.

A "file" is a resource's Concise Bounded Description, previewable as a property
table, JSON-LD, or Turtle, with its incoming references listed underneath.

## A resource is a folder *and* a file

The folders do not bottom out, because the graph doesn't. Expand an instance and
its **outgoing links become its children**, labelled by the predicate that got
you there, plus a `↩ referenced by` folder for everything pointing back. So you
descend `Creature → A-Acererak the Archlich → printedIn → Adventures in the
Forgotten Realms → ↩ referenced by → …` for as long as the data keeps going.
That is not a limitation of the browser; it is the honest shape of RDF, and the
tree is the first place it has ever looked like a normal thing to do.

Ancestors travel with each node, so a link back to something already on the path
is marked `↺` rather than pretending to be new. Literals are deliberately *not*
children — they are the contents of the file, and live in the preview pane.

Two layouts, switched in the toolbar:

- **Tree** — nested and expandable. The twisty expands; clicking the name opens
  the preview. (They are separate targets on purpose: a resource is both a
  folder and a file, so clicking its name must not collapse it.)
- **Icons** — one folder at a time as tiles, with a breadcrumb, like a normal
  file manager. Tiles show a **real thumbnail** wherever the resource has one:
  all 34,633 Magic cards render their card art, `bioexplora` its specimens,
  `arxiu` its scans.

## Naming things

IRIs make serviceable filenames but terrible reading. The **name by** control
lists every literal-valued predicate the schema knows about, with counts, and
lets you pick which one supplies display names — because every dataset names
things its own way and `rdfs:label` is not always present. Default is the usual
set (`rdfs:label`, `skos:prefLabel`, `schema:name`, `dcterms:title`, …), tried
in order. Pick `oracleText` on `mtg` and every card is titled by its rules text;
that is silly, and it is also the point — the file does not decide for you.

## Extract

Select a class, predicate, or named-graph folder and pull it out as **CSV**,
**JSON**, or **N-Triples**, with a row cap you control. This is the half of the
archive metaphor that RDF tooling never offers: `rete export` is all-or-nothing,
and the alternative is writing SPARQL. "Tick the `Person` folder, get a
spreadsheet" is the actual product here.

## Measured

Every number below is from the footer's traffic meter, which reads
`RemoteGraph.stats()` — cumulative physical bytes and requests.

| Archive | Size | Open | Traffic to open |
|---|---|---|---|
| World Cup 2022 | 216 KB | instant | 216 KB / 2 req (small files just get read) |
| Lombardi | 1016 KB | instant | 760 KB / 6 req |
| Wikidata slice | 1.39 GB | ~1 s warm | 10.5 MB / 18 req — **0.74%** |
| data.bnf.fr | 7.16 GB, 673.5M quads | 5.7 s cold | 18.9 MB / 23 req — **0.26%** |

Listing a class of 1,067,797 Wikidata items — the first 200, with labels — took
the session to 109 MB. That cost is dominated by **dictionary chunk faults**
while decoding subject IDs, not by the `?s a <C>` scan itself, which is a
contiguous POS range. It is the same effect noted for huge-dictionary remote
opens elsewhere in the project.

## What broke, honestly

- **Files built without a pyramid have no class list.** `gharchive`, `orcid`,
  `dblp` and the `wikidata-xxl` shards carry `schemaMetaLen = 0`, so Types and
  Predicates are empty for them — deriving classes would mean scanning 17 GB.
  The page says so plainly and the Namespace and Sections views still work. The
  catalog deliberately ships only files that do carry one.
- **Labels cannot be on the critical path.** Resolving display names for a page
  of 200 IRIs is expensive on a big remote file, so listings paint local names
  immediately and patch labels in as background chunks land.
- **Old files are refused.** The current WASM build reads format `0x05` only; a
  `0x04` file dropped on the page reports that instead of half-working.
- **`?p IN (…) || REGEX(…)` does not parse.** The engine rejects the whole query
  with `parse error: expected ENCODE_FOR_URI`, though `FILTER(?p IN (…))` and
  `FILTER(REGEX(STR(?p), …))` each parse fine on their own. The decoration pass
  therefore runs two plain queries instead of one clever disjunction — worth
  knowing before anyone else writes that filter and concludes their data is
  missing. Everything else needed here (`STR`, `REGEX`, `VALUES`, `IN`,
  `DESCRIBE`, `LIMIT`/`OFFSET`) works.
- **Thumbnails needed a third attempt.** `loading="lazy"` never fires on an
  `<img>` that is not yet in the document, and an IntersectionObserver rooted on
  the `overflow:auto` pane reported nothing in a short viewport — 200 tiles sat
  blank in both cases. What works is a bounded FIFO (6 in flight, DOM order,
  dropped on navigation): no geometry dependence at all.

## Transferring it to the explorer

The split is deliberate:

- **`js/rete-fs.js`** — the whole projection. Header parsing, the five views,
  lazy listings, file bodies, extract. No DOM, no `fetch` of its own; it takes an
  `engine` object (`query` / `prefix` / `text`) plus the file's self-description.
  This is the piece that moves.
- **`js/fs-worker.js`** — the engine bridge. Owns one `Graph` or `RemoteGraph`
  and forwards the progress hook. Classic worker, because remote reads use
  synchronous XHR.
- **`js/app.js`** — chrome only: tree, tabs, preview, status bar.

Dropping this into `docs/explorer.html` means keeping `rete-fs.js` as-is and
re-implementing the ~400 lines of `app.js` against the explorer's own layout.
The `engine` interface is the seam: a Tauri build would swap the worker for
native `rete-core` IPC and change nothing else.
