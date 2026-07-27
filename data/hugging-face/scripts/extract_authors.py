#!/usr/bin/env python3
"""Extract the author universe (users + orgs) from the hub-stats parquets.

Output: raw/authors/authors_seed.tsv (+ .parquet) — one row per distinct name,
with a user/org type hint where a source already told us, and activity stats
used to prioritise the API harvest (most-connected names first).

Run in Docker:
  MSYS_NO_PATHCONV=1 docker run --rm -v "D:/pro/rete:/w" -w //w python:3.12-slim \
    bash -c "pip -q install duckdb && python data/hugging-face/scripts/extract_authors.py"
"""
import os
import duckdb

BASE = os.path.join(os.path.dirname(os.path.abspath(__file__)), "..")
HS = os.path.join(BASE, "raw", "hub-stats")
OUT = os.path.join(BASE, "raw", "authors")
os.makedirs(OUT, exist_ok=True)

con = duckdb.connect()

con.execute(f"""
CREATE TEMP TABLE repo_authors AS
WITH m AS (
  SELECT author AS name, count(*) AS n_models, sum(likes) AS likes,
         sum(coalesce(downloadsAllTime,0)) AS downloads
  FROM read_parquet('{HS}/models.parquet') WHERE author IS NOT NULL GROUP BY 1
), d AS (
  SELECT author AS name, count(*) AS n_datasets, sum(likes) AS likes,
         sum(coalesce(downloadsAllTime,0)) AS downloads
  FROM read_parquet('{HS}/datasets.parquet') WHERE author IS NOT NULL GROUP BY 1
), s AS (
  SELECT author AS name, count(*) AS n_spaces, sum(likes) AS likes
  FROM read_parquet('{HS}/spaces.parquet') WHERE author IS NOT NULL GROUP BY 1
)
SELECT coalesce(m.name, d.name, s.name) AS name,
       coalesce(m.n_models, 0) AS n_models,
       coalesce(d.n_datasets, 0) AS n_datasets,
       coalesce(s.n_spaces, 0) AS n_spaces,
       coalesce(m.likes,0) + coalesce(d.likes,0) + coalesce(s.likes,0) AS total_likes,
       coalesce(m.downloads,0) + coalesce(d.downloads,0) AS total_downloads
FROM m FULL JOIN d ON m.name = d.name FULL JOIN s ON coalesce(m.name, d.name) = s.name
""")

# Names with a known type from the daily-papers / posts structs (free classification).
con.execute(f"""
CREATE TEMP TABLE typed_names AS
SELECT DISTINCT name, kind FROM (
  SELECT submittedBy.name AS name, submittedBy.type AS kind
    FROM read_parquet('{HS}/daily_papers.parquet') WHERE submittedBy.name IS NOT NULL
  UNION ALL
  SELECT a.user.name, a.user.type
    FROM (SELECT unnest(paper_authors) AS a FROM read_parquet('{HS}/daily_papers.parquet'))
    WHERE a.user.name IS NOT NULL
  UNION ALL
  SELECT "paper_organization.name", 'org'
    FROM read_parquet('{HS}/daily_papers.parquet') WHERE "paper_organization.name" IS NOT NULL
  UNION ALL
  SELECT name, 'user' FROM read_parquet('{HS}/posts.parquet') WHERE name IS NOT NULL
  UNION ALL
  SELECT m.name, m.type
    FROM (SELECT unnest(mentions) AS m FROM read_parquet('{HS}/posts.parquet'))
    WHERE m.name IS NOT NULL
  UNION ALL
  SELECT c.name, c.type
    FROM (SELECT unnest(commentators) AS c FROM read_parquet('{HS}/posts.parquet'))
    WHERE c.name IS NOT NULL
) WHERE kind IN ('user', 'org')
""")

# One hint per name (org wins ties — org endpoint is the rarer guess).
con.execute("""
CREATE TEMP TABLE seed AS
SELECT coalesce(r.name, t.name) AS name,
       coalesce(t.kind, '') AS kind_hint,
       coalesce(r.n_models, 0) AS n_models,
       coalesce(r.n_datasets, 0) AS n_datasets,
       coalesce(r.n_spaces, 0) AS n_spaces,
       coalesce(r.total_likes, 0) AS total_likes,
       coalesce(r.total_downloads, 0) AS total_downloads
FROM repo_authors r
FULL JOIN (SELECT name, min(kind) AS kind FROM typed_names GROUP BY 1) t ON r.name = t.name
""")

con.execute(f"""
COPY (SELECT * FROM seed
      ORDER BY total_likes DESC, total_downloads DESC,
               n_models + n_datasets + n_spaces DESC, name)
TO '{OUT}/authors_seed.tsv' (DELIMITER '\t', HEADER)
""")
con.execute(f"COPY seed TO '{OUT}/authors_seed.parquet' (FORMAT parquet)")

for row in con.execute("""
  SELECT count(*), sum(CASE WHEN kind_hint='user' THEN 1 ELSE 0 END),
         sum(CASE WHEN kind_hint='org' THEN 1 ELSE 0 END),
         sum(CASE WHEN kind_hint='' THEN 1 ELSE 0 END)
  FROM seed""").fetchall():
    print(f"seed: {row[0]:,} names  (typed user: {row[1]:,}, typed org: {row[2]:,}, unknown: {row[3]:,})")
