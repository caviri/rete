#!/usr/bin/env python3
"""OSS GitHub Benchmark -> per-class Parquet companions + flat triples + DuckDB.

Projects the graph into a small relational schema so the same facts can be queried
in SQL (DuckDB-WASM reads these straight over HTTP range) and compared against the
rete engine.

Tables (under build/oss-github-benchmark-tables/):
  institutions.parquet        235   one row per benchmark institution + aggregates
  organizations.parquet       386   GitHub orgs, with owning institution
  repositories.parquet     17,143   one row per repo, freshest crawl record
  users.parquet           127,180   one row per GitHub login, freshest record
  repo_institution.parquet 17,661   the many-to-many attribution edge
  triples.parquet       2,087,049   every fact, flat (subject, predicate, object …)

The flat table uses the same 7 columns as the other rete datasets:
subject, predicate, object, otype, value, datatype, lang.
"""
from __future__ import annotations

import gzip
import json
import re
from collections import defaultdict
from pathlib import Path

import pyarrow as pa
import pyarrow.parquet as pq

BASE = Path(__file__).resolve().parents[2] / "data" / "digital-sustainability-oss-github-benchmark"
RAW = BASE / "raw"
BUILD = BASE / "build"
OUT = BUILD / "oss-github-benchmark-tables"

# Reuse the converter's license map so SQL and SPARQL agree on SPDX.
import importlib.util

_spec = importlib.util.spec_from_file_location("ossb_nt", Path(__file__).with_name("build_nt.py"))
_mod = importlib.util.module_from_spec(_spec)
_spec.loader.exec_module(_mod)
SPDX, dt, slug = _mod.SPDX, _mod.dt, _mod.slug

INST = "https://w3id.org/rete/oss-benchmark/institution/"


def load(p: Path):
    return json.loads(p.read_text(encoding="utf-8"))


def write(name: str, rows: list[dict], schema: pa.Schema) -> None:
    OUT.mkdir(parents=True, exist_ok=True)
    cols = {f.name: [r.get(f.name) for r in rows] for f in schema}
    tbl = pa.Table.from_pydict(cols, schema=schema)
    pq.write_table(tbl, OUT / f"{name}.parquet", compression="zstd")
    print(f"  {name+'.parquet':30} {len(rows):>9,} rows")


def build_institutions() -> list[dict]:
    paged = load(RAW / "institutions_paginated.json")
    files = load(RAW / "institution_files.json")["shortname_to_file"]
    summary = {i["shortname"]: i for i in paged["institutions"]}
    rows = []
    for sn, stem in files.items():
        f = RAW / "institutions" / f"{stem}.json"
        if not f.exists():
            continue
        d, su = load(f), summary.get(sn, {})
        rows.append({
            "institution_iri": INST + slug(sn),
            "shortname": sn,
            "name": su.get("name_de") or sn,
            "sector": d.get("sector") or su.get("sector"),
            "location": su.get("location"),
            "created_at": dt(su.get("created_at")),
            "avatar": d.get("avatar") or su.get("avatar"),
            "num_repos": d.get("num_repos"),
            "num_orgs": d.get("num_orgs"),
            "num_members": d.get("num_members"),
            "total_contributors": d.get("total_num_contributors"),
            "total_commits": d.get("total_num_commits"),
            "total_stars": d.get("total_num_stars"),
            "total_watchers": d.get("total_num_watchers"),
            "total_forks_in_repos": d.get("total_num_forks_in_repos"),
            "total_issues": d.get("total_issues"),
            "total_issues_closed": d.get("total_issues_closed"),
            "total_pull_requests": d.get("total_pull_requests"),
            "total_pull_requests_closed": d.get("total_pull_requests_closed"),
            "total_comments": d.get("total_comments"),
        })
    s = pa.schema([
        ("institution_iri", pa.string()), ("shortname", pa.string()), ("name", pa.string()),
        ("sector", pa.string()), ("location", pa.string()), ("created_at", pa.string()),
        ("avatar", pa.string()), ("num_repos", pa.int64()), ("num_orgs", pa.int64()),
        ("num_members", pa.int64()), ("total_contributors", pa.int64()),
        ("total_commits", pa.int64()), ("total_stars", pa.int64()),
        ("total_watchers", pa.int64()), ("total_forks_in_repos", pa.int64()),
        ("total_issues", pa.int64()), ("total_issues_closed", pa.int64()),
        ("total_pull_requests", pa.int64()), ("total_pull_requests_closed", pa.int64()),
        ("total_comments", pa.int64()),
    ])
    write("institutions", rows, s)
    return rows


def canonical_orgs() -> dict[str, str]:
    """lowercased handle -> canonical org IRI (see build_nt.py: GitHub org names are
    case-insensitive but IRIs are not; the institution listing has the real casing)."""
    files = load(RAW / "institution_files.json")["shortname_to_file"]
    canon: dict[str, str] = {}
    for stem in set(files.values()):
        f = RAW / "institutions" / f"{stem}.json"
        if not f.exists():
            continue
        for o in load(f).get("orgs") or []:
            nm = (o.get("name") or "").strip()
            u = (o.get("url") or (f"https://github.com/{nm}" if nm else "")).strip()
            if u:
                canon[u.rsplit("/", 1)[-1].lower()] = u
    return canon


CANON: dict[str, str] = {}


def org_iri(handle: str) -> str | None:
    h = (handle or "").strip()
    if not h:
        return None
    return CANON.get(h.lower()) or f"https://github.com/{h}"


def build_organizations() -> None:
    files = load(RAW / "institution_files.json")["shortname_to_file"]
    seen: dict[str, dict] = {}
    for sn, stem in files.items():
        f = RAW / "institutions" / f"{stem}.json"
        if not f.exists():
            continue
        for o in load(f).get("orgs") or []:
            name = (o.get("name") or "").strip()
            url = (o.get("url") or (f"https://github.com/{name}" if name else "")).strip()
            if not url:
                continue
            # An org can belong to several institutions; keep one row per org and
            # list the institutions.
            r = seen.setdefault(url, {
                "org_iri": url, "name": name, "description": o.get("description"),
                "avatar": o.get("avatar"), "created_at": dt(o.get("created_at")),
                "location": o.get("locations"), "email": o.get("email"),
                "institution_shortnames": [], "num_repos_listed": len(o.get("repos") or []),
            })
            r["institution_shortnames"].append(sn)
    # orgs discovered only through repositories
    for f in sorted((RAW / "repositories").glob("page_*.json")):
        for rec in load(f)["repositories"]:
            org = (rec.get("organization") or "").strip()
            if not org:
                continue
            url = org_iri(org)
            if url and url not in seen:
                seen[url] = {"org_iri": url, "name": org, "description": None,
                             "avatar": None, "created_at": None, "location": None,
                             "email": None, "institution_shortnames": [],
                             "num_repos_listed": 0}
    rows = list(seen.values())
    s = pa.schema([
        ("org_iri", pa.string()), ("name", pa.string()), ("description", pa.string()),
        ("avatar", pa.string()), ("created_at", pa.string()), ("location", pa.string()),
        ("email", pa.string()), ("institution_shortnames", pa.list_(pa.string())),
        ("num_repos_listed", pa.int64()),
    ])
    write("organizations", rows, s)


def build_repositories() -> None:
    known = set(load(RAW / "institution_files.json")["shortname_to_file"])
    best: dict[str, dict] = {}
    claims: dict[str, set[str]] = defaultdict(set)
    for f in sorted((RAW / "repositories").glob("page_*.json")):
        for r in load(f)["repositories"]:
            u = (r.get("url") or "").rstrip("/")
            if not u:
                continue
            claims[u].add(r["institution"])
            prev = best.get(u)
            if prev is None or (r.get("timestamp") or "") > (prev.get("timestamp") or ""):
                best[u] = r

    rows, edges = [], []
    for u, r in best.items():
        lic = r.get("license")
        lic = None if lic in ("none", "Other") else lic
        rows.append({
            "repo_iri": u,
            "name": r.get("name"),
            "org": r.get("organization"),
            # Join organizations on this, NOT on `org`: the handle's casing is
            # ambiguous, the IRI is canonical.
            "org_iri": org_iri(r.get("organization") or ""),
            "description": r.get("description"),
            "license_name": lic,
            "spdx_id": SPDX.get(lic) if lic else None,
            "is_fork": str(r.get("fork")).lower() == "true",
            "archived": bool(r.get("archived")),
            "num_stars": r.get("num_stars"),
            "num_forks": r.get("num_forks"),
            "num_watchers": r.get("num_watchers"),
            "num_commits": r.get("num_commits"),
            "num_contributors": r.get("num_contributors"),
            "has_own_commits": r.get("has_own_commits"),
            "issues_all": r.get("issues_all"),
            "issues_closed": r.get("issues_closed"),
            "pull_requests_all": r.get("pull_requests_all"),
            "pull_requests_closed": r.get("pull_requests_closed"),
            "comments": r.get("comments"),
            "created_at": dt(r.get("created_at")),
            "updated_at": dt(r.get("updated_at")),
            "crawled_at": dt(r.get("timestamp")),
            "uuid": r.get("uuid"),
            "num_institutions": len(claims[u]),
            "institution_shortnames": sorted(claims[u]),
        })
        for sn in sorted(claims[u]):
            # `listed` is false for the 11 stale/renamed institution keys that repo
            # records reference but the institution listing does not contain; those
            # rows have no matching institutions.parquet row, so filter on it before
            # joining (they also inflate the published num_repos upstream).
            listed = sn in known
            edges.append({"repo_iri": u, "institution_shortname": sn,
                          "institution_iri": INST + slug(sn) if listed else None,
                          "listed": listed})

    s = pa.schema([
        ("repo_iri", pa.string()), ("name", pa.string()), ("org", pa.string()),
        ("org_iri", pa.string()), ("description", pa.string()), ("license_name", pa.string()), ("spdx_id", pa.string()),
        ("is_fork", pa.bool_()), ("archived", pa.bool_()), ("num_stars", pa.int64()),
        ("num_forks", pa.int64()), ("num_watchers", pa.int64()), ("num_commits", pa.int64()),
        ("num_contributors", pa.int64()), ("has_own_commits", pa.int64()),
        ("issues_all", pa.int64()), ("issues_closed", pa.int64()),
        ("pull_requests_all", pa.int64()), ("pull_requests_closed", pa.int64()),
        ("comments", pa.int64()), ("created_at", pa.string()), ("updated_at", pa.string()),
        ("crawled_at", pa.string()), ("uuid", pa.string()), ("num_institutions", pa.int64()),
        ("institution_shortnames", pa.list_(pa.string())),
    ])
    write("repositories", rows, s)
    write("repo_institution", edges, pa.schema([
        ("repo_iri", pa.string()), ("institution_shortname", pa.string()),
        ("institution_iri", pa.string()), ("listed", pa.bool_()),
    ]))


def build_users() -> None:
    people: dict[str, dict] = {}
    for f in sorted((RAW / "users").glob("page_*.json")):
        for p in load(f)["users"]:
            login = (p.get("login") or "").strip()
            if not login:
                continue
            prev = people.get(login)
            if prev is None or (p.get("updated_at") or "") > (prev.get("updated_at") or ""):
                people[login] = p
    rows = [{
        "person_iri": f"https://github.com/{login}",
        "login": login,
        "name": p.get("name"),
        "company": p.get("company"),
        "location": p.get("location"),
        "twitter_username": p.get("twitter_username"),
        "avatar_url": p.get("avatar_url"),
        "followers": p.get("followers"),
        "public_repos": p.get("public_repos"),
        "public_gists": p.get("public_gists"),
        "created_at": dt(p.get("created_at")),
        "updated_at": dt(p.get("updated_at")),
    } for login, p in people.items()]
    write("users", rows, pa.schema([
        ("person_iri", pa.string()), ("login", pa.string()), ("name", pa.string()),
        ("company", pa.string()), ("location", pa.string()), ("twitter_username", pa.string()),
        ("avatar_url", pa.string()), ("followers", pa.int64()), ("public_repos", pa.int64()),
        ("public_gists", pa.int64()), ("created_at", pa.string()), ("updated_at", pa.string()),
    ]))


_NT = re.compile(r"^(<[^>]*>|_:[^\s]+) (<[^>]*>) (.*) \.$")
_LIT = re.compile(r'^"(.*)"(?:\^\^<([^>]*)>|@([A-Za-z0-9-]+))?$', re.S)
_UNESC = {"\\\\": "\\", '\\"': '"', "\\n": "\n", "\\r": "\r", "\\t": "\t"}


def unescape(s: str) -> str:
    return re.sub(r"\\[\\\"nrt]", lambda m: _UNESC[m.group()], s)


def build_triples() -> None:
    """Flat table: one row per fact, matching the other rete datasets' 7 columns."""
    subj, pred, obj, otype, value, datatype, lang = [], [], [], [], [], [], []
    with gzip.open(BUILD / "oss-github-benchmark.nt.gz", "rt", encoding="utf-8") as f:
        for line in f:
            m = _NT.match(line.rstrip("\n"))
            if not m:
                continue
            s, p, o = m.groups()
            subj.append(s.strip("<>"))
            pred.append(p.strip("<>"))
            obj.append(o)
            if o.startswith("<"):
                otype.append("iri"); value.append(o.strip("<>"))
                datatype.append(None); lang.append(None)
            elif o.startswith("_:"):
                otype.append("bnode"); value.append(o)
                datatype.append(None); lang.append(None)
            else:
                lm = _LIT.match(o)
                otype.append("literal")
                value.append(unescape(lm.group(1)) if lm else o)
                datatype.append(lm.group(2) if lm else None)
                lang.append(lm.group(3) if lm else None)
    OUT.mkdir(parents=True, exist_ok=True)
    tbl = pa.table({"subject": subj, "predicate": pred, "object": obj, "otype": otype,
                    "value": value, "datatype": datatype, "lang": lang})
    pq.write_table(tbl, OUT / "triples.parquet", compression="zstd")
    print(f"  {'triples.parquet':30} {tbl.num_rows:>9,} rows")


def build_duckdb() -> None:
    import duckdb

    db = BUILD / "oss-github-benchmark.duckdb"
    db.unlink(missing_ok=True)
    con = duckdb.connect(str(db))
    for p in sorted(OUT.glob("*.parquet")):
        con.execute(f"CREATE TABLE {p.stem} AS SELECT * FROM read_parquet('{p.as_posix()}')")
    con.close()
    print(f"  {'oss-github-benchmark.duckdb':30} {db.stat().st_size / 1e6:>9,.1f} MB")


def main() -> None:
    global CANON
    CANON = canonical_orgs()
    print("building companions ->", OUT)
    build_institutions()
    build_organizations()
    build_repositories()
    build_users()
    build_triples()
    build_duckdb()


if __name__ == "__main__":
    main()
