# Format 0x06: measurements (WS20, 2026-10-31)

The numbers behind `2026-10-31-format-0x06-design.md`. Scripts: `scripts/format06/`.

## Method

- **Engine:** rete CLI 0.4.2, the `ghcr.io/caviri/rete-cli:0.4.2` image (= origin/main 3469d64c). Every run is a
  cold process: `sparql-url`, `card-url` or `search-url` against a URL.
- **Transport:** nginx (`nginx.conf`, `sendfile off`) behind `lat_proxy.py`, which logs each request's arrival time
  and `Range` and holds it 50 ms before forwarding. Requests in flight together arrive within a few ms; a dependent
  request arrives at least 50 ms after its predecessor. A **round trip (RT)** is a wave: a request that arrives more
  than 25 ms after the first request of the current wave opens a new one. The OC Meta open went through
  `lat_proxy_https.py` to R2 (30 ms added, plus R2's own latency).
- **Modes:** *no cache* is `RETE_BLOCK_KB=0`: one request per logical read. *Cache* is the default client: a
  read-through cache of aligned 64 KiB blocks (files ≤ 100 MB) or 128 KiB (larger), up to 16 requests in flight.
- **Bytes** are the sum of the `Range` lengths the proxy saw. The `HEAD` the CLI sends first counts as one RT and
  zero bytes.
- **Layout maps:** `layout05.py` reads a 0x05 file's framing and directories only and labels every byte:
  `root` (header, container framing, preambles, schema), `dir` (chunk directories, tile directories and synopses,
  text token table, quad directories, label directory nodes), `card`, `meta` (legacy pyramid-meta), `build`, `dead`
  (dictionary restart tables) and `cold` (payload). The regions of each file sum to its size exactly. On the
  published OC Meta it read 40 MB of directories in 77 requests, never a payload byte.
- **0x06 sizes** (`hot06.py`): each 0x05 directory compressed with zstd level 9 (the token table and label nodes are
  already compressed and are counted as they are); leaves of 16 KiB, doubled per unit, largest first, until the root
  is at most 40 KiB; root entries 7–14 B per leaf; a 256 B header and a section table of 24 B per section; the
  front ordered root, card, stats+schema, leaves (dict, index, quad, labels, token table).
- **0x06 simulation** (`sim.py`), from each *no cache* trace: a read that starts in a hot region is served by the
  first read of N bytes; payload reads keep their 0x05 wave and bytes; a payload read whose leaf block lies past N,
  and was not fetched before, adds one RT before its wave (all leaves of a wave in parallel). Variants: *exact*
  (payload read exactly), *cache* (payload through 64/128 KiB blocks, as the default client), each with and
  without *leaf prediction* (the leaf a later wave needs is fetched with the previous payload wave, from the id
  range that wave already knows). **"0x06" in the summary tables is the variant with fewer RT, ties to fewer
  bytes.** A file no larger than N costs one read.
- **Meta projection:** the OC small traces replayed against the 0x06 front of the published Meta file (sizes from its
  directories; dictionary and tile-directory compressibility measured on samples of Meta's own: dict 0.252, tile
  directories + synopses 0.624, `oc-meta-zratios.json`), payload bytes as on the small file.

## Reproduce

```sh
# layouts (local files; a https URL reads only framing + directories)
python layout05.py FILE.rete --json layouts/x.json
# serve + proxy (Docker, 512m each), then the runs
docker network create ws20-net
docker run -d --name ws20-nginx --network ws20-net --memory 512m -v nginx.conf:/etc/nginx/conf.d/default.conf:ro -v <dir>:/data/<x>:ro nginx:alpine
docker run -d --name ws20-proxy --network ws20-net --memory 512m -p 127.0.0.1:18080:8080 -v <scripts>:/p -v <log>:/log python:3.12-slim python3 /p/lat_proxy.py 8080 ws20-nginx 50 /log/proxy50.log
python drive.py jobs_oc.json res/oc.jsonl nocache && python drive.py jobs_oc.json res/oc.jsonl cache
python analyse.py log/proxy50.log res/oc.jsonl,res/cat.jsonl <served>=layouts/x.json ... --rtt 50 --out res/all_an.json
python sim.py res/all_an.json --files files.json --out res/all_sim.json --project "oc/...quad.rete=layouts/oc-meta.json,oc-meta-model.json"
python summarize.py res/all_sim.json files.json
```

Client containers ran with `--memory 4g --cpus 4`; the two `repyramid` rebuilds with `--memory 4g --cpus 6`.

## Files

| file | source | size | why |
|---|---|--:|---|
| `opencitations-2026-06-small-full-labels-quad` | `data/scripts/development/opencitations/out/files-0.4.0` | 204.6 MB | quad index, text (kind 6), labels (8), schema (9) |
| davidrumsey, tree-city-inventory, swissubase | local copies, same content hash as R2 | 76.4 / 25.1 / 45.6 MB | text index, legacy labels in pyramid-meta, pre-#198 chunk keys (davidrumsey, swissubase) |
| mtg, arxiu | R2 (local copies are format 0x04) | 16.3 / 72.2 MB | text index, legacy labels |
| mtg-modern, tci-modern | `rete repyramid --pyramid-algo types --text-index --label-index --card` of mtg and tree-city-inventory, 0.4.2 | 16.4 / 23.8 MB | the 0.4 layout: kinds 8 and 9, separator keys |
| card-full, named-graphs-only, worldcup2026 | gate fixtures (`tests/gate/.cache`) | 20 KB / 14 KB / 1.9 KB | card + build record, kind 5, cardless |
| opencitations-meta | R2, directories only | 62.9 GB | full scale: text (kind 11), labels, schema, data only |

No published catalog file other than opencitations-meta carries kinds 8, 9, 10 or 11 (header survey of all 111
catalog URLs, every one format 0x05).

## OC Meta, measured

| | RT | requests | bytes |
|---|--:|--:|--:|
| 0x05 lazy open, no cache (`SELECT * { ?s ?p ?o } LIMIT 0`) | 131 | 137 | 88,700,886 |
| 0x05 lazy open, cache | 56 | 60 | 69,468,160 |
| Meta's own build record: 8 starter queries, cold, no cache | – | 137–176 | 88.7–89.9 MB |

Directories of the published file (bytes): dict chunk directories 11,038,170 (keys 9,568,236); tile directories
15,525,854; tile synopses 34,762,493; label directory nodes 29,425,655 (root 37,190, one level); text token table
187,930,892; schema 4,375; card 51,896; build record 4,041; restart tables (dead) 589,462,774.

## Dictionary directories re-keyed

`rekey.py` decodes every chunk and recomputes shortest separators, as `rete convert` would:

| file | 0x05 chunk dirs raw → re-keyed | zstd → re-keyed zstd |
|---|--:|--:|
| davidrumsey | 263,694 → 50,268 | 97,692 → 18,354 |
| swissubase | 505,537 → 54,168 | 164,402 → 16,733 |
| arxiu | 114,510 → 59,439 | 39,308 → 17,280 |
| mtg | 35,057 → 9,139 | 12,945 → 4,414 |
| tree-city-inventory | 26,826 → 10,426 | 3,855 → 2,275 |

The re-keyed tree-city-inventory directory is byte-for-byte the size `repyramid` wrote (10,426), which checks the
estimator.

### Hot set per file (KiB)

| file | size MiB | 0x05 hot: dirs+roots / card / token table | 0x05 dead (restart tables) | 0x06 root | card z | schema z | leaves z (w/o token table) | token table | 0x06 front w/o token table |
|---|--:|--:|--:|--:|--:|--:|--:|--:|--:|
| oc-small | 195.1 | 776 / 61 / 3,078 | 1,919 | 3.5 | 11 | 1.9 | 537 | 3,078 | 553 |
| davidrumsey | 72.9 | 284 / 59 / 1,574 | 492 | 2.4 | 6.4 | 0.0 | 113 | 1,574 | 122 |
| tree-city-inventory | 23.9 | 40 / 42 / 1,380 | 152 | 1.8 | 4.8 | 0.0 | 12 | 1,380 | 19 |
| swissubase | 43.5 | 516 / 58 / 1,287 | 496 | 2.4 | 7.4 | 0.0 | 173 | 1,287 | 182 |
| mtg | 15.6 | 40 / 52 / 232 | 60 | 1.4 | 6.9 | 0.0 | 16 | 232 | 24 |
| arxiu | 68.9 | 146 / 34 / 871 | 646 | 1.4 | 4.0 | 0.0 | 58 | 871 | 63 |
| mtg-modern | 15.7 | 33 / 51 / 232 | 60 | 1.5 | 6.6 | 6.4 | 9.2 | 232 | 24 |
| tci-modern | 22.7 | 27 / 36 / 1,380 | 152 | 1.9 | 4.3 | 1.0 | 11 | 1,380 | 18 |
| card-full | 0.0 | 1.2 / 15 / 0.0 | 0.0 | 0.6 | 2.5 | 0.1 | 0.2 | 0.0 | 3.4 |
| named-graphs-only | 0.0 | 1.0 / 9.0 / 0.0 | 0.0 | 0.6 | 1.5 | 0.0 | 0.1 | 0.0 | 2.1 |
| worldcup2026 | 0.0 | 1.2 / 0.0 / 0.0 | 0.0 | 0.6 | 0.0 | 0.1 | 0.2 | 0.0 | 0.8 |

### OpenCitations small (195 MiB, quad index + text + labels): round trips / KiB

| query | rows | 0x05 no cache | 0x05 cache | 0x06 N=16K | 0x06 N=64K | 0x06 N=256K | 0x05 hot read / hot set |
|---|--:|--:|--:|--:|--:|--:|--:|
| open | 1 | 53 / 335 | 9 / 1,920 | 3 / 53 | 2 / 95 | 2 / 287 | 226 / 3,915 |
| card | 1 | 5 / 65 | 5 / 65 | 1 / 16 | 1 / 64 | 1 / 256 | 62 / 3,915 |
| demo1-doi-to-work | 1 | 56 / 552 | 13 / 2,560 | 6 / 302 | 5 / 312 | 5 / 504 | 226 / 3,915 |
| c02 | 2 | 56 / 536 | 13 / 3,712 | 6 / 283 | 5 / 296 | 5 / 488 | 226 / 3,915 |
| c07 | 22 | 63 / 1,448 | 18 / 6,132 | 11 / 4,664 | 10 / 4,672 | 10 / 4,864 | 230 / 3,915 |
| demo3-journal-issn | 2 | 122 / 6,846 | 27 / 10,112 | 20 / 8,635 | 19 / 8,645 | 19 / 8,832 | 226 / 3,915 |
| journal_of | 1000 | 7 / 124 | 6 / 756 | 4 / 284 | 4 / 332 | 4 / 524 | 34 / 3,915 |
| t03-rare-mendelianae | 1 | 18 / 3,151 | 9 / 3,968 | 4 / 50 | 4 / 95 | 4 / 287 | 3,116 / 3,915 |
| t01-common-study | 3589 | 18 / 4,787 | 9 / 5,760 | 4 / 1,691 | 4 / 1,731 | 4 / 1,923 | 3,116 / 3,915 |
| c08 | 2 | 62 / 650 | 20 / 3,828 | 9 / 210 | 8 / 227 | 8 / 419 | 463 / 3,915 |
| ng-sample | 10 | 62 / 644 | 14 / 3,200 | 8 / 180 | 7 / 199 | 7 / 391 | 460 / 3,915 |
| X2-text-deletions-by-source | 50 | 1221 / 32,913 | 160 / 37,120 | 150 / 35,363 | 149 / 35,382 | 149 / 35,574 | 736 / 3,915 |
| ty-class-hist | 29 | 68 / 969 | 17 / 3,328 | 13 / 1,841 | 12 / 1,856 | 12 / 2,048 | 226 / 3,915 |
| ti-extent | 1 | 126 / 1,052 | 16 / 3,072 | 12 / 1,574 | 11 / 1,600 | 11 / 1,792 | 226 / 3,915 |

### Catalog files (5 published 0x05 + 2 rebuilt with labels/schema): median over the 7 files, round trips / KiB

| query | 0x05 no cache | 0x05 cache | 0x06 N=16K | 0x06 N=64K | 0x06 N=256K |
|---|--:|--:|--:|--:|--:|
| open | 52 / 139 | 10 / 896 | 3 / 66 | 2 / 114 | 2 / 306 |
| card | 4 / 53 | 4 / 53 | 1 / 16 | 1 / 64 | 1 / 256 |
| iri | 56 / 329 | 13 / 1,536 | 6 / 888 | 6 / 896 | 5 / 1,088 |
| obj-join | 53 / 191 | 10 / 1,152 | 3 / 220 | 3 / 256 | 2 / 448 |
| label | 12 / 251 | 5 / 512 | 2 / 27 | 2 / 75 | 2 / 267 |
| text | 14 / 1,381 | 8 / 1,728 | 4 / 61 | 4 / 109 | 4 / 296 |
| classes | 66 / 249 | 13 / 1,088 | 7 / 492 | 7 / 512 | 7 / 704 |

### Catalog files, every run: 0x05 no cache / cache / 0x06 best at 16K, 64K, 256K (RT / KiB)

| file | query | rows | 0x05 no cache | 0x05 cache | 0x06 16K | 0x06 64K | 0x06 256K |
|---|---|--:|--:|--:|--:|--:|--:|
| davidrumsey | open | 1 | 50 / 540 | 10 / 1,088 | 3 / 132 | 3 / 180 | 2 / 369 |
| davidrumsey | card | 1 | 4 / 60 | 4 / 60 | 1 / 16 | 1 / 64 | 1 / 256 |
| davidrumsey | iri | 45 | 58 / 1,244 | 15 / 3,840 | 10 / 3,122 | 10 / 3,122 | 9 / 3,264 |
| davidrumsey | obj-join | 0 | 51 / 454 | 10 / 1,152 | 4 / 58 | 4 / 106 | 3 / 282 |
| davidrumsey | label | 2 | 12 / 1,334 | 5 / 1,664 | 2 / 28 | 2 / 76 | 2 / 268 |
| davidrumsey | text | 47 | 14 / 1,685 | 7 / 1,920 | 4 / 122 | 4 / 170 | 4 / 362 |
| davidrumsey | classes | 20 | 96 / 1,069 | 24 / 2,176 | 18 / 1,394 | 18 / 1,410 | 18 / 1,600 |
| tree-city-inventory | open | 1 | 53 / 138 | 11 / 960 | 2 / 66 | 2 / 114 | 2 / 306 |
| tree-city-inventory | card | 1 | 4 / 43 | 4 / 43 | 1 / 16 | 1 / 64 | 1 / 256 |
| tree-city-inventory | iri | 3 | 56 / 161 | 11 / 960 | 3 / 208 | 3 / 256 | 3 / 448 |
| tree-city-inventory | obj-join | 13 | 57 / 201 | 13 / 1,280 | 5 / 592 | 5 / 640 | 5 / 832 |
| tree-city-inventory | label | 3 | 13 / 144 | 6 / 384 | 2 / 40 | 2 / 88 | 2 / 280 |
| tree-city-inventory | text | 5 | 15 / 1,429 | 8 / 1,792 | 4 / 56 | 4 / 104 | 4 / 296 |
| tree-city-inventory | classes | 6 | 58 / 226 | 13 / 1,088 | 7 / 154 | 7 / 202 | 7 / 394 |
| swissubase | open | 1 | 53 / 704 | 11 / 1,536 | 3 / 78 | 3 / 126 | 2 / 316 |
| swissubase | card | 1 | 4 / 59 | 4 / 59 | 1 / 16 | 1 / 64 | 1 / 256 |
| swissubase | iri | 10 | 57 / 894 | 12 / 2,240 | 6 / 1,055 | 6 / 1,088 | 5 / 1,216 |
| swissubase | obj-join | 0 | 54 / 668 | 10 / 1,472 | 3 / 159 | 3 / 207 | 2 / 384 |
| swissubase | label | 3 | 14 / 723 | 6 / 1,024 | 2 / 27 | 2 / 75 | 2 / 267 |
| swissubase | text | 45 | 16 / 1,381 | 9 / 1,792 | 4 / 73 | 4 / 121 | 4 / 313 |
| swissubase | classes | 17 | 72 / 834 | 17 / 1,984 | 10 / 689 | 10 / 722 | 10 / 896 |
| mtg | open | 1 | 51 / 139 | 10 / 832 | 3 / 62 | 2 / 110 | 2 / 302 |
| mtg | card | 1 | 4 / 53 | 4 / 53 | 1 / 16 | 1 / 64 | 1 / 256 |
| mtg | iri | 37 | 56 / 329 | 13 / 1,344 | 7 / 861 | 6 / 896 | 6 / 1,088 |
| mtg | obj-join | 0 | 52 / 149 | 10 / 832 | 3 / 220 | 2 / 256 | 2 / 448 |
| mtg | label | 1 | 12 / 251 | 6 / 512 | 2 / 46 | 2 / 94 | 2 / 286 |
| mtg | text | 1 | 14 / 283 | 8 / 576 | 4 / 62 | 4 / 110 | 3 / 286 |
| mtg | classes | 20 | 70 / 191 | 12 / 1,024 | 6 / 476 | 6 / 512 | 6 / 704 |
| arxiu | open | 1 | 52 / 222 | 10 / 896 | 3 / 56 | 2 / 102 | 2 / 294 |
| arxiu | card | 1 | 4 / 37 | 4 / 37 | 1 / 16 | 1 / 64 | 1 / 256 |
| arxiu | iri | 14 | 56 / 405 | 14 / 1,600 | 6 / 888 | 5 / 896 | 5 / 1,088 |
| arxiu | obj-join | 0 | 53 / 191 | 11 / 960 | 3 / 220 | 2 / 256 | 2 / 448 |
| arxiu | label | 12 | 12 / 727 | 5 / 960 | 2 / 20 | 2 / 68 | 2 / 260 |
| arxiu | text | 15 | 14 / 911 | 7 / 1,280 | 4 / 57 | 4 / 105 | 4 / 297 |
| arxiu | classes | 5 | 58 / 249 | 13 / 1,216 | 6 / 492 | 6 / 512 | 6 / 704 |
| mtg-modern | open | 1 | 45 / 111 | 9 / 768 | 3 / 63 | 2 / 110 | 2 / 302 |
| mtg-modern | card | 1 | 5 / 56 | 5 / 56 | 1 / 16 | 1 / 64 | 1 / 256 |
| mtg-modern | iri | 37 | 50 / 299 | 13 / 1,536 | 7 / 980 | 6 / 1,024 | 6 / 1,216 |
| mtg-modern | obj-join | 0 | 46 / 119 | 10 / 832 | 3 / 212 | 2 / 256 | 2 / 448 |
| mtg-modern | label | 2 | 4 / 74 | 4 / 256 | 3 / 26 | 2 / 73 | 2 / 265 |
| mtg-modern | text | 1 | 14 / 283 | 8 / 576 | 4 / 61 | 4 / 109 | 3 / 286 |
| mtg-modern | classes | 20 | 66 / 252 | 12 / 1,024 | 8 / 532 | 8 / 576 | 8 / 768 |
| tci-modern | open | 1 | 53 / 120 | 10 / 896 | 2 / 66 | 2 / 114 | 2 / 306 |
| tci-modern | card | 1 | 5 / 40 | 5 / 40 | 1 / 16 | 1 / 64 | 1 / 256 |
| tci-modern | iri | 3 | 56 / 148 | 9 / 832 | 3 / 272 | 3 / 320 | 3 / 512 |
| tci-modern | obj-join | 13 | 57 / 187 | 12 / 1,152 | 5 / 592 | 5 / 640 | 5 / 832 |
| tci-modern | label | 3 | 4 / 49 | 3 / 165 | 3 / 24 | 2 / 72 | 2 / 264 |
| tci-modern | text | 5 | 15 / 1,429 | 8 / 1,728 | 4 / 55 | 4 / 103 | 4 / 295 |
| tci-modern | classes | 6 | 60 / 211 | 11 / 960 | 7 / 400 | 7 / 448 | 7 / 640 |
| card-full | open | 1 | 59 / 2.2 | 2 / 20 | 2 / 16 | 1 / 20 | 1 / 20 |
| card-full | card | 1 | 3 / 19 | 3 / 19 | 1 / 16 | 1 / 20 | 1 / 20 |
| card-full | all | 7 | 63 / 2.6 | 2 / 20 | 2 / 80 | 1 / 20 | 1 / 20 |
| named-graphs-only | open | 1 | 52 / 1.6 | 2 / 14 | 1 / 14 | 1 / 14 | 1 / 14 |
| named-graphs-only | card | 1 | 3 / 13 | 3 / 13 | 1 / 14 | 1 / 14 | 1 / 14 |
| named-graphs-only | all | 0 | 53 / 1.7 | 2 / 14 | 1 / 14 | 1 / 14 | 1 / 14 |
| worldcup2026 | open | 1 | 59 / 2.2 | 2 / 1.9 | 1 / 1.9 | 1 / 1.9 | 1 / 1.9 |
| worldcup2026 | card | 1 | 2 / 1.0 | 2 / 1.0 | 1 / 1.9 | 1 / 1.9 | 1 / 1.9 |
| worldcup2026 | all | 7 | 63 / 2.6 | 2 / 1.9 | 1 / 1.9 | 1 / 1.9 | 1 / 1.9 |
| named-graphs-only | graph | 6 | 57 / 2.6 | 2 / 14 | 1 / 14 | 1 / 14 | 1 / 14 |

### Projected to OpenCitations Meta (59 GiB): the small file's query shapes against Meta's 0x06 front

| query | 0x06 N=16K | 0x06 N=64K | 0x06 N=256K |
|---|--:|--:|--:|
| open | 4 / 84 | 3 / 111 | 3 / 303 |
| card | 3 / 47 | 1 / 64 | 1 / 256 |
| demo1-doi-to-work | 7 / 396 | 6 / 423 | 6 / 599 |
| c02 | 7 / 444 | 6 / 471 | 6 / 663 |
| c07 | 12 / 5,157 | 11 / 5,182 | 11 / 5,358 |
| demo3-journal-issn | 21 / 9,248 | 20 / 9,275 | 20 / 9,451 |
| journal_of | 5 / 389 | 4 / 416 | 4 / 608 |
| t03-rare-mendelianae | 5 / 324 | 4 / 351 | 4 / 543 |
| t01-common-study | 5 / 1,991 | 4 / 2,018 | 4 / 2,210 |
| ty-class-hist | 14 / 1,988 | 12 / 2,015 | 12 / 2,191 |
| ti-extent | 13 / 1,684 | 11 / 1,711 | 11 / 1,887 |

## Every OC small run, every variant

RT / KiB at N = 16, 64, 256 KiB. 0x05 columns are RT / requests / KiB.

| file | query | rows | 0x05 no cache: RT / req / KiB | 0x05 cache: RT / req / KiB | 0x06 exact 16K / 64K / 256K: RT / KiB | 0x06 exact+predict | 0x06 cache | 0x06 cache+predict | hot touched / total KiB |
|---|---|--:|--:|--:|--:|--:|--:|--:|--:|
| oc-small | open | 1 | 53 / 69 / 335 | 9 / 16 / 1,920 | 3 / 53, 2 / 95, 2 / 287 | 3 / 53, 2 / 95, 2 / 287 | 3 / 150, 2 / 192, 2 / 384 | 3 / 150, 2 / 192, 2 / 384 | 226 / 3,915 |
| oc-small | card | 1 | 5 / 5 / 65 | 5 / 5 / 65 | 1 / 16, 1 / 64, 1 / 256 | 1 / 16, 1 / 64, 1 / 256 | 1 / 16, 1 / 64, 1 / 256 | 1 / 16, 1 / 64, 1 / 256 | 62 / 3,915 |
| oc-small | demo1-doi-to-work | 1 | 56 / 76 / 552 | 13 / 21 / 2,560 | 9 / 302, 5 / 312, 5 / 504 | 6 / 302, 5 / 312, 5 / 504 | 9 / 1,206, 5 / 1,216, 5 / 1,408 | 6 / 1,206, 5 / 1,216, 5 / 1,408 | 226 / 3,915 |
| oc-small | c02 | 2 | 56 / 80 / 536 | 13 / 27 / 3,712 | 8 / 283, 5 / 296, 5 / 488 | 6 / 283, 5 / 296, 5 / 488 | 8 / 1,971, 5 / 1,984, 5 / 2,176 | 6 / 1,971, 5 / 1,984, 5 / 2,176 | 226 / 3,915 |
| oc-small | c06 | 2 | 59 / 81 / 1,058 | 17 / 25 / 3,584 | 15 / 813, 11 / 823, 10 / 1,010 | 11 / 813, 10 / 823, 10 / 1,010 | 14 / 2,235, 10 / 2,245, 9 / 2,432 | 10 / 2,235, 9 / 2,245, 9 / 2,432 | 226 / 3,915 |
| oc-small | c07 | 22 | 63 / 103 / 1,448 | 18 / 32 / 6,132 | 16 / 1,196, 11 / 1,204, 11 / 1,396 | 12 / 1,196, 11 / 1,204, 11 / 1,396 | 15 / 4,664, 10 / 4,672, 10 / 4,864 | 11 / 4,664, 10 / 4,672, 10 / 4,864 | 230 / 3,915 |
| oc-small | demo3-journal-issn | 2 | 122 / 147 / 6,846 | 27 / 39 / 10,112 | 77 / 6,601, 73 / 6,611, 72 / 6,798 | 73 / 6,601, 72 / 6,611, 72 / 6,798 | 24 / 8,635, 20 / 8,645, 19 / 8,832 | 20 / 8,635, 19 / 8,645, 19 / 8,832 | 226 / 3,915 |
| oc-small | c05 | 1 | 58 / 83 / 594 | 15 / 28 / 3,840 | 10 / 341, 7 / 354, 7 / 546 | 8 / 341, 7 / 354, 7 / 546 | 10 / 2,355, 7 / 2,368, 7 / 2,560 | 8 / 2,355, 7 / 2,368, 7 / 2,560 | 226 / 3,915 |
| oc-small | journal_of | 1000 | 7 / 7 / 124 | 6 / 6 / 756 | 5 / 54, 5 / 102, 5 / 294 | 5 / 54, 5 / 102, 5 / 294 | 4 / 284, 4 / 332, 4 / 524 | 4 / 284, 4 / 332, 4 / 524 | 34 / 3,915 |
| oc-small | acta_universitatis | 5 | 5 / 5 / 107 | 5 / 5 / 628 | 3 / 37, 3 / 85, 3 / 277 | 3 / 37, 3 / 85, 3 / 277 | 3 / 156, 3 / 204, 3 / 396 | 3 / 156, 3 / 204, 3 / 396 | 34 / 3,915 |
| oc-small | t01-common-study | 3589 | 18 / 20 / 4,787 | 9 / 11 / 5,760 | 5 / 1,691, 4 / 1,731, 4 / 1,923 | 4 / 1,691, 4 / 1,731, 4 / 1,923 | 5 / 2,088, 4 / 2,128, 4 / 2,320 | 4 / 2,088, 4 / 2,128, 4 / 2,320 | 3,116 / 3,915 |
| oc-small | t03-rare-mendelianae | 1 | 18 / 18 / 3,151 | 9 / 9 / 3,968 | 5 / 50, 4 / 95, 4 / 287 | 4 / 50, 4 / 95, 4 / 287 | 5 / 291, 4 / 336, 4 / 528 | 4 / 291, 4 / 336, 4 / 528 | 3,116 / 3,915 |
| oc-small | t05-and-border-regions | 2 | 19 / 20 / 3,166 | 10 / 10 / 4,224 | 7 / 80, 6 / 126, 6 / 318 | 5 / 80, 5 / 126, 5 / 318 | 7 / 563, 6 / 608, 6 / 800 | 5 / 563, 5 / 608, 5 / 800 | 3,116 / 3,915 |
| oc-small | c08 | 2 | 62 / 84 / 650 | 20 / 30 / 3,828 | 14 / 210, 11 / 227, 11 / 419 | 9 / 210, 8 / 227, 8 / 419 | 14 / 1,637, 11 / 1,654, 11 / 1,846 | 9 / 1,637, 8 / 1,654, 8 / 1,846 | 463 / 3,915 |
| oc-small | c09 | 1 | 63 / 82 / 637 | 19 / 28 / 3,572 | 13 / 197, 10 / 214, 10 / 406 | 8 / 197, 7 / 214, 7 / 406 | 13 / 1,381, 10 / 1,398, 10 / 1,590 | 8 / 1,381, 7 / 1,398, 7 / 1,590 | 463 / 3,915 |
| oc-small | X3-text-title-history | 1 | 63 / 82 / 640 | 19 / 28 / 3,584 | 13 / 202, 11 / 221, 11 / 413 | 9 / 202, 8 / 221, 8 / 413 | 13 / 1,507, 11 / 1,526, 11 / 1,718 | 9 / 1,507, 8 / 1,526, 8 / 1,718 | 460 / 3,915 |
| oc-small | ng-list | 100 | 64 / 80 / 617 | 13 / 20 / 2,560 | 14 / 143, 10 / 172, 10 / 364 | 10 / 143, 9 / 172, 9 / 364 | 13 / 1,087, 9 / 1,117, 9 / 1,309 | 9 / 1,087, 8 / 1,117, 8 / 1,309 | 460 / 3,915 |
| oc-small | ng-sample | 10 | 62 / 81 / 644 | 14 / 23 / 3,200 | 13 / 180, 8 / 199, 8 / 391 | 8 / 180, 7 / 199, 7 / 391 | 13 / 1,482, 8 / 1,501, 8 / 1,693 | 8 / 1,482, 7 / 1,501, 7 / 1,693 | 460 / 3,915 |
| oc-small | X2-text-deletions-by-source | 50 | 1221 / 1242 / 32,913 | 160 / 167 / 37,120 | 1156 / 32,262, 1154 / 32,281, 1154 / 32,473 | 1147 / 32,262, 1146 / 32,281, 1146 / 32,473 | 159 / 35,363, 157 / 35,382, 157 / 35,574 | 150 / 35,363, 149 / 35,382, 149 / 35,574 | 736 / 3,915 |
| oc-small | ov-one-row | 1 | 59 / 75 / 400 | 12 / 19 / 2,560 | 12 / 136, 7 / 160, 7 / 352 | 8 / 136, 7 / 160, 7 / 352 | 11 / 936, 6 / 960, 6 / 1,152 | 7 / 936, 6 / 960, 6 / 1,152 | 227 / 3,915 |
| oc-small | ov-ask-pred | 1 | 53 / 70 / 391 | 10 / 17 / 2,176 | 5 / 107, 3 / 151, 3 / 343 | 4 / 107, 3 / 151, 3 / 343 | 5 / 403, 3 / 448, 3 / 640 | 4 / 403, 3 / 448, 3 / 640 | 226 / 3,915 |
| oc-small | ty-class-hist | 29 | 68 / 84 / 969 | 17 / 24 / 3,328 | 23 / 714, 17 / 729, 17 / 921 | 18 / 714, 17 / 729, 17 / 921 | 18 / 1,841, 12 / 1,856, 12 / 2,048 | 13 / 1,841, 12 / 1,856, 12 / 2,048 | 226 / 3,915 |
| oc-small | lb-labels | 50 | 58 / 96 / 2,291 | 13 / 29 / 6,784 | 11 / 2,041, 7 / 2,051, 7 / 2,243 | 8 / 2,041, 7 / 2,051, 7 / 2,243 | 11 / 5,302, 7 / 5,312, 7 / 5,504 | 8 / 5,302, 7 / 5,312, 7 / 5,504 | 226 / 3,915 |
| oc-small | ti-extent | 1 | 126 / 142 / 1,052 | 16 / 23 / 3,072 | 80 / 786, 75 / 812, 75 / 1,004 | 76 / 786, 75 / 812, 75 / 1,004 | 16 / 1,574, 11 / 1,600, 11 / 1,792 | 12 / 1,574, 11 / 1,600, 11 / 1,792 | 226 / 3,915 |
|---|--:|--:|--:|--:|
