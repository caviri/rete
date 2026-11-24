# Chrome — Retelens

Retelens is a Chrome extension that opens `.rete` files where they are hosted.
A `.rete` link normally downloads the file. With Retelens the page instead
shows the file's Dataset Card, tables from its example queries, the results as
a table, cards or a graph, and a SPARQL console. Like the
[JavaScript client](javascript.md), whose engine it carries, it reads only the
byte ranges a query needs: the 1.6 GB Wikidata file on Zenodo answers its
first query after reading about 12 MB.

> Source and build: [`clients/chrome/`](https://github.com/caviri/rete/tree/main/clients/chrome).
> Experimental, like the format: a file written by a newer rete may need a newer Retelens.

## Install

1. Download [retelens.zip](https://github.com/caviri/rete/releases/latest/download/retelens.zip)
   from the latest release and unzip it.
2. Open `chrome://extensions` and turn on **Developer mode**.
3. Choose **Load unpacked** and pick the unzipped folder.

Each release attaches the same extension as `retelens-<version>.zip`. It is
built with the engine of that release, so it reads what that release's CLI
writes.

## Where it works

| Where | What you get |
|---|---|
| Hugging Face dataset | A viewer in place of "The dataset viewer is not available", a Data Studio tab (full width at `/viewer`), chips for format, size and licence, and the card in the sidebar |
| Hugging Face storage bucket | The viewer on a `.rete` file's page, with a switch to the full-width Data Studio |
| GitHub | The viewer inside the file box of a `.rete` file, with a Data Studio button in its toolbar, in GitHub's light or dark theme; *Open in viewer* links in folders and READMEs |
| Zenodo | A Preview button on each `.rete` file, a preview panel above the file list, a graph card in the sidebar and a `rete 0x07 · 119.9 M statements` label in the header |
| Any link ending in `.rete` | The viewer opens instead of a download, with the file's name, where it lives, its format and size, and a Download button |

On sites other than these, Retelens asks for access before it reads a file.
The toolbar button opens a URL or a local file. Right-click a `.rete` link and
choose *Preview with rete* to open it.

Try it on [katospiegel/rete-graphs](https://huggingface.co/datasets/katospiegel/rete-graphs),
[a GitHub file](https://github.com/caviri/rete/blob/main/docs/plaza/data/linked-jazz.rete)
or [Zenodo record 24131107](https://zenodo.org/records/24131107).

## The viewer

- **Graph and Query pickers.** The query list shows each example's title,
  description and SPARQL, with Copy and *Edit in console*.
- **Table, Cards, Graph.** Cards show a thumbnail when a column holds image
  links. Search, per-column filters and sorting work on the results; they are
  exact up to 10,000 rows, and above that the bar says so and can write the
  filter into the query instead.
- **Graph view.** Force, hierarchical, radial, circle, grid, tree, hive plot,
  circos and chord layouts. It can group nodes by class, namespace, community
  or component, colour and size them by degree, and expand a node to its
  neighbours. It has a legend, a minimap, zoom, fullscreen, and export as PNG,
  SVG or JSON. It draws up to 1,500 nodes and 3,000 edges.
- **Side panel.** Examples, the SPARQL console, the schema, and the Dataset
  Card: attribution, licence, provenance and statistics. On Zenodo the card is
  merged with the record's metadata, and both are shown where they disagree.

## Turning it off

On Hugging Face, GitHub and Zenodo, right-click the page and use *rete viewer*
to turn the viewer off for that page, that repository (dataset, bucket or
record), or the whole site. The site is restored at once. The popup lists what
is off and turns it on again.

## Reads and caching

The engine reads lazily over HTTP `Range`. Zenodo limits how many requests a
client makes, so Retelens reads Zenodo files in 1 MiB chunks and waits out a
`429`. Chunks are kept in the browser (IndexedDB, up to 1 GiB), so a second
visit to the same file costs one request. The popup's *Advanced* section sets
the chunk size per site and clears the cache.

## Privacy and permissions

Retelens has no server and collects nothing. Queries run in the browser, and
files are read directly from the site that hosts them.

| Permission | Why |
|---|---|
| huggingface.co, hf.co, github.com, raw.githubusercontent.com, zenodo.org | Show the viewer in those pages and read their files (Zenodo's file URLs send no CORS headers) |
| All sites (optional, asked for) | Read a `.rete` file from any other site, and mark `.rete` links there |
| `declarativeNetRequestWithHostAccess` | Open a `.rete` link in the viewer instead of downloading it |
| `downloads` | The viewer's Download button |
| `scripting` | Load the graph view into a page only when it is first opened |
| `contextMenus`, `storage` | The right-click menus; settings, recent files and the off switches |

## Limits

- It reads the formats its engine reads (0x05 to 0x08). An older file shows an
  error that names its format.
- Private and gated files are not supported: no credentials are passed to the
  engine.
- Reads are serial, and nothing cancels a running query.
- A CONSTRUCT or DESCRIBE without a `LIMIT` on a file over 5 MB asks before it
  runs.
- The page integrations follow each site's markup. A redesign of Hugging Face,
  GitHub or Zenodo can break them until Retelens is updated; the standalone
  viewer is not affected.
