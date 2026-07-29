# wikiart

Raw snapshot of **WikiArt.org** — the visual-art encyclopedia — harvested from its
keyless public JSON API. Intended as the first pillar of a large art knowledge
graph.

- Source: <https://www.wikiart.org> (API docs: <https://www.wikiart.org/en/App/GetApi>)
- Snapshot: **2026-07-26**
- Access: no API key needed for the endpoints used here (see *Endpoints* below)
- `robots.txt`: `User-agent: * / Allow: /` with a published sitemap — crawling is
  permitted; the harvest still goes through the JSON API rather than scraping HTML.

## Licence — read before republishing

WikiArt publishes **no open licence** for its corpus, and the material is mixed:

| Layer | Status |
| --- | --- |
| Factual metadata (artist, title, year, style, genre, medium, dimensions, holding gallery) | Facts — not copyrightable in themselves; the *selection/arrangement* may attract database rights in some jurisdictions |
| `description` prose | **Original text written for WikiArt** — copyrighted, not freely licensed |
| Images of public-domain works | Public domain (the underlying work); WikiArt asserts nothing extra |
| Images of in-copyright works | Shown by WikiArt under a **fair-use** rationale — "low resolution copies unsuitable for commercial use" ([Terms](https://www.wikiart.org/en/terms-of-use), updated 2016-10-05) |

Consequences for a published `.rete`:
- The factual layer is the safe core to publish, with attribution.
- The `description` prose should **not** be redistributed wholesale — link to the
  WikiArt page instead, or keep descriptions local-only.
- Images are **referenced by URL, not mirrored** (see *Assets*).
- Attribution string: `Data from WikiArt.org (https://www.wikiart.org), harvested 2026-07-26.`

## Layout

```
data/wikiart/
  README.md
  SHA256SUMS.txt
  raw/
    sitemap/                     16 sitemap XMLs — the authoritative inventory
    dictionaries/                13 controlled vocabularies (group-NN-<slug>.json)
    categories/                  6 facet vocabularies w/ artist counts +
                                 group labels in 30 languages
    artists_alphabet.json        COMPLETE artist list (numeric contentId)
    artists.jsonl                rich artist records (Mongo id, biography, gender)
    artists_recovered.jsonl      Mongo ids recovered for artists past the API wall
    imagejson_failures.json      per-id failure counts (retires dead ids)
    paintings_app.jsonl          COMPLETE painting inventory (numeric contentId)
    painting_index.jsonl         painting Mongo ids (enrichment layer)
    paintings.jsonl              v2 detail: description, tags[], galleries[]
    paintings_imagejson.jsonl    App detail: dictionaries[], auction/price, technique
    assets/
      images.urls.txt            image URL manifest (223,094 URLs)
      all_assets.tsv             painting_id -> image URL, with dimensions
      webp/<2hex>/<cid>.webp     1200px WebP mirror, sharded 256 ways
      webp_manifest.tsv          per-image: variant used, bytes in/out, w, h
  scripts/                       the harvest (committed; raw/ is gitignored)
```

## Scale

359 MB on disk, 45 files, all checksummed in `SHA256SUMS.txt`.

| | sitemap declares | harvested | status |
| --- | --- | --- | --- |
| paintings (inventory) | 221,558 | **223,095** | ✅ complete (exceeds sitemap — it lags) |
| painting detail (App/ImageJson) | — | **223,094** | ✅ complete (1 id absent from WikiArt itself) |
| image URLs | — | **223,094** | ✅ 100% |
| WebP image mirror (1200px) | — | **223,082** | ✅ 99.995%, 23.4 GB |
| artists | 5,761 slugs | **5,755** | ✅ complete |
| artist rich records (v2) | — | 5,100 + 428 recovered | ✅ **96.1%** Mongo-id coverage |
| v2 dictionaries | — | **2,681** across 13 groups | ✅ |
| facet vocabularies + counts | — | **725** across 6 facets | ✅ group labels in 30 languages |
| painting detail (v2 prose) | — | 0 | ○ optional, needs an API key |
| albums (curated collections) | 180,387 | 0 | ○ future |

Coverage spans **-3050 to 2025** — Ancient Egyptian to contemporary.

### Field fill on the 223,094 detail records

`style` 91.2% · `genre` 90.4% · `dictionaries[]` 94.8% · `tags` 65.8% ·
`sizeX`/`sizeY` 24.5% · `galleryName` 21.0% · `location` 12.4% · `serie` 8.0% ·
`period` 4.1% · `description` 2.4% · `auction` 0.1%

Top styles: Realism (18,511), Romanticism (17,953), Impressionism (16,277),
Expressionism (11,626), Baroque (9,670).
Top genres: portrait (29,018), genre painting (24,297), landscape (24,119),
abstract (18,739).
Top holdings: Private Collection (16,242), Tretyakov (1,419), Hermitage (983),
Louvre (943), the Met (821).

### Field fill on the 5,100 rich artist records

`gender` 88.8% (3,964 male / 563 female) · `relatedArtists` 63.9% ·
`wikipediaUrl` 60.5% · `biography` 44.3% · `activeYears*` 10.9%

## The two JSON surfaces — and which one to rely on

WikiArt exposes the same corpus through two surfaces with **different,
non-interchangeable identifiers**. They join on `(artistUrl, url)` — the slug
pair. The decisive difference is that **only one of them is metered**.

| | `/en/App/*` (site's own AJAX) | `/en/api/2/*` (documented v2 API) |
| --- | --- | --- |
| id | numeric `contentId` | 24-hex Mongo id |
| **quota** | **none observed — unmetered** | **metered**: keyless use dies with `500 {"Message":"Free API limit exceeded"}` |
| pagination | none — whole list per request | opaque `paginationToken`, 60/page |
| completeness | **complete** | partial — quota + *the wall* (below) |
| unique fields | `dictionaries[]` (multi-valued vocabulary links), `auction`/`yearOfTrade`/`lastPrice`, `technique`, `material` | `description` prose, `tags[]`, `galleries[]`, `styles[]`/`genres[]`/`media[]` as arrays, artist `biography`/`gender`/`relatedArtists` |

**The App layer is the spine of this harvest.** It is unmetered, slug-driven,
and covers 100% of artists and paintings including per-painting detail. The v2
layer is treated as *optional enrichment*: every v2 phase is quota-guarded, fails
fast, and never overwrites good data with nothing.

Getting a free API key (<https://www.wikiart.org/en/App/GetApi/GetKeys>, needs a
WikiArt account) would lift the v2 quota and make the enrichment phases
practical at 221k-request scale. Not done here — the App layer already carries
the graph.

`contentId` is also the id used by the third-party WikiArt image dumps
(ArtGAN / `huggan/wikiart`), so it is the join key to those if images are wanted
in bulk.

`contentId` is also the id used by the third-party WikiArt image dumps
(ArtGAN / `huggan/wikiart`), so it is the join key to those if images are wanted
in bulk.

## Endpoints used

| Endpoint | Yields |
| --- | --- |
| `/sitemap/sitemap_index.xml` → 16 shards | inventory ground truth |
| `/en/api/2/DictionariesByGroup?group=N` | vocabularies; groups 1–3, 7–16 are populated (0, 4–6, 17+ are empty) |
| `/en/App/Artist/AlphabetJson?v=new` | complete artist list |
| `/en/api/2/UpdatedArtists` | rich artist records (paginated) |
| `/en/App/Painting/PaintingsByArtist?artistUrl=<slug>&json=2` | an artist's complete oeuvre |
| `/en/api/2/PaintingsByArtist?id=<artistMongoId>` | painting Mongo ids (paginated) |
| `/en/api/2/Painting?id=<mongoId>` | richest painting record |
| `/en/App/Painting/ImageJson/<contentId>` | painting record for *every* painting |
| `/en/artists-by-<facet>?json=2` | facet vocabularies: `Dictionaries[]` (slug, title, artist `Count`) + `Categories[]` (group headings in 30 languages). Facets: `art-movement`, `nation`, `century`, `field`, `painting-school`, `art-institution` |

Dictionary groups: 1 periods, 2 **styles**, 3 genres, 7 art movements,
8 galleries, 9 auctions, 10 nationalities, 11 fields, 12 media,
13 art institutions, 14 series, 15 countries, 16 misc.

## Gotchas (all verified against the live API)

1. **`/en/api/2/*` is quota-metered for keyless use.** Once spent, *every* v2
   endpoint returns `HTTP 500 {"Exception":{"Message":"Free API limit
   exceeded"}}` — indistinguishable from a transient 500 except by the body, so
   naive retry logic hammers a wall it cannot pass. `_wa.py` parses the body and
   raises `QuotaExceeded` immediately. The quota appears to reset with time;
   `/en/App/*` is unaffected throughout. Budget the v2 layer carefully, or get a
   key.
2. **Pagination tokens are already percent-encoded.** `paginationToken` comes
   back as base64 with `+`, `/`, `=` written `%2b`, `%2f`, `%3d`. Passing it
   through `urlencode` re-encodes the `%` to `%25` and the next request 500s.
   It must be appended to the URL verbatim.
3. **`UpdatedArtists` hits a hard wall.** Past ~5,100 artists a page 500s with
   *"Year, Month, and Day parameters describe an un-representable Date"* — one
   artist has a birth/death date .NET cannot serialise, poisoning that page of
   60. It is deterministic, and there is no way past it:
   `fromDate` parses only as an epoch integer and is then **inert** (every value
   returns the identical first page); every ISO/US/.NET date form 500s with
   *"Failed to parse date"*; `page=N` is ignored on all v2 endpoints;
   `/en/api/2/Artist?id=` and `/en/App/Artist/ArtistJson/<id>` both 302 to an
   error page. The slug layer is unaffected, so it is used as the spine and the
   missing Mongo ids are recovered via `PaintingSearch` on the artist's name.
4. **`/en/api/2/Painting?id=` accepts only the Mongo id.** A numeric `contentId`
   returns `404 Painting not found`.
5. **Two dictionary id spaces.** The v2 vocabularies are keyed by Mongo id,
   but `AlphabetJson` and `ImageJson` reference vocabularies by **numeric** id
   (`"dictionaries":[465,1192]`). No endpoint was found that serves the numeric
   vocabulary (`/en/App/Dictionary/*` all 302 to an error page). The flat
   `style`/`genre`/`material`/`technique` label fields on every ImageJson record
   carry the same information in readable form, so the numeric→label mapping can
   be inferred from co-occurrence across the corpus if the multi-valued links are
   wanted. Open item.
6. **ASP.NET dates.** Dates serialise as `/Date(1234567890000)/` (ms since
   epoch, may be negative). `deathDay` for living artists is the sentinel
   `/Date(253402300799999)/` = 9999-12-31, **not** null — treat it as "alive".
7. **`dictonaries` is misspelled** in `AlphabetJson` (`dictonaries`), but spelled
   `dictionaries` everywhere else.
8. **`completitionYear`** (sic) is the completion-year field throughout.
9. Image URLs carry a size token before the extension —
   `...the-starry-night-1889.jpg!Large.jpg`. Stripping `!Large.jpg` gives the
   original upload; other tokens (`!Portrait.jpg`, `!PinterestSmall.jpg`) are
   server-side derivatives.
10. **A few ids exist in a listing but have no detail document.** `ImageJson`
    302s them to an error page that itself answers **500**, so `get()` cannot
    tell them from a transient failure and retries forever. `harvest_imagejson.py`
    keeps a persistent per-id failure counter (`imagejson_failures.json`) and
    retires an id after 3 whole failed runs. Exactly one id is affected across
    the corpus: `197943` ("The Death of Marat", Jacques-Louis David).
11. **Image URLs keep their diacritics and must be percent-encoded.** Slugs like
    `the-town-hall-in-mödling-1842.jpg` and `…ortaköy…` appear verbatim in the
    `image` field. `urllib` refuses a non-ASCII URL with `UnicodeEncodeError`,
    which looks like a dead image but is really an unsent request — encoding the
    path returns HTTP 200. This accounted for **every** failure in the first
    37k of the image mirror (0.15%). `mirror_images_webp.py:ascii_url()`.
12. **A few source images are corrupt on WikiArt's side.** The response matches
    its `Content-Length` byte for byte, yet the image data stops early —
    Pillow raises `image file is truncated`, or `UnidentifiedImageError` even
    though the PNG header is valid (`file` reads it as `PNG 3000x1695`). Setting
    `ImageFile.LOAD_TRUNCATED_IMAGES = True` recovers most of them. Not a
    download bug — verified the full read equals the declared length.
13. `artistName` casing is inconsistent between surfaces — v2 sometimes returns
   `"da Vinci Leonardo"` (sort form) where the App layer returns
   `"Leonardo da Vinci"`. Prefer the App form for display, the slug for identity.

## Assets — images

`raw/assets/images.urls.txt` holds one URL per painting (223,094, on
`uploads{0-9}.wikiart.org`); `all_assets.tsv` maps each back to its painting id
and pixel dimensions.

### The variant chain (important)

WikiArt serves size derivatives via a token before the extension. **The `image`
field in the metadata always records the `!Large` URL, but that derivative is a
genuine 404 for roughly a quarter of the corpus** — and `!HD` is missing for
exactly those same works, so only the original gives near-total coverage.
Measured over a 150-URL sample:

| variant | long edge | present | median | est. 223k |
| --- | --- | --- | --- | --- |
| `!HD.jpg` | 1200px | 17.3% | 191 KB | 49 GB |
| `!Large.jpg` | 600px | 72.7% | 58 KB | 14 GB |
| *(no token)* — original | native, up to 5315×4436 | **99.3%** | 164 KB | 87 GB |

Other tokens exist (`!HalfHD`, `!Blog`, `!Portrait`, `!PinterestSmall`);
`!Square` and `!ArtistImage` 404. Any mirror must therefore walk a fallback
chain, not trust the recorded URL.

### The WebP mirror

`scripts/mirror_images_webp.py` fetches `!HD` → original → `!Large`, downscales
anything above a 1200px long edge (LANCZOS, never upscales), and encodes WebP
q80/method 6. It is **streaming**: each image is downloaded into memory and only
the WebP is written, so the ~76 GB of source JPEG never touches disk.

Output is sharded 256 ways — 223k files in one directory is pathological on NTFS:

```
raw/assets/webp/<contentId & 0xff, 2 hex>/<contentId>.webp
raw/assets/webp_manifest.tsv    content_id, variant used, src_bytes, webp_bytes, w, h
raw/assets/webp_failed.txt      ids where no variant resolved
```

**Completed run (2026-07-27):**

| | |
| --- | --- |
| mirrored | **223,082 / 223,094 — 99.995%** |
| source fetched | 54.0 GB (streamed, never written to disk) |
| WebP written | **23.4 GB** — 56.6% smaller |
| mean / median | 103 KB / 80 KB per image |
| variant used | original 169,309 · `!HD` 53,764 · `!Large` 9 |
| long edge | median 970px; 31% hit the 1200px cap |
| throughput | 20.8 img/s at 24 workers (≈3 h wall) |

The 12 that never resolved are all one artist (`erfan-rohina`) whose files are
deleted from WikiArt's storage while the API still lists the works — every
variant 404s. They are listed in `webp_failed.txt`.

```bash
bash data/wikiart/scripts/download.sh 7          # opt-in; resumable
```

Resumable and atomic (`.part` then rename), so re-run until it reports
`0 remaining`. Tunable: `WIKIART_IMG_WORKERS` (24), `WIKIART_WEBP_Q` (80),
`WIKIART_MAX_EDGE` (1200).

**Licence note:** mirroring is fine for local research, but WikiArt shows
in-copyright works under a fair-use rationale for *low-resolution* copies. The
factual layer is the safe thing to republish; the images and the `description`
prose are not.

## Reproduce

```bash
bash data/wikiart/scripts/download.sh            # all phases in order
bash data/wikiart/scripts/download.sh 4b         # or one phase; all are resumable
```

Phases: `0` sitemaps · `1a` v2 dictionaries · `1b` facet vocabularies ·
`2` artists · `3` painting inventory · `4a` v2 detail · `4b` App detail ·
`5` image manifest · `6` profile · `7` WebP image mirror (opt-in, not in `all`).

Phases 4a/4b are ~223k requests each and take hours; both are resumable, so
re-run until they report `0 remaining`. Concurrency is `WIKIART_WORKERS`
(default 12; 24 parallel requests on the App layer measured clean, no rate
limiting observed there).

**Do not edit `download.sh` while it is running.** Bash reads scripts
incrementally from disk by byte offset, so an in-flight edit shifts the parse
position and the tail of the script dies with a bogus syntax error. Run a single
phase directly if you need to change the orchestrator meanwhile:

```bash
MSYS_NO_PATHCONV=1 docker run --rm -v "D:/pro/rete:/w" -w //w \
  -e PYTHONUNBUFFERED=1 python:3.12-slim python data/wikiart/scripts/harvest_imagejson.py
```

## Next step

Model as a graph and build `wikiart.rete` — hand off to the **rete-from-graph**
skill. The natural shape: `Artist` and `Artwork` nodes, with the 13 dictionaries
as SKOS concept schemes, aligned to CIDOC-CRM / Wikidata / Getty AAT+ULAN for
federation with the other art sources.
