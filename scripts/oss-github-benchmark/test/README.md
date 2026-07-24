# Negative fixture for the SHACL shapes

`violations-fixture.nt` is four deliberately broken repositories, one per constraint
family, so a run that reports `conforms: true` on the real graph is meaningful
rather than vacuous — a shape that never fires would otherwise pass silently.

| node | defect | expected component |
|---|---|---|
| `x/bad1` | 9 closed issues out of 5 total | `LessThanOrEqualsConstraintComponent` |
| `x/bad2` | license IRI that is not SPDX | `PatternConstraintComponent` |
| `x/bad3` | `ossb:isFork` as a plain string (the raw API shape) | `DatatypeConstraintComponent` |
| `x/bad4` | negative star count, and no institution attribution | `MinInclusive` + `MinCount` |

```bash
D=data/digital-sustainability-oss-github-benchmark/build
.claude/skills/rete-from-graph/scripts/rete build \
  /work/scripts/oss-github-benchmark/test/violations-fixture.nt \
  /work/scripts/oss-github-benchmark/ossbenchmark.ttl -o /work/$D/bad.rete
.claude/skills/rete-from-graph/scripts/rete shacl /work/$D/bad.rete \
  --shapes /work/scripts/oss-github-benchmark/shapes.ttl
```

Expected with the **full** `shapes.ttl`: `conforms: false (9 result(s))` and a
non-zero exit — the five components above, plus four more because the fixture's
placeholder institution `.../institution/fake` is missing `schema:name`,
`ossb:shortName`, `ossb:sector` and `ossb:numRepos`. Validating `Repository.ttl`
alone yields exactly the five. Anything fewer means a constraint stopped firing.

Note the fixture deliberately does **not** test `pullRequestsClosed >
pullRequestsAll`: that constraint is intentionally absent, because upstream
duplicates its closed-issue count into `pull_requests_closed` and the real data
violates it 3,741 times. See `ossbenchmark.ttl`.
