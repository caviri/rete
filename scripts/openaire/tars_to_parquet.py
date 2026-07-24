"""Stream the OpenAIRE Research Graph Dump v3.0 (2021, Zenodo recid 4707307)
into Parquet — without extracting any archive.

Entities and their inputs (all in data/openaire/2021):
  publication            publication_1..9.tar      -> parquet-publication
  dataset                dataset_1.tar             -> parquet-dataset
  otherresearchproduct   otheresearchproduct_1.tar -> parquet-otherresearchproduct
  software               software.tar              -> parquet-software
  relation               relation_1..3.tar         -> parquet-relation (edge table)
  project                project.tar               -> parquet-project
  organization           organization.tar          -> parquet-organization
  datasource             datasource.tar            -> parquet-datasource
  communities            communities_infrastructures.tar -> parquet-communities
  concepts               concepts_detection_2025-06-26.tar.gz (EPFL GraphOntology,
                         a DIFFERENT dataset: Wikipedia pages ES dump) -> parquet-concepts

Result records (publication/dataset/software/otherresearchproduct) share one
schema: flattened scalars incl. `pid_doi` (first DOI in pid[], joins against
the DataCite tables), every nested field as a JSON-string column, extra_json
catch-all. Relations become a lean edge table.

Same streaming pipeline as scripts/datacite/: members parsed by a process
pool, rolling zstd Parquet files, per-entity `_checkpoint.json` resume.
macOS AppleDouble members (`._*`) are skipped. The 4.3 GB single-member
concepts file is streamed in line-aligned blocks instead of whole members.

Usage:
  python scripts/openaire/tars_to_parquet.py                # all entities
  python scripts/openaire/tars_to_parquet.py --entity relation
  python scripts/openaire/tars_to_parquet.py --entity publication --max-units 3 --out-base /tmp/t --fresh
"""

import argparse
import gzip
import json
import os
import re
import time
import tarfile
from collections import deque
from concurrent.futures import ProcessPoolExecutor

import orjson
import pyarrow as pa
import pyarrow.parquet as pq

BASE = r"D:\pro\rete\data\openaire\2021"
OUT_BASE = r"D:\pro\rete\data\openaire"
CONCEPTS_TAR = "concepts_detection_2025-06-26.tar.gz"
CONCEPTS_BLOCK = 64 << 20  # 64 MiB line-aligned blocks


def _dumps(v):
    return orjson.dumps(v).decode()


def _str(v):
    if v is None or isinstance(v, str):
        return v
    if isinstance(v, (dict, list)):
        return _dumps(v)
    return str(v)


def _j(v):
    return _dumps(v) if v else None


def _year(s):
    if isinstance(s, str) and len(s) >= 4 and s[:4].isdigit():
        y = int(s[:4])
        return y if y > 0 else None
    return None


def _num(v):
    try:
        return float(v) if v is not None else None
    except (TypeError, ValueError):
        return None


# ----------------------------------------------------------------- entities

RESULT_SCALARS = {
    "id", "type", "maintitle", "subtitle", "publicationdate", "publisher",
    "language", "bestaccessright", "dateofcollection", "lastupdatetimestamp",
    "embargoenddate", "programmingLanguage", "pid",
}
RESULT_JSON = {
    "author_json": "author",
    "pid_json": "pid",
    "original_id_json": "originalId",
    "description_json": "description",
    "subjects_json": "subjects",
    "instance_json": "instance",
    "country_json": "country",
    "contributor_json": "contributor",
    "coverage_json": "coverage",
    "format_json": "format",
    "source_json": "source",
    "container_json": "container",
    "geolocation_json": "geolocation",
    "documentation_url_json": "documentationUrl",
    "contactgroup_json": "contactgroup",
    "contactperson_json": "contactperson",
    "tool_json": "tool",
}
RESULT_KNOWN = RESULT_SCALARS | set(RESULT_JSON.values())

RESULT_SCHEMA = pa.schema(
    [
        ("id", pa.string()),
        ("type", pa.string()),
        ("pid_doi", pa.string()),
        ("maintitle", pa.string()),
        ("subtitle", pa.string()),
        ("publicationdate", pa.string()),
        ("publication_year", pa.int32()),
        ("publisher", pa.string()),
        ("language_code", pa.string()),
        ("bestaccessright_label", pa.string()),
        ("embargoenddate", pa.string()),
        ("programming_language", pa.string()),
        ("dateofcollection", pa.string()),
        ("lastupdatetimestamp", pa.int64()),
    ]
    + [(c, pa.string()) for c in RESULT_JSON]
    + [("extra_json", pa.string())]
)


def extract_result(rec):
    pid_doi = None
    for p in rec.get("pid") or []:
        if isinstance(p, dict) and str(p.get("scheme", "")).lower() == "doi":
            v = p.get("value")
            pid_doi = v.lower() if isinstance(v, str) else v
            break
    lang = rec.get("language") or {}
    bar = rec.get("bestaccessright") or {}
    row = {
        "id": rec.get("id"),
        "type": rec.get("type"),
        "pid_doi": pid_doi,
        "maintitle": rec.get("maintitle"),
        "subtitle": rec.get("subtitle"),
        "publicationdate": rec.get("publicationdate"),
        "publication_year": _year(rec.get("publicationdate")),
        "publisher": rec.get("publisher"),
        "language_code": lang.get("code") if isinstance(lang, dict) else _str(lang),
        "bestaccessright_label": bar.get("label") if isinstance(bar, dict) else _str(bar),
        "embargoenddate": rec.get("embargoenddate"),
        "programming_language": rec.get("programmingLanguage"),
        "dateofcollection": rec.get("dateofcollection"),
        "lastupdatetimestamp": rec.get("lastupdatetimestamp"),
    }
    for col, key in RESULT_JSON.items():
        row[col] = _j(rec.get(key))
    extra = {k: v for k, v in rec.items() if k not in RESULT_KNOWN}
    row["extra_json"] = _j(extra)
    return row


RELATION_SCHEMA = pa.schema(
    [
        ("source_id", pa.string()),
        ("source_type", pa.string()),
        ("target_id", pa.string()),
        ("target_type", pa.string()),
        ("rel_name", pa.string()),
        ("rel_type", pa.string()),
        ("provenance", pa.string()),
        ("trust", pa.string()),
        ("extra_json", pa.string()),
    ]
)


def extract_relation(rec):
    s = rec.get("source") or {}
    t = rec.get("target") or {}
    r = rec.get("reltype") or {}
    p = rec.get("provenance") or {}
    extra = {k: v for k, v in rec.items()
             if k not in ("source", "target", "reltype", "provenance")}
    return {
        "source_id": s.get("id") if isinstance(s, dict) else _str(s),
        "source_type": s.get("type") if isinstance(s, dict) else None,
        "target_id": t.get("id") if isinstance(t, dict) else _str(t),
        "target_type": t.get("type") if isinstance(t, dict) else None,
        "rel_name": r.get("name") if isinstance(r, dict) else _str(r),
        "rel_type": r.get("type") if isinstance(r, dict) else None,
        "provenance": p.get("provenance") if isinstance(p, dict) else _str(p),
        "trust": _str(p.get("trust")) if isinstance(p, dict) else None,
        "extra_json": _j(extra),
    }


PROJECT_SCHEMA = pa.schema(
    [
        ("id", pa.string()),
        ("code", pa.string()),
        ("acronym", pa.string()),
        ("title", pa.string()),
        ("callidentifier", pa.string()),
        ("startdate", pa.string()),
        ("enddate", pa.string()),
        ("funded_amount", pa.float64()),
        ("total_cost", pa.float64()),
        ("currency", pa.string()),
        ("oa_mandate_datasets", pa.bool_()),
        ("oa_mandate_publications", pa.bool_()),
        ("funding_json", pa.string()),
        ("h2020programme_json", pa.string()),
        ("subject_json", pa.string()),
        ("extra_json", pa.string()),
    ]
)
PROJECT_KNOWN = {
    "id", "code", "acronym", "title", "callidentifier", "startdate", "enddate",
    "granted", "openaccessmandatefordataset", "openaccessmandateforpublications",
    "funding", "h2020programme", "subject",
}


def extract_project(rec):
    g = rec.get("granted") or {}
    extra = {k: v for k, v in rec.items() if k not in PROJECT_KNOWN}
    return {
        "id": rec.get("id"),
        "code": rec.get("code"),
        "acronym": rec.get("acronym"),
        "title": rec.get("title"),
        "callidentifier": rec.get("callidentifier"),
        "startdate": rec.get("startdate"),
        "enddate": rec.get("enddate"),
        "funded_amount": _num(g.get("fundedamount")) if isinstance(g, dict) else None,
        "total_cost": _num(g.get("totalcost")) if isinstance(g, dict) else None,
        "currency": g.get("currency") if isinstance(g, dict) else None,
        "oa_mandate_datasets": rec.get("openaccessmandatefordataset"),
        "oa_mandate_publications": rec.get("openaccessmandateforpublications"),
        "funding_json": _j(rec.get("funding")),
        "h2020programme_json": _j(rec.get("h2020programme")),
        "subject_json": _j(rec.get("subject")),
        "extra_json": _j(extra),
    }


ORG_SCHEMA = pa.schema(
    [
        ("id", pa.string()),
        ("legalname", pa.string()),
        ("legalshortname", pa.string()),
        ("websiteurl", pa.string()),
        ("country_code", pa.string()),
        ("alternativenames_json", pa.string()),
        ("pid_json", pa.string()),
        ("extra_json", pa.string()),
    ]
)
ORG_KNOWN = {"id", "legalname", "legalshortname", "websiteurl", "country",
             "alternativenames", "pid"}


def extract_org(rec):
    c = rec.get("country") or {}
    extra = {k: v for k, v in rec.items() if k not in ORG_KNOWN}
    return {
        "id": rec.get("id"),
        "legalname": rec.get("legalname"),
        "legalshortname": rec.get("legalshortname"),
        "websiteurl": rec.get("websiteurl"),
        "country_code": c.get("code") if isinstance(c, dict) else _str(c),
        "alternativenames_json": _j(rec.get("alternativenames")),
        "pid_json": _j(rec.get("pid")),
        "extra_json": _j(extra),
    }


DATASOURCE_SCHEMA = pa.schema(
    [
        ("id", pa.string()),
        ("officialname", pa.string()),
        ("englishname", pa.string()),
        ("websiteurl", pa.string()),
        ("openairecompatibility", pa.string()),
        ("datasourcetype_value", pa.string()),
        ("datasourcetype_scheme", pa.string()),
        ("versioning", pa.bool_()),
        ("original_id_json", pa.string()),
        ("contenttypes_json", pa.string()),
        ("languages_json", pa.string()),
        ("journal_json", pa.string()),
        ("policies_json", pa.string()),
        ("subjects_json", pa.string()),
        ("extra_json", pa.string()),
    ]
)
DATASOURCE_KNOWN = {
    "id", "officialname", "englishname", "websiteurl", "openairecompatibility",
    "datasourcetype", "versioning", "originalId", "contenttypes", "languages",
    "journal", "policies", "subjects",
}


def extract_datasource(rec):
    dt = rec.get("datasourcetype") or {}
    extra = {k: v for k, v in rec.items() if k not in DATASOURCE_KNOWN}
    return {
        "id": rec.get("id"),
        "officialname": rec.get("officialname"),
        "englishname": rec.get("englishname"),
        "websiteurl": rec.get("websiteurl"),
        "openairecompatibility": _str(rec.get("openairecompatibility")),
        "datasourcetype_value": dt.get("value") if isinstance(dt, dict) else _str(dt),
        "datasourcetype_scheme": dt.get("scheme") if isinstance(dt, dict) else None,
        "versioning": rec.get("versioning"),
        "original_id_json": _j(rec.get("originalId")),
        "contenttypes_json": _j(rec.get("contenttypes")),
        "languages_json": _j(rec.get("languages")),
        "journal_json": _j(rec.get("journal")),
        "policies_json": _j(rec.get("policies")),
        "subjects_json": _j(rec.get("subjects")),
        "extra_json": _j(extra),
    }


COMMUNITY_SCHEMA = pa.schema(
    [
        ("id", pa.string()),
        ("acronym", pa.string()),
        ("name", pa.string()),
        ("type", pa.string()),
        ("description", pa.string()),
        ("zenodo_community", pa.string()),
        ("subject_json", pa.string()),
        ("extra_json", pa.string()),
    ]
)
COMMUNITY_KNOWN = {"id", "acronym", "name", "type", "description",
                   "zenodo_community", "subject"}


def extract_community(rec):
    extra = {k: v for k, v in rec.items() if k not in COMMUNITY_KNOWN}
    return {
        "id": rec.get("id"),
        "acronym": rec.get("acronym"),
        "name": rec.get("name"),
        "type": rec.get("type"),
        "description": rec.get("description"),
        "zenodo_community": rec.get("zenodo_community"),
        "subject_json": _j(rec.get("subject")),
        "extra_json": _j(extra),
    }


CONCEPTS_SCHEMA = pa.schema(
    [
        ("es_id", pa.string()),
        ("page_id", pa.int64()),
        ("title", pa.string()),
        ("text", pa.string()),
        ("extra_json", pa.string()),
    ]
)


def extract_concept(rec):
    src = rec.get("_source") or {}
    extra = {k: v for k, v in rec.items() if k not in ("_id", "_source")}
    extra.update({k: v for k, v in src.items() if k not in ("id", "title", "text")})
    return {
        "es_id": rec.get("_id"),
        "page_id": src.get("id"),
        "title": src.get("title"),
        "text": src.get("text"),
        "extra_json": _j(extra),
    }


ENTITIES = {
    "publication": ("publication_*.tar", RESULT_SCHEMA, "extract_result"),
    "dataset": ("dataset_*.tar", RESULT_SCHEMA, "extract_result"),
    "otherresearchproduct": ("otheresearchproduct_*.tar", RESULT_SCHEMA, "extract_result"),
    "software": ("software.tar", RESULT_SCHEMA, "extract_result"),
    "relation": ("relation_*.tar", RELATION_SCHEMA, "extract_relation"),
    "project": ("project.tar", PROJECT_SCHEMA, "extract_project"),
    "organization": ("organization.tar", ORG_SCHEMA, "extract_org"),
    "datasource": ("datasource.tar", DATASOURCE_SCHEMA, "extract_datasource"),
    "communities": ("communities_infrastructures.tar", COMMUNITY_SCHEMA, "extract_community"),
    "concepts": (CONCEPTS_TAR, CONCEPTS_SCHEMA, "extract_concept"),
}


def build_batch(rows_cols, schema):
    try:
        return pa.RecordBatch.from_pydict(rows_cols, schema=schema)
    except (pa.ArrowInvalid, pa.ArrowTypeError):
        arrays = []
        for field in schema:
            vals = rows_cols[field.name]
            try:
                arrays.append(pa.array(vals, type=field.type))
            except (pa.ArrowInvalid, pa.ArrowTypeError):
                if pa.types.is_string(field.type):
                    vals = [_str(v) for v in vals]
                elif pa.types.is_integer(field.type):
                    fixed = []
                    for v in vals:
                        try:
                            fixed.append(int(v) if v is not None else None)
                        except (TypeError, ValueError):
                            fixed.append(None)
                    vals = fixed
                elif pa.types.is_boolean(field.type):
                    vals = [v if v is None or isinstance(v, bool)
                            else str(v).lower() in ("true", "1") for v in vals]
                elif pa.types.is_floating(field.type):
                    vals = [_num(v) for v in vals]
                arrays.append(pa.array(vals, type=field.type))
        return pa.RecordBatch.from_arrays(arrays, schema=schema)


def parse_unit(entity, name, data):
    """Worker: one work unit (tar member bytes, or a raw line block)."""
    _, schema, extract_name = ENTITIES[entity]
    extract = globals()[extract_name]
    if name.endswith(".gz"):
        data = gzip.decompress(data)
    cols = {f.name: [] for f in schema}
    n_bad = 0
    first_error = None
    for line in data.splitlines():
        if not line.strip():
            continue
        try:
            row = extract(orjson.loads(line))
            for k, v in row.items():
                cols[k].append(v)
        except Exception as e:  # noqa: BLE001
            n_bad += 1
            if first_error is None:
                first_error = f"{entity}/{name}: {e!r} :: {line[:200]!r}"
    return build_batch(cols, schema), n_bad, first_error


# ------------------------------------------------------------- work units

def natural_key(name):
    m = re.match(r"(.*?)(\d+)?(\.tar(\.gz)?)$", name)
    return (m.group(1), int(m.group(2)) if m.group(2) else 0)


def iter_units(entity, base):
    """Yield (unit_name, bytes) work units for an entity, in stable order."""
    import fnmatch
    pattern = ENTITIES[entity][0]
    tars = sorted(
        (f for f in os.listdir(base) if fnmatch.fnmatch(f, pattern)),
        key=natural_key,
    )
    if not tars:
        raise SystemExit(f"{entity}: no tars matching {pattern} in {base}")
    if entity == "concepts":
        # single huge gz member: stream line-aligned blocks instead
        with tarfile.open(os.path.join(base, tars[0]), mode="r|*") as tf:
            for member in tf:
                bn = os.path.basename(member.name)
                if not member.isfile() or bn.startswith("._") or not bn.endswith(".jsonl.gz"):
                    continue
                f = gzip.open(tf.extractfile(member))
                i = 0
                while True:
                    block = f.readlines(CONCEPTS_BLOCK)
                    if not block:
                        break
                    yield f"concepts#{i:05d}", b"".join(block)
                    i += 1
        return
    for tarname in tars:
        with tarfile.open(os.path.join(base, tarname), mode="r|*") as tf:
            for member in tf:
                bn = os.path.basename(member.name)
                if (member.isfile() and not bn.startswith("._")
                        and (bn.endswith(".json.gz") or bn.endswith(".jsonl.gz")
                             or bn.endswith(".json"))):
                    yield member.name, tf.extractfile(member).read()


# ------------------------------------------------------------ writer/resume

class RollingWriter:
    def __init__(self, out_dir, schema, rows_per_file, chunk_rows, checkpoint):
        self.out_dir = out_dir
        self.schema = schema
        self.rows_per_file = rows_per_file
        self.chunk_rows = chunk_rows
        self.checkpoint = checkpoint
        self.writer = None
        self.file_index = checkpoint["files"]
        self.pending = []
        self.pending_rows = 0
        self.file_rows = 0
        self.units_in_file = 0

    def _flush_chunk(self):
        if not self.pending:
            return
        if self.writer is None:
            path = os.path.join(self.out_dir, f"part-{self.file_index:05d}.parquet")
            self.writer = pq.ParquetWriter(
                path, self.schema, compression="zstd", compression_level=3
            )
        table = pa.Table.from_batches(self.pending, schema=self.schema)
        self.writer.write_table(table, row_group_size=self.chunk_rows)
        self.file_rows += table.num_rows
        self.pending = []
        self.pending_rows = 0

    def _close_file(self):
        self._flush_chunk()
        if self.writer is not None:
            self.writer.close()
            self.writer = None
            self.file_index += 1
            self.checkpoint["files"] = self.file_index
            self.checkpoint["members_done"] += self.units_in_file
            self.checkpoint["rows"] += self.file_rows
            save_checkpoint(self.out_dir, self.checkpoint)
        self.file_rows = 0
        self.units_in_file = 0

    def add_unit_batch(self, batch):
        self.pending.append(batch)
        self.pending_rows += batch.num_rows
        self.units_in_file += 1
        if self.pending_rows >= self.chunk_rows:
            self._flush_chunk()
        if self.file_rows >= self.rows_per_file:
            self._close_file()

    def finalize(self):
        self._close_file()


def checkpoint_path(out_dir):
    return os.path.join(out_dir, "_checkpoint.json")


def save_checkpoint(out_dir, cp):
    tmp = checkpoint_path(out_dir) + ".tmp"
    with open(tmp, "w", encoding="utf-8") as f:
        json.dump(cp, f)
    os.replace(tmp, checkpoint_path(out_dir))


def load_checkpoint(out_dir, fresh):
    if not fresh and os.path.exists(checkpoint_path(out_dir)):
        with open(checkpoint_path(out_dir), encoding="utf-8") as f:
            return json.load(f)
    return {"members_done": 0, "files": 0, "rows": 0}


# --------------------------------------------------------------------- main

def run_entity(entity, args):
    out_dir = os.path.join(args.out_base, f"parquet-{entity}")
    os.makedirs(out_dir, exist_ok=True)
    cp = load_checkpoint(out_dir, args.fresh)
    if cp.get("done"):
        print(f"[{entity}] already complete: {cp['rows']:,} rows in "
              f"{cp['files']} files — skipping", flush=True)
        return
    skip = cp["members_done"]
    if skip:
        print(f"[{entity}] resuming: skipping {skip} units "
              f"({cp['rows']:,} rows in {cp['files']} files)", flush=True)

    schema = ENTITIES[entity][1]
    rows_per_file = 5_000_000 if entity == "relation" else args.rows_per_file
    writer = RollingWriter(out_dir, schema, rows_per_file, args.chunk_rows, cp)
    total_bad = 0
    first_error = None
    n_seen = 0
    n_submitted = 0
    n_written = 0
    t0 = time.time()
    inflight = deque()
    max_inflight = args.workers

    def drain_one():
        nonlocal total_bad, first_error, n_written
        batch, n_bad, err = inflight.popleft().result()
        total_bad += n_bad
        if err and first_error is None:
            first_error = err
        writer.add_unit_batch(batch)
        n_written += 1
        if n_written % 50 == 0:
            print(f"[{entity}] [{(time.time()-t0)/60:6.1f} min] units {skip + n_written:>5}  "
                  f"rows {cp['rows'] + writer.file_rows + writer.pending_rows:>12,}  "
                  f"bad {total_bad}", flush=True)

    with ProcessPoolExecutor(max_workers=args.workers) as pool:
        for name, data in iter_units(entity, args.base):
            n_seen += 1
            if n_seen <= skip:
                continue
            if args.max_units is not None and n_submitted >= args.max_units:
                break
            inflight.append(pool.submit(parse_unit, entity, name, data))
            n_submitted += 1
            if len(inflight) >= max_inflight:
                drain_one()
        while inflight:
            drain_one()

    writer.finalize()
    cp["done"] = True
    save_checkpoint(out_dir, cp)
    print(f"[{entity}] DONE in {(time.time()-t0)/60:.1f} min: {cp['rows']:,} rows, "
          f"{cp['files']} files, {total_bad} bad lines", flush=True)
    if first_error:
        print(f"[{entity}] first bad line: {first_error}", flush=True)


def main():
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("--entity", default="all",
                    help="one of: " + ", ".join(ENTITIES) + ", or 'all'")
    ap.add_argument("--base", default=BASE)
    ap.add_argument("--out-base", default=OUT_BASE)
    ap.add_argument("--workers", type=int, default=min(14, max(4, os.cpu_count() - 4)))
    ap.add_argument("--rows-per-file", type=int, default=500_000)
    ap.add_argument("--chunk-rows", type=int, default=100_000)
    ap.add_argument("--max-units", type=int, default=None)
    ap.add_argument("--fresh", action="store_true")
    args = ap.parse_args()

    if args.entity == "all":
        order = ["communities", "organization", "datasource", "project",
                 "software", "otherresearchproduct", "dataset", "relation",
                 "publication", "concepts"]
        for entity in order:
            run_entity(entity, args)
    else:
        run_entity(args.entity, args)


if __name__ == "__main__":
    main()
