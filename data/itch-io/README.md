# itch-io — itch.io indie game catalogue

The indie-marketplace counterpart to `steamspy`, from **open, robots-allowed
itch.io sources**. itch.io has **no public bulk-metadata API** (its serverside
API is OAuth/per-developer), so this uses the two open surfaces instead.

## Sources & license

| layer | source | what |
|---|---|---|
| **complete index** | **official sitemap** `itch.io/sitemap.xml` → 39 `games*.xml` | ~**1.95 M** game URLs (author subdomain + slug live in the URL) |
| **metadata** | **browse feed** `itch.io/games/<sort>?format=json&page=N` | per-game cells: game_id, URL, title, author, price, platforms, genre, cover |

> `robots.txt` **allows `/games`** (disallows only `/search`, `/embed*`, and the
> `/*/download/` routes) and publishes the sitemap — so both layers are the
> sanctioned way in, no scraping of blocked paths. Browse pages return a JSON
> envelope whose `content` is an HTML fragment of ~36 `game_cell` blocks.

**License / attribution.** itch.io games are published by individual creators
under varied per-game licenses; the *catalogue metadata* here is factual. Any
published `.rete` should be **metadata-only, attribution-required** (itch.io +
the creators), **not CC0**. Requests are polite (~1/1.2 s).

## Layout

```
data/itch-io/
  raw/
    sitemap/games*.xml         # the 39 raw game sub-sitemaps
    game_urls.txt              # ~1.95M game URLs, deduped (the index)
    browse/newest_page{NNNNN}.json   # {page,num_items,content} — metadata cells
  scripts/
    fetch_sitemap.py           # index (fast, ~2 min)
    fetch_browse.py            # metadata cells, paginated + resumable
    download.sh                # orchestrator (Docker): sitemap -> browse
  README.md · SHA256SUMS.txt
```

## Schema

- **game_urls.txt**: one URL per line, `https://<author>.itch.io/<slug>` (author
  + slug are the natural keys; author is the subdomain).
- **browse cell** (parsed from `content`): `game_id, url, title, author, price`
  (absent = free), platform icons (windows/mac/linux/android/web), `genre`,
  cover image URL, short blurb.

## Reproduce

```bash
bash data/itch-io/scripts/download.sh   # sitemap then browse, resumable
```
Both harvesters skip files already on disk. `data/` is gitignored — the scripts
*are* the dataset in the repo.

## Status

**Downloading** — sitemap index (~2 min, complete) then browse metadata (long,
resumable). The `.rete` build is later (`rete-from-graph`): an indie-games graph
(game → author/creator, genre/tag, platform; price/free as datatype props),
cross-linkable with `steamspy` on title/author where games ship on both stores.
