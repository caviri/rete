#!/usr/bin/env python3
"""Consolidate the API harvests (JSONL) + derived pointer tables into parquet/.

The six hub-stats parquets in raw/hub-stats/ stay as-is — they ARE the canonical
models/datasets/spaces/papers/posts tables. This script adds the people layer
and the cross-link (pointer) tables:

  parquet/users.parquet            one row per user profile (incl. numBuckets)
  parquet/orgs.parquet             one row per org profile (incl. numBuckets)
  parquet/profile_misses.parquet   names that 404'd on both endpoints
  parquet/org_members.parquet      org→user edges (members API ∪ user-overview orgs[])
  parquet/followers.parquet        follower→followee edges (users and orgs)
  parquet/following.parquet        user→followed edges (if harvested)
  parquet/repo_papers.parquet      model/dataset → arxiv id  (from tags "arxiv:…")
  parquet/model_base_models.parquet model → base model (+relation)
  parquet/paper_hf_authors.parquet daily-paper → author (name + linked HF user)
  parquet/space_links.parquet      space → referenced models/datasets (from cardData)

Run in Docker:
  MSYS_NO_PATHCONV=1 docker run --rm -v "D:/pro/rete:/w" -w //w python:3.12-slim \
    bash -c "pip -q install duckdb && python data/hugging-face/scripts/to_parquet.py"
"""
import glob
import os
import duckdb

BASE = os.path.join(os.path.dirname(os.path.abspath(__file__)), "..")
HS = os.path.join(BASE, "raw", "hub-stats")
APIRAW = os.path.join(BASE, "raw", "api")
OUT = os.path.join(BASE, "parquet")
os.makedirs(OUT, exist_ok=True)

con = duckdb.connect()
con.execute("SET preserve_insertion_order=false")


def have(pattern):
    return bool(glob.glob(pattern))


# ---- people ----------------------------------------------------------------
prof_glob = os.path.join(APIRAW, "profiles", "profiles-*.jsonl")
if have(prof_glob):
    con.execute(f"""
    CREATE TEMP TABLE profiles AS
    SELECT * FROM read_json('{prof_glob}', format='newline_delimited',
                            union_by_name=true, maximum_object_size=33554432, ignore_errors=true)
    """)
    # newest record per name wins (re-runs may duplicate)
    con.execute("""
    CREATE TEMP TABLE profiles_1 AS
    SELECT * FROM (SELECT *, row_number() OVER (PARTITION BY name ORDER BY fetched_at DESC) rn
                   FROM profiles) WHERE rn = 1
    """)

    con.execute(f"""
    COPY (
      SELECT name, data._id AS _id, data.fullname AS fullname, data.avatarUrl AS avatar_url,
             data.isPro AS is_pro,
             data.numModels AS num_models, data.numDatasets AS num_datasets,
             data.numSpaces AS num_spaces, data.numKernels AS num_kernels,
             data.numBuckets AS num_buckets, data.numDiscussions AS num_discussions,
             data.numPapers AS num_papers, data.numUpvotes AS num_upvotes,
             data.numLikes AS num_likes, data.numFollowers AS num_followers,
             data.numFollowing AS num_following, data.numFollowingOrgs AS num_following_orgs,
             fetched_at
      FROM profiles_1 WHERE status='ok' AND kind='user'
    ) TO '{OUT}/users.parquet' (FORMAT parquet, COMPRESSION zstd)
    """)

    con.execute(f"""
    COPY (
      SELECT name, data._id AS _id, data.fullname AS fullname, data.avatarUrl AS avatar_url,
             data.details AS details, data.isVerified AS is_verified, data.plan AS plan,
             data.numUsers AS num_users, data.numModels AS num_models,
             data.numDatasets AS num_datasets, data.numSpaces AS num_spaces,
             data.numKernels AS num_kernels, data.numBuckets AS num_buckets,
             data.numPapers AS num_papers, data.numFollowers AS num_followers,
             fetched_at
      FROM profiles_1 WHERE status='ok' AND kind='org'
    ) TO '{OUT}/orgs.parquet' (FORMAT parquet, COMPRESSION zstd)
    """)

    con.execute(f"""
    COPY (SELECT name, status, fetched_at FROM profiles_1 WHERE status != 'ok')
    TO '{OUT}/profile_misses.parquet' (FORMAT parquet, COMPRESSION zstd)
    """)

    # org membership from the user side (overview.orgs[]) …
    con.execute("""
    CREATE TEMP TABLE member_edges AS
    SELECT o.name AS org, p.name AS "user", 'user_overview' AS source
    FROM (SELECT name, unnest(data.orgs) AS o FROM profiles_1
          WHERE status='ok' AND kind='user') p(name, o)
    """)
    # … plus the members API if harvested
    mem_glob = os.path.join(APIRAW, "members", "members-*.jsonl")
    if have(mem_glob):
        con.execute(f"""
        INSERT INTO member_edges
        SELECT src AS org, target.user AS "user", 'members_api' AS source
        FROM read_json('{mem_glob}', format='newline_delimited',
                       union_by_name=true, maximum_object_size=33554432, ignore_errors=true)
        WHERE kind = 'members' AND target.user IS NOT NULL
        """)
    con.execute(f"""
    COPY (SELECT org, "user", min(source) AS source FROM member_edges
          WHERE org IS NOT NULL AND "user" IS NOT NULL GROUP BY 1, 2)
    TO '{OUT}/org_members.parquet' (FORMAT parquet, COMPRESSION zstd)
    """)

for kind, cols in [("followers", 'target.user AS follower, src AS followee, src_kind AS followee_kind'),
                   ("following", 'src AS follower, target.user AS followee, target.type AS followee_kind')]:
    g = os.path.join(APIRAW, kind, f"{kind}-*.jsonl")
    if have(g):
        con.execute(f"""
        COPY (SELECT DISTINCT {cols}
              FROM read_json('{g}', format='newline_delimited',
                             union_by_name=true, maximum_object_size=33554432, ignore_errors=true)
              WHERE kind = '{kind}' AND target.user IS NOT NULL)
        TO '{OUT}/{kind}.parquet' (FORMAT parquet, COMPRESSION zstd)
        """)

# ---- pointer tables from hub-stats ------------------------------------------
con.execute(f"""
COPY (
  SELECT 'model' AS repo_type, id AS repo_id, author,
         replace(t, 'arxiv:', '') AS arxiv_id
  FROM (SELECT id, author, unnest(tags) AS t FROM read_parquet('{HS}/models.parquet'))
  WHERE t LIKE 'arxiv:%'
  UNION ALL
  SELECT 'dataset', id, author, replace(t, 'arxiv:', '')
  FROM (SELECT id, author, unnest(tags) AS t FROM read_parquet('{HS}/datasets.parquet'))
  WHERE t LIKE 'arxiv:%'
) TO '{OUT}/repo_papers.parquet' (FORMAT parquet, COMPRESSION zstd)
""")

con.execute(f"""
COPY (
  SELECT id AS model_id, author, baseModels.relation AS relation, b.id AS base_model_id
  FROM (SELECT id, author, baseModels, unnest(baseModels.models) AS b
        FROM read_parquet('{HS}/models.parquet') WHERE baseModels IS NOT NULL)
  WHERE b.id IS NOT NULL
) TO '{OUT}/model_base_models.parquet' (FORMAT parquet, COMPRESSION zstd)
""")

con.execute(f"""
COPY (
  SELECT paper_id, a.name AS author_name, a.user.name AS hf_user,
         a.status AS claim_status, a.hidden AS hidden
  FROM (SELECT paper_id, unnest(paper_authors) AS a
        FROM read_parquet('{HS}/daily_papers.parquet'))
) TO '{OUT}/paper_hf_authors.parquet' (FORMAT parquet, COMPRESSION zstd)
""")

# model → dataset it was trained/tuned on: tags "dataset:…" ∪ card metadata
# ($.datasets can be a JSON array or a bare string)
con.execute(f"""
COPY (
  WITH card_ds AS (
    SELECT id, author,
           unnest(coalesce(try_cast(json_extract(cardData,'$.datasets') AS VARCHAR[]),
                           [json_extract_string(cardData,'$.datasets')])) AS dataset_id
    FROM read_parquet('{HS}/models.parquet') WHERE cardData IS NOT NULL
  ), tag_ds AS (
    SELECT id, author, replace(t, 'dataset:', '') AS dataset_id
    FROM (SELECT id, author, unnest(tags) AS t FROM read_parquet('{HS}/models.parquet'))
    WHERE t LIKE 'dataset:%'
  )
  SELECT id AS model_id, author, dataset_id, min(source) AS source FROM (
    SELECT *, 'card' AS source FROM card_ds WHERE dataset_id IS NOT NULL
    UNION ALL
    SELECT *, 'tags' FROM tag_ds
  ) GROUP BY 1, 2, 3
) TO '{OUT}/model_datasets.parquet' (FORMAT parquet, COMPRESSION zstd)
""")

# space → models/datasets: the Hub-computed links from the expand[] sweep
# (harvest_spaces_links.py) ∪ what the space card declares
sl_glob = os.path.join(APIRAW, "space_links", "spaces-expand-*.jsonl")
con.execute(f"""
COPY (
  WITH api_links AS (
    {f'''SELECT id, split_part(id, '/', 1) AS author, 'model' AS ref_type,
           unnest(models) AS ref_id, 'api' AS source
    FROM read_json('{sl_glob}', format='newline_delimited',
                   union_by_name=true, ignore_errors=true)
    UNION ALL
    SELECT id, split_part(id, '/', 1), 'dataset', unnest(datasets), 'api'
    FROM read_json('{sl_glob}', format='newline_delimited',
                   union_by_name=true, ignore_errors=true)'''
     if have(sl_glob) else
     "SELECT NULL AS id, NULL AS author, NULL AS ref_type, NULL AS ref_id, NULL AS source WHERE false"}
  ), cards AS (
    SELECT id, author, cardData FROM read_parquet('{HS}/spaces.parquet')
    WHERE cardData IS NOT NULL
  ), card_links AS (
    SELECT id, author, 'model' AS ref_type,
           unnest(coalesce(try_cast(json_extract(cardData,'$.models') AS VARCHAR[]), [])) AS ref_id,
           'card' AS source
    FROM cards
    UNION ALL
    SELECT id, author, 'dataset',
           unnest(coalesce(try_cast(json_extract(cardData,'$.datasets') AS VARCHAR[]), [])), 'card'
    FROM cards
  )
  SELECT id AS space_id, author, ref_type, ref_id, min(source) AS source
  FROM (SELECT * FROM api_links UNION ALL SELECT * FROM card_links)
  WHERE ref_id IS NOT NULL AND id IS NOT NULL
  GROUP BY 1, 2, 3, 4
) TO '{OUT}/space_links.parquet' (FORMAT parquet, COMPRESSION zstd)
""")

for f in sorted(glob.glob(os.path.join(OUT, "*.parquet"))):
    n = con.execute(f"SELECT count(*) FROM read_parquet('{f}')").fetchone()[0]
    print(f"{os.path.basename(f):32s} {n:>12,} rows  {os.path.getsize(f)/1e6:8.1f} MB")
