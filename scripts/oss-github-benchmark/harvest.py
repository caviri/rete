#!/usr/bin/env python3
"""Harvest the Digital Sustainability OSS GitHub Benchmark (ossbenchmark.com) into raw JSON.

Public API surface (from oss-api/src/api/api.controller.ts):
  GET /api/api/latestUpdate                  -> crawl timestamp
  GET /api/api/completeInstitutionSummaries  -> all institution summaries (AuthGuard commented out upstream)
  GET /api/api/paginatedInstitutions         -> institution summaries + sector histogram
  GET /api/api/singleInstitution?name=<sn>   -> full institution incl. orgs[] and aggregate stats
  GET /api/api/paginatedRepositories         -> repositories (count uncapped)
  GET /api/api/paginatedUsers                -> users (count capped at 200 by the DTO)

Everything lands under data/digital-sustainability-oss-github-benchmark/raw/.
Re-running is safe: existing files are skipped unless --force.
"""
from __future__ import annotations

import argparse
import json
import re
import sys
import time
from pathlib import Path
from urllib.error import HTTPError, URLError
from urllib.parse import urlencode
from urllib.request import Request, urlopen

API = "https://ossbenchmark.com/api/api"
RAW = Path(__file__).resolve().parents[2] / "data" / "digital-sustainability-oss-github-benchmark" / "raw"
UA = "rete-harvester/1.0 (+https://github.com/caviri/rete)"

# The DTO caps users at 200/page; repositories and institutions are effectively uncapped.
USER_PAGE = 200
REPO_PAGE = 2000


def get(path: str, params: dict | None = None, retries: int = 5) -> dict:
    url = f"{API}/{path}"
    if params:
        url += "?" + urlencode(params)
    last = None
    for attempt in range(retries):
        try:
            req = Request(url, headers={"User-Agent": UA, "Accept": "application/json"})
            with urlopen(req, timeout=180) as r:
                return json.loads(r.read().decode("utf-8"))
        except (HTTPError, URLError, TimeoutError, json.JSONDecodeError) as e:
            last = e
            wait = 2 ** attempt
            print(f"    retry {attempt + 1}/{retries} after {wait}s ({e})", file=sys.stderr)
            time.sleep(wait)
    raise RuntimeError(f"GET {url} failed after {retries} tries: {last}")


def safe_names(shortnames: list[str]) -> dict[str, str]:
    """Map shortname -> filename stem that survives a case-insensitive filesystem.

    Upstream shortnames collide on Windows ('Swisstopo' vs 'swisstopo' are two
    distinct institutions) and some carry trailing spaces, which Windows strips.
    The shortname is preserved inside each file, so the stem is cosmetic.
    """
    out: dict[str, str] = {}
    used: set[str] = set()
    for sn in shortnames:
        stem = re.sub(r'[\\/:*?"<>|]', "_", sn).strip(" .") or "unnamed"
        base, i = stem, 1
        while stem.lower() in used:
            i += 1
            stem = f"{base}~{i}"
        used.add(stem.lower())
        out[sn] = stem
    return out


def write(rel: str, obj, force: bool = False) -> Path:
    p = RAW / rel
    p.parent.mkdir(parents=True, exist_ok=True)
    if p.exists() and not force:
        print(f"  skip (exists) {rel}")
        return p
    p.write_text(json.dumps(obj, ensure_ascii=False, indent=1), encoding="utf-8")
    print(f"  wrote {rel} ({p.stat().st_size:,} bytes)")
    return p


def harvest_institutions(force: bool) -> list[str]:
    """Summaries + per-institution detail. Returns the shortname list."""
    print("institutions")
    # NB: /completeInstitutionSummaries exists on main but NOT on the deployed API
    # (it answers with the Angular SPA shell), so paginatedInstitutions with a large
    # count is the authoritative full listing. It also carries the sector histogram.
    paged = get(
        "paginatedInstitutions",
        {"search": "", "sort": "num_repos", "direction": "DESC",
         "page": 0, "count": 100000, "includeForks": "false"},
    )
    write("institutions_paginated.json", paged, force)

    shortnames = [i["shortname"] for i in paged["institutions"]]
    print(f"  {len(shortnames)} institutions")

    stems = safe_names(shortnames)
    write("institution_files.json", {"shortname_to_file": stems}, force=True)

    for n, sn in enumerate(shortnames, 1):
        rel = f"institutions/{stems[sn]}.json"
        if (RAW / rel).exists() and not force:
            continue
        detail = get("singleInstitution", {"name": sn})
        write(rel, detail, force)
        if n % 25 == 0:
            print(f"  ... {n}/{len(shortnames)}")
    return shortnames


def harvest_paged(kind: str, path: str, key: str, page_size: int, extra: dict,
                  force: bool, id_field: str) -> int:
    """Page through a list endpoint, one file per page."""
    print(kind)
    params = {"search": "", "direction": "DESC", "page": 0, "count": 1, **extra}
    total = get(path, params)["total"]
    pages = -(-total // page_size)
    print(f"  {total:,} {kind} over {pages} pages of {page_size}")

    for page in range(pages):
        rel = f"{kind}/page_{page:04d}.json"
        if (RAW / rel).exists() and not force:
            continue
        params = {"search": "", "direction": "DESC", "page": page, "count": page_size, **extra}
        data = get(path, params)
        got = len(data[key])
        write(rel, data, force)
        if got == 0:
            print(f"  page {page} empty — stopping early")
            break

    verify_paged(kind, key, id_field, total, page_size)
    return total


def verify_paged(kind: str, key: str, id_field: str, total: int, page_size: int) -> None:
    """Assert the pages actually cover the collection.

    Mongo paginates by skip/limit, so a non-unique sort key silently reshuffles
    documents between requests: pages then overlap and other records are never
    returned. Distinct ids must equal the reported total.
    """
    seen: set[str] = set()
    records = 0
    short = []
    files = sorted((RAW / kind).glob("page_*.json"))
    for n, f in enumerate(files):
        recs = json.loads(f.read_text(encoding="utf-8"))[key]
        records += len(recs)
        for rec in recs:
            seen.add(rec[id_field])
        # Every page must be full except the last; a short page in the middle means
        # the API skipped records.
        if n < len(files) - 1 and len(recs) != page_size:
            short.append((f.name, len(recs)))
    dup = records - len(seen)
    print(f"  verify: {records:,} records, {len(seen):,} distinct {id_field}, API total {total:,}")

    if records != total:
        raise SystemExit(f"INCOMPLETE {kind}: retrieved {records:,} records != API total {total:,}")

    # Duplicates are expected in small numbers: the benchmark stores one document per
    # (entity, claiming institution), so the same GitHub login legitimately appears
    # twice with different crawl timestamps. A LARGE duplicate share instead means the
    # sort key was not unique and skip/limit paging reshuffled documents between
    # requests, silently dropping others (measured with sort=repos: 53% duplicates).
    if dup:
        pct = 100.0 * dup / total
        print(f"  note: {dup:,} duplicate {id_field} values ({pct:.2f}%) — upstream stores "
              f"one document per claiming institution")
        if pct > 2.0:
            raise SystemExit(
                f"UNSTABLE PAGING for {kind}: {pct:.1f}% duplicate {id_field}. The sort key "
                f"must exist on the documents AND be unique, or records are dropped."
            )
    if short:
        raise SystemExit(f"GAPS in {kind}: short pages before the last one: {short[:5]}")


def main() -> None:
    ap = argparse.ArgumentParser()
    ap.add_argument("--force", action="store_true", help="re-download files that already exist")
    ap.add_argument("--only", choices=["institutions", "repositories", "users"], help="harvest one slice")
    args = ap.parse_args()

    RAW.mkdir(parents=True, exist_ok=True)
    counts = {}

    latest = get("latestUpdate")
    write("latest_update.json", latest, force=True)  # provenance: always refresh
    print(f"source last crawled: {latest.get('updatedDate')}\n")

    if args.only in (None, "institutions"):
        counts["institutions"] = len(harvest_institutions(args.force))
    if args.only in (None, "repositories"):
        # Same stability requirement as users: 'num_stars' is not unique and pages
        # overlapped (23 duplicate uuids between page 0 and 1). 'uuid' is unique.
        counts["repositories"] = harvest_paged(
            "repositories", "paginatedRepositories", "repositories", REPO_PAGE,
            {"sort": "uuid", "includeForks": "true"}, args.force, id_field="uuid")
    if args.only in (None, "users"):
        # MUST sort on a field that exists AND is unique. The user documents have no
        # 'repos' field, so Mongo's sort degenerates and pages overlap badly
        # (measured: 103/200 duplicate logins between adjacent pages, yielding only
        # 60,467 distinct logins out of 127,529 records). 'login' is unique -> stable.
        counts["users"] = harvest_paged(
            "users", "paginatedUsers", "users", USER_PAGE,
            {"sort": "login"}, args.force, id_field="login")

    manifest = {
        "source": "https://ossbenchmark.com",
        "api_base": API,
        "upstream_repo": "https://github.com/digital-sustainability/oss-github-benchmark",
        "source_last_crawled": latest.get("updatedDate"),
        "harvested_at": time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime()),
        "counts": counts,
        "endpoints": {
            "institutions_paginated.json": "GET /paginatedInstitutions (full listing + sector histogram)",
            "institutions/<shortname>.json": "GET /singleInstitution?name=<shortname>",
            "repositories/page_NNNN.json": f"GET /paginatedRepositories (count={REPO_PAGE}, includeForks=true)",
            "users/page_NNNN.json": f"GET /paginatedUsers (count={USER_PAGE})",
        },
    }
    write("manifest.json", manifest, force=True)
    print(f"\ndone: {counts}")


if __name__ == "__main__":
    main()
