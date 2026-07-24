"""Stream the OpenAIRE Graph Dataset v11.1.1 (2026, Zenodo 20428976) into
Parquet — without extracting any archive.

Structurally different from the 2021 v3.0 dump (see tars_to_parquet.py):
  * result fields are camelCase & plural (mainTitle, authors, pids, …);
  * relations are split into per-semantic tars (product_Cites_*, datasource_hosts_*,
    …) with {relType{name,type}, source, target, sourceType, targetType,
    provenance, validated} — source/target are bare id STRINGS;
  * a new `person` entity + four bespoke person-relation tars
    (authorship {person,product,rank}, coAuthorship {author1,author2,
    coauthoredProducts}, authorAffiliation {person,organization,period},
    projectParticipation {person,project,roleInProject}).

Same engine as the 2021 script: members streamed from the tar, parsed by a
process pool, written as rolling zstd Parquet with per-target checkpoint/resume.
Analytically useful scalars are flattened to typed columns; every nested field
is kept whole as a JSON-string column; unknown keys land in extra_json. Nothing
is dropped except the derivable suffix in ids.

DISK GUARD: before opening each new Parquet part the free space on the output
volume is checked; below --min-free-gib the target stops cleanly (checkpointed,
resumable) instead of filling the disk. Built for a nearly-full drive.

Usage:
  python scripts/openaire/tars_to_parquet_2026.py --entity organization
  python scripts/openaire/tars_to_parquet_2026.py --entity small     # all that fit
  python scripts/openaire/tars_to_parquet_2026.py --entity all
  python scripts/openaire/tars_to_parquet_2026.py --entity publication --max-units 2 --fresh
"""

import argparse
import fnmatch
import gzip
import json
import os
import re
import shutil
import time
import tarfile
from collections import deque
from concurrent.futures import ProcessPoolExecutor

import orjson
import pyarrow as pa
import pyarrow.parquet as pq

BASE = r"D:\pro\rete\data\openaire\2026"
OUT_BASE = r"D:\pro\rete\data\openaire\2026"
GiB = 1 << 30


class DiskFull(Exception):
    pass


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


def _int(v):
    try:
        return int(v) if v is not None else None
    except (TypeError, ValueError):
        return None


def _first_pid(pids, scheme):
    for p in pids or []:
        if isinstance(p, dict) and str(p.get("scheme", "")).lower() == scheme:
            v = p.get("value")
            return v.lower() if scheme == "doi" and isinstance(v, str) else v
    return None


# ============================================================ RESULT entities

RESULT_KNOWN = {
    "id", "type", "pids", "mainTitle", "subTitle", "publicationDate", "publisher",
    "language", "bestAccessRight", "embargoEndDate", "dateOfCollection",
    "lastUpdateTimeStamp", "isGreen", "openAccessColor", "isInDiamondJournal",
    "publiclyFunded", "size", "version", "codeRepositoryUrl", "programmingLanguage",
    "indicators", "authors", "originalIds", "descriptions", "subjects", "instances",
    "countries", "contributors", "coverages", "formats", "sources", "container",
    "geoLocations", "documentationUrls", "contactPeople", "contactGroups", "tools",
}
RESULT_JSON = {
    "authors_json": "authors",
    "pids_json": "pids",
    "original_ids_json": "originalIds",
    "descriptions_json": "descriptions",
    "subjects_json": "subjects",
    "instances_json": "instances",
    "countries_json": "countries",
    "contributors_json": "contributors",
    "coverages_json": "coverages",
    "formats_json": "formats",
    "sources_json": "sources",
    "container_json": "container",
    "geolocations_json": "geoLocations",
    "documentation_urls_json": "documentationUrls",
    "contact_people_json": "contactPeople",
    "contact_groups_json": "contactGroups",
    "tools_json": "tools",
    "indicators_json": "indicators",
}
RESULT_SCHEMA = pa.schema(
    [
        ("id", pa.string()),
        ("type", pa.string()),
        ("pid_doi", pa.string()),
        ("main_title", pa.string()),
        ("sub_title", pa.string()),
        ("publication_date", pa.string()),
        ("publication_year", pa.int32()),
        ("publisher", pa.string()),
        ("language_code", pa.string()),
        ("best_access_right_code", pa.string()),
        ("best_access_right_label", pa.string()),
        ("embargo_end_date", pa.string()),
        ("date_of_collection", pa.string()),
        ("last_update_timestamp", pa.int64()),
        ("is_green", pa.bool_()),
        ("open_access_color", pa.string()),
        ("is_in_diamond_journal", pa.bool_()),
        ("publicly_funded", pa.string()),
        ("size", pa.string()),
        ("version", pa.string()),
        ("code_repository_url", pa.string()),
        ("programming_language", pa.string()),
        ("citation_count", pa.float64()),
        ("influence", pa.float64()),
        ("popularity", pa.float64()),
        ("impulse", pa.float64()),
        ("downloads", pa.int64()),
        ("views", pa.int64()),
    ]
    + [(c, pa.string()) for c in RESULT_JSON]
    + [("extra_json", pa.string())]
)


def extract_result(rec):
    lang = rec.get("language") or {}
    bar = rec.get("bestAccessRight") or {}
    ind = rec.get("indicators") or {}
    ci = (ind.get("citationImpact") or {}) if isinstance(ind, dict) else {}
    uc = (ind.get("usageCounts") or {}) if isinstance(ind, dict) else {}
    row = {
        "id": rec.get("id"),
        "type": rec.get("type"),
        "pid_doi": _first_pid(rec.get("pids"), "doi"),
        "main_title": rec.get("mainTitle"),
        "sub_title": rec.get("subTitle"),
        "publication_date": rec.get("publicationDate"),
        "publication_year": _year(rec.get("publicationDate")),
        "publisher": rec.get("publisher"),
        "language_code": lang.get("code") if isinstance(lang, dict) else _str(lang),
        "best_access_right_code": bar.get("code") if isinstance(bar, dict) else None,
        "best_access_right_label": bar.get("label") if isinstance(bar, dict) else _str(bar),
        "embargo_end_date": rec.get("embargoEndDate"),
        "date_of_collection": rec.get("dateOfCollection"),
        "last_update_timestamp": _int(rec.get("lastUpdateTimeStamp")),
        "is_green": rec.get("isGreen"),
        "open_access_color": rec.get("openAccessColor"),
        "is_in_diamond_journal": rec.get("isInDiamondJournal"),
        "publicly_funded": _str(rec.get("publiclyFunded")),
        "size": rec.get("size"),
        "version": rec.get("version"),
        "code_repository_url": rec.get("codeRepositoryUrl"),
        "programming_language": rec.get("programmingLanguage"),
        "citation_count": _num(ci.get("citationCount")),
        "influence": _num(ci.get("influence")),
        "popularity": _num(ci.get("popularity")),
        "impulse": _num(ci.get("impulse")),
        "downloads": _int(uc.get("downloads")) if isinstance(uc, dict) else None,
        "views": _int(uc.get("views")) if isinstance(uc, dict) else None,
    }
    for col, key in RESULT_JSON.items():
        row[col] = _j(rec.get(key))
    row["extra_json"] = _j({k: v for k, v in rec.items() if k not in RESULT_KNOWN})
    return row


# ================================================================ RELATION

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
        ("validated", pa.bool_()),
        ("extra_json", pa.string()),
    ]
)
RELATION_KNOWN = {"source", "target", "sourceType", "targetType", "relType",
                  "provenance", "validated"}


def extract_relation(rec):
    rt = rec.get("relType") or {}
    p = rec.get("provenance") or {}
    return {
        "source_id": _str(rec.get("source")),
        "source_type": rec.get("sourceType"),
        "target_id": _str(rec.get("target")),
        "target_type": rec.get("targetType"),
        "rel_name": rt.get("name") if isinstance(rt, dict) else _str(rt),
        "rel_type": rt.get("type") if isinstance(rt, dict) else None,
        "provenance": p.get("provenance") if isinstance(p, dict) else _str(p),
        "trust": _str(p.get("trust")) if isinstance(p, dict) else None,
        "validated": rec.get("validated"),
        "extra_json": _j({k: v for k, v in rec.items() if k not in RELATION_KNOWN}),
    }


# ================================================================ PERSON

PERSON_SCHEMA = pa.schema(
    [
        ("id", pa.string()),
        ("given_name", pa.string()),
        ("family_name", pa.string()),
        ("biography", pa.string()),
        ("consent", pa.bool_()),
        ("pid_orcid", pa.string()),
        ("alternative_names_json", pa.string()),
        ("pids_json", pa.string()),
        ("subject_json", pa.string()),
        ("extra_json", pa.string()),
    ]
)
PERSON_KNOWN = {"id", "givenName", "familyName", "biography", "consent", "pids",
                "alternativeNames", "subject"}


def extract_person(rec):
    return {
        "id": rec.get("id"),
        "given_name": rec.get("givenName"),
        "family_name": rec.get("familyName"),
        "biography": rec.get("biography") or None,
        "consent": rec.get("consent"),
        "pid_orcid": _first_pid(rec.get("pids"), "orcid"),
        "alternative_names_json": _j(rec.get("alternativeNames")),
        "pids_json": _j(rec.get("pids")),
        "subject_json": _j(rec.get("subject")),
        "extra_json": _j({k: v for k, v in rec.items() if k not in PERSON_KNOWN}),
    }


# --- bespoke person relations ---

AUTHORSHIP_SCHEMA = pa.schema(
    [("person_id", pa.string()), ("product_id", pa.string()),
     ("rank", pa.int32()), ("extra_json", pa.string())]
)


def extract_authorship(rec):
    return {"person_id": _str(rec.get("person")), "product_id": _str(rec.get("product")),
            "rank": _int(rec.get("rank")),
            "extra_json": _j({k: v for k, v in rec.items()
                              if k not in ("person", "product", "rank")})}


COAUTHORSHIP_SCHEMA = pa.schema(
    [("author1_id", pa.string()), ("author2_id", pa.string()),
     ("coauthored_products", pa.int32()), ("extra_json", pa.string())]
)


def extract_coauthorship(rec):
    return {"author1_id": _str(rec.get("author1")), "author2_id": _str(rec.get("author2")),
            "coauthored_products": _int(rec.get("coauthoredProducts")),
            "extra_json": _j({k: v for k, v in rec.items()
                              if k not in ("author1", "author2", "coauthoredProducts")})}


AFFILIATION_SCHEMA = pa.schema(
    [("person_id", pa.string()), ("organization_id", pa.string()),
     ("period_json", pa.string()), ("extra_json", pa.string())]
)


def extract_affiliation(rec):
    return {"person_id": _str(rec.get("person")),
            "organization_id": _str(rec.get("organization")),
            "period_json": _j(rec.get("period")),
            "extra_json": _j({k: v for k, v in rec.items()
                              if k not in ("person", "organization", "period")})}


PARTICIPATION_SCHEMA = pa.schema(
    [("person_id", pa.string()), ("project_id", pa.string()),
     ("role", pa.string()), ("extra_json", pa.string())]
)


def extract_participation(rec):
    return {"person_id": _str(rec.get("person")), "project_id": _str(rec.get("project")),
            "role": rec.get("roleInProject"),
            "extra_json": _j({k: v for k, v in rec.items()
                              if k not in ("person", "project", "roleInProject")})}


# ================================================================ ORG / DS / PROJECT / COMMUNITY

ORG_SCHEMA = pa.schema(
    [("id", pa.string()), ("legal_name", pa.string()), ("legal_short_name", pa.string()),
     ("website_url", pa.string()), ("country_code", pa.string()), ("country_label", pa.string()),
     ("alternative_names_json", pa.string()), ("pids_json", pa.string()), ("extra_json", pa.string())]
)
ORG_KNOWN = {"id", "legalName", "legalShortName", "websiteUrl", "country",
             "alternativeNames", "pids"}


def extract_org(rec):
    c = rec.get("country") or {}
    return {
        "id": rec.get("id"),
        "legal_name": rec.get("legalName"),
        "legal_short_name": rec.get("legalShortName"),
        "website_url": rec.get("websiteUrl"),
        "country_code": c.get("code") if isinstance(c, dict) else _str(c),
        "country_label": c.get("label") if isinstance(c, dict) else None,
        "alternative_names_json": _j(rec.get("alternativeNames")),
        "pids_json": _j(rec.get("pids")),
        "extra_json": _j({k: v for k, v in rec.items() if k not in ORG_KNOWN}),
    }


DATASOURCE_SCHEMA = pa.schema(
    [("id", pa.string()), ("official_name", pa.string()), ("english_name", pa.string()),
     ("website_url", pa.string()), ("openaire_compatibility", pa.string()),
     ("datasource_type_value", pa.string()), ("datasource_type_scheme", pa.string()),
     ("versioning", pa.bool_()), ("original_ids_json", pa.string()),
     ("content_types_json", pa.string()), ("languages_json", pa.string()),
     ("journal_json", pa.string()), ("policies_json", pa.string()),
     ("subjects_json", pa.string()), ("pids_json", pa.string()), ("extra_json", pa.string())]
)
DATASOURCE_KNOWN = {"id", "officialName", "englishName", "websiteUrl", "openaireCompatibility",
                    "type", "versioning", "originalIds", "contentTypes", "languages",
                    "journal", "policies", "subjects", "pids"}


def extract_datasource(rec):
    dt = rec.get("type") or {}
    return {
        "id": rec.get("id"),
        "official_name": rec.get("officialName"),
        "english_name": rec.get("englishName"),
        "website_url": rec.get("websiteUrl"),
        "openaire_compatibility": _str(rec.get("openaireCompatibility")),
        "datasource_type_value": dt.get("value") if isinstance(dt, dict) else _str(dt),
        "datasource_type_scheme": dt.get("scheme") if isinstance(dt, dict) else None,
        "versioning": rec.get("versioning"),
        "original_ids_json": _j(rec.get("originalIds")),
        "content_types_json": _j(rec.get("contentTypes")),
        "languages_json": _j(rec.get("languages")),
        "journal_json": _j(rec.get("journal")),
        "policies_json": _j(rec.get("policies")),
        "subjects_json": _j(rec.get("subjects")),
        "pids_json": _j(rec.get("pids")),
        "extra_json": _j({k: v for k, v in rec.items() if k not in DATASOURCE_KNOWN}),
    }


PROJECT_SCHEMA = pa.schema(
    [("id", pa.string()), ("code", pa.string()), ("acronym", pa.string()),
     ("title", pa.string()), ("call_identifier", pa.string()), ("start_date", pa.string()),
     ("end_date", pa.string()), ("funded_amount", pa.float64()), ("total_cost", pa.float64()),
     ("currency", pa.string()), ("oa_mandate_datasets", pa.bool_()),
     ("oa_mandate_publications", pa.bool_()), ("summary", pa.string()),
     ("keywords", pa.string()), ("website_url", pa.string()),
     ("fundings_json", pa.string()), ("h2020programmes_json", pa.string()),
     ("subjects_json", pa.string()), ("pids_json", pa.string()), ("extra_json", pa.string())]
)
PROJECT_KNOWN = {"id", "code", "acronym", "title", "callIdentifier", "startDate", "endDate",
                 "granted", "openAccessMandateForDataset", "openAccessMandateForPublications",
                 "summary", "keywords", "websiteUrl", "fundings", "h2020Programmes",
                 "subjects", "pids"}


def extract_project(rec):
    g = rec.get("granted") or {}
    return {
        "id": rec.get("id"),
        "code": rec.get("code"),
        "acronym": rec.get("acronym"),
        "title": rec.get("title"),
        "call_identifier": rec.get("callIdentifier"),
        "start_date": rec.get("startDate"),
        "end_date": rec.get("endDate"),
        "funded_amount": _num(g.get("fundedAmount")) if isinstance(g, dict) else None,
        "total_cost": _num(g.get("totalCost")) if isinstance(g, dict) else None,
        "currency": g.get("currency") if isinstance(g, dict) else None,
        "oa_mandate_datasets": rec.get("openAccessMandateForDataset"),
        "oa_mandate_publications": rec.get("openAccessMandateForPublications"),
        "summary": rec.get("summary"),
        "keywords": _str(rec.get("keywords")),
        "website_url": rec.get("websiteUrl"),
        "fundings_json": _j(rec.get("fundings")),
        "h2020programmes_json": _j(rec.get("h2020Programmes")),
        "subjects_json": _j(rec.get("subjects")),
        "pids_json": _j(rec.get("pids")),
        "extra_json": _j({k: v for k, v in rec.items() if k not in PROJECT_KNOWN}),
    }


COMMUNITY_SCHEMA = pa.schema(
    [("id", pa.string()), ("acronym", pa.string()), ("name", pa.string()),
     ("type", pa.string()), ("description", pa.string()), ("zenodo_community", pa.string()),
     ("subjects_json", pa.string()), ("extra_json", pa.string())]
)
COMMUNITY_KNOWN = {"id", "acronym", "name", "type", "description", "zenodoCommunity", "subjects"}


def extract_community(rec):
    return {
        "id": rec.get("id"),
        "acronym": rec.get("acronym"),
        "name": rec.get("name"),
        "type": rec.get("type"),
        "description": rec.get("description"),
        "zenodo_community": _str(rec.get("zenodoCommunity")),
        "subjects_json": _j(rec.get("subjects")),
        "extra_json": _j({k: v for k, v in rec.items() if k not in COMMUNITY_KNOWN}),
    }


# ============================================================ target registry
# name -> (glob patterns, out subdir, schema, extractor, approx GiB of tars)

TARGETS = {
    "communities":    (["communities_infrastructures.tar"], "parquet-communities", COMMUNITY_SCHEMA, "extract_community"),
    "organization":   (["organization.tar"], "parquet-organization", ORG_SCHEMA, "extract_org"),
    "datasource":     (["datasource.tar"], "parquet-datasource", DATASOURCE_SCHEMA, "extract_datasource"),
    "project":        (["project.tar"], "parquet-project", PROJECT_SCHEMA, "extract_project"),
    "person":         (["person.tar"], "parquet-person", PERSON_SCHEMA, "extract_person"),
    "software":       (["software.tar"], "parquet-software", RESULT_SCHEMA, "extract_result"),
    "person_participation": (["person_projectParticipation.tar"], "parquet-person_participation", PARTICIPATION_SCHEMA, "extract_participation"),
    "person_affiliation":   (["person_authorAffiliation.tar"], "parquet-person_affiliation", AFFILIATION_SCHEMA, "extract_affiliation"),
    "person_coauthorship":  (["person_coAuthorship_*.tar"], "parquet-person_coauthorship", COAUTHORSHIP_SCHEMA, "extract_coauthorship"),
    "person_authorship":    (["person_authorship_*.tar"], "parquet-person_authorship", AUTHORSHIP_SCHEMA, "extract_authorship"),
    "otherresearchproduct": (["otherresearchproduct_*.tar"], "parquet-otherresearchproduct", RESULT_SCHEMA, "extract_result"),
    "dataset":        (["dataset_*.tar"], "parquet-dataset", RESULT_SCHEMA, "extract_result"),
    "publication":    (["publication_*.tar"], "parquet-publication", RESULT_SCHEMA, "extract_result"),
    "relation":       (["product_*.tar", "datasource_hosts_*.tar", "datasource_provides_*.tar",
                        "organization_IsChildOf.tar", "organization_provides.tar",
                        "project_hasParticipant.tar", "project_produces.tar"],
                       "parquet-relation", RELATION_SCHEMA, "extract_relation"),
}

# Small targets that fit on a nearly-full drive (parquet output ≲ 12 GiB).
SMALL = ["communities", "organization", "datasource", "project", "software",
         "person_participation", "person_affiliation", "person",
         "person_coauthorship", "person_authorship"]
# Everything, small first so partial disk-limited runs still land useful tables.
ALL = SMALL + ["otherresearchproduct", "dataset", "relation", "publication"]

EXTRACTORS = {
    "extract_result": extract_result, "extract_relation": extract_relation,
    "extract_person": extract_person, "extract_authorship": extract_authorship,
    "extract_coauthorship": extract_coauthorship, "extract_affiliation": extract_affiliation,
    "extract_participation": extract_participation, "extract_org": extract_org,
    "extract_datasource": extract_datasource, "extract_project": extract_project,
    "extract_community": extract_community,
}


# ============================================================ batch / parse

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
                    vals = [_int(v) for v in vals]
                elif pa.types.is_boolean(field.type):
                    vals = [v if v is None or isinstance(v, bool)
                            else str(v).lower() in ("true", "1") for v in vals]
                elif pa.types.is_floating(field.type):
                    vals = [_num(v) for v in vals]
                arrays.append(pa.array(vals, type=field.type))
        return pa.RecordBatch.from_arrays(arrays, schema=schema)


def parse_unit(target, name, data):
    _, _, schema, extract_name = TARGETS[target]
    extract = EXTRACTORS[extract_name]
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
                first_error = f"{target}/{name}: {e!r} :: {line[:200]!r}"
    return build_batch(cols, schema), n_bad, first_error


# ============================================================ work units

def natural_key(name):
    m = re.match(r"(.*?)(\d+)?(\.tar(\.gz)?)$", name)
    return (m.group(1), int(m.group(2)) if m.group(2) else 0)


def list_tars(target, base):
    patterns = TARGETS[target][0]
    names = [f for f in os.listdir(base)
             if any(fnmatch.fnmatch(f, p) for p in patterns)]
    return sorted(set(names), key=natural_key)


def iter_units(target, base):
    for tarname in list_tars(target, base):
        with tarfile.open(os.path.join(base, tarname), mode="r|*") as tf:
            for member in tf:
                bn = os.path.basename(member.name)
                if (member.isfile() and not bn.startswith("._") and member.size
                        and (bn.endswith(".json.gz") or bn.endswith(".jsonl.gz")
                             or bn.endswith(".json") or "." not in bn)):
                    # extensionless members (e.g. the single-file communities tar)
                    # are raw JSON-lines; parse_unit only gunzips *.gz names.
                    yield member.name, tf.extractfile(member).read()


# ============================================================ writer / resume

class RollingWriter:
    def __init__(self, out_dir, schema, rows_per_file, chunk_rows, checkpoint, min_free):
        self.out_dir = out_dir
        self.schema = schema
        self.rows_per_file = rows_per_file
        self.chunk_rows = chunk_rows
        self.checkpoint = checkpoint
        self.min_free = min_free
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
            if shutil.disk_usage(self.out_dir).free < self.min_free:
                raise DiskFull(
                    f"free space below {self.min_free / GiB:.0f} GiB before new part")
            path = os.path.join(self.out_dir, f"part-{self.file_index:05d}.parquet")
            self.writer = pq.ParquetWriter(path, self.schema, compression="zstd",
                                           compression_level=3)
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


# ============================================================ run

def run_target(target, args):
    out_dir = os.path.join(args.out_base, TARGETS[target][1])
    os.makedirs(out_dir, exist_ok=True)
    cp = load_checkpoint(out_dir, args.fresh)
    if cp.get("done"):
        print(f"[{target}] already complete: {cp['rows']:,} rows in {cp['files']} files", flush=True)
        return True
    skip = cp["members_done"]
    if skip:
        print(f"[{target}] resuming: skip {skip} units ({cp['rows']:,} rows)", flush=True)

    schema = TARGETS[target][2]
    rows_per_file = 5_000_000 if target == "relation" else args.rows_per_file
    writer = RollingWriter(out_dir, schema, rows_per_file, args.chunk_rows, cp,
                           args.min_free_gib * GiB)
    total_bad = 0
    first_error = None
    n_seen = n_submitted = n_written = 0
    t0 = time.time()
    inflight = deque()

    def drain_one():
        nonlocal total_bad, first_error, n_written
        batch, n_bad, err = inflight.popleft().result()
        total_bad += n_bad
        if err and first_error is None:
            first_error = err
        writer.add_unit_batch(batch)
        n_written += 1
        if n_written % 50 == 0:
            free = shutil.disk_usage(out_dir).free / GiB
            print(f"[{target}] [{(time.time()-t0)/60:6.1f}m] units {skip+n_written:>5}  "
                  f"rows {cp['rows']+writer.file_rows+writer.pending_rows:>13,}  "
                  f"bad {total_bad}  free {free:.0f}GiB", flush=True)

    try:
        with ProcessPoolExecutor(max_workers=args.workers) as pool:
            for name, data in iter_units(target, args.base):
                n_seen += 1
                if n_seen <= skip:
                    continue
                if args.max_units is not None and n_submitted >= args.max_units:
                    break
                inflight.append(pool.submit(parse_unit, target, name, data))
                n_submitted += 1
                if len(inflight) >= args.workers:
                    drain_one()
            while inflight:
                drain_one()
    except DiskFull as e:
        writer.finalize()
        print(f"[{target}] STOPPED (disk): {e}. {cp['rows']:,} rows in {cp['files']} files "
              f"so far — resumable.", flush=True)
        return False

    writer.finalize()
    cp["done"] = True
    save_checkpoint(out_dir, cp)
    print(f"[{target}] DONE in {(time.time()-t0)/60:.1f}m: {cp['rows']:,} rows, "
          f"{cp['files']} files, {total_bad} bad", flush=True)
    if first_error:
        print(f"[{target}] first bad: {first_error}", flush=True)
    return True


def main():
    ap = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    ap.add_argument("--entity", default="small",
                    help="a target name, 'small', or 'all'. Targets: " + ", ".join(TARGETS))
    ap.add_argument("--base", default=BASE)
    ap.add_argument("--out-base", default=OUT_BASE)
    ap.add_argument("--workers", type=int, default=min(14, max(4, (os.cpu_count() or 8) - 4)))
    ap.add_argument("--rows-per-file", type=int, default=500_000)
    ap.add_argument("--chunk-rows", type=int, default=100_000)
    ap.add_argument("--max-units", type=int, default=None)
    ap.add_argument("--min-free-gib", type=int, default=3,
                    help="stop before free space drops below this many GiB")
    ap.add_argument("--fresh", action="store_true")
    args = ap.parse_args()

    if args.entity in ("small", "all"):
        order = SMALL if args.entity == "small" else ALL
    else:
        order = [args.entity]
    for target in order:
        if not run_target(target, args):
            print("Halting: disk guard tripped. Free space and re-run to resume.", flush=True)
            break


if __name__ == "__main__":
    main()
