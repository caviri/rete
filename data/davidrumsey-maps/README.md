# davidrumsey-maps

Raw snapshot of the **David Rumsey Historical Map Collection** — the complete
online catalog (150,017 items as of 2026-08-03) of 16th–21st century maps,
atlases, globes and charts, served by a LUNA instance at www.davidrumsey.com.
The physical collection lives at the David Rumsey Map Center, Stanford.

- Source page: https://www.davidrumsey.com/luna/servlet/view/all
- License: **CC BY-NC-SA 3.0** — images & descriptive data copyright
  Cartography Associates, free to download and reuse non-commercially with
  attribution (attribution: "David Rumsey Historical Map Collection,
  www.davidrumsey.com")
- Snapshot: IIIF harvest started 2026-08-03 (collection total 150,017)
- Contact/UA used for harvest: `rete-dataset-harvester/1.0`

## How the site is harvested (endpoints)

Three lanes, used in this order:

| Lane | Endpoint | Role |
|---|---|---|
| **Catalog (primary)** | `/luna/servlet/as/search?q=&lc=RUMSEY~8~1&bs=500&os=<n>&sort=Pub_List_No_InitialSort` | 500 records/call: id, displayName, ~38 metadata labels, urlSize0-4, IIIF manifest URL. 301 sequential calls ≈ whole catalog. |
| IIIF collection (fallback) | `/luna/servlet/iiif/collection/s/<token>/<page>` | robots-clean id enumeration, 10/page (slow: server caps per-IP connections). |
| IIIF manifests (archival) | `/luna/servlet/iiif/m/<id>/manifest` | canonical per-item record; adds master pixel width/height. |

Plus per item: `/luna/servlet/iiif/<id>/info.json` and the IIIF Image API
(level 2) at `/luna/servlet/iiif/<id>/<region>/<size>/<rot>/default.jpg`.

Notes: the LUNA search paths are robots-disallowed and occasionally answer
with a reCAPTCHA interstitial — the harvester detects non-JSON, backs off long,
and resumes; batches are sequential by design (parallel connections from one IP
get refused). The IIIF `<token>` is minted per search session (the API response
carries it as `iiifCollection`); if minting hits the captcha, pass `--token`
from a browser session.

Pre-rendered JPEG tiers + masters (URL scheme derived from each record's
"Download 1" field, e.g. `image=/229/18059000.jp2` → path `229/18059000`):

| Tier | ~px | ~bytes/map | ×150k | URL |
|---|---|---|---|---|
| Size0 | 96 | 10KB | ~1.5GB | `www.davidrumsey.com/rumsey/Size0/RUMSEY~8~1/<path>.jpg` |
| Size1 | 192 | 39KB | ~6GB | …`/Size1/`… |
| Size2 | 768 | 157KB | ~25GB | …`/Size2/`… |
| Size3 | 1536 | 623KB | ~80GB | `media.davidrumsey.com/MediaManager/srvr?mediafile=/Size3/RUMSEY~8~1/<path>.jpg` |
| Size4 | 3072 | 2.3MB | ~300GB | …`/Size4/`… |
| JP2 master | full (sample: 26367×24342) | ~100MB | **4–15TB** | `www.davidrumsey.com/static/jp2k/<path>.jp2` (302 from `/rumsey/download.pl?image=/<path>.jp2`) |
| Export API | full-res JPEG | — | — | `/luna/servlet/detail/export?mediaId=<id>&xres=7` (browser-facing; robots-disallowed — prefer IIIF/static) |

This snapshot: **full metadata + Size0 + Size2 for every item**; deep zoom
stays live via the IIIF Image API. `size3`/`masters` stages exist but are
opt-in (masters only ever for curated subsets — the full set is TB-scale).

## Layout

```
data/davidrumsey-maps/
  README.md
  SHA256SUMS.txt
  raw/
    catalog/os_<offset>.json.gz   # one gzipped API batch of 500 (~300MB total)
    items_index.tsv           # <id>\t<title> — all 150,017 items
    manifest_urls.txt         # one IIIF manifest URL per item
    manifests/<xx>/<id>.json.gz   # OPTIONAL archival lane (~600MB)
    derived/rumsey_items.jsonl.gz # flattened metadata, one JSON/item
    assets/size{0..4}.tsv     # <relpath>\t<url> per tier
    assets/jp2_masters.tsv    # <relpath>\t<url> full-res masters
    images/size0/<path>.jpg   # ~96px thumbs, all items (~1.5GB)
    images/size2/<path>.jpg   # ~768px, all items (~25GB)
  scripts/
    download.sh               # staged orchestrator (catalog→extract→thumbs→size2)
    harvest_catalog.py        # PRIMARY: LUNA API batches of 500 → catalog/ (~301 calls)
    harvest_cycle.sh          # trails a running catalog harvest with extract+fetch cycles
    enumerate_iiif.py         # FALLBACK: IIIF collection pagination → items_index.tsv
    harvest_manifests.py      # ARCHIVAL: 150k IIIF manifests, keep-alive, gzipped
    extract_metadata.py       # catalog (+manifest overlay) → JSONL + per-tier URL TSVs
    fetch_tiles.py            # <relpath>\t<url> TSV downloader (keep-alive/resume/atomic)
    inspect.py                # schema/statistics profiler
```

## Dataset shape

One JSONL record per item: `id` (LUNA id, e.g. `RUMSEY~8~1~382463~90148394`),
`title`, `fields` (label → list of values; ~38 distinct labels incl. Author(s),
Date, Publisher, Type, City, Country, Region, State/Province, World Area,
Subject, Event, Scale, Obj/Pub dimensions, List No, Pub List No, Pub Note,
Image No), `width`/`height` (master px, when manifests harvested), `iiif_image`
(Image API base), `iiif_manifest`, `detail_url`, `image_path`, `jp2_url`,
`url_size0..4`.
Run `scripts/inspect.py` after the harvest for fill rates + distributions
(paste its output here).

## Reproduce

```bash
bash data/davidrumsey-maps/scripts/download.sh            # all default stages, sequential
bash data/davidrumsey-maps/scripts/download.sh manifests  # or one stage
MSYS_NO_PATHCONV=1 docker run --rm -v "$PWD:/w" -w //w python:3.12-slim \
    python data/davidrumsey-maps/scripts/inspect.py
```

Faster wall-clock: start `download.sh catalog` and, in a second terminal,
`bash data/davidrumsey-maps/scripts/harvest_cycle.sh` — it repeatedly
re-extracts and sweeps the image tiers while the catalog is still landing,
and exits when both have converged.

Every stage is resume-safe (re-run to continue/retry failures). Stages refuse
to start without enough free disk. Two hard-won operational notes:
- **Keep-alive or crawl**: per-request TLS handshakes cap ~0.5 files/s; the
  persistent-connection fetchers sustain ~20/s at 6 workers.
- **One connection for the API lane**: /luna/servlet refuses parallel
  connections per IP; batch sequentially there, parallelise only against the
  static image hosts.

## Next step

`davidrumsey.rete` via the rete-from-graph skill (items + fields + IIIF links),
georeferencing enrichment (Allmaps/Georeferencer bboxes for the georeferenced
subset), then an OldMapsOnline-style map-first playground over it.
