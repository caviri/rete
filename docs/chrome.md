# Chrome — Retelens

Retelens is a browser extension for Chrome and Brave that opens `.rete` files where they are hosted.
It also opens PMTiles maps, and on request turns a web page's schema.org data into a graph.
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
2. Open `chrome://extensions` (in Brave, `brave://extensions`) and turn on **Developer mode**.
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
| Any link ending in `.pmtiles` | A map of the archive instead of a download (see [PMTiles](#pmtiles)) |
| Any web page, when you ask | *Read this page's data* turns the page's schema.org data into a graph (see [A page's data](#a-pages-data)) |

On sites other than these, Retelens asks for access before it reads a file.
The toolbar button opens a URL or a local file. Right-click a `.rete` link and
choose *Preview with rete* to open it.

Try it on [katospiegel/rete-graphs](https://huggingface.co/datasets/katospiegel/rete-graphs),
[a GitHub file](https://github.com/caviri/rete/blob/main/docs/plaza/data/linked-jazz.rete)
or [Zenodo record 24131107](https://zenodo.org/records/24131107).

## The viewer

- **Several graphs.** When a dataset, repository, record or bucket has more
  than one `.rete` file, Retelens opens none of them: it lists them and asks
  which one to open. **About** shows a file's card (its title, description,
  statements, licence, keywords) by reading only that card, a few kB; **Open**
  opens it. The file the README names as its default (below) comes first,
  marked, its card already shown; a box filters a long list. A link that
  names a file (`?file=` in the Data Studio, a file row on Zenodo) opens that
  one directly, and the Graph picker switches between them afterwards.
- **Graph and Query pickers.** The query list shows each example's title,
  description and SPARQL, with Copy and *Edit in console*.
- **Table, Cards, Graph, Map.** Search, per-column filters and sorting work on
  the results; they are exact up to 10,000 rows, and above that the bar says so
  and can write the filter into the query instead.
- **Column statistics.** Under each column's name, as in Hugging Face's dataset
  viewer: a histogram with the minimum and maximum for numbers and dates, one
  bar per value when a column has few of them, the namespaces of a column of
  IRIs, the string lengths of other text, and the share of empty cells. Hover
  a bar to read its range or value and count; click it to filter the column to
  it, and again to clear. They count every row up to 10,000, else the first
  rows, which a tooltip says.
- **Cells.** A value is drawn for what it is, in the table and in the cards: an
  image as a thumbnail (a larger copy on hover, full size on click), a IIIF
  manifest as its first page with the other pages a click away and the
  manifest's metadata, a WKT geometry as a small locator map that opens a full
  map, audio and video as players, a PDF or a web page as a link. Each column's
  menu has *Show as* to force a type (Text, Link, Button, Image, IIIF, Map,
  Audio, Video, PDF, Page preview, Markdown, Number); the cards' *Fields* dialog
  sets the same and hides fields.
- **Cards.** The first picture fills the top of the card and the fields follow,
  each name above its value. A click opens the card whole, with the others on
  either side: arrow keys, swipe or drag move between them.
- **Map.** Offered when a result has a WKT column, or latitude and longitude
  columns: up to 2,000 rows on one map, with a popup per place. The basemap is
  OpenStreetMap (ODbL) as a PMTiles archive in the Protomaps layout: streets,
  water, land use and places, a world extract to zoom 12 (18 GB), on
  `data.graphplaza.com/osm-basemap/` and in the
  [rete-public](https://huggingface.co/buckets/katospiegel/rete-public) bucket.
  Like any file it is read by HTTP `Range`, only the tiles in view and only
  when a map is open, and drawn in the Protomaps light or dark style. The
  popup's *Advanced* section takes the URL of another archive.
- **Graph view.** Force, hierarchical, radial, circle, grid, tree, hive plot,
  circos and chord layouts. It can group nodes by class, namespace, community
  or component, colour and size them by degree, and expand a node to its
  neighbours. It has a legend, a minimap, zoom, fullscreen, and export as PNG,
  SVG or JSON. It draws up to 1,500 nodes and 3,000 edges.
- **Query hints.** Each query in the lists shows the view that suits it (Map,
  Table, Cards, Graph) and what its columns hold, and opens in that view. A
  query states them in comments, so every SPARQL tool ignores them; without a
  `view:` line the view is guessed from the query (a WKT geometry: Map; a
  CONSTRUCT: Graph; pictures: Cards). See
  [Hints in example queries](dataset-card-reference.md#hints-in-example-queries).
- **Layers.** A column of `.pmtiles` URLs (a raster such as an elevation model,
  or vector tiles) is drawn in the Map view between the basemap and the
  result's shapes, with a switch; in the table it is a *Map layer* button.
- **Side panel.** Examples, the SPARQL console, the schema, and the Dataset
  Card: attribution, licence, provenance and statistics. On Zenodo the card is
  merged with the record's metadata, and both are shown where they disagree.
- **SPARQL console.** The query is coloured as in the query block (keywords,
  IRIs, literals, variables, comments) and its lines are numbered;
  `Ctrl`/`Cmd`+`Enter` runs it, `Tab` indents.
- **While a file opens** the viewer says so (the engine starts, then reads
  the file's header and card). On a Hugging Face dataset with no file its own
  viewer can turn into a table, Retelens takes the viewer box at once instead
  of first waiting to see whether that viewer shows one.

## The example: the comarca de Antequera

The viewer's start page and the popup open an example, also the default file of
the [katospiegel/rete-graphs](https://huggingface.co/datasets/katospiegel/rete-graphs)
dataset: the comarca de Antequera (Málaga) with El Chorro and the Desfiladero
de los Gaitanes, Fuente de Piedra and El Torcal. It has every kind of
geospatial data a graph can hold: polygons (the comarca, its municipalities,
the protected natural areas, Natura 2000 sites and wetlands), lines (trails,
the Caminito del Rey, rivers), points (towns, peaks, viewpoints) and a raster
(an elevation model as a PMTiles file the graph points to, and the elevation of
every polygon and point computed from it). Its twelve example queries carry
hints, so each opens in its view. Sources: REDIAM (Junta de Andalucía),
OpenStreetMap, Wikidata and the Copernicus DEM, credited in the file's card.

A dataset README marks a default file the way it chooses Hugging Face's
default subset: a `configs` entry with `default: true` whose `data_files`
name the `.rete` file. It comes first in the list of graphs, with its card.

## PMTiles

A `.pmtiles` link (or file dropped on the viewer) opens as a map: the archive's
header and metadata next to it (tile type, zooms, bounds, attribution), each
vector layer in its own colour with its fields, and the features under a click.
Raster archives are drawn as images, over the open basemap; an archive in the
Protomaps basemap layout gets the Protomaps style. Tiles are read by HTTP
`Range`, a few at a time.

The same map replaces the "binary file" box on a `.pmtiles` file page: a
Hugging Face dataset or bucket file, and a GitHub file (one kept with Git LFS is
read from its `media.githubusercontent.com` address, where its bytes are).

A `.rete` file can also carry a PMTiles archive as a section of its own
([`scripts/embed_tiles.py`](https://github.com/caviri/rete/blob/main/scripts/embed_tiles.py),
as in the playground's `geoadmin-tiles`). The Map view then draws those tiles
under the result, and the features whose properties match a value of the
result are drawn on top.

## A page's data

Many pages describe what they show for search engines: a recipe, a product, an
event, an article, in schema.org JSON-LD or microdata. Right-click a page and
choose *Read this page's data* (or use the toolbar button, or a keyboard
shortcut you set in `chrome://extensions/shortcuts`). A new tab shows:

- **Reader**: the things the page describes, without the page around them. A
  recipe reads as its photo, times, yield and rating, the ingredients and the
  steps in the page's order; reviews are folded away. The page's own site and
  breadcrumbs are under *About the page itself*.
- **Explore the graph**: the same data as a `.rete` file built in the tab, with
  the viewer: starter queries, Table, Cards, Graph and the SPARQL console.
- **Download .rete** and **JSON-LD**.

Retelens reads the page only when you ask, and nothing leaves the browser. It
reads JSON-LD, microdata, and Open Graph and description meta tags (the page
itself, as one node). JSON-LD is read as written, with two repairs it lists
under *What was read*: schema.org's context is used inline (it is never
fetched), and relative links resolve against the page's address.

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
files are read directly from the site that hosts them. Pictures, IIIF manifests
and media in a result load from the sites they name; a map reads its basemap
archive (by default from `data.graphplaza.com`) when it is opened.

| Permission | Why |
|---|---|
| huggingface.co, hf.co, github.com, raw.githubusercontent.com, zenodo.org | Show the viewer in those pages and read their files (Zenodo's file URLs send no CORS headers) |
| All sites (optional, asked for) | Read a `.rete` file from any other site, and mark `.rete` links there |
| `declarativeNetRequestWithHostAccess` | Open a `.rete` link in the viewer instead of downloading it |
| `downloads` | The viewer's Download button |
| `scripting` | Load the graph view and the maps into a page only when they are first opened; read a page's data when you ask |
| `activeTab` | *Read this page's data*: access to the page you are on, only when you choose it |
| `contextMenus`, `storage` | The right-click menus; settings, recent files and the off switches |

## Limits

- It reads the formats its engine reads (0x05 to 0x08). An older file shows an
  error that names its format.
- Private and gated files are not supported: no credentials are passed to the
  engine.
- Reads are serial, and nothing cancels a running query.
- On GitHub, the page's rules can block pictures from other sites; Retelens
  says so and offers the file in its own viewer, where they load.
- Tiles inside a `.rete` are found in format 0x05 files only: the later formats
  keep no section directory for `embed_tiles.py` to add to.
- *Read this page's data* does not read RDFa yet, nor JSON-LD remote contexts
  other than schema.org's (their terms are read in the context's namespace).
  Browser pages and the extension stores cannot be read.
- `.pmtiles` links open in the viewer, but pages are not scanned for them as
  they are for `.rete` names.
- A CONSTRUCT or DESCRIBE without a `LIMIT` on a file over 5 MB asks before it
  runs.
- The page integrations follow each site's markup. A redesign of Hugging Face,
  GitHub or Zenodo can break them until Retelens is updated; the standalone
  viewer is not affected.
