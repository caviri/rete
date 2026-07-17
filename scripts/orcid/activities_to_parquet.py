"""Stream the ORCID Public Data File 2025 *activities* tarballs into Parquet —
the full activity records the summaries file only summarises.

Input:  data/orcid/ORCID_2025_10_activities_{0..9,X}.tar.gz  (11 files, ~220 GB
        compressed / 3.7 TB XML). One XML per activity:
        <checksum>/<3-digit>/<iD>/<activity type>/<iD>_<type>_<putcode>.xml

Output (data/orcid/parquet-activities/<table>/part-<tar>-*.parquet):
  work           full work record incl. **contributors_json** (the
                 co-authorship graph: co-author names + ORCIDs + roles),
                 abstract (short_description), language, country, doi, all
                 external ids
  affiliation    employment/education/distinction/invited-position/membership/
                 qualification/service — full record (department, dates, org +
                 disambiguated id, url)
  funding        full funding incl. amount, organization-defined-type,
                 contributors, external ids
  peer_review    reviewer role, review type/group, convening org, identifiers
  research_resource  title, dates, hosts, external ids

Each activity file is routed to a table by its type folder. Records are parsed
in a process pool over batches of member bytes; per-table rolling zstd writers.
Resume is per-tarball: output files are tagged part-<tar>-NNNNN, a tarball is
checkpointed only when fully done, and a partially-done tarball's tagged files
are deleted and redone on restart (no duplicates).

Usage:
  python scripts/orcid/activities_to_parquet.py
  python scripts/orcid/activities_to_parquet.py --only 0,X
  python scripts/orcid/activities_to_parquet.py --tar ...activities_X.tar.gz --max-batches 10 --out /tmp/t --fresh
"""

import argparse
import glob
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

DATA = r"D:\pro\rete\data\orcid"
OUT = r"D:\pro\rete\data\orcid\parquet-activities"
BATCH = 4000

AFF_FOLDERS = {
    "employments": "employment", "educations": "education",
    "distinctions": "distinction", "invited-positions": "invited-position",
    "memberships": "membership", "qualifications": "qualification",
    "services": "service",
}
FOLDER_TABLE = {"works": "work", "fundings": "funding",
                "peer-reviews": "peer_review",
                "research-resources": "research_resource", **{f: "affiliation" for f in AFF_FOLDERS}}

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
    return [] if el is None else [c for c in el if L(c.tag) == name]


def read_date(el):
    if el is None:
        return (None, None, None)
    return (child_text(el, "year"), child_text(el, "month"), child_text(el, "day"))


def to_int(s):
    try:
        return int(s)
    except (TypeError, ValueError):
        return None


def source_name(root):
    src = child(root, "source")
    return child_text(src, "source-name") if src is not None else None


def org_block(el):
    org = child(el, "organization")
    if org is None:
        return {}
    addr = child(org, "address")
    dis = child(org, "disambiguated-organization")
    return {
        "org_name": child_text(org, "name"),
        "org_city": child_text(addr, "city") if addr is not None else None,
        "org_region": child_text(addr, "region") if addr is not None else None,
        "org_country": child_text(addr, "country") if addr is not None else None,
        "org_id": child_text(dis, "disambiguated-organization-identifier") if dis is not None else None,
        "org_id_source": child_text(dis, "disambiguation-source") if dis is not None else None,
    }


def ext_ids(container):
    out = []
    for eid in children(container, "external-id"):
        out.append({
            "type": child_text(eid, "external-id-type"),
            "value": child_text(eid, "external-id-value") or child_text(eid, "external-id-normalized"),
            "url": child_text(eid, "external-id-url"),
            "relationship": child_text(eid, "external-id-relationship"),
        })
    return out


def first_doi(eids):
    for e in eids:
        if e["type"] == "doi" and e["value"]:
            return e["value"].lower()
    return None


def orcid_of(root):
    p = root.get("path")
    if p:
        segs = p.strip("/").split("/")
        if segs:
            return segs[0]
    return None


# --------------------------------------------------------------- schemas

WORK_COLS = ["orcid", "put_code", "title", "subtitle", "translated_title",
             "journal_title", "type", "pub_year", "pub_month", "pub_day",
             "doi", "url", "language_code", "country", "short_description",
             "source_name", "n_contributors", "external_ids_json",
             "contributors_json"]
AFF_COLS = ["orcid", "aff_type", "put_code", "department", "role_title",
            "start_year", "start_month", "start_day", "end_year", "end_month",
            "end_day", "org_name", "org_city", "org_region", "org_country",
            "org_id", "org_id_source", "url", "source_name"]
FUND_COLS = ["orcid", "put_code", "title", "type", "org_defined_type",
             "start_year", "end_year", "amount", "currency", "org_name",
             "org_country", "org_id", "org_id_source", "source_name",
             "external_ids_json", "contributors_json"]
PR_COLS = ["orcid", "put_code", "reviewer_role", "review_type",
           "review_group_id", "completion_year", "org_name", "org_country",
           "org_id", "org_id_source", "source_name", "review_ids_json"]
RR_COLS = ["orcid", "put_code", "title", "start_year", "end_year",
           "hosts_json", "external_ids_json", "source_name"]


def _schema(cols, ints):
    return pa.schema([(c, pa.int32() if c in ints else pa.string()) for c in cols])


SCHEMAS = {
    "work": _schema(WORK_COLS, {"pub_year", "pub_month", "pub_day", "n_contributors"}),
    "affiliation": _schema(AFF_COLS, {"start_year", "start_month", "start_day",
                                      "end_year", "end_month", "end_day"}),
    "funding": _schema(FUND_COLS, {"start_year", "end_year"}),
    "peer_review": _schema(PR_COLS, {"completion_year"}),
    "research_resource": _schema(RR_COLS, {"start_year", "end_year"}),
}


def extract_work(root, orcid):
    title_el = child(root, "title")
    y, m, d = read_date(child(root, "publication-date"))
    eids = ext_ids(child(root, "external-ids"))
    contribs = []
    cs = child(root, "contributors")
    for c in children(cs, "contributor"):
        co = child(c, "contributor-orcid")
        attrs = child(c, "contributor-attributes")
        contribs.append({
            "name": child_text(c, "credit-name"),
            "orcid": child_text(co, "path") if co is not None else None,
            "role": child_text(attrs, "contributor-role") if attrs is not None else None,
            "seq": child_text(attrs, "contributor-sequence") if attrs is not None else None,
        })
    return {
        "orcid": orcid, "put_code": root.get("put-code"),
        "title": child_text(title_el, "title") if title_el is not None else None,
        "subtitle": child_text(title_el, "subtitle") if title_el is not None else None,
        "translated_title": child_text(title_el, "translated-title") if title_el is not None else None,
        "journal_title": child_text(root, "journal-title"),
        "type": child_text(root, "type"),
        "pub_year": to_int(y), "pub_month": to_int(m), "pub_day": to_int(d),
        "doi": first_doi(eids), "url": child_text(root, "url"),
        "language_code": child_text(root, "language-code"),
        "country": child_text(root, "country"),
        "short_description": child_text(root, "short-description"),
        "source_name": source_name(root), "n_contributors": len(contribs),
        "external_ids_json": orjson.dumps(eids).decode() if eids else None,
        "contributors_json": orjson.dumps(contribs).decode() if contribs else None,
    }


def extract_aff(root, orcid, aff_type):
    sy, sm, sd = read_date(child(root, "start-date"))
    ey, em, ed = read_date(child(root, "end-date"))
    o = org_block(root)
    return {
        "orcid": orcid, "aff_type": aff_type, "put_code": root.get("put-code"),
        "department": child_text(root, "department-name"),
        "role_title": child_text(root, "role-title"),
        "start_year": to_int(sy), "start_month": to_int(sm), "start_day": to_int(sd),
        "end_year": to_int(ey), "end_month": to_int(em), "end_day": to_int(ed),
        "org_name": o.get("org_name"), "org_city": o.get("org_city"),
        "org_region": o.get("org_region"), "org_country": o.get("org_country"),
        "org_id": o.get("org_id"), "org_id_source": o.get("org_id_source"),
        "url": child_text(root, "url"), "source_name": source_name(root),
    }


def extract_funding(root, orcid):
    title_el = child(root, "title")
    sy, _, _ = read_date(child(root, "start-date"))
    ey, _, _ = read_date(child(root, "end-date"))
    o = org_block(root)
    amt = child(root, "amount")
    eids = ext_ids(child(root, "external-ids"))
    contribs = []
    cs = child(root, "contributors")
    for c in children(cs, "contributor"):
        co = child(c, "contributor-orcid")
        contribs.append({"name": child_text(c, "credit-name"),
                         "orcid": child_text(co, "path") if co is not None else None})
    return {
        "orcid": orcid, "put_code": root.get("put-code"),
        "title": child_text(title_el, "title") if title_el is not None else None,
        "type": child_text(root, "type"),
        "org_defined_type": child_text(root, "organization-defined-type"),
        "start_year": to_int(sy), "end_year": to_int(ey),
        "amount": (amt.text.strip() if amt is not None and amt.text else None),
        "currency": amt.get("currency-code") if amt is not None else None,
        "org_name": o.get("org_name"), "org_country": o.get("org_country"),
        "org_id": o.get("org_id"), "org_id_source": o.get("org_id_source"),
        "source_name": source_name(root),
        "external_ids_json": orjson.dumps(eids).decode() if eids else None,
        "contributors_json": orjson.dumps(contribs).decode() if contribs else None,
    }


def extract_peer_review(root, orcid):
    cy, _, _ = read_date(child(root, "review-completion-date"))
    o = org_block(root) if child(root, "convening-organization") is None else None
    conv = child(root, "convening-organization")
    if conv is not None:
        addr = child(conv, "address")
        dis = child(conv, "disambiguated-organization")
        o = {
            "org_name": child_text(conv, "name"),
            "org_country": child_text(addr, "country") if addr is not None else None,
            "org_id": child_text(dis, "disambiguated-organization-identifier") if dis is not None else None,
            "org_id_source": child_text(dis, "disambiguation-source") if dis is not None else None,
        }
    o = o or {}
    rids = ext_ids(child(root, "review-identifiers"))
    return {
        "orcid": orcid, "put_code": root.get("put-code"),
        "reviewer_role": child_text(root, "reviewer-role"),
        "review_type": child_text(root, "review-type"),
        "review_group_id": child_text(root, "review-group-id"),
        "completion_year": to_int(cy),
        "org_name": o.get("org_name"), "org_country": o.get("org_country"),
        "org_id": o.get("org_id"), "org_id_source": o.get("org_id_source"),
        "source_name": source_name(root),
        "review_ids_json": orjson.dumps(rids).decode() if rids else None,
    }


def extract_research_resource(root, orcid):
    # research-resource wraps a proposal with title/hosts/external-ids
    prop = child(root, "proposal") or root
    title_el = child(prop, "title")
    sy, _, _ = read_date(child(prop, "start-date"))
    ey, _, _ = read_date(child(prop, "end-date"))
    hosts = child(prop, "hosts")
    host_names = [child_text(o, "name") for o in children(hosts, "organization")]
    eids = ext_ids(child(prop, "external-ids"))
    return {
        "orcid": orcid, "put_code": root.get("put-code"),
        "title": child_text(title_el, "title") if title_el is not None else None,
        "start_year": to_int(sy), "end_year": to_int(ey),
        "hosts_json": orjson.dumps([h for h in host_names if h]).decode() if host_names else None,
        "external_ids_json": orjson.dumps(eids).decode() if eids else None,
        "source_name": source_name(root),
    }


def parse_batch(items):
    """Worker: list of (folder, xml_bytes) -> ({table: RecordBatch}, n_bad, err)."""
    acc = {t: [] for t in SCHEMAS}
    n_bad = 0
    first_error = None
    for folder, data in items:
        try:
            root = ET.fromstring(data)
            if L(root.tag) == "error":
                continue
            orcid = orcid_of(root)
            table = FOLDER_TABLE.get(folder)
            if table == "work":
                acc["work"].append(extract_work(root, orcid))
            elif table == "affiliation":
                acc["affiliation"].append(extract_aff(root, orcid, AFF_FOLDERS[folder]))
            elif table == "funding":
                acc["funding"].append(extract_funding(root, orcid))
            elif table == "peer_review":
                acc["peer_review"].append(extract_peer_review(root, orcid))
            elif table == "research_resource":
                acc["research_resource"].append(extract_research_resource(root, orcid))
        except Exception as e:  # noqa: BLE001
            n_bad += 1
            if first_error is None:
                first_error = f"{folder}: {e!r} :: {data[:160]!r}"
    batches = {}
    for t, rows in acc.items():
        schema = SCHEMAS[t]
        cols = {f.name: [] for f in schema}
        for r in rows:
            for f in schema:
                cols[f.name].append(r.get(f.name))
        try:
            batches[t] = pa.RecordBatch.from_pydict(cols, schema=schema)
        except (pa.ArrowInvalid, pa.ArrowTypeError):
            arrays = []
            for f in schema:
                vals = cols[f.name]
                try:
                    arrays.append(pa.array(vals, type=f.type))
                except (pa.ArrowInvalid, pa.ArrowTypeError):
                    if pa.types.is_integer(f.type):
                        vals = [to_int(v) for v in vals]
                    else:
                        vals = [v if v is None else str(v) for v in vals]
                    arrays.append(pa.array(vals, type=f.type))
            batches[t] = pa.RecordBatch.from_arrays(arrays, schema=schema)
    return batches, n_bad, first_error


# ------------------------------------------------------------ writer/resume

class TableWriter:
    def __init__(self, out_dir, schema, prefix, rows_per_file, chunk_rows):
        self.out_dir = out_dir
        self.schema = schema
        self.prefix = prefix
        self.rows_per_file = rows_per_file
        self.chunk_rows = chunk_rows
        self.writer = None
        self.file_index = 0
        self.pending = []
        self.pending_rows = 0
        self.file_rows = 0
        self.total_rows = 0
        os.makedirs(out_dir, exist_ok=True)

    def _flush(self):
        if not self.pending:
            return
        if self.writer is None:
            path = os.path.join(self.out_dir, f"part-{self.prefix}-{self.file_index:05d}.parquet")
            self.writer = pq.ParquetWriter(path, self.schema, compression="zstd", compression_level=3)
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
            self._close_file()

    def _close_file(self):
        self._flush()
        if self.writer is not None:
            self.writer.close()
            self.writer = None
            self.file_index += 1
            self.file_rows = 0

    def close(self):
        self._close_file()


def cp_path(out):
    return os.path.join(out, "_checkpoint.json")


def load_cp(out, fresh):
    if not fresh and os.path.exists(cp_path(out)):
        with open(cp_path(out), encoding="utf-8") as f:
            return json.load(f)
    return {"done": [], "rows": {}}


def save_cp(out, cp):
    tmp = cp_path(out) + ".tmp"
    with open(tmp, "w", encoding="utf-8") as f:
        json.dump(cp, f, indent=1)
    os.replace(tmp, cp_path(out))


def tar_tag(path):
    # ORCID_2025_10_activities_X.tar.gz -> X
    base = os.path.basename(path)
    return base.replace("ORCID_2025_10_activities_", "").split(".")[0]


def delete_tag_files(out, tag):
    for t in SCHEMAS:
        for f in glob.glob(os.path.join(out, t, f"part-{tag}-*.parquet")):
            os.remove(f)


def process_tar(path, args, cp):
    tag = tar_tag(path)
    delete_tag_files(args.out, tag)  # clear any partial output from a prior crash
    writers = {t: TableWriter(os.path.join(args.out, t), SCHEMAS[t], tag,
                              args.rows_per_file, args.chunk_rows) for t in SCHEMAS}
    total_bad = 0
    first_error = None
    n_batches = 0
    t0 = time.time()
    inflight = deque()
    max_inflight = args.workers * 2
    tar_size = os.path.getsize(path)
    raw = open(path, "rb")
    tf = tarfile.open(fileobj=raw, mode="r|*")

    def drain_one():
        nonlocal total_bad, first_error
        batches, n_bad, err = inflight.popleft().result()
        total_bad += n_bad
        if err and first_error is None:
            first_error = err
        for t, b in batches.items():
            writers[t].add(b)

    batch = []
    with ProcessPoolExecutor(max_workers=args.workers) as pool:
        for m in tf:
            if not (m.isfile() and m.name.endswith(".xml")):
                continue
            parts = m.name.split("/")
            if len(parts) < 5:
                continue
            folder = parts[3]
            if folder not in FOLDER_TABLE:
                continue
            batch.append((folder, tf.extractfile(m).read()))
            if len(batch) >= args.batch:
                inflight.append(pool.submit(parse_batch, batch))
                batch = []
                if len(inflight) >= max_inflight:
                    drain_one()
                n_batches += 1
                if args.max_batches is not None and n_batches >= args.max_batches:
                    break
                if n_batches % 100 == 0:
                    frac = raw.tell() / tar_size
                    el = time.time() - t0
                    print(f"  [{tag}] {el/60:6.1f}m  batches {n_batches:>5}  "
                          f"works {writers['work'].total_rows:>11,}  "
                          f"tar {frac*100:4.1f}%  bad {total_bad}", flush=True)
        if batch and not (args.max_batches is not None and n_batches >= args.max_batches):
            inflight.append(pool.submit(parse_batch, batch))
        while inflight:
            drain_one()

    for w in writers.values():
        w.close()
    tf.close()
    raw.close()
    rows = {t: writers[t].total_rows for t in writers}
    print(f"[{tag}] DONE in {(time.time()-t0)/60:.1f}m: "
          + ", ".join(f"{t} {rows[t]:,}" for t in rows) + f", bad {total_bad}", flush=True)
    if first_error:
        print(f"[{tag}] first bad: {first_error}", flush=True)
    return tag, rows, total_bad


def main():
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("--data", default=DATA)
    ap.add_argument("--out", default=OUT)
    ap.add_argument("--tar", default=None, help="single tarball path (test)")
    ap.add_argument("--only", default=None, help="comma list of tags e.g. 0,X")
    ap.add_argument("--workers", type=int, default=min(14, max(4, os.cpu_count() - 4)))
    ap.add_argument("--batch", type=int, default=BATCH)
    ap.add_argument("--rows-per-file", type=int, default=3_000_000)
    ap.add_argument("--chunk-rows", type=int, default=200_000)
    ap.add_argument("--max-batches", type=int, default=None)
    ap.add_argument("--fresh", action="store_true")
    args = ap.parse_args()

    os.makedirs(args.out, exist_ok=True)
    cp = load_cp(args.out, args.fresh)

    if args.tar:
        tars = [args.tar]
    else:
        tars = sorted(glob.glob(os.path.join(args.data, "ORCID_2025_10_activities_*.tar.gz")))
    if args.only:
        want = set(args.only.split(","))
        tars = [t for t in tars if tar_tag(t) in want]

    t0 = time.time()
    for path in tars:
        tag = tar_tag(path)
        if tag in cp["done"] and args.max_batches is None:
            print(f"[{tag}] already done ({cp['rows'].get(tag)}) — skipping", flush=True)
            continue
        print(f"=== tarball {tag} ({os.path.getsize(path)/1e9:.1f} GB) ===", flush=True)
        _, rows, _ = process_tar(path, args, cp)
        if args.max_batches is None:
            cp["done"].append(tag)
            cp["rows"][tag] = rows
            save_cp(args.out, cp)

    # grand totals
    grand = {}
    for rows in cp["rows"].values():
        for t, n in rows.items():
            grand[t] = grand.get(t, 0) + n
    print(f"\nALL DONE in {(time.time()-t0)/60:.1f} min. Totals:", flush=True)
    for t, n in grand.items():
        print(f"  {t:18s} {n:,}", flush=True)


if __name__ == "__main__":
    main()
