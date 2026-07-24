#!/usr/bin/env python3
"""Profile the GH Archive Parquet tables in data/github-archive/data/.

Usage (Docker-only, from the repo root):
    MSYS_NO_PATHCONV=1 docker run --rm -v "D:/pro/rete:/w" -w //w \
      python:3.12-slim bash -c \
      "pip -q install duckdb pandas && python data/github-archive/scripts/report.py"

NOTE: this file is deliberately NOT named inspect.py — a script named
inspect.py shadows the stdlib `inspect` module and breaks duckdb's import.

Prints global stats plus three case studies (one human user, one org, one
repo) showing what a day of events reveals about each entity.
"""
import pathlib

import duckdb

DATA = (pathlib.Path(__file__).resolve().parent.parent / "data").as_posix()
con = duckdb.connect()
for t in ("events", "push_commits", "pull_requests", "issues", "releases"):
    con.execute(f"CREATE VIEW {t} AS SELECT * FROM '{DATA}/{t}/*.parquet'")


def show(title, sql, max_rows=25):
    print(f"\n== {title} " + "=" * max(0, 70 - len(title)))
    print(con.execute(sql).fetchdf().to_string(index=False, max_rows=max_rows))


show("Day at a glance", """
    SELECT count(*) AS events,
           count(DISTINCT actor_id) AS actors,
           count(DISTINCT repo_id)  AS repos,
           count(DISTINCT org_id)   AS orgs,
           min(created_at) AS first_event, max(created_at) AS last_event
    FROM events
""")

show("Events by type", """
    SELECT type, count(*) AS n,
           round(100.0 * count(*) / sum(count(*)) OVER (), 1) AS pct
    FROM events GROUP BY type ORDER BY n DESC
""")

show("Bot share (login ends in [bot])", """
    SELECT (actor_login ILIKE '%[bot]' OR actor_login ILIKE '%-bot') AS is_bot,
           count(*) AS events, count(DISTINCT actor_id) AS actors
    FROM events GROUP BY 1
""")

# --- pick case-study entities: busiest human user, busiest org, a repo with
# --- diverse activity (not just pushes)
user = con.execute("""
    SELECT actor_login FROM events
    WHERE actor_login NOT ILIKE '%[bot]' AND actor_login NOT ILIKE '%-bot'
      AND actor_login NOT ILIKE 'bot-%'
    GROUP BY 1
    HAVING count(DISTINCT type) >= 3 AND count(DISTINCT repo_id) >= 3
       AND count(*) < 2000  -- unlabelled automation pushes 10k+/day
    ORDER BY count(DISTINCT type) DESC, count(*) DESC LIMIT 1
""").fetchone()[0]

org = con.execute("""
    SELECT org_login FROM events WHERE org_login IS NOT NULL
    GROUP BY 1 ORDER BY count(DISTINCT repo_id) DESC, count(*) DESC LIMIT 1
""").fetchone()[0]

repo = con.execute("""
    SELECT repo_name FROM events
    GROUP BY 1
    ORDER BY count(DISTINCT type) DESC, count(DISTINCT actor_id) DESC LIMIT 1
""").fetchone()[0]

print(f"\n#### case studies: user={user!r} org={org!r} repo={repo!r}")

show(f"USER {user}: what they did (per type/action)", f"""
    SELECT type, action, count(*) AS n,
           count(DISTINCT repo_name) AS repos_touched
    FROM events WHERE actor_login = '{user}'
    GROUP BY 1, 2 ORDER BY n DESC
""")

show(f"USER {user}: repos they touched", f"""
    SELECT repo_name, org_login, count(*) AS events,
           list(DISTINCT type) AS event_types
    FROM events WHERE actor_login = '{user}'
    GROUP BY 1, 2 ORDER BY events DESC LIMIT 15
""")

show(f"USER {user}: hour-of-day activity histogram", f"""
    SELECT hour(created_at) AS utc_hour, count(*) AS n,
           repeat('#', count(*)::INT) AS bar
    FROM events WHERE actor_login = '{user}' GROUP BY 1 ORDER BY 1
""")

show(f"ORG {org}: repo portfolio active today", f"""
    SELECT repo_name, count(*) AS events, count(DISTINCT actor_login) AS actors,
           list(DISTINCT type) AS event_types
    FROM events WHERE org_login = '{org}'
    GROUP BY 1 ORDER BY events DESC LIMIT 15
""")

show(f"ORG {org}: external vs member-ish contributors (top actors)", f"""
    SELECT actor_login, count(*) AS events,
           count(DISTINCT repo_name) AS repos,
           list(DISTINCT type) AS event_types
    FROM events WHERE org_login = '{org}'
    GROUP BY 1 ORDER BY events DESC LIMIT 15
""")

show(f"REPO {repo}: full activity ledger by type/action", f"""
    SELECT type, action, count(*) AS n, count(DISTINCT actor_login) AS actors
    FROM events WHERE repo_name = '{repo}'
    GROUP BY 1, 2 ORDER BY n DESC
""")

show(f"REPO {repo}: PRs opened/merged today", f"""
    SELECT action, number, pr_author, merged, additions, deletions,
           changed_files, left(title, 60) AS title
    FROM pull_requests WHERE repo_name = '{repo}'
    ORDER BY event_id DESC LIMIT 15
""")

show(f"REPO {repo}: pushes by branch/actor (head shas survive the 2025 feed cut)", f"""
    SELECT ref, actor_login, count(*) AS pushes,
           any_value(push_head) AS example_head_sha
    FROM events WHERE repo_name = '{repo}' AND type = 'PushEvent'
    GROUP BY 1, 2 ORDER BY pushes DESC LIMIT 10
""")

show("Commit-level detail availability (push_commits is EMPTY for post-Aug-2025 days)", """
    SELECT count(*) AS commit_rows FROM push_commits
""")

show(f"REPO {repo}: who starred / forked it today", f"""
    SELECT type, count(*) AS n, list(actor_login)[:12] AS sample_actors
    FROM events WHERE repo_name = '{repo}' AND type IN ('WatchEvent','ForkEvent')
    GROUP BY 1
""")

show("Releases today: biggest publishers", """
    SELECT repo_name, count(*) AS releases, list(tag_name)[:6] AS sample_tags
    FROM releases GROUP BY 1 ORDER BY releases DESC LIMIT 10
""")

show("Cross-entity graph shape: actor↔repo edge counts", """
    SELECT count(*) AS actor_repo_pairs,
           count(DISTINCT actor_id) AS actors, count(DISTINCT repo_id) AS repos
    FROM (SELECT DISTINCT actor_id, repo_id FROM events)
""")
