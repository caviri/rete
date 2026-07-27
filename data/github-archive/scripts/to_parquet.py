#!/usr/bin/env python3
"""Convert one UTC day of GH Archive hourly .json.gz files into Parquet tables.

Usage (Docker-only, from the repo root):
    MSYS_NO_PATHCONV=1 docker run --rm -v "D:/pro/rete:/w" -w //w \
      -e DAY=2026-07-22 python:3.12-slim bash -c \
      "pip -q install duckdb && python data/github-archive/scripts/to_parquet.py"

Outputs into data/github-archive/data/, ONE PARQUET PER TABLE PER HOUR
(memory-bounded — a whole day in one DuckDB query OOMs at ~5.5 GiB):
    events/<DAY>-<H>.parquet        common envelope + cheap per-type scalars
    push_commits/<DAY>-<H>.parquet  one row per commit inside PushEvents
                                    (EMPTY for days after ~Aug 2025: GitHub
                                    stripped commits[] from the public feed;
                                    only head/before shas remain, kept in
                                    events.push_head / events.push_before)
    pull_requests/<DAY>-<H>.parquet one row per PullRequestEvent
    issues/<DAY>-<H>.parquet        IssuesEvent + IssueCommentEvent rows
    releases/<DAY>-<H>.parquet      one row per ReleaseEvent

Read each table as a glob: SELECT * FROM 'data/events/*.parquet'.
The raw payload is NOT kept in events (issue/PR bodies dominate the size);
the detail tables carry the useful payload fields instead.
"""
import os
import pathlib

import duckdb

DAY = os.environ.get("DAY", "2026-07-22")
BASE = pathlib.Path(__file__).resolve().parent.parent
RAW = BASE / "raw"
OUT = BASE / "data"

TABLES = {
    "events": """
        SELECT
            id::BIGINT                              AS id,
            type,
            created_at,
            public,
            (actor->>'id')::BIGINT                  AS actor_id,
            actor->>'login'                         AS actor_login,
            (repo->>'id')::BIGINT                   AS repo_id,
            repo->>'name'                           AS repo_name,
            (org->>'id')::BIGINT                    AS org_id,
            org->>'login'                           AS org_login,
            payload->>'action'                      AS action,
            COALESCE(payload->'pull_request'->>'number',
                     payload->'issue'->>'number',
                     payload->>'number')::BIGINT    AS number,
            payload->>'ref'                         AS ref,
            payload->>'ref_type'                    AS ref_type,
            (payload->>'size')::INT                 AS push_size,
            (payload->>'distinct_size')::INT        AS push_distinct_size,
            (payload->>'push_id')::BIGINT           AS push_id,
            payload->>'head'                        AS push_head,
            payload->>'before'                      AS push_before,
            payload->'forkee'->>'full_name'         AS forkee_full_name
        FROM {src}
    """,
    "push_commits": """
        SELECT
            e.id::BIGINT                            AS event_id,
            e.created_at,
            e.actor->>'login'                       AS actor_login,
            e.repo->>'name'                         AS repo_name,
            e.payload->>'ref'                       AS ref,
            c.value->>'sha'                         AS sha,
            c.value->'author'->>'name'              AS author_name,
            c.value->'author'->>'email'             AS author_email,
            left(c.value->>'message', 4000)         AS message,
            (c.value->>'distinct')::BOOLEAN         AS distinct_commit
        FROM {src} AS e, json_each(e.payload->'commits') AS c
        WHERE e.type = 'PushEvent'
    """,
    "pull_requests": """
        SELECT
            id::BIGINT                                          AS event_id,
            created_at,
            actor->>'login'                                     AS actor_login,
            repo->>'name'                                       AS repo_name,
            payload->>'action'                                  AS action,
            (payload->'pull_request'->>'number')::BIGINT        AS number,
            payload->'pull_request'->>'title'                   AS title,
            payload->'pull_request'->>'state'                   AS state,
            (payload->'pull_request'->>'merged')::BOOLEAN       AS merged,
            payload->'pull_request'->'user'->>'login'           AS pr_author,
            payload->'pull_request'->'merged_by'->>'login'      AS merged_by,
            (payload->'pull_request'->>'additions')::BIGINT     AS additions,
            (payload->'pull_request'->>'deletions')::BIGINT     AS deletions,
            (payload->'pull_request'->>'changed_files')::BIGINT AS changed_files,
            (payload->'pull_request'->>'commits')::BIGINT       AS commits,
            payload->'pull_request'->'base'->>'ref'             AS base_ref,
            payload->'pull_request'->'head'->>'ref'             AS head_ref,
            payload->'pull_request'->>'created_at'              AS pr_created_at,
            payload->'pull_request'->>'merged_at'               AS pr_merged_at
        FROM {src}
        WHERE type = 'PullRequestEvent'
    """,
    "issues": """
        SELECT
            id::BIGINT                                  AS event_id,
            created_at,
            type,
            actor->>'login'                             AS actor_login,
            repo->>'name'                               AS repo_name,
            payload->>'action'                          AS action,
            (payload->'issue'->>'number')::BIGINT       AS number,
            payload->'issue'->>'title'                  AS title,
            payload->'issue'->>'state'                  AS state,
            payload->'issue'->'user'->>'login'          AS issue_author,
            (payload->'issue'->>'comments')::BIGINT     AS comments,
            (SELECT list(l.value->>'name')
               FROM json_each(payload->'issue'->'labels') AS l) AS labels,
            left(payload->'comment'->>'body', 4000)     AS comment_body
        FROM {src}
        WHERE type IN ('IssuesEvent', 'IssueCommentEvent')
    """,
    # repo_snapshots is generated below — embedded full repo objects from
    # PR base/head + forkee. NOTE: extraction must stay inside each branch;
    # passing a JSON column through a subquery boundary trips a DuckDB
    # binder bug ("Failed to cast value to numerical ... source column").
    "repo_snapshots": None,
    "releases": """
        SELECT
            id::BIGINT                                      AS event_id,
            created_at,
            actor->>'login'                                 AS actor_login,
            repo->>'name'                                   AS repo_name,
            payload->'release'->>'tag_name'                 AS tag_name,
            payload->'release'->>'name'                     AS release_name,
            (payload->'release'->>'prerelease')::BOOLEAN    AS prerelease,
            (SELECT count(*)
               FROM json_each(payload->'release'->'assets')) AS asset_count
        FROM {src}
        WHERE type = 'ReleaseEvent'
    """,
}

_SNAP_BRANCH = """
    SELECT
        id::BIGINT                          AS event_id,
        created_at                          AS observed_at,
        {p}->>'full_name'                   AS full_name,
        ({p}->>'id')::BIGINT                AS repo_gh_id,
        '{role}'                            AS role,
        {p}->'owner'->>'login'              AS owner_login,
        {p}->'owner'->>'type'               AS owner_type,
        left({p}->>'description', 2000)     AS description,
        {p}->>'homepage'                    AS homepage,
        {p}->>'language'                    AS language,
        {p}->'license'->>'spdx_id'          AS license_spdx,
        (SELECT list(t.value->>'$')
           FROM json_each({p}->'topics') AS t) AS topics,
        ({p}->>'stargazers_count')::BIGINT  AS stars,
        ({p}->>'forks_count')::BIGINT       AS forks,
        ({p}->>'open_issues_count')::BIGINT AS open_issues,
        ({p}->>'size')::BIGINT              AS size_kb,
        ({p}->>'fork')::BOOLEAN             AS is_fork,
        ({p}->>'archived')::BOOLEAN         AS archived,
        {p}->>'default_branch'              AS default_branch,
        {p}->>'created_at'                  AS repo_created_at,
        {p}->>'pushed_at'                   AS repo_pushed_at
    FROM {{src}}
    WHERE type = '{etype}'
"""
# NOTE: no JSON extraction in WHERE clauses — this DuckDB build miscompiles
# "type = '...' AND payload->...->>'k' IS NOT NULL" into a cast of the whole
# JSON column ("Failed to cast value to numerical"). Filter on the extracted
# VARCHAR column in an outer SELECT instead.

TABLES["repo_snapshots"] = (
    "SELECT * FROM ("
    + " UNION ALL ".join(
        _SNAP_BRANCH.format(p=p, role=role, etype=etype)
        for p, role, etype in [
            ("payload->'pull_request'->'base'->'repo'", "pr_base", "PullRequestEvent"),
            ("payload->'pull_request'->'head'->'repo'", "pr_head", "PullRequestEvent"),
            ("payload->'forkee'", "forkee", "ForkEvent"),
        ])
    + ") WHERE full_name IS NOT NULL"
    + " QUALIFY row_number() OVER (PARTITION BY full_name, role"
    + " ORDER BY observed_at DESC) = 1"
)

con = duckdb.connect()
con.execute("SET preserve_insertion_order=false; SET threads=2; "
            "SET memory_limit='12GB'; SET temp_directory='/tmp/duck_spill';")

for table in TABLES:
    (OUT / table).mkdir(parents=True, exist_ok=True)

for hour in range(24):
    gz = RAW / f"{DAY}-{hour}.json.gz"
    if not gz.exists():
        print(f"MISSING {gz.name} — skipped")
        continue
    src = (f"read_json('{gz.as_posix()}', format='newline_delimited', "
           "columns={id:'VARCHAR', type:'VARCHAR', public:'BOOLEAN', "
           "created_at:'TIMESTAMP', actor:'JSON', repo:'JSON', org:'JSON', "
           "payload:'JSON'}, maximum_object_size=134217728)")
    for table, sql in TABLES.items():
        path = (OUT / table / f"{DAY}-{hour}.parquet").as_posix()
        if os.path.exists(path):
            continue
        # write to a tmp name then rename: several day-pipelines run
        # concurrently over the same table dirs, and a half-written file
        # must never be visible under its final name
        con.execute(f"COPY ({sql.format(src=src)}) TO '{path}.tmp' "
                    "(FORMAT parquet, COMPRESSION zstd)")
        os.replace(f"{path}.tmp", path)
    print(f"hour {hour:2d} done")

for table in TABLES:
    # per-DAY glob only: a cross-day glob races with other concurrently
    # running day-pipelines and can read a file mid-write
    glob = (OUT / table / f"{DAY}-*.parquet").as_posix()
    n = con.execute(f"SELECT count(*) FROM '{glob}'").fetchone()[0]
    size = sum(f.stat().st_size
               for f in (OUT / table).glob(f"{DAY}-*.parquet"))
    print(f"{table:14s} {n:>12,} rows  {size/1e6:8,.1f} MB")
print("done ->", OUT)
