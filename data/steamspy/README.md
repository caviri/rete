# steamspy — Steam games catalogue (SteamSpy + Steam Store appdetails)

A SteamDB-equivalent games dataset assembled from **open Steam APIs**.
`steamdb.info` itself is Cloudflare-protected and scraping-hostile, so this goes
to the primary/official sources instead.

## Sources & license

| layer | source | what |
|---|---|---|
| index + stats | **SteamSpy** `api.php?request=all` | one row per tracked game: owners, review counts, playtime, price, CCU |
| rich metadata | **Steam Store** `store.steampowered.com/api/appdetails` | type, description, genres, categories, developers/publishers, release date, platforms, price, Metacritic, screenshots, movies, DLC… |

> `api.steampowered.com/ISteamApps/GetAppList/v2` returns **404** in this
> environment (blocked / method-not-found), so **SteamSpy `all` is the app
> index**. It covers every game SteamSpy tracks (owners > 0), which is the
> meaningful "games" universe — the full GetAppList is mostly DLC/tools/videos.

**License / attribution.** Steam store data is © **Valve**; SteamSpy stats are
from **steamspy.com**. Both are public APIs of *factual* game metadata, but
neither is openly licensed — so any published `.rete` should be
**metadata-only, attribution-required** (cite Steam/Valve + SteamSpy), not CC0.
Requests are rate-limited politely (SteamSpy 1/60 s; Store ~1/1.6 s).

Snapshot date: **2026-07-30** (harvest in progress; dated by file mtimes / `SHA256SUMS.txt`).

## Layout

```
data/steamspy/
  raw/
    steamspy/all_page{NNNN}.json   # 1000 games/page; the index + core stats
    appdetails/{appid}.json        # the appdetails `data` object per game
                                   #   (or {"success": false} for delisted/region-locked)
  scripts/
    fetch_steamspy.py              # paginate SteamSpy `all` (resumable, 1/60s)
    fetch_appdetails.py            # harvest appdetails per appid (resumable, ~1/1.6s)
    download.sh                    # orchestrator: steamspy -> appdetails (Docker)
    inspect.py                     # schema profiler (pending — run after data lands)
  README.md · SHA256SUMS.txt
```

## Schema

**SteamSpy `all` row** (`raw/steamspy/*.json`, keyed by appid):
`appid, name, developer, publisher, score_rank, positive, negative, userscore,
owners` (a range string e.g. `"1,000,000 .. 2,000,000"`)`, average_forever,
average_2weeks, median_forever, median_2weeks, price, initialprice, discount, ccu`.

**Steam appdetails `data`** (`raw/appdetails/{appid}.json`): `type, name,
steam_appid, is_free, dlc[], detailed_description, short_description, about_the_game,
supported_languages, developers[], publishers[], price_overview{}, packages,
platforms{windows,mac,linux}, categories[], genres[], release_date{}, metacritic{},
recommendations{}, achievements{}, screenshots[], movies[], support_info{}, …`.

## Reproduce

```bash
bash data/steamspy/scripts/download.sh   # runs both layers, resumable
```
Both harvesters skip files already on disk, so a killed run just continues.
`data/` is gitignored — the scripts *are* the dataset in the repo.

## Status (2026-07-30)

- **SteamSpy index — complete:** 87 pages, **86,544 games** (`raw/steamspy/all_page*.json`).
- **Steam appdetails — downloading:** ~20.7k / 82.5k fetched (~325 MB so far),
  **~98%** return a `data` object; the ~2% `{"success": false}` are delisted /
  region-locked apps. Resumable (skips files already on disk); ~1–2 days total.

The `.rete` build is a later step (`rete-from-graph`): a games graph — games with
their genres/categories/tags, developer/publisher agents, DLC/base-game edges,
and the SteamSpy ownership/review/playtime stats as datatype properties. Its indie
counterpart is `data/itch-io/` (itch.io catalogue).
