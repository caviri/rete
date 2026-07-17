"""Stream the ORCID Public Data File 2025 *summaries* tarball into several
joinable Parquet tables — without extracting the 864 GB of XML.

Input:  data/orcid/ORCID_2025_10_summaries.tar.gz
        (~20M+ members: summaries/<3-digit>/<iD>.xml, one namespaced XML per
         ORCID record: person data + an activities-summary of works,
         affiliations and fundings)

Output tables (data/orcid/parquet-summaries/<table>/part-*.parquet):
  person       one row per ORCID: name, locale, history flags, country,
               keyword/other-name/external-id JSON, + activity counts
  work         one row per work-summary: orcid, put_code, title, type,
               pub year, journal, **doi** (join key to DataCite/OpenAIRE),
               all external ids as JSON
  affiliation  one row per employment/education/distinction/invited-position/
               membership/qualification/service summary: orcid, aff_type,
               role, dates, org name+country + **disambiguated org id (ROR/
               GRID/RINGGOLD/FUNDREF)** — the org graph join key
  funding      one row per funding-summary: orcid, title, type, dates, org

XML is parsed namespace-agnostically (match on local tag name). Records are
parsed in a process pool over batches of member bytes; the main thread streams
the tar and routes returned column-batches to per-table rolling zstd writers.
Resumable at closed-file granularity via _checkpoint.json.

Usage:
  python scripts/orcid/summaries_to_parquet.py
  python scripts/orcid/summaries_to_parquet.py --max-batches 20 --out /tmp/t --fresh
"""

import argparse
import json
import os
import time
import tarfile
from collections import deque
from concurrent.futures import ProcessPoolExecutor

import xml.etree.ElementTree as ET
import orjson
import pyarrow as pa
import pyarrow.parquet as pq

TAR = r"D:\pro\rete\data\orcid\ORCID_2025_10_summaries.tar.gz"
OUT = r"D:\pro\rete\data\orcid\parquet-summaries"
BATCH_RECORDS = 3000          # members per work unit shipped to a worker
AFFILIATION_TYPES = {
    "employment-summary": "employment",
    "education-summary": "education",
    "distinction-summary": "distinction",
    "invited-position-summary": "invited-position",
    "membership-summary": "membership",
    "qualification-summary": "qualification",
    "service-summary": "service",
}

# --------------------------------------------------------------- XML helpers

def L(tag):
    return tag.rsplit("}", 1)[-1]


def child(el, name):
    if el is None:
        return None
    for c in el:
        if L(c.tag) == name:
            return c
    return None


def child_text(el, name):
    c = child(el, name)
    return c.text.strip() if (c is not None and c.text) else None


def children(el, name):
    return [c for c in el] if el is None else [c for c in el if L(c.tag) == name]


def descendants(el, name):
    return [d for d in el.iter() if L(d.tag) == name] if el is not None else []


def read_date(el):
    if el is None:
        return (None, None, None)
    return (child_text(el, "year"), child_text(el, "month"), child_text(el, "day"))


def to_int(s):
    try:
        return int(s)
    except (TypeError, ValueError):
        return None


def org_info(summary):
    org = child(summary, "organization")
    if org is None:
        return {}
    addr = child(org, "address")
    disamb = child(org, "disambiguated-organization")
    return {
        "org_name": child_text(org, "name"),
        "org_city": child_text(addr, "city") if addr is not None else None,
        "org_region": child_text(addr, "region") if addr is not None else None,
        "org_country": child_text(addr, "country") if addr is not None else None,
        "org_id": child_text(disamb, "disambiguated-organization-identifier") if disamb is not None else None,
        "org_id_source": child_text(disamb, "disambiguation-source") if disamb is not None else None,
    }


def source_name(summary):
    src = child(summary, "source")
    return child_text(src, "source-name") if src is not None else None


# --------------------------------------------------------------- extraction

PERSON_COLS = [
    "orcid", "given_names", "family_name", "credit_name", "biography", "locale",
    "creation_method", "submission_date", "last_modified_date", "claimed",
    "verified_email", "country", "n_other_names", "n_keywords",
    "n_external_ids", "n_works", "n_employments", "n_educations", "n_fundings",
    "n_distinctions", "n_invited_positions", "n_memberships", "n_qualifications",
    "n_services", "n_peer_reviews", "n_research_resources",
    "other_names_json", "keywords_json", "external_ids_json",
    "researcher_urls_json",
]
WORK_COLS = [
    "orcid", "put_code", "title", "subtitle", "journal_title", "type",
    "pub_year", "pub_month", "pub_day", "doi", "url", "source_name",
    "external_ids_json",
]
AFF_COLS = [
    "orcid", "aff_type", "put_code", "department", "role_title",
    "start_year", "start_month", "start_day", "end_year", "end_month", "end_day",
    "org_name", "org_city", "org_region", "org_country", "org_id",
    "org_id_source", "source_name",
]
FUND_COLS = [
    "orcid", "put_code", "title", "type", "start_year", "end_year",
    "org_name", "org_country", "org_id", "org_id_source", "source_name",
    "external_ids_json",
]

PERSON_SCHEMA = pa.schema(
    [(c, pa.int32() if c.startswith("n_") else (
        pa.bool_() if c in ("claimed", "verified_email") else pa.string()))
     for c in PERSON_COLS]
)
WORK_SCHEMA = pa.schema(
    [(c, pa.int32() if c in ("pub_year", "pub_month", "pub_day") else pa.string())
     for c in WORK_COLS]
)
AFF_SCHEMA = pa.schema(
    [(c, pa.int32() if c.endswith(("_year", "_month", "_day")) else pa.string())
     for c in AFF_COLS]
)
FUND_SCHEMA = pa.schema(
    [(c, pa.int32() if c.endswith("_year") else pa.string()) for c in FUND_COLS]
)
SCHEMAS = {"person": PERSON_SCHEMA, "work": WORK_SCHEMA,
           "affiliation": AFF_SCHEMA, "funding": FUND_SCHEMA}


def ext_ids(container):
    """List of {type,value,url,relationship} from an <external-ids> element."""
    out = []
    if container is None:
        return out
    for eid in children(container, "external-id"):
        out.append({
            "type": child_text(eid, "external-id-type"),
            "value": child_text(eid, "external-id-value")
                     or child_text(eid, "external-id-normalized"),
            "url": child_text(eid, "external-id-url"),
            "relationship": child_text(eid, "external-id-relationship"),
        })
    return out


def first_doi(eids):
    for e in eids:
        if e["type"] == "doi" and e["value"]:
            return e["value"].lower()
    return None


def parse_record(xml_bytes):
    """One ORCID summary XML -> {'person': row, 'work': [...], ...}, or None
    for an <error> placeholder (deactivated/deprecated record)."""
    root = ET.fromstring(xml_bytes)
    if L(root.tag) == "error":
        return None
    orcid = root.get("path")
    ident = child(root, "orcid-identifier")
    if not orcid and ident is not None:
        orcid = child_text(ident, "path")

    person = child(root, "person")
    acts = child(root, "activities-summary")

    # ---- person ----
    name_el = child(person, "name") if person is not None else None
    other_el = child(person, "other-names") if person is not None else None
    kw_el = child(person, "keywords") if person is not None else None
    xid_el = child(person, "external-identifiers") if person is not None else None
    url_el = child(person, "researcher-urls") if person is not None else None
    addr_el = child(person, "addresses") if person is not None else None
    bio_el = child(person, "biography") if person is not None else None
    history = child(root, "history")
    prefs = child(root, "preferences")

    other_names = [child_text(o, "content") for o in children(other_el, "other-name")]
    other_names = [o for o in other_names if o]
    keywords = [child_text(k, "content") for k in children(kw_el, "keyword")]
    keywords = [k for k in keywords if k]
    person_xids = []
    for x in children(xid_el, "external-identifier"):
        person_xids.append({
            "type": child_text(x, "external-id-type"),
            "value": child_text(x, "external-id-value"),
            "url": child_text(x, "external-id-url"),
            "relationship": child_text(x, "external-id-relationship"),
        })
    rurls = []
    for u in children(url_el, "researcher-url"):
        rurls.append({"name": child_text(u, "url-name"), "url": child_text(u, "url")})
    country = None
    if addr_el is not None:
        a = child(addr_el, "address")
        country = child_text(a, "country") if a is not None else None

    def count(name):
        return len(descendants(acts, name)) if acts is not None else 0

    person_row = {
        "orcid": orcid,
        "given_names": child_text(name_el, "given-names"),
        "family_name": child_text(name_el, "family-name"),
        "credit_name": child_text(name_el, "credit-name"),
        "biography": child_text(bio_el, "content") if bio_el is not None else None,
        "locale": child_text(prefs, "locale") if prefs is not None else None,
        "creation_method": child_text(history, "creation-method") if history is not None else None,
        "submission_date": child_text(history, "submission-date") if history is not None else None,
        "last_modified_date": child_text(history, "last-modified-date") if history is not None else None,
        "claimed": (child_text(history, "claimed") == "true") if history is not None else None,
        "verified_email": (child_text(history, "verified-email") == "true") if history is not None else None,
        "country": country,
        "n_other_names": len(other_names),
        "n_keywords": len(keywords),
        "n_external_ids": len(person_xids),
        "n_works": count("work-summary"),
        "n_employments": count("employment-summary"),
        "n_educations": count("education-summary"),
        "n_fundings": count("funding-summary"),
        "n_distinctions": count("distinction-summary"),
        "n_invited_positions": count("invited-position-summary"),
        "n_memberships": count("membership-summary"),
        "n_qualifications": count("qualification-summary"),
        "n_services": count("service-summary"),
        "n_peer_reviews": count("peer-review-summary"),
        "n_research_resources": count("research-resource-summary"),
        "other_names_json": orjson.dumps(other_names).decode() if other_names else None,
        "keywords_json": orjson.dumps(keywords).decode() if keywords else None,
        "external_ids_json": orjson.dumps(person_xids).decode() if person_xids else None,
        "researcher_urls_json": orjson.dumps(rurls).decode() if rurls else None,
    }

    works, affs, funds = [], [], []
    if acts is not None:
        for ws in descendants(acts, "work-summary"):
            y, m, d = read_date(child(ws, "publication-date"))
            eids = ext_ids(child(ws, "external-ids"))
            title_el = child(ws, "title")
            works.append({
                "orcid": orcid,
                "put_code": ws.get("put-code"),
                "title": child_text(title_el, "title") if title_el is not None else None,
                "subtitle": child_text(title_el, "subtitle") if title_el is not None else None,
                "journal_title": child_text(ws, "journal-title"),
                "type": child_text(ws, "type"),
                "pub_year": to_int(y), "pub_month": to_int(m), "pub_day": to_int(d),
                "doi": first_doi(eids),
                "url": child_text(ws, "url"),
                "source_name": source_name(ws),
                "external_ids_json": orjson.dumps(eids).decode() if eids else None,
            })
        for local, atype in AFFILIATION_TYPES.items():
            for s in descendants(acts, local):
                sy, sm, sd = read_date(child(s, "start-date"))
                ey, em, ed = read_date(child(s, "end-date"))
                info = org_info(s)
                affs.append({
                    "orcid": orcid, "aff_type": atype, "put_code": s.get("put-code"),
                    "department": child_text(s, "department-name"),
                    "role_title": child_text(s, "role-title"),
                    "start_year": to_int(sy), "start_month": to_int(sm), "start_day": to_int(sd),
                    "end_year": to_int(ey), "end_month": to_int(em), "end_day": to_int(ed),
                    "org_name": info.get("org_name"), "org_city": info.get("org_city"),
                    "org_region": info.get("org_region"), "org_country": info.get("org_country"),
                    "org_id": info.get("org_id"), "org_id_source": info.get("org_id_source"),
                    "source_name": source_name(s),
                })
        for fs in descendants(acts, "funding-summary"):
            sy, _, _ = read_date(child(fs, "start-date"))
            ey, _, _ = read_date(child(fs, "end-date"))
            info = org_info(fs)
            eids = ext_ids(child(fs, "external-ids"))
            title_el = child(fs, "title")
            funds.append({
                "orcid": orcid, "put_code": fs.get("put-code"),
                "title": child_text(title_el, "title") if title_el is not None else None,
                "type": child_text(fs, "type"),
                "start_year": to_int(sy), "end_year": to_int(ey),
                "org_name": info.get("org_name"), "org_country": info.get("org_country"),
                "org_id": info.get("org_id"), "org_id_source": info.get("org_id_source"),
                "source_name": source_name(fs),
                "external_ids_json": orjson.dumps(eids).decode() if eids else None,
            })
    return {"person": [person_row], "work": works,
            "affiliation": affs, "funding": funds}


def build_batch(rows, schema):
    cols = {f.name: [] for f in schema}
    for r in rows:
        for f in schema:
            cols[f.name].append(r.get(f.name))
    try:
        return pa.RecordBatch.from_pydict(cols, schema=schema)
    except (pa.ArrowInvalid, pa.ArrowTypeError):
        arrays = []
        for f in schema:
            vals = cols[f.name]
            try:
                arrays.append(pa.array(vals, type=f.type))
            except (pa.ArrowInvalid, pa.ArrowTypeError):
                if pa.types.is_integer(f.type):
                    vals = [to_int(v) for v in vals]
                elif pa.types.is_boolean(f.type):
                    vals = [v if isinstance(v, bool) or v is None else None for v in vals]
                else:
                    vals = [v if v is None else str(v) for v in vals]
                arrays.append(pa.array(vals, type=f.type))
        return pa.RecordBatch.from_arrays(arrays, schema=schema)


def parse_batch(members):
    """Worker: list of xml byte-strings -> {table: RecordBatch}, n_bad, err."""
    acc = {"person": [], "work": [], "affiliation": [], "funding": []}
    n_bad = n_error = 0
    first_error = None
    for data in members:
        try:
            out = parse_record(data)
            if out is None:  # <error> placeholder = deactivated record
                n_error += 1
                continue
            for t in acc:
                acc[t].extend(out[t])
        except Exception as e:  # noqa: BLE001
            n_bad += 1
            if first_error is None:
                first_error = f"{e!r} :: {data[:160]!r}"
    batches = {t: build_batch(rows, SCHEMAS[t]) for t, rows in acc.items()}
    return batches, n_bad, n_error, first_error


# ------------------------------------------------------------ writer/resume

class TableWriter:
    def __init__(self, out_dir, schema, rows_per_file, chunk_rows, start_index):
        self.out_dir = out_dir
        self.schema = schema
        self.rows_per_file = rows_per_file
        self.chunk_rows = chunk_rows
        self.writer = None
        self.file_index = start_index
        self.pending = []
        self.pending_rows = 0
        self.file_rows = 0
        self.total_rows = 0
        os.makedirs(out_dir, exist_ok=True)

    def _flush(self):
        if not self.pending:
            return
        if self.writer is None:
            path = os.path.join(self.out_dir, f"part-{self.file_index:05d}.parquet")
            self.writer = pq.ParquetWriter(path, self.schema, compression="zstd",
                                           compression_level=3)
        table = pa.Table.from_batches(self.pending, schema=self.schema)
        self.writer.write_table(table, row_group_size=self.chunk_rows)
        self.file_rows += table.num_rows
        self.pending = []
        self.pending_rows = 0

    def add(self, batch):
        if batch.num_rows:
            self.pending.append(batch)
            self.pending_rows += batch.num_rows
            self.total_rows += batch.num_rows
        if self.pending_rows >= self.chunk_rows:
            self._flush()
        if self.file_rows >= self.rows_per_file:
            self.close_current()

    def close_current(self):
        self._flush()
        if self.writer is not None:
            self.writer.close()
            self.writer = None
            self.file_index += 1
            self.file_rows = 0

    def close(self):
        self.close_current()


def cp_path(out):
    return os.path.join(out, "_checkpoint.json")


def save_cp(out, cp):
    tmp = cp_path(out) + ".tmp"
    with open(tmp, "w", encoding="utf-8") as f:
        json.dump(cp, f)
    os.replace(tmp, cp_path(out))


def load_cp(out, fresh):
    if not fresh and os.path.exists(cp_path(out)):
        with open(cp_path(out), encoding="utf-8") as f:
            return json.load(f)
    return {"batches_done": 0, "rows": {}, "file_index": {}}


def main():
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("--tar", default=TAR)
    ap.add_argument("--out", default=OUT)
    ap.add_argument("--workers", type=int, default=min(14, max(4, os.cpu_count() - 4)))
    ap.add_argument("--batch-records", type=int, default=BATCH_RECORDS)
    ap.add_argument("--rows-per-file", type=int, default=2_000_000)
    ap.add_argument("--chunk-rows", type=int, default=200_000)
    ap.add_argument("--max-batches", type=int, default=None)
    ap.add_argument("--fresh", action="store_true")
    args = ap.parse_args()

    os.makedirs(args.out, exist_ok=True)
    cp = load_cp(args.out, args.fresh)
    skip = cp["batches_done"]
    if skip:
        print(f"resuming: skipping {skip} batches "
              f"({sum(cp['rows'].values()):,} rows already written)", flush=True)

    writers = {
        t: TableWriter(os.path.join(args.out, t), SCHEMAS[t],
                       args.rows_per_file, args.chunk_rows,
                       cp["file_index"].get(t, 0))
        for t in SCHEMAS
    }
    for t in writers:
        writers[t].total_rows = cp["rows"].get(t, 0)

    tar_size = os.path.getsize(args.tar)
    raw = open(args.tar, "rb")
    tf = tarfile.open(fileobj=raw, mode="r|*")

    total_bad = 0
    total_error = 0
    first_error = None
    n_seen_batches = 0
    n_submitted = 0
    n_done = skip
    t0 = time.time()
    inflight = deque()
    max_inflight = args.workers * 2

    def commit_checkpoint():
        cp["batches_done"] = n_done
        cp["rows"] = {t: writers[t].total_rows for t in writers}
        cp["file_index"] = {t: writers[t].file_index for t in writers}
        save_cp(args.out, cp)

    def drain_one():
        nonlocal total_bad, total_error, first_error, n_done
        batches, n_bad, n_error, err = inflight.popleft().result()
        total_bad += n_bad
        total_error += n_error
        if err and first_error is None:
            first_error = err
        for t, b in batches.items():
            writers[t].add(b)
        n_done += 1
        if n_done % 200 == 0:
            for t in writers:
                writers[t].close_current()
            commit_checkpoint()
            frac = raw.tell() / tar_size
            el = time.time() - t0
            eta = el / frac * (1 - frac) if frac > 0 else 0
            print(f"[{el/60:6.1f} min] batches {n_done:>6}  "
                  f"persons {writers['person'].total_rows:>11,}  "
                  f"works {writers['work'].total_rows:>12,}  "
                  f"tar {frac*100:4.1f}%  eta {eta/60:5.1f}m  bad {total_bad}",
                  flush=True)

    batch = []
    with ProcessPoolExecutor(max_workers=args.workers) as pool:
        for m in tf:
            if not (m.isfile() and m.name.endswith(".xml")):
                continue
            batch.append(tf.extractfile(m).read())
            if len(batch) >= args.batch_records:
                n_seen_batches += 1
                if n_seen_batches > skip:
                    if args.max_batches is not None and n_submitted >= args.max_batches:
                        batch = []
                        break
                    inflight.append(pool.submit(parse_batch, batch))
                    n_submitted += 1
                    if len(inflight) >= max_inflight:
                        drain_one()
                batch = []
        if batch and not (args.max_batches is not None and n_submitted >= args.max_batches):
            n_seen_batches += 1
            if n_seen_batches > skip:
                inflight.append(pool.submit(parse_batch, batch))
        while inflight:
            drain_one()

    for t in writers:
        writers[t].close()
    commit_checkpoint()
    tf.close()
    raw.close()

    el = time.time() - t0
    print(f"DONE in {el/60:.1f} min:", flush=True)
    for t in writers:
        print(f"  {t:12s} {writers[t].total_rows:>13,} rows", flush=True)
    print(f"  deactivated (error) records skipped: {total_error:,}", flush=True)
    print(f"  bad records: {total_bad}", flush=True)
    if first_error:
        print("  first bad:", first_error, flush=True)


if __name__ == "__main__":
    main()
