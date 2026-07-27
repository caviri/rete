# deps-dev — the deps.dev cross-ecosystem dependency graph

**deps.dev (Google's Open Source Insights)** turned into a queryable knowledge
graph: the version-pinned **dependency *and* dependents** network of seven package
ecosystems — **npm, PyPI, Maven, Go, Cargo, NuGet, RubyGems**.

- **Status: SHIPPED** — **2,555,320,067 triples** across **8 federated `.rete`
  shards** (~12 GB) on R2, registered in the playground.
- Source: <https://deps.dev/> · docs <https://docs.deps.dev/bigquery/v1/>
- **License: CC-BY 4.0** — "Includes data from deps.dev (Open Source Insights) by
  Google, licensed under CC BY 4.0."
- Snapshot: **`2026-07-13 21:01:00 UTC`**.

---

## What did it cost? **$0.** (and the ~$580 trap we avoided)

The BigQuery download billed **nothing** — it stayed inside BigQuery's **free
1 TiB/month** query tier. But it was easy to get wrong: deps.dev's obvious,
documented entry point (the `*Latest` views) is a money trap.

Every base table keeps **~218 historical snapshots** and the `*Latest` views
**don't prune** — a naive `SELECT *` scans *all* of them. A free `--dry_run`
(which prints the exact billable bytes without running the query) showed what the
naive path would have cost:

| The naive query | Bytes scanned | Cost @ $6.25/TiB |
|---|---|---|
| `SELECT * FROM DependenciesLatest` (the edges) | ~102 TB | **~$582** |
| `SELECT * FROM DependencyGraphEdgesLatest` | ~315 TB | ~$1,791 |
| `SELECT * FROM DependentsLatest` | ~88 TB | ~$499 |
| `SELECT * FROM PackageVersionsLatest` | ~11 TB | ~$65 |

**None of these were run.** How we dodged it:

1. **Always `--dry_run` first** — free, and it reveals the bill before anything
   executes (`01_probe_sizes.sh`).
2. **Query the partitioned *base* tables with a LITERAL snapshot filter** —
   `WHERE SnapshotAt = TIMESTAMP('2026-07-13 …')` prunes to a **single** snapshot
   (~150× less). The edge data dropped from **102 TB → 709 GB**.

Pruned per-snapshot scan for the whole download:

| Table (one snapshot) | Scan | Cost |
|---|---|---|
| Advisories | 0.26 GB | ~$0 |
| Projects | 0.60 GB | ~$0 |
| PackageVersionToProject | 22 GB | ~$0.12 |
| PackageVersions | 110 GB | ~$0.62 |
| Dependencies (edges, depth=1) | 709 GB | ~$4.03 |

**Total ≈ 842 GB — under the free 1 TiB/month → actual bill $0.** The only ongoing
cost is trivial R2 storage (~12 GB shards + ~87 GB Parquet backup ≈ a couple $/mo,
clearable anytime).

---

## The published graph (8 federated shards)

One logical graph, split into 8 independent shards that the playground
**auto-federates** (every query UNIONs across them). Public at
`https://data.graphplaza.com/deps-dev/deps-dev-<shard>.rete`:

| Shard | Triples | Size | Shard | Triples | Size |
|---|---|---|---|---|---|
| npm-0 | 687,436,028 | 3.04 GB | go | 426,161,489 | 2.81 GB |
| npm-1 | 678,047,234 | 2.94 GB | nuget | 162,726,660 | 0.73 GB |
| maven | 389,532,310 | 1.38 GB | pypi | 149,006,769 | 0.74 GB |
| cargo | 42,366,165 | 0.21 GB | rubygems | 20,043,412 | 0.11 GB |

Why 8 shards for 7 ecosystems: **npm is split into 2** (by package-name hash) —
its ~1.37 B triples were too big to build in one pass on this disk. The shards
**merge for free** because IRIs are global/canonical (no `owl:sameAs`):

- package version → `https://deps.dev/<system>/<name>/<version>`
- project → `https://github.com/<owner>/<repo>`
- `deps:purl` (`pkg:npm/lodash@4.17.21`) = the cross-registry join key

**Federation is UNION + routing, not cross-shard JOINs** — so example queries use
constant-IRI star queries or single-ecosystem-shard joins. Projects/advisories
live in the first shard of each ecosystem.

### The model

One directed graph: `packageVersion --deps:dependsOn--> packageVersion` (resolved,
version-pinned, direct). **Dependents come free** as the reverse: "what depends on
B@v?" = `?x deps:dependsOn <B@v>` — no separate table, no transitive closure.
Nodes carry licenses (SPDX IRIs), advisories, SLSA provenance, registry links;
`deps:hasProject` → source `Project` (stars/forks/OSS-Fuzz); `deps:hasAdvisory`
→ `Advisory` (GHSA/OSV, CVSS, CVE aliases). Full ontology: `deps-dev.ttl`
(OWL 2 QL, reuses schema.org + Dublin Core + SPDX). Example queries:
`example-queries.rq`.

---

## The raw data (`raw/`, ~87 GB Parquet)

The intermediate — one BigQuery snapshot exported to Parquet before graph-building.
Backed up on R2 at `https://data.graphplaza.com/deps-dev/raw/<file>.parquet`.

| File | Rows | Size |
|---|---|---|
| PackageVersions.parquet | 161,888,666 | 64.21 GB |
| dependency_edges.parquet | 570,601,975 | 17.03 GB |
| PackageVersionToProject.parquet | 172,093,192 | 5.29 GB |
| Projects.parquet | 5,122,936 | 0.32 GB |
| Advisories.parquet | 272,582 | 0.07 GB |

---

## Pipeline (BigQuery → Parquet → N-Triples → `.rete` → R2)

1. **BigQuery → Parquet** (`00_auth.sh` → `01_probe_sizes.sh` → `02_export.sh` +
   `02b_export_edges.sh`, via `export_direct.py`). Needs OAuth + a billing-enabled
   project; the SnapshotAt-literal prune above keeps it in the free tier.
2. **Metadata artifacts** — `deps-dev.ttl` (ontology), `schema.json` (JSON Schema),
   `croissant.jsonld` (`emit_meta.py`, generated from the known schema — pyarrow's
   footer parse *hangs* on these many-row-group files).
3. **Parquet → N-Triples** (`deps_dev_to_nt.py`) — DuckDB filters each table by
   `System` (and, for npm, a name hash) and streams `<s> <p> <o> .`.
4. **N-Triples → `.rete`** — `rete build --memory-budget-mb 16000 --tmp-dir …`
   (external, bounded RAM; the in-RAM build OOMs past ~149 M triples). NT is
   `gzip -1`'d (~13×) and stream-fed (`zcat | rete build -`) so only the gzip +
   spill sit on disk. One shard per ecosystem; npm as 2.
5. **Publish** (`upload_r2.py` + `skills/rete-publish/scripts/upload_bucket.sh`) —
   upload to R2, register in `web/playground-src/catalog.js` (`shards:[…]`).

---

## Layout

```
data/deps-dev/
  README.md  deps-dev.ttl  schema.json  croissant.jsonld  example-queries.rq
  SHA256SUMS.txt
  raw/                       # the 5 Parquet files (gitignored; on R2)
  scripts/                   # the recipe (committed)
    config.env.example  _lib.sh  download.sh
    00_auth.sh  01_probe_sizes.sh  probe_snapshot.py     # auth + free size/$ probe
    02_export.sh  02b_export_edges.sh  export_direct.py  # BigQuery -> Parquet
    emit_meta.py                                          # schema.json + croissant
    deps_dev_to_nt.py                                     # Parquet -> N-Triples
    upload_r2.py                                          # -> R2
```

## Reproduce

```bash
cd data/deps-dev/scripts && cp config.env.example config.env   # set GCP_PROJECT
gcloud auth application-default login                          # OAuth (no API key)
bash 00_auth.sh && bash 01_probe_sizes.sh                      # verify + free $ probe
bash 02_export.sh && bash 02b_export_edges.sh                  # -> raw/*.parquet

# per ecosystem: Parquet -> N-Triples (streaming) -> external build -> shard
SYSTEM=CARGO python deps_dev_to_nt.py                          # -> deps-dev-cargo.nt
rete build deps-dev-cargo.nt deps-dev.ontology.nt \
  -o deps-dev-cargo.rete --memory-budget-mb 16000 --tmp-dir _spill --card
# npm: CHUNK_MOD=2 CHUNK_IDX=0|1  (2 shards), gzip-stream the NT to fit disk
```

## Gotchas (hard-won)

- **BigQuery needs OAuth** — an **API key is rejected** (`API keys are not supported
  by this API`). Use `gcloud auth application-default login`; the `bq` CLI uses a
  *separate* credential store from ADC, so this pipeline is Python-client + ADC.
- **`*Latest` views = the $580 trap** — they scan all snapshots; filter the base
  table by a **literal** `SnapshotAt` (see the cost section).
- **BigQuery Sandbox caps storage at 10 GB** → large results fail with
  `Quota exceeded: free storage`. Attach a billing account (still ~$0).
- **Parquet corruption is silent** — Docker's Windows bind-mount corrupted
  ~1,700 row groups of the first `Projects.parquet` under heavy concurrent writes.
  Size **and** SHA still "matched" (intra-file bad pages, not truncation); only a
  full row-group decode caught it (`../../scripts/verify_parquet.py`). Re-pulled
  clean.
- **DuckDB's Python fetch is buggy on deeply-nested columns** — `fetchmany` →
  "integer cast", `fetch_record_batch`/`fetch_arrow_table` → crash/OOM. Fix:
  wrap nested columns in `to_json()` (all columns become scalar) **and** stream
  with `fetch_record_batch` (bounded memory).
- **pyarrow footer parse hangs** on these many-row-group files → generate
  schema/croissant from the *known* schema (`emit_meta.py`), don't re-read.
- **External-build spill ≈ input NT size** — but a matching-size local NT + spill
  won't both fit tight disk; `gzip -1` the NT (~13×) and stream it into the build.
- **In-RAM `rete build` OOMs** past ~149 M triples on a 47 GB VM (even
  `--no-pyramid`); use `--memory-budget-mb`. The `types` pyramid is the slow part
  and isn't needed for SPARQL — shards are `--no-pyramid`.
- **Federation ≠ joins** — client federation is UNION + term routing, so a query
  joining a subject in one shard to an object's metadata in another won't resolve.
