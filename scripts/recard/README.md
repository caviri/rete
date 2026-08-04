# `scripts/recard` — bring published `.rete` files up to the current Dataset Card

A `.rete` published six months ago carries the card its builder could write then.
Since #153/#154 a card also carries a **build record** (provenance, parameters,
measured query costs) and **graph-scoped starter queries**. The second one is not
cosmetic: on a file whose statements all live in named graphs, a starter query
written for the default graph returns **zero rows**, and a newcomer reads that as
a broken file. That is the bug `nkod.rete` (the Czech national open-data
catalogue, 31,974 named graphs, empty default graph) shipped with.

These scripts re-card an existing file **without touching its data** and prove
both halves of that claim before replacing anything.

```
survey.sh          which published files are broken, which are merely dated
recard.sh          re-card ONE file, end to end, with both proofs
recard_batch.sh    the catalog-scale driver: resumable, idempotent
card_tools.py      the JSON surgery (carry curated fields, classify, verify)
```

Everything runs in the `rete-dev` image; the shell scripts re-execute themselves
inside Docker, so the paths you pass are **container** paths (the repo is `/work`).

---

## Quick start

```sh
docker compose build dev                       # once
docker compose run --rm dev cargo build --release -p rete-cli

# 1. what is actually broken?  (2 range requests per file, ~6 KB each)
bash scripts/recard/survey.sh

# 2. fix one file
bash scripts/recard/recard.sh \
  --source https://datagov-cz.github.io/rete-test/nkod.rete \
  --out /work/dev/recard/out/nkod/nkod.rete

# 3. or work the list the survey wrote
bash scripts/recard/recard_batch.sh --list /work/dev/recard/survey/todo.txt
```

Outputs land under `/work/dev/recard/` (git-ignored): `survey/`, `out/`,
`state/` (receipts) and `work/` (scratch).

---

## Why a re-card is not a one-liner

* `rete repyramid old.rete -o new.rete --card` **re-derives the whole card from
  the `.rete` itself** — no source RDF needed. That is the primitive.
* But `--card` takes the **curated** half only from flags or `--card-file`. A
  bare `--card` silently drops the publisher's `title`/`source`/`license`. So the
  old card must be read first and handed back in — `card_tools.py curated` does
  exactly that, and `verify` fails the run if any curated field went missing.
* The card sits at byte 1024 and is **inside the blake3 content hash**, so there
  is no in-place byte swap. Every re-card rewrites the file, which is why the
  data has to be proven unchanged afterwards rather than assumed.

## The two proofs

**The data is unchanged.** Both files are serialized to N-Quads and hashed as
they stream (`rete export --format nq | sha256sum`) — constant memory, no temp
files. Equal streams prove equal data. Only if the streams differ in order does
the script fall back to the sorted `cmp`, which needs scratch disk. Either way a
mismatch is fatal: the original is left alone and the rebuilt file is kept for
inspection.

**The new starter queries answer.** The row counts are already in the rebuilt
file: `rete build`/`repyramid` run every starter query once against the finished
image and record `rows` in the build-info section. `card_tools.py verify` fails
if any query returns zero (except `top-dangling`, which is legitimately empty on
a fully-described graph — extend with `--allow-empty`), if `ov-one-row` does not
return exactly one row, or if a named-graph-only file still ships a
default-graph query.

That gate is not decorative. On `mtg` it caught a **live card-generator bug**:
`lb-labels` conjoins the top class with the top label predicate independently —

```sparql
SELECT ?s ?label WHERE { ?s a mtg:Ruling ; schema:name ?label } LIMIT 50
```

— and mtg's most frequent class (`mtg:Ruling`, 821k instances) is precisely the
one that carries no `schema:name`. 22 of the 23 starter queries answer; that one
returns zero, on a default-graph file, in current `rete-cli`. The template needs
a class that actually has the label predicate, not the two most frequent things
picked separately. Until that is fixed upstream, `--allow-empty "lb-labels"`
lets such a dataset through **explicitly**, which is the point — the default is
to stop.

---

## Which rebuild engine, and the RAM ceiling

`repyramid` loads the file, decodes it, and materializes **every quad as owned
strings** before re-assembling. Measured peak RSS (VmHWM, one process, inside
the dev image):

| file | bytes | statements | peak RSS | × file size | per statement | wall |
|---|---:|---:|---:|---:|---:|---:|
| `geoadmin.rete` | 40.3 MB | 372,508 | 778 MiB | 19.8× | 2,138 B | 8.5 s |
| `nkod.rete` | 71.2 MB | 2,282,441 | 1.18 GiB | 17.4× | 543 B | 65 s |
| `aifdb.rete` | 96.9 MB | 9,144,988 | 3.19 GiB | 34.5× | 365 B | 133 s |
| `cordis.rete` | 801 MB | 26,363,545 | **16.07 GiB** | 21.0× | 654 B | 247 s |

**Rule of thumb: `repyramid` needs ~20× the file size in RAM** (17–35× over the
measured range; the ratio tracks statement count more than bytes, so a
literal-heavy graph sits at the low end of the ×-column and a triple-dense one at
the high end). On a 48 GB machine that puts the ceiling at roughly **2 GB of
`.rete`**. 22 of the 98 published catalog files are ≥ 1 GB, and the six largest —
`crossref` 56.1 GB, `datacite` 48.6 GB, `opencitations` 33.4 GB, `wikiart`
23.7 GB, `orcid` 16.3 GB, `gharchive` 15.9 GB — are 8× to 28× past it.

### The streaming alternative that exists today

There is one, and it is not `--memory-budget-mb`:

```sh
rete export in.rete --format nq > staged.nq          # constant memory
rete build staged.nq -o out.rete --card-file curated.json
```

* `export --format nq` uses `dump_each` — **measured peak RSS 2.9–3.0 MiB**
  regardless of size (3.0 MiB exporting 71 MB → 679 MB of N-Quads; 2.9 MiB
  exporting 801 MB → 6.88 GB). This half really is constant memory.
* `rete build` on an N-Quads **file** takes the two-pass streaming assembler
  (`assemble_dataset_streaming_algo`), which never materializes the string quad
  multiset — only the dictionary, the id-triples and the index. It keeps named
  graphs, derives the **full** profile, generates the graph-scoped starter
  queries, and measures them. On `nkod.rete` the two paths produce the **same
  blake3 content hash** (`673fb14a9190be3ffec205340e2b4513`) — and since the
  content hash covers everything except the unhashed build-info section, the two
  images are equal apart from the four bytes that say `repyramid` vs `build`.

| file | `repyramid` peak | `export` peak | `build .nq` peak | staged `.nq` |
|---|---:|---:|---:|---:|
| `nkod.rete` (71 MB) | 1.18 GiB / 65 s | 3.0 MiB / 24 s | 949 MiB / 83 s | 679 MB |
| `cordis.rete` (801 MB) | **16.07 GiB** / 247 s | 2.9 MiB / 260 s | **7.00 GiB** / 437 s | 6.88 GB |

So the streaming path costs **~2.3× less RAM** and **~2.8× more wall clock**,
and buys the difference with disk. On a 48 GB machine that moves the ceiling
from roughly **2 GB** of `.rete` to roughly **5 GB** — better, not unbounded.

That is what `--mode stream` runs, and what `--mode auto` picks above
`--stream-above-mb` (default 192). The staged N-Quads is roughly **9–10× the
`.rete`**, so budget the disk before starting a batch.

### What does NOT work

`rete build --memory-budget-mb N` is the genuinely bounded builder, but it is
the wrong tool here on two counts, both hard:

1. **It rejects named graphs outright** — `external build supports the default
   graph only (named graph … found)`. Named-graph files are precisely the
   population this pipeline exists for.
2. **Its card has no profile and no starter queries.** The external path calls
   `curated_counts_card`, which writes curated fields plus top-line counts and
   nothing else (deriving the profile lists needs unbounded RAM). A re-card that
   produced that would be a downgrade.

You can see the consequence in the survey: `crossref`, `datacite`, `dblp`,
`deps-dev`, `epfl-graph`, `gharchive`, `gharchive-2026-06`, `opencitations`,
`orcid` and `wikiart` all report **0 starter queries** — they were built on that
path.

### The honest boundary

**The big ones need engine work.** Above roughly 2 GB `repyramid` is out, and
above the point where dictionary + id-triples + the output image stop fitting,
so is the staged-N-Quads path. The missing piece is small and specific: the
two-pass assembler already accepts *any* re-readable quad source, and a
lazily-opened `.rete` **is** re-readable (`dump_each` streams it). A
`repyramid --stream` that feeds `assemble_dataset_streaming_algo` straight from
the source `.rete` would remove the text detour and the 10× disk bill, and would
be the same order of RAM as `build` from `.nq` — but it is engine code, and it is
deliberately **not** in these scripts.

---

## What this costs

Bandwidth, not CPU. Re-carding rewrites the file, so a remote dataset must be
**downloaded in full** and — if you publish the result — **uploaded in full**.

* The survey is free: 2 range requests per file, ~6 KB each, **~0.6 MB for the
  whole 98-dataset catalog** (one outlier: `geoadmin-tiles` carries an embedded
  PMTiles section ahead of its metadata, so its coalesced card range is 117 MB —
  survey it last, or not at all).
* The published catalog is **248.35 GB across 98 files** (HEAD-probed
  2026-08-05; 22 are ≥ 1 GB, 36 are ≥ 100 MB). Re-carding all of it means
  **248 GB down and 248 GB back up** — at 100 Mbit/s that is about 5.5 hours
  each way, before any CPU.
* Use `--max-mb` to stay inside a budget, and `--mirror` (plus `RECARD_MOUNT`)
  to point at a local copy you already have and skip the download entirely. Most
  of these datasets were built locally, so the mirror is usually the right
  answer: it removes the download half of the bill outright.
* R2 egress is free and uploads are not billed per byte, so the money cost is
  ~0; the wall-clock cost is real.

`recard_batch.sh` prints the known source bytes of its plan before it starts, and
`--dry-run` prints the plan and stops.

## Publishing the result

These scripts do **not** upload — the rebuilt file lands in `--out-dir` and stops
there, so a human decides. The publish step is the repo's existing one:

```sh
python scripts/r2_upload_files.py dev/recard/out/nkod/nkod.rete=nkod/nkod.rete
python scripts/check_dataset_catalog.py --all      # re-probe the Range/CORS contract
```

Re-uploading changes each file's content hash, so `web/datasets.lock.json` has to
be regenerated afterwards — that is a release action, not a scripting one.

## Resumability and idempotency

`recard.sh` writes a receipt to `<work>/state/<name>.json` recording the source's
header content hash, the output's, the mode, the curated fields carried and the
measured row counts. On a re-run it compares both hashes and exits early if the
work is already done; `--force` overrides. Because the rebuilt file is only moved
into place after both proofs pass, an interrupted run leaves the destination
either absent or intact — never half-written. `recard_batch.sh` adds a per-key
log and a `summary.tsv`, and continues past failures unless `--stop-on-error`.

## Survey verdicts

| verdict | meaning |
|---|---|
| `CARDLESS` | no Dataset Card at all |
| `ZERO-ROWS` | named-graph-only, but starter queries scan the default graph — **broken today** |
| `MIXED-HIDDEN` | data in both, but no starter query looks inside a named graph |
| `DATED` | scope-correct, but no build record / profile cap / `ov-one-row` |
| `CURRENT` | nothing to do |
| `UNREADABLE` | the card tier itself failed (HTTP, format, parse) |

`survey.sh` writes `todo.txt` with every key that is not `CURRENT`, worst first —
that is the order `recard_batch.sh` will work them in.

### What the first full run found (2026-08-05, 98 catalog datasets, 0.6 MB)

| verdict | count |
|---|---:|
| `CARDLESS` | 14 |
| `ZERO-ROWS` | **1** |
| `MIXED-HIDDEN` | 0 |
| `DATED` | 83 |
| `CURRENT` | 0 |

So the alarming case is **rare and specific**: exactly one published file,
`switzerland-fedlex` (1.04 GB, 66.4 M quads across 497,905 named graphs, empty
default graph), ships 8 starter queries of which 6 scan the default graph and
return zero rows. Everything else with data in named graphs is either fine or
has no card at all. The other 97 are a metadata refresh, not a fire.

Two secondary findings worth knowing before planning the work:

* **Ten of the largest datasets carry a counts-only card** — `crossref`,
  `datacite`, `dblp`, `deps-dev`, `epfl-graph`, `gharchive`,
  `gharchive-2026-06`, `opencitations`, `orcid`, `wikiart` all report **0
  starter queries**, because they were built through
  `build --memory-budget-mb`. Those are also the files no rebuild path here can
  handle (`datacite` alone is 52 GB / 18.1 G quads). They need the engine work,
  not these scripts.
* **`geoadmin-tiles` costs 117 MB to survey**, not 6 KB: its embedded PMTiles
  section sits ahead of the metadata, so the "coalesced metadata range" spans it.
  Worth a look at the section ordering.

## Known limits

* A cardless source has no curated fields to carry; the re-card gives it a
  derived card only. Titles and licences for those have to come from a
  hand-written `--card-file`.
* Sharded datasets are surveyed by their first shard unless `--include-shards`.
  Re-carding a shard set means re-carding every shard.
* `--pyramid-algo` defaults to `louvain` (matching `repyramid`'s own default),
  not to whatever the file was originally built with — the build record that
  would say is exactly what these older files lack. Pass `--pyramid-algo types`
  where the dataset's recipe used it.
* The data proof compares N-Quads. It is exact for the RDF, and it is the same
  check `rete export` round-trips are documented against; it does not compare
  non-graph sections (text index, PMTiles), which a re-card does not carry over
  unless the corresponding flag is passed.
