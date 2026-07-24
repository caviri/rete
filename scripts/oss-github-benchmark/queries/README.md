# Example queries — OSS GitHub Benchmark

Each query joins several dimensions rather than reading one column, and each was
run against `web/oss-github-benchmark.rete` before being committed.

```bash
.claude/skills/rete-from-graph/scripts/rete sparql /work/web/oss-github-benchmark.rete \
  "$(cat scripts/oss-github-benchmark/queries/01-licensing-practice-by-sector.rq)"
```

| # | Query | Joins |
|---|---|---|
| 01 | Licensing practice by sector | sector → institution → org → repo → SPDX license → stars |
| 02 | Institution engagement funnel | institution → sector → org → repo → stars/commits/contributors/issues, with a closure rate |
| 03 | Cross-institution collaboration | repo → every claiming institution → sector, + stars/commits/license |
| 04 | Developers linked to institutions | person → company text → institution → sector, + followers/repos/location/account age |
| 05 | Repository maintenance health | repo → last-modified bucket → archived/licensed/fork → stars/commits/contributors |
| 06 | Published vs derived audit | institution published totals ↔ the same measures re-summed over its repos |
| 07 | **Table replica — institutions ranking** | reproduces the site's main table (Institution, #repos, Sector, Location, Created, Members), sort `num_repos` DESC |
| 08 | **Table replica — repositories ranking** | reproduces the site's repo table (Name, Institution, Org, Comments, Issues, PRs, Commits, Contributors, Forks, Stars, Own commits, Created, Updated, Fork?, License), sort `num_commits` DESC |
| 09 | **Table replica — users ranking** | reproduces the site's user table (Name, Github user, Company, Location, Twitter, Public repos, Public gists, Followers, Created, Updated), sort `followers` DESC |

Queries 07–09 mirror the three tables `ossbenchmark.com` actually renders (columns
and default sort taken from its Angular `*-ranking.component.ts`). Two honest gaps:
the institutions table uses the published `num_repos`, so it reproduces the site's
double-count (EPFL 1361, not the 1147 distinct — see query 06); and the users table's
"Contributions" column has no source edge (the paginated `/users` endpoint never
exposes it), so query 09 omits that one column.

## What they surface

**01** — Communities are the only sector preferring copyleft (130) over permissive
(90); every other sector inverts that. IT: 5,387 non-fork repos, 2,550 licensed.

**03** — the collaboration surface is dominated by corporate/community stewardship
pairs: VSHN AG + k8up, Puzzle ITC + Hitobito, Adfinis AG + Caluma.

**05** — 66% of code untouched since 2021 carries no license at all, versus 35% of
currently-maintained code. Archiving bumps GitHub's `updated_at`, so archived repos
land in recent buckets — the oldest bucket shows 0 archived for that reason, not
because old code was never retired.

**06** — the published `num_repos` **double-counts when an organization belongs to
two institutions**. `BertschiAG` is claimed by institutions `Bertschi` and
`BertschiAG`, so its 21 repositories are published as 42; the derived
`COUNT(DISTINCT ?repo)` gives the true 21. Same for VSHN/k8up, Puzzle/Hitobito,
Adfinis/Caluma and swisstopo/Swisstopo. The audit also reveals two distinct
institutions sharing the display name "Universität Bern" (shortnames `unibe` and
`Universität Bern`).

## Engine gotchas these queries hit

- **`REGEX` only evaluates a CONSTANT pattern.** `REGEX(?s, "\bFoo\b")` matches 19
  rows; `REGEX(?s, CONCAT("\b","Foo","\b"))` matches **0**, silently, with no error.
  Query 04 therefore does whole-word matching with `REPLACE` (constant pattern) plus
  `CONTAINS` on space-padded strings. `REPLACE` with a constant pattern is fine.
- **`REGEX` inside `BIND` yields unbound**, even for a constant pattern — it is
  usable in `FILTER`.
- **An `OPTIONAL` whose only content is `FILTER NOT EXISTS` + `BIND` never binds.**
  Counting "repos without a license" that way returns 0 everywhere. Count the
  licensed ones and subtract (query 05).
- **Count the entity, not the attribute.** `COUNT(DISTINCT ?license)` caps at the
  number of distinct licenses (31), not repositories — `BIND(?repo AS ?licensedRepo)`
  inside the `OPTIONAL` and count that (query 01).

## What SHACL found that the queries did not

Running `shapes.ttl` over the real graph surfaced two upstream defects and one bug
in this build — none of which the SELECT queries above would have shown:

- `sh:lessThanOrEquals` → **`pull_requests_closed` duplicates `issues_closed`** in
  all 17,661 records, exceeding `pull_requests_all` 3,741 times. PR-closure rates are
  not computable from this source.
- `sh:class` → **286 dangling institution references** from 11 stale upstream keys,
  now kept as `ossb:unlistedInstitutionKey` literals.
- The org-casing split (13 organizations minted as two nodes each) was caught by
  query 02 disagreeing with its SQL equivalent.

## SQL equivalents

The same facts are in `build/oss-github-benchmark-tables/*.parquet` and
`build/oss-github-benchmark.duckdb`. Join repositories to organizations on
**`org_iri`, never on `org`** — the handle's casing is ambiguous (13 organizations
appear under two casings upstream), the IRI is canonical.

```sql
SELECT i.name, i.sector, COUNT(DISTINCT r.repo_iri) AS repos, SUM(r.num_stars) AS stars
FROM repositories r
JOIN organizations o ON r.org_iri = o.org_iri
JOIN institutions  i ON list_contains(o.institution_shortnames, i.shortname)
WHERE NOT r.is_fork
GROUP BY i.name, i.sector ORDER BY stars DESC;
```
