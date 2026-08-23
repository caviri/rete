---
name: rete-local-query
description: Query a LOCAL .rete file efficiently — the lazy range-read path that opens a multi-GB file without loading it whole, which CLI commands are lazy vs read-whole, the tuning knobs (RETE_BLOCK_KB, RETE_LOCAL_LAZY_ABOVE_MB, the 256 MiB block-cache cap), how to preview a query's byte/RAM cost (rete cost / rete why), how to prove a memory bound, and the real memory-scaling limits at billion-triple scale. Use whenever a local query is slow, OOMs, or you need to bound/preview its I/O and RAM. Complements rete-sparql (which is about writing the query).
---

# Querying a local `.rete` file (the lazy range-read path)

**rete-sparql** is about *writing* a correct query. This skill is about *executing*
it against a **local file** efficiently: opening a multi-GB `.rete` without reading
it whole, knowing which commands are lazy, the knobs that govern I/O and memory, how
to preview a query's cost, and — importantly — the memory limits that still bite at
billion-triple scale.

## Why lazy is possible — the mental model

A `.rete` is three things (see the format-internals notes):
1. a **dictionary** — every IRI/literal interned **once** to an integer id;
2. **permutation indexes** that store **id-triples**, not the string values (that is
   why 6 orderings of ~10 B triples fit in tens of GB, not terabytes — they are ids,
   not repeated values). Six by default (SPO/POS/OSP/SOP/PSO/OPS); a file built with
   `rete build --permutations 3` carries only SPO/POS/OSP, which route every pattern
   identically — `rete info` / `rete stats` report which set a file has;
3. a **summary/pyramid** (class histogram + community super-edges).

So answering a query = resolve the bound terms to ids (a few dictionary lookups) →
read the matching **slice of one permutation** → map result ids back to strings (a few
more dictionary lookups). Each of those is a small, **addressable byte range**. Lazy
open reads exactly those ranges on demand through a positional file handle + a bounded
block cache, instead of reading the whole file and decoding the whole dictionary up
front.

## Lazy vs read-whole — which commands, and when it kicks in

- Files **larger than `RETE_LOCAL_LAZY_ABOVE_MB` (default 1024 MiB)** open through the
  lazy range path **automatically**. Smaller files use the read-it-all path, which is
  faster when the whole graph will be touched anyway.
- **Lazy (range-read) commands:** `query`, `bgp`, `sparql`, `cypher`, `why`, `stats`,
  `search`, `graphs`, `summary`, `predicates`, `schema`, `export`, `reach`, `shacl`,
  `communities`, `federate`, `reason`, `serve`.
- **`info` and `card`** were the last read-whole commands — fixed in PR #97 (they
  now cost two small range reads, ~1 s on a 52 GB file under a 1 GiB cap). On a
  binary older than #97 they still slurp the file — don't run them on >10 GB there.
- **`FROM <g>`**: a single-graph `FROM` borrows the graph's index (no copy, PR #97);
  a **multi-graph** `FROM` still merges all triples into a temporary index — the
  one remaining whole-graph materialization.

## The knobs (environment variables)

| var | default | what it does |
|---|---|---|
| `RETE_LOCAL_LAZY_ABOVE_MB` | `1024` | file-size threshold to switch to the lazy path. Lower it (e.g. `RETE_LOCAL_LAZY_ABOVE_MB=64`) to force lazy on smaller files; raise it to force read-whole (faster for a full scan you'll consume entirely). |
| `RETE_BLOCK_KB` | `64` (auto **128** for files >100 MB) | the range-read **block size** — the KB fetched per cache miss. The reader **coalesces byte-adjacent blocks into one request**, so a selective query only pays for the handful of *scattered* blocks it touches. Keep it near the 64 KiB dictionary-chunk size: bigger blocks over-fetch on scattered faults, smaller blocks add round-trips. |
| `RETE_DICT_RESTART_INTERVAL` | build-time | dictionary front-coding restart interval — sets how big the in-memory restart-offset table is (a 50 M-term section's table is ~24 MiB). Larger interval → smaller table, slower term seeks. Set at **build**, not at query time. |

**Not env-configurable:** the block **cache** is capped at **256 MiB resident**
(`DEFAULT_CACHE_CAP`). Old blocks evict, so cache RAM never grows unbounded no matter
how long you keep querying an open file (e.g. `rete serve`).

## Preview and observe a query's cost

- **Before** running — `rete cost <file> "<sparql>"` prints the range-read **byte
  cost** (add `--explain` for the per-pattern breakdown, `--json` for machine output).
  Use it to decide whether a query is a KB lookup or a GB scan *before* you pay for it.
- **After** running — `rete why <file> S P O` (with `?` for variables) prints which
  permutation answered it and the **exact byte ranges** read. This is how you confirm a
  query touched KB, not GB.

```bash
rete cost web/mydata.rete "SELECT ?o WHERE { <https://ex.org/s> <https://ex.org/p> ?o }"
rete why  web/mydata.rete "https://ex.org/s" "?" "?"      # ranges actually read
```

## Memory — what lazy fixes, and what it does NOT

**Lazy open fixes the *file-load* OOM.** A huge file / huge dictionary is never
slurped. Validated: a 23.5 GB WikiArt `.rete` whose dictionary is 23.4 GB of embedded
images went from `Cannot allocate memory` (wanted ~55 GB) to a **1.8 s point query**.

It does **not** make every query low-RAM. Two things still scale with the **data**, not
the file, so they can use many GB regardless of lazy file access:

1. **Summary/pyramid ops** — `predicates`, `summary`, `stats`, `schema` load the
   summary; on a billion-triple graph that is several GB.
2. **Aggregations / large result sets** — the engine retains solutions proportional to
   matches, so a `COUNT`/`GROUP BY` over a class with hundreds of millions of instances
   grows RAM ∝ matches.

> **Proven at scale — `datacite.rete` (52 GB, 9.83 B triples, 1.885 B terms), under
> hard Docker memory caps:** `LIMIT 1` point query → 6 s @ 2 GB; `COUNT` of a class →
> 779,399 in 4 s @ 2 GB; `GROUP BY ?type` over the full **1.38 B-row** type slice →
> 30 groups in 131 s @ 4 GB (~10.5 M rows/s). Aggregation streams (PR #96); the lazy
> open reads only directories. Blocking ops (`ORDER BY`, `DISTINCT`) still materialize
> their input — those are the remaining GB-scale risks on huge results.

> **THE diagnostic that matters — "every query OOMs at any cap":** if OOM wall-time
> **scales linearly with the memory cap at a constant ~130–180 MB/s** (8 GB→44 s,
> 16 GB→103 s, 40 GB→301 s…), the binary is **reading the whole file** — the lazy
> routing is not active. It is NOT the query, NOT the graph size, NOT "the engine
> can't do it". Check `strings target/release/rete | grep -c RETE_LOCAL_LAZY_ABOVE_MB`
> (0 = a binary/branch without the lazy-local routing, e.g. a stale checkout from
> main's broken window before c5944de3, or a botched merge). Rebuild from a tree that
> has it — and remember cargo can serve a **stale binary** after 9p-mounted edits:
> `touch` the sources and re-verify with `strings` before trusting any measurement.

**Rule of thumb**

| query shape | bytes read | resident RAM |
|---|---|---|
| point lookup (bound S, few results) | hundreds of KB | small |
| selective join, small output | KB–MB | modest |
| `COUNT` / `GROUP BY` (streaming aggregation) | the matching slice | **O(#groups)** — proven 4 GB for 1.38 B rows |
| `ORDER BY` **with** `LIMIT` | the matching slice | O(offset+limit) — top-k bounded upstream |
| `DISTINCT` | the matching slice | O(distinct rows) seen-set — streams |
| plain `ORDER BY` (no LIMIT) / huge `SELECT *` / multi-graph `FROM` | the whole slice | materialized — the remaining risks |

## Prove a query's memory bound

Run under a hard cgroup cap — **completes = bounded within N; exit code 137 = OOM**:

```bash
docker run --rm --memory=8g --memory-swap=8g -v "$PWD:/work" -w //work rete-dev:latest \
  //work/target/release/rete sparql //work/web/mydata.rete "<query>"
# or: bash skills/rete-local-query/scripts/memcap_query.sh web/mydata.rete "<query>" 8g
```
`--memory-swap` equal to `--memory` disables swap, so you measure a *true* RAM bound.

## Docker-on-Windows gotchas (this repo)

- **Path mangling:** prefix `MSYS_NO_PATHCONV=1`, mount `-v "D:/pro/rete:/work"`,
  workdir `-w //work`, binary `//work/target/release/rete`.
- **The 9p bind-mount** makes reads slower than native (the 1.38 B-row scan above ran
  ~10.5 M rows/s over 9p), but a *lazy* point query is still seconds even on a busy
  98%-full drive. If a point query takes >100 s, do NOT blame the mount first — apply
  the constant-MB/s diagnostic above; every "9p is catastrophically slow" reading in
  this repo's history turned out to be a read-the-whole-file binary.
- **Format version:** the current binary reads stable `0x05` and paired-index
  `0x06`. Experimental `0x01`–`0x04` files and unknown `0x07+` generations error
  with *"unsupported .rete format"* — rebuild old files from RDF with `rete build`.

## Remote is the *more* lazy path

The `-url` commands (`sparql-url`, `query-url`, `why-url`, `summary-url`, `card-url`)
range-read the file over HTTP — **including the dictionary index** — so a huge graph
served from R2/HTTP can have a **lower memory floor** than the same file opened locally.
If a local query on a ~billion-term file is RAM-bound, try it over HTTP:

```bash
rete sparql-url https://data.graphplaza.com/<ds>/<ds>.rete "<query>"
```

## See also

- **rete-sparql** — writing correct SPARQL + the gotchas that silently return 0 rows.
- **rete-catalog** — discovering and opening already-published `.rete` files.
- format internals — dictionary + permutations + summary layout.
