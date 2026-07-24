# github-archive

One full UTC day of the **GH Archive** public GitHub event stream
(https://www.gharchive.org/), converted to Parquet as groundwork for a
`github-archive.rete`.

- Source: `https://data.gharchive.org/YYYY-MM-DD-H.json.gz` (one gzipped
  NDJSON file per UTC hour, H = 0..23 **without** zero-padding; published
  ~5 minutes past each hour, since 2011-02-12)
- Snapshot day: **2026-07-22** (24 files, 476 MB gz total, ~20 MB/hour)
- Downloaded: 2026-07-23
- License: GH Archive imposes no license of its own on the event data; the
  events are the public GitHub timeline (GitHub ToS apply). Attribution:
  "GH Archive (gharchive.org)". Commit messages / issue titles / comment
  bodies remain © their authors.

> **Not the same thing as the Marketplace link.** The Google Cloud
> Marketplace "GitHub Data / github-repos" dataset
> (`bigquery-public-data.github_repos`) is a BigQuery-ONLY ~3 TB snapshot of
> repo *contents* (files, source text, licenses, languages, commit history)
> limited to open-source-licensed repos — no HTTP download, and `contents`
> alone is ~2.3 TB scanned per naive query (see the deps.dev BigQuery-cost
> lessons). GH Archive, used here, is the *event stream* (who did what,
> when, to which repo) and is freely range-fetchable per hour. GH Archive
> also has its own BigQuery mirror (`githubarchive.day/month/year` tables)
> for historical aggregation without downloading years of hours.

## Layout

```
data/github-archive/
  README.md
  SHA256SUMS.txt            # checksums of the 24 raw hourly files
  report-2026-07-22.txt     # saved profiler output (the case studies below)
  raw/                      # 2026-07-22-{0..23}.json.gz, as-is
    compare/                # one pre-feed-cut hour (2025-07-22-12) for reference
  data/                     # Parquet, one file per table per hour (read as globs)
    events/*.parquet        # 1 row per event: the common envelope
    push_commits/*.parquet  # 1 row per commit inside PushEvents
    pull_requests/*.parquet # 1 row per PullRequestEvent
    issues/*.parquet        # IssuesEvent + IssueCommentEvent rows
    releases/*.parquet      # 1 row per ReleaseEvent
  scripts/
    download.sh             # fetch a day of hourly files (idempotent, resumable)
    to_parquet.py           # DuckDB NDJSON→Parquet, per-hour (memory-bounded)
    report.py               # profiler + user/org/repo case studies
```

## Event model

Every event shares an envelope; `payload` varies by `type`:

| envelope field | meaning |
|---|---|
| `id` | globally unique event id (int64) |
| `type` | one of ~15 event types (PushEvent, PullRequestEvent, IssuesEvent, IssueCommentEvent, WatchEvent=star, ForkEvent, CreateEvent, DeleteEvent, ReleaseEvent, PullRequestReviewEvent, …) |
| `created_at` | UTC timestamp |
| `actor` | `{id, login, avatar_url}` — the user who acted |
| `repo` | `{id, name}` — `name` is `"owner/repo"` |
| `org` | `{id, login}` — only when the repo belongs to an organization |
| `payload` | per-type detail: full PR object (additions/deletions/merged_by/…) for PR events, issue + comment body for issue events, tag for releases, `ref`/`ref_type` for create/delete, forkee for forks. For pushes: `commits[]` + `size` on pre-Aug-2025 days, only `push_id/ref/head/before` after (see below) |

`events/*.parquet` keeps the envelope plus cheap per-type scalars
(`action`, `number`, `ref`, `ref_type`, `push_size`, `forkee_full_name`);
the heavy payload fields live in the four detail tables. Full payloads
(issue bodies, PR diffs stats, review states) can always be re-extracted
from `raw/` — the raw hours are the ground truth.

## ⚠️ The feed collapsed around Aug–Sep 2025 — measure before you trust

Hourly file sizes date the change: 2025-07-22 → 92 MB, 2025-10-22 → 31 MB,
2026-07-22 → 21 MB. Verified against a pre-cut hour kept at
`raw/compare/2025-07-22-12.json.gz`:

| signal | 2025-07-22 hour 12 | 2026-07-22 hour 12 |
|---|---|---|
| PushEvent | 106,122 — full `commits[]` (sha, message, author name+email), `size` | 166,950 — only `push_id, ref, head, before` |
| PullRequestEvent | 13,617 | 77 |
| WatchEvent (star) | 5,099 | 23 |
| ForkEvent | 1,230 | 10 |
| IssuesEvent | 2,814 | 22 |

Pushes *grew*; everything else dropped ~150–200× and push payloads were
stripped to bare shas. The non-push events that do appear still carry the
full rich objects (PR additions/deletions/merged_by, issue labels, comment
bodies, release tags). **For rich multi-signal graphs, build from archive
days before Aug 2025** — the same scripts work unchanged, and
`push_commits` fills with commit-level rows there.

## Dataset shape (2026-07-22, one day)

- **3,927,554 events** · 428,745 distinct actors · 618,999 distinct repos ·
  42,246 distinct orgs · 656,549 distinct actor→repo edges
- By type: PushEvent 93.7%, CreateEvent 4.1%, DeleteEvent 1.8%, then a
  long tail (6,545 PR events, 5,345 issue rows, 1,347 stars, 509 releases)
- Named `…[bot]`/`…-bot` accounts: 3,653 actors → 359,993 events (9.2%);
  plenty more automation is unlabelled (single-repo accounts pushing
  10k+/day)
- Parquet: 251 MB total; `events/` 260.1 MB uncompressed→zstd across 24
  files; detail tables ~2 MB combined

## What one day tells you about one entity (verified in report-2026-07-22.txt)

- **A user** (case study `kim-em`): every public action with timestamp —
  which repos/orgs they touch (11 repos across leanprover, own projects, a
  collaborator's), their role mix (65 pushes, PRs opened/merged, reviews,
  issue comments), branch names they work on, and an hour-of-day activity
  rhythm. Collaboration edges fall out of co-activity in the same repos.
- **An organization** (case study `static-web-apps-testing-org`): which
  repos are alive today, per-repo actor lists, bot-vs-human contributor
  split, release cadence. (Amusingly, the busiest "org" by active repos is
  Azure's synthetic-testing org — 2,851 throwaway repos churned by two
  bots; a nice reminder that activity ≠ importance.)
- **A repository** (case study `n8n-io/n8n`): an activity ledger — pushes
  per branch with head shas and who pushed (feature branches named after
  tickets, a merge-queue bot on master, a release-helper bot on
  `release/2.31.5`), PRs opened, review comments, branch create/delete,
  and the exact users who starred/forked it that day.

What GH Archive canNOT give (any era): profile metadata (name/bio/
followers), repo descriptions/topics/star *totals* (only deltas), file
contents, private activity. Post-cut it also can't give commit messages/
emails or representative star/PR/issue coverage. Those need the GitHub
REST/GraphQL API, or the BigQuery datasets (`githubarchive.*` for history,
`github_repos` for contents).

## Reproduce

```bash
bash data/github-archive/scripts/download.sh 2026-07-22
MSYS_NO_PATHCONV=1 docker run --rm -v "D:/pro/rete:/w" -w //w -e DAY=2026-07-22 \
  python:3.12-slim bash -c "pip -q install duckdb && python data/github-archive/scripts/to_parquet.py"
MSYS_NO_PATHCONV=1 docker run --rm -v "D:/pro/rete:/w" -w //w \
  python:3.12-slim bash -c "pip -q install duckdb pandas && python data/github-archive/scripts/report.py"
```

Gotchas encountered (already handled by the scripts):

- A whole day in one DuckDB `read_json` query OOMs (~5.5 GiB) — convert
  **per hour** and read the output as a glob.
- Don't name a script `inspect.py` — it shadows the stdlib module duckdb
  imports.
- `json_each()` emits its own `id` column — alias the event source and
  qualify `e.id` when unnesting commits.

## The .rete pipeline (raw → Parquet → RDF-star → .rete)

Staged builds from the RICH pre-cut era (July 2025). Parquet is always the
durable intermediate; RDF is emitted from Parquet, never from raw JSON.

```
scripts/download.sh <DAY>                     # raw hours
scripts/to_parquet.py   (DAY=… in Docker)     # 6 tables incl. repo_snapshots
scripts/to_rdf.py       (DAY=… in Docker)     # rdf/<DAY>/{events,commits,prs,issues,releases,snapshots}.nt
scripts/build_rete.sh <name> <DAY> [DAY…]     # merge ontology + day sections → web/<name>.rete
```

Model: real GitHub URLs as entity IRIs (`https://github.com/<login>`,
`…/<owner>/<repo>`, `…/pull/<n>`, `…/commit/<sha>`); every event a
`prov:Activity` subclass node (`gh:PushEvent` …) with `prov:atTime`,
`prov:wasAssociatedWith`, `gh:repo`; PROV + schema.org + SPDX for repo
metadata. **RDF-star provenance**: volatile observations (`gh:stars`,
`gh:forks`, `gh:openIssues`, `gh:pushedAt`) and social edges
(`gh:starred`/`gh:forked`) are annotated
`<< s p o >> prov:generatedAtTime|atTime t ; prov:wasGeneratedBy <event>`.
Commit author emails are NOT emitted to RDF (Parquet keeps them locally).

Built so far (both `rete verify` green):

- **`web/gharchive-2025-07-22.rete`** — 57,532,651 triples, 15.07M terms,
  678 MB, typed pyramid + card. freeCodeCamp's star count observed rising
  423,984 → 423,990 across the day, each observation timestamped and
  linked to its generating event.
- **`web/gharchive-2025-07-16-22.rete`** — 376,292,864 triples, 83.9M
  terms, 4.06 GB, built with `--memory-budget-mb 16384` (external build,
  no pyramid → prefer selective queries). Lazy `sparql-url` reads touch
  only 24–91 MB of the 4 GB file per query. Week-scale provenance works:
  microsoft/vscode's star count tracked hour-by-hour over 7 days.

- **`web/gharchive-2025-07.rete`** — the full month of July 2025:
  **1,644,492,596 triples, 334.5M terms, 17.1 GB** — built by streaming
  all 31 days of `.nt.gz` through
  `{ cat ontology.nt; gzip -dc rdf/2025-07-*/*.nt.gz; } | rete build -
  --format nt --memory-budget-mb 16384` in a detached container (~4.5 h,
  42 chunks). Month-scale provenance: microsoft/vscode's star count
  time-resolved from 174,029 (Jul 1, 00:33) to 175,188 (Jul 31, 23:54);
  a selective query touches ~34 MB of the 17 GB file.

Month pipeline economics: `day_pipeline.py` (download → Parquet →
`GZIP=1` RDF → delete raw) runs ~15–18 min/day for rich-era days
(~4 min/day post-cut); three parallel pipelines cover a month in a few
hours. `.nt.gz` is ~1 GB/day vs 8 GB plain — a plain-NT month (~280 GB)
would not have fit on disk.

## The post-cut feed, measured properly (June 2026, full month)

- **`data/github-archive/gharchive-2026-06.rete`** — 729,745,084 triples,
  163.5M terms, 6.49 GB. 112.2M events: PushEvent 95.4M (85%),
  CreateEvent 10.0M, DeleteEvent 3.9M, PullRequestEvent 1.16M,
  issue events 844k, WatchEvent 310,744, ReleaseEvent 69,837.
- **The cut deepens over time**: stars 3.7M/month (July 2025) → 310,744
  (June 2026, ~12×) → ~200× low by late July 2026 (the single-day
  measurements above). Don't extrapolate one day to an era.
- **Asymmetric survival**: `pull_request` objects are EMPTY (1.16M PR
  events, zero titles/diff-stats/merged-by, no embedded base/head repos —
  so the RDF-star star-count observation layer is forkee-only here:
  68,891 snapshots, all ≤1 star), while ISSUE events kept titles, labels
  and comment bodies, and releases kept tags/names.
- Push `head`/`before` shas exist in the Parquet tables but are not
  emitted to RDF.

Pipeline gotchas (all already handled in the scripts):

- This DuckDB build miscompiles `WHERE type = '…' AND payload->…->>'k' IS
  NOT NULL` (casts the whole JSON column) — never put JSON extraction in a
  WHERE; filter on materialized columns in an outer SELECT.
- The RDF emitter is per-section and resumable (`rdf/<DAY>/<section>.nt`,
  atomic `.part` renames); rows with null essentials (sha, repo_name,
  login) are skipped — one such row used to kill a 40-minute run.
- Long jobs must run as DETACHED docker containers (`docker run -d --name
  …`, no `--rm`) — attached wrappers get killed in this environment and
  `--rm` destroys the crash logs with them.
