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

The LUNA search HTML/API paths (`/luna/servlet/search*`, `/luna/servlet/as*`)
are robots-disallowed and sit behind a reCAPTCHA wall. The **IIIF endpoints are
the sanctioned machine lane** (robots-clean, no captcha) and expose everything:

| What | URL |
|---|---|
| Collection paging (10 items/page) | `/luna/servlet/iiif/collection/s/<token>/<page>` |
| Item manifest (all 27 metadata fields) | `/luna/servlet/iiif/m/<id>/manifest` |
| Image info | `/luna/servlet/iiif/<id>/info.json` |
| Image tiles/derivatives (IIIF Image API level 2) | `/luna/servlet/iiif/<id>/<region>/<size>/<rot>/default.jpg` |

The `<token>` is minted per search session (one call to `/luna/servlet/as/search`
returns its `iiifCollection` URL); if that call hits the captcha, grab the token
from a browser session and pass `--token`.

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
    enum_pages.jsonl          # per-page enumeration state (resumable)
    items_index.tsv           # <id>\t<title> — all 150,017 items
    manifest_urls.txt         # one IIIF manifest URL per item
    manifests/<xx>/<id>.json.gz   # one gzipped IIIF manifest per item (~600MB)
    derived/rumsey_items.jsonl.gz # flattened metadata, one JSON/item
    assets/size{0..4}.tsv     # <relpath>\t<url> per tier
    assets/jp2_masters.tsv    # <relpath>\t<url> full-res masters
    images/size0/<path>.jpg   # ~96px thumbs, all items (~1.5GB)
    images/size2/<path>.jpg   # ~768px, all items (~25GB)
  scripts/
    download.sh               # staged orchestrator (enum→manifests→extract→thumbs→size2)
    enumerate_iiif.py         # IIIF collection pagination → items_index.tsv
    harvest_manifests.py      # 150k manifests, resume-safe, gzipped
    extract_metadata.py       # manifests → JSONL + per-tier URL TSVs
    fetch_tiles.py            # <relpath>\t<url> TSV downloader (resume/atomic)
    inspect.py                # schema/statistics profiler
```

## Dataset shape

One JSONL record per item: `id` (LUNA id, e.g. `RUMSEY~8~1~382463~90148394`),
`title`, `fields` (label → list of values; 27 distinct labels incl. Author(s),
Date, Publisher, Type, City, Scale, Obj/Pub dimensions, List No, Pub List No,
Pub Note, Image No), `width`/`height` (master pixels), `iiif_image` (Image API
base), `thumbnail`, `detail_url`, `image_path`, `jp2_url`.
Run `scripts/inspect.py` after the harvest for fill rates + distributions
(paste its output here).

## Reproduce

```bash
bash data/davidrumsey-maps/scripts/download.sh            # all default stages
bash data/davidrumsey-maps/scripts/download.sh manifests  # or one stage
MSYS_NO_PATHCONV=1 docker run --rm -v "$PWD:/w" -w //w python:3.12-slim \
    python data/davidrumsey-maps/scripts/inspect.py
```

Every stage is resume-safe (re-run to continue/retry failures). Stages refuse
to start without enough free disk.

## Next step

`davidrumsey.rete` via the rete-from-graph skill (items + fields + IIIF links),
georeferencing enrichment (Allmaps/Georeferencer bboxes for the georeferenced
subset), then an OldMapsOnline-style map-first playground over it.
