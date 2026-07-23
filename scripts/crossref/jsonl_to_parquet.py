"""Convert the Crossref Public Data File (March 2026 torrent: 35,908 numbered
jsonl.gz files, 5,000 DOI-sorted records each, ~208 GiB / ~1.1 TB of JSON,
~179.5M works) into two joinable Parquet tables.

Unlike the DataCite/ORCID/OpenAIRE sources (single tar streamed by the main
thread), the Crossref dump is thousands of INDEPENDENT gzip files — so each
worker owns a contiguous chunk of source files end-to-end and writes its own
Parquet parts directly. No pipes full of records, no rolling writers, no
checkpoint file: a task is done iff both of its final part files exist, which
makes resume = "skip tasks whose outputs are already there".

Output tables (out/<table>/part-*.parquet):
  works  one row per DOI: typed scalars (doi, type, title, container, dates,
         counts, issn/isbn, abstract, …), every nested field kept whole as a
         JSON-string column (author_json, funder_json, license_json, …),
         unknown keys in extra_json. The `reference` array is NOT here — it
         becomes the refs table. Dropped as derivable/constant: URL
         (= doi.org/<doi>), score (always 1.0 in the dump), source
         (= "Crossref"), reference-count (deprecated alias of
         references-count) — each lands in extra_json if it ever deviates.
  refs   one row per reference entry (the citation edge list, Crossref's
         answer to DataCite's PID Links): doi -> ref_doi when the reference
         is DOI-matched, plus key/index, who asserted the DOI, year, the raw
         `unstructured` citation string, rest in rest_json.

DISK GUARD: workers check free space before every flush; below --min-free-gib
they stop cleanly (the run stays resumable) instead of filling the drive.

Usage:
  python scripts/crossref/jsonl_to_parquet.py                    # full run, resumable
  python scripts/crossref/jsonl_to_parquet.py --max-tasks 2 --files-per-task 3 --out C:/tmp/t --fresh
"""

import argparse
import glob
import os
import re
import gzip
import shutil
import time
from concurrent.futures import ProcessPoolExecutor, as_completed

import orjson
import pyarrow as pa
import pyarrow.parquet as pq

SRC = r"D:\pro\rete\data\crossref\March 2026 Public Data File from Crossref"
OUT = r"D:\pro\rete\data\crossref\parquet-2026"
GiB = 1 << 30
FLUSH_FILES = 8            # source files per Arrow flush (~40k works rows)


class DiskFull(Exception):
    pass


def _dumps(v):
    return orjson.dumps(v).decode()


def _j(v):
    return _dumps(v) if v else None


I32_MIN, I32_MAX = -(1 << 31), (1 << 31) - 1


def _int(v):
    try:
        return int(v) if v is not None else None
    except (TypeError, ValueError):
        return None


def _i32(v):
    """int that fits in a Parquet int32; None otherwise (Crossref carries some
    malformed years/counts far outside int32 — they overflow pa's C long)."""
    n = _int(v)
    return n if n is not None and I32_MIN <= n <= I32_MAX else None


_YEAR_RE = re.compile(r"([12][0-9]{3})")


def salvage_year(v):
    """Best-effort real 4-digit year from a messy Crossref reference `year`.

    The dump's non-integer year values are ~97% recoverable: author-year
    disambiguators ('2020a', '2019b'), YYYYMMDD timestamps ('20200101000000'),
    date ranges ('2019-2020' -> first year). Returns the first plausible
    1000..2030 year found; None for genuine junk ('..', bare '98', 'n.d.')."""
    if v is None:
        return None
    m = _YEAR_RE.search(str(v))
    if m:
        y = int(m.group(1))
        if 1000 <= y <= 2030:
            return y
    return None


def ref_year(v):
    """int32 year for a reference under the invariant `year is a real
    1000..2030 year or NULL`: keep a plausible clean int, else salvage a year
    from the digits of a messy/implausible value (YYYYMMDD, '2020a', '2019-20',
    …), else None (sentinels like 0 and junk like 98 become NULL)."""
    n = _int(v)
    if n is not None and 1000 <= n <= 2030:
        return n
    return salvage_year(v)


def _first(v):
    """First element of Crossref's 1-element string arrays ('title', …)."""
    if isinstance(v, list):
        return v[0] if v else None
    return v


def _pdate(obj):
    """{'date-parts': [[2013, 7, 4]]} -> '2013-07-04' (partial dates kept partial)."""
    if not isinstance(obj, dict):
        return None
    parts = obj.get("date-parts") or []
    p = parts[0] if parts and isinstance(parts[0], list) else None
    if not p or p[0] is None:
        return None
    try:
        out = f"{int(p[0]):04d}"
        if len(p) > 1 and p[1]:
            out += f"-{int(p[1]):02d}"
        if len(p) > 2 and p[2]:
            out += f"-{int(p[2]):02d}"
        return out
    except (TypeError, ValueError):
        return None


def _dt(obj):
    """created/deposited/indexed carry a full ISO 'date-time'; fall back to date-parts."""
    if not isinstance(obj, dict):
        return None
    return obj.get("date-time") or _pdate(obj)


def _year(obj):
    d = _pdate(obj)
    return int(d[:4]) if d else None


# --------------------------------------------------------------------- schema

WORK_JSON = {
    "author_json": "author",
    "editor_json": "editor",
    "translator_json": "translator",
    "chair_json": "chair",
    "license_json": "license",
    "link_json": "link",
    "funder_json": "funder",
    "assertion_json": "assertion",
    "relation_json": "relation",
    "update_to_json": "update-to",
    "updated_by_json": "updated-by",
    "alternative_id_json": "alternative-id",
    "archive_json": "archive",
    "event_json": "event",
    "institution_json": "institution",
    "journal_issue_json": "journal-issue",
    "content_domain_json": "content-domain",
    "clinical_trial_number_json": "clinical-trial-number",
    "aliases_json": "aliases",
    "free_to_read_json": "free-to-read",
    "review_json": "review",
    "standards_body_json": "standards-body",
    "subject_json": "subject",
}

FIRST_OF = {
    "title": "title",
    "subtitle": "subtitle",
    "original_title": "original-title",
    "container_title": "container-title",
    "short_container_title": "short-container-title",
}

WORK_KNOWN = {
    "DOI", "prefix", "member", "type", "publisher", "publisher-location",
    "volume", "issue", "page", "article-number", "language",
    "ISSN", "issn-type", "ISBN", "isbn-type",
    "is-referenced-by-count", "references-count", "reference-count",
    "abstract", "resource", "URL", "score", "source", "update-policy",
    "created", "deposited", "indexed", "issued", "published",
    "published-print", "published-online", "accepted", "reference",
} | set(WORK_JSON.values()) | set(FIRST_OF.values())

WORKS_SCHEMA = pa.schema(
    [
        ("doi", pa.string()),
        ("prefix", pa.string()),
        ("member", pa.string()),
        ("type", pa.string()),
        ("title", pa.string()),
        ("subtitle", pa.string()),
        ("original_title", pa.string()),
        ("container_title", pa.string()),
        ("short_container_title", pa.string()),
        ("publisher", pa.string()),
        ("publisher_location", pa.string()),
        ("volume", pa.string()),
        ("issue", pa.string()),
        ("page", pa.string()),
        ("article_number", pa.string()),
        ("language", pa.string()),
        ("issn", pa.string()),
        ("issn_json", pa.string()),
        ("isbn", pa.string()),
        ("isbn_json", pa.string()),
        ("issued", pa.string()),
        ("issued_year", pa.int32()),
        ("published", pa.string()),
        ("published_print", pa.string()),
        ("published_online", pa.string()),
        ("accepted", pa.string()),
        ("created", pa.string()),
        ("deposited", pa.string()),
        ("indexed", pa.string()),
        ("is_referenced_by_count", pa.int32()),
        ("references_count", pa.int32()),
        ("abstract", pa.string()),
        ("resource_url", pa.string()),
        ("update_policy", pa.string()),
    ]
    + [(name, pa.string()) for name in WORK_JSON]
    + [("extra_json", pa.string())]
)

REF_KNOWN = {"key", "DOI", "doi-asserted-by", "year", "unstructured"}

REFS_SCHEMA = pa.schema(
    [
        ("doi", pa.string()),
        ("ref_index", pa.int32()),
        ("key", pa.string()),
        ("ref_doi", pa.string()),
        ("doi_asserted_by", pa.string()),
        ("year", pa.int32()),
        ("unstructured", pa.string()),
        ("rest_json", pa.string()),
    ]
)


# -------------------------------------------------------------- record -> rows

def work_row(r):
    extra = {k: v for k, v in r.items() if k not in WORK_KNOWN}

    doi = r.get("DOI")
    doi = doi.lower() if isinstance(doi, str) else doi

    # constants / derivables that only survive if they deviate
    if r.get("source") not in (None, "Crossref"):
        extra["source"] = r["source"]
    if r.get("score") not in (None, 0, 0.0, 1, 1.0):
        extra["score"] = r["score"]
    rc, rsc = r.get("reference-count"), r.get("references-count")
    if rc is not None and rc != rsc:
        extra["reference-count"] = rc

    # int32 columns: keep the raw value if it won't fit (corrupt but not dropped)
    irbc = _i32(r.get("is-referenced-by-count"))
    if irbc is None and r.get("is-referenced-by-count") is not None:
        extra["is-referenced-by-count_raw"] = str(r["is-referenced-by-count"])
    refs_ct = _i32(r.get("references-count"))
    if refs_ct is None and r.get("references-count") is not None:
        extra["references-count_raw"] = str(r["references-count"])

    row = {
        "doi": doi,
        "prefix": r.get("prefix"),
        "member": r.get("member"),
        "type": r.get("type"),
        "publisher": r.get("publisher"),
        "publisher_location": r.get("publisher-location"),
        "volume": r.get("volume"),
        "issue": r.get("issue"),
        "page": r.get("page"),
        "article_number": r.get("article-number"),
        "language": r.get("language"),
        "issn": _first(r.get("ISSN")),
        "issn_json": _j(r.get("issn-type")),
        "isbn": _first(r.get("ISBN")),
        "isbn_json": _j(r.get("isbn-type")),
        "issued": _pdate(r.get("issued")),
        "issued_year": _i32(_year(r.get("issued"))),
        "published": _pdate(r.get("published")),
        "published_print": _pdate(r.get("published-print")),
        "published_online": _pdate(r.get("published-online")),
        "accepted": _pdate(r.get("accepted")),
        "created": _dt(r.get("created")),
        "deposited": _dt(r.get("deposited")),
        "indexed": _dt(r.get("indexed")),
        "is_referenced_by_count": irbc,
        "references_count": refs_ct,
        "abstract": r.get("abstract"),
        "update_policy": r.get("update-policy"),
    }

    for col, key in FIRST_OF.items():
        v = r.get(key)
        row[col] = _first(v)
        if isinstance(v, list) and len(v) > 1:
            extra[key + "_rest"] = v[1:]

    resource = r.get("resource")
    if isinstance(resource, dict):
        primary = resource.get("primary")
        row["resource_url"] = primary.get("URL") if isinstance(primary, dict) else None
        rest = {k: v for k, v in resource.items() if k != "primary"}
        if isinstance(primary, dict):
            prest = {k: v for k, v in primary.items() if k != "URL"}
            if prest:
                rest["primary"] = prest
        if rest:
            extra["resource"] = rest
    else:
        row["resource_url"] = None

    for col, key in WORK_JSON.items():
        row[col] = _j(r.get(key))
    row["extra_json"] = _j(extra)
    return row


def ref_rows(doi, refs):
    out = []
    for i, ref in enumerate(refs):
        if not isinstance(ref, dict):
            continue
        rest = {k: v for k, v in ref.items() if k not in REF_KNOWN}
        rdoi = ref.get("DOI")
        y_raw = ref.get("year")
        year = ref_year(y_raw)
        if year is None and y_raw not in (None, ""):
            rest["year_raw"] = str(y_raw)     # unrecoverable year kept, not dropped
        out.append(
            {
                "doi": doi,
                "ref_index": i,
                "key": ref.get("key"),
                "ref_doi": rdoi.lower() if isinstance(rdoi, str) else rdoi,
                "doi_asserted_by": ref.get("doi-asserted-by"),
                "year": year,
                "unstructured": ref.get("unstructured"),
                "rest_json": _j(rest),
            }
        )
    return out


# --------------------------------------------------------------------- worker

def convert_task(tid, paths, out_dir, min_free_gib):
    """Parse a chunk of source gz files, write works/refs part-<tid>.parquet."""
    works_final = os.path.join(out_dir, "works", f"part-{tid:05d}.parquet")
    refs_final = os.path.join(out_dir, "refs", f"part-{tid:05d}.parquet")
    works_tmp, refs_tmp = works_final + ".tmp", refs_final + ".tmp"

    n_works = n_refs = n_bad = 0
    wbuf, rbuf = [], []
    ww = pq.ParquetWriter(works_tmp, WORKS_SCHEMA, compression="zstd", compression_level=6)
    rw = pq.ParquetWriter(refs_tmp, REFS_SCHEMA, compression="zstd", compression_level=6)

    def flush():
        nonlocal wbuf, rbuf
        if shutil.disk_usage(out_dir).free < min_free_gib * GiB:
            raise DiskFull(f"free space below {min_free_gib} GiB")
        if wbuf:
            ww.write_table(pa.Table.from_pylist(wbuf, schema=WORKS_SCHEMA))
            wbuf = []
        if rbuf:
            rw.write_table(pa.Table.from_pylist(rbuf, schema=REFS_SCHEMA))
            rbuf = []

    try:
        for fi, path in enumerate(paths):
            with gzip.open(path, "rb") as f:
                for line in f:
                    try:
                        r = orjson.loads(line)
                    except orjson.JSONDecodeError:
                        n_bad += 1
                        continue
                    row = work_row(r)
                    wbuf.append(row)
                    n_works += 1
                    refs = r.get("reference")
                    if refs:
                        rr = ref_rows(row["doi"], refs)
                        rbuf.extend(rr)
                        n_refs += len(rr)
            if (fi + 1) % FLUSH_FILES == 0:
                flush()
        flush()
        ww.close()
        rw.close()
        os.replace(works_tmp, works_final)
        os.replace(refs_tmp, refs_final)
    except BaseException:
        ww.close()
        rw.close()
        for t in (works_tmp, refs_tmp):
            if os.path.exists(t):
                os.remove(t)
        raise
    return tid, n_works, n_refs, n_bad


# ----------------------------------------------------------------------- main

def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--src", default=SRC)
    ap.add_argument("--out", default=OUT)
    ap.add_argument("--files-per-task", type=int, default=72)
    ap.add_argument("--workers", type=int, default=18)
    ap.add_argument("--max-tasks", type=int, default=None)
    ap.add_argument("--min-free-gib", type=int, default=25)
    ap.add_argument("--fresh", action="store_true", help="delete existing output first")
    args = ap.parse_args()

    if args.fresh and os.path.isdir(args.out):
        shutil.rmtree(args.out)
    for sub in ("works", "refs"):
        os.makedirs(os.path.join(args.out, sub), exist_ok=True)

    files = glob.glob(os.path.join(args.src, "*.jsonl.gz"))
    files.sort(key=lambda p: int(re.search(r"(\d+)\.jsonl\.gz$", p).group(1)))
    if not files:
        raise SystemExit(f"no *.jsonl.gz under {args.src}")
    tasks = [
        (tid, files[i : i + args.files_per_task])
        for tid, i in enumerate(range(0, len(files), args.files_per_task))
    ]
    if args.max_tasks:
        tasks = tasks[: args.max_tasks]

    def done(tid):
        return all(
            os.path.exists(os.path.join(args.out, sub, f"part-{tid:05d}.parquet"))
            for sub in ("works", "refs")
        )

    # sweep stale tmp files (crashed/killed run), then skip completed tasks
    for sub in ("works", "refs"):
        for t in glob.glob(os.path.join(args.out, sub, "*.tmp")):
            os.remove(t)
    todo = [t for t in tasks if not done(t[0])]
    print(f"{len(files)} source files -> {len(tasks)} tasks "
          f"({len(tasks) - len(todo)} already done, {len(todo)} to run, "
          f"{args.workers} workers)", flush=True)
    if not todo:
        return

    t0 = time.time()
    n_works = n_refs = n_bad = n_done = 0
    aborted = None
    with ProcessPoolExecutor(max_workers=args.workers) as ex:
        futs = {
            ex.submit(convert_task, tid, paths, args.out, args.min_free_gib): tid
            for tid, paths in todo
        }
        for fut in as_completed(futs):
            try:
                tid, w, r, b = fut.result()
            except DiskFull as e:
                if aborted is None:
                    aborted = str(e)
                    print(f"DISK GUARD: {e} -- cancelling remaining tasks "
                          f"(re-run the same command to resume)", flush=True)
                    for f2 in futs:
                        f2.cancel()
                continue
            n_done += 1
            n_works += w
            n_refs += r
            n_bad += b
            el = time.time() - t0
            eta = el / n_done * (len(todo) - n_done)
            print(f"[{n_done}/{len(todo)}] part-{tid:05d}  "
                  f"works {n_works:,}  refs {n_refs:,}  bad {n_bad}  "
                  f"{el/60:.1f} min elapsed, ~{eta/60:.0f} min left", flush=True)

    status = f"ABORTED ({aborted})" if aborted else "DONE"
    print(f"{status}: {n_done} tasks this run, works {n_works:,}, "
          f"refs {n_refs:,}, bad lines {n_bad}, {(time.time()-t0)/60:.1f} min", flush=True)


if __name__ == "__main__":
    main()
