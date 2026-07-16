#!/usr/bin/env python3
"""Open-Pulse chosen JSON-LD  ->  per-class Parquet tables + DuckDB.

Reads the dedup manifest produced by build_nt.py (chosen_files.tsv) and projects
the ontology graph into a small relational schema — one wide table per class plus
edge tables — that a data scientist can query with pandas/DuckDB without touching
RDF.

Tables (under <out-dir>/parquet/):
  repositories.parquet          seed schema:SoftwareSourceCode rows
  users.parquet                 seed schema:Person rows
  organizations.parquet         seed org:Organization rows
  contributions.parquet         pulse:Contribution edges (person -> repo)
  memberships.parquet           org:Membership edges (person -> org)
  scholarly_articles.parquet    schema:ScholarlyArticle rows

Seed rows come from the file whose source_url IS that entity (authoritative,
richest). Edge/article rows are collected from every chosen graph and deduped by
@id. Finally everything is loaded into <out-dir>/open-pulse.duckdb.
"""
import argparse
import json
import os

import pyarrow as pa
import pyarrow.parquet as pq


GH2 = "https://github.com/https://github.com/"
GH1 = "https://github.com/"


def norm_iri(s):
    """Collapse the extractor's double github:// prefix bug (matches build_nt)."""
    if not isinstance(s, str) or GH2 not in s:
        return s
    while GH2 in s:
        s = s.replace(GH2, GH1)
    return s


# ---- value coercion helpers ------------------------------------------------
def _id(v):
    if isinstance(v, dict):
        return norm_iri(v.get("@id"))
    if isinstance(v, str):
        return norm_iri(v)
    return None


def ids(v):
    if v is None:
        return []
    if isinstance(v, list):
        return [x for x in (_id(e) for e in v) if x]
    x = _id(v)
    return [x] if x else []


def sc(v):
    """A single scalar (str/int/float/bool); drop refs/lists/null."""
    if isinstance(v, (str, int, float, bool)):
        return v
    return None


def sid(v):
    return _id(v)


def strs(v):
    if v is None:
        return []
    if isinstance(v, list):
        return [x for x in v if isinstance(x, str)]
    return [v] if isinstance(v, str) else []


G = "gme-internal:"


def repo_row(n):
    return {
        "id": norm_iri(n.get("@id")),
        "name": sc(n.get("schema:name")),
        "handle": sc(n.get("pulse:githubRepositoryHandle")),
        "repository_type": sid(n.get("pulse:repositoryType")),
        "owned_by": sid(n.get("pulse:ownedBy")),
        "is_fork_of": sid(n.get("pulse:isForkOf")),
        "stars": sc(n.get("pulse:githubRepoStars")),
        "forks": sc(n.get("pulse:githubRepoForks")),
        "date_created": sc(n.get("schema:dateCreated")),
        "license": sid(n.get("schema:license")),
        "primary_language": sc(n.get(G + "primary_language")),
        "description": sc(n.get(G + "description")),
        "homepage": sc(n.get(G + "homepage")),
        "size_kb": sc(n.get(G + "size_kb")),
        "archived": sc(n.get(G + "archived")),
        "disabled": sc(n.get(G + "disabled")),
        "open_issues": sc(n.get(G + "open_issues_count")),
        "watchers": sc(n.get(G + "watchers_count")),
        "subscribers": sc(n.get(G + "subscribers_count")),
        "pushed_at": sc(n.get(G + "pushed_at")),
        "updated_at": sc(n.get(G + "updated_at")),
        "license_name": sc(n.get(G + "license_name")),
        "has_issues": sc(n.get(G + "has_issues")),
        "has_wiki": sc(n.get(G + "has_wiki")),
        "has_pages": sc(n.get(G + "has_pages")),
        "has_discussions": sc(n.get(G + "has_discussions")),
        "programming_language": strs(n.get("schema:programmingLanguage")),
        "keywords": strs(n.get(G + "keywords")),
        "discipline": ids(n.get("pulse:discipline")),
        "citations": ids(n.get("schema:citation")),
        "authors": ids(n.get("schema:author")),
    }


def user_row(n):
    return {
        "id": norm_iri(n.get("@id")),
        "name": sc(n.get("schema:name")),
        "github_url": sid(n.get("pulse:githubUsername")),
        "orcid": sid(n.get("pulse:orcidIdentifier")),
        "infoscience_id": sc(n.get("pulse:infosciencePersonIdentifier")),
        "url": sid(n.get("schema:url")),
        "email": sc(n.get("schema:email")),
        "location": sc(n.get(G + "location")),
        "company": sc(n.get(G + "company")),
        "blog": sc(n.get(G + "blog")),
        "bio": sc(n.get(G + "bio")),
        "avatar_url": sc(n.get(G + "avatar_url")),
        "public_repos": sc(n.get(G + "public_repos")),
        "followers": sc(n.get(G + "followers_count")),
        "following": sc(n.get(G + "following_count")),
        "github_created_at": sc(n.get(G + "github_created_at")),
        "github_updated_at": sc(n.get(G + "github_updated_at")),
        "is_stub": bool(n.get(G + "stub")) or None,
        "owns": ids(n.get("pulse:owns")),
    }


def _denorm_github(iri):
    """Repair the extractor's double-prefixed org IRI for a clean join key."""
    if isinstance(iri, str):
        while iri.startswith("https://github.com/https://github.com/"):
            iri = iri[len("https://github.com/"):]
    return iri


def _github_link(n):
    """Clean GitHub URL for an org: explicit handle, else a github html_url."""
    h = _denorm_github(sid(n.get("pulse:githubOrganizationHandle")))
    if h:
        return h
    html = sc(n.get(G + "html_url"))
    return html if isinstance(html, str) and "github.com/" in html else None


def _nested(v, key):
    return v.get(key) if isinstance(v, dict) else None


def org_row(n):
    country = n.get(G + "ror_country")
    return {
        "id": norm_iri(n.get("@id")),
        "github_url": _github_link(n),
        "name": sc(n.get("schema:name")),
        "handle": sid(n.get("pulse:githubOrganizationHandle")),
        "is_ror": str(n.get("@id", "")).startswith("https://ror.org/") or None,
        "org_type": sid(n.get("pulse:OrganizationType")),
        "identifier": sc(n.get("schema:identifier")),
        "infoscience_id": sc(n.get("pulse:infoscienceOrganizationIdentifier")),
        "followers": sc(n.get("pulse:githubOrgFollowers")),
        "unit_of": ids(n.get("org:unitOf")),
        "location": sc(n.get(G + "location")),
        "description": sc(n.get(G + "description")),
        "blog": sc(n.get(G + "blog")),
        "email": sc(n.get(G + "email")),
        "company": sc(n.get(G + "company")),
        "ror_country": _nested(country, "country_name"),
        "ror_country_code": _nested(country, "country_code"),
        "ror_status": sc(n.get(G + "ror_status")),
        "ror_established": sc(n.get(G + "ror_established")),
        "ror_types": strs(n.get(G + "ror_types")),
        "aliases": strs(n.get(G + "aliases")),
        "acronyms": strs(n.get(G + "acronyms")),
        "public_repos": sc(n.get(G + "public_repos")),
        "github_created_at": sc(n.get(G + "github_created_at")),
    }


def contribution_row(n):
    return {
        "id": norm_iri(n.get("@id")),
        "author": sid(n.get("schema:author")),
        "contribution_to": sid(n.get("pulse:contributionTo")),
        "count": sc(n.get("pulse:contributionCount")),
        "first_date": sc(n.get("pulse:firstContributionDate")),
        "last_date": sc(n.get("pulse:lastContributionDate")),
    }


def membership_row(n):
    _id_ = norm_iri(n.get("@id")) or ""
    person = _id_.split("__", 1)[0] if "__" in _id_ else None
    return {
        "id": _id_,
        "person": person,
        "organization": sid(n.get("org:organization")),
        "role": sc(n.get("org:role")),
        "begin": sc(n.get("time:hasBeginning")),
        "end": sc(n.get("time:hasEnd")),
    }


def article_row(n):
    return {
        "id": norm_iri(n.get("@id")),
        "name": sc(n.get("schema:name")),
        "identifier": sc(n.get("schema:identifier")),
        "date_published": sc(n.get("schema:datePublished")),
        "infoscience_id": sc(n.get("pulse:infoscienceArticleIdentifier")),
        "source_organization": sid(n.get("schema:sourceOrganization")),
        "authors": ids(n.get("schema:author")),
    }


def type_of(n):
    t = n.get("@type")
    return t if isinstance(t, str) else (t[0] if isinstance(t, list) and t else None)


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--out-dir", required=True)
    args = ap.parse_args()
    pdir = os.path.join(args.out_dir, "parquet")
    os.makedirs(pdir, exist_ok=True)

    manifest = os.path.join(args.out_dir, "chosen_files.tsv")
    # Faithful all-nodes projection: every typed node from every chosen graph,
    # deduped by @id keeping the *richest* copy (a node is fullest where it was
    # the crawl seed; elsewhere it appears as a stub). This mirrors the .rete
    # node set so every edge foreign-key resolves to a row.
    ROW = {
        "schema:SoftwareSourceCode": repo_row,
        "schema:Person": user_row,
        "org:Organization": org_row,
        "pulse:Contribution": contribution_row,
        "org:Membership": membership_row,
        "schema:ScholarlyArticle": article_row,
    }
    tables = {t: {} for t in ROW}   # type -> {id: row}
    nkeys = {}                       # id -> richest key count seen

    with open(manifest, "r", encoding="utf-8") as fh:
        header = fh.readline()  # source_url \t detected_type \t recency \t node_count \t path
        rows = [ln.rstrip("\n").split("\t") for ln in fh]

    for i, cols in enumerate(rows):
        su, path = cols[0], cols[4]
        try:
            with open(path, "r", encoding="utf-8") as f:
                graph = json.load(f)["output"]["@graph"]
        except Exception:
            continue
        for n in graph:
            t = type_of(n)
            fn = ROW.get(t)
            if fn is None:
                continue
            nid = n.get("@id")
            if not nid:
                continue
            k = len(n)
            if nid not in nkeys or k > nkeys[nid]:
                nkeys[nid] = k
                tables[t][nid] = fn(n)
        if (i + 1) % 10000 == 0:
            print(f"  parquet scan {i + 1}/{len(rows)}", flush=True)

    repos = list(tables["schema:SoftwareSourceCode"].values())
    users = list(tables["schema:Person"].values())
    orgs = list(tables["org:Organization"].values())
    contribs = tables["pulse:Contribution"]
    members = tables["org:Membership"]
    articles = tables["schema:ScholarlyArticle"]

    def write(name, records):
        if not records:
            print(f"  (skip {name}: 0 rows)")
            return 0
        tbl = pa.Table.from_pylist(records)
        pq.write_table(tbl, os.path.join(pdir, name), compression="zstd")
        print(f"  wrote {name}: {len(records)} rows, {len(tbl.column_names)} cols")
        return len(records)

    counts = {
        "repositories.parquet": write("repositories.parquet", repos),
        "users.parquet": write("users.parquet", users),
        "organizations.parquet": write("organizations.parquet", orgs),
        "contributions.parquet": write("contributions.parquet", list(contribs.values())),
        "memberships.parquet": write("memberships.parquet", list(members.values())),
        "scholarly_articles.parquet": write("scholarly_articles.parquet", list(articles.values())),
    }
    with open(os.path.join(args.out_dir, "parquet_stats.json"), "w", encoding="utf-8") as fh:
        json.dump(counts, fh, indent=2)
    print(json.dumps(counts, indent=2))


if __name__ == "__main__":
    main()
