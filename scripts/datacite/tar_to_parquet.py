"""Stream the DataCite Public Data File tar.gz into Parquet — without ever
extracting the archive.

Members are read sequentially from the compressed tar stream (a few MB each),
parsed into Arrow batches by a process pool, and written as rolling zstd
Parquet files. Peak disk usage = the tar itself + the Parquet output.

Handles both public-data-file layouts:
  2023:   ./<prefix>/part_NNNNN.jsonl              flat attribute records
  2024+:  dois/updated_YYYY-MM/part_NNNN.jsonl.gz  {id, attributes, relationships}

Schema: analytically useful scalars are flattened to typed columns; every
nested field is kept whole as a JSON-string column (NULL when empty); any
unknown attribute key lands in extra_json. Nothing is dropped except the
derivable `suffix` (doi = prefix + "/" + suffix).

Resumable: a checkpoint is written each time an output file closes. On
restart, members already in closed Parquet files are skipped and the
partial last file is overwritten.

Usage:
  python tar_to_parquet.py                       # full run, defaults below
  python tar_to_parquet.py --max-members 40 --out <dir>   # quick test slice
"""

import argparse
import gzip
import json
import os
import sys
import time
import tarfile
from collections import deque
from concurrent.futures import ProcessPoolExecutor

import orjson
import pyarrow as pa
import pyarrow.parquet as pq

DEFAULT_TAR = r"D:\pro\rete\data\datacite\DataCite_Public_Data_File_2023.tar.gz"
DEFAULT_OUT = r"D:\pro\rete\data\datacite\parquet-2023"

# attribute keys flattened to typed columns
SCALAR_KEYS = {
    "doi", "prefix", "suffix", "state", "source", "isActive", "created",
    "registered", "updated", "published", "publicationYear", "language",
    "version", "metadataVersion", "schemaVersion", "url", "publisher",
    "reason", "types",
    # 2024+ usage/citation metrics
    "citationCount", "referenceCount", "viewCount", "downloadCount",
    "versionCount", "versionOfCount", "partCount", "partOfCount",
}
METRIC_KEYS = [
    ("citation_count", "citationCount"),
    ("reference_count", "referenceCount"),
    ("view_count", "viewCount"),
    ("download_count", "downloadCount"),
    ("version_count", "versionCount"),
    ("version_of_count", "versionOfCount"),
    ("part_count", "partCount"),
    ("part_of_count", "partOfCount"),
]
# attribute keys kept as JSON-string columns (column name -> attribute key)
JSON_KEYS = {
    "container_json": "container",
    "creators_json": "creators",
    "titles_json": "titles",
    "subjects_json": "subjects",
    "contributors_json": "contributors",
    "dates_json": "dates",
    "related_identifiers_json": "relatedIdentifiers",
    "related_items_json": "relatedItems",
    "descriptions_json": "descriptions",
    "geo_locations_json": "geoLocations",
    "funding_references_json": "fundingReferences",
    "rights_list_json": "rightsList",
    "identifiers_json": "identifiers",
    "alternate_identifiers_json": "alternateIdentifiers",
    "sizes_json": "sizes",
    "formats_json": "formats",
    "content_url_json": "contentUrl",
    # 2024+ usage/citation time series
    "citations_over_time_json": "citationsOverTime",
    "views_over_time_json": "viewsOverTime",
    "downloads_over_time_json": "downloadsOverTime",
}
KNOWN_KEYS = SCALAR_KEYS | set(JSON_KEYS.values()) | {"titles"}

SCHEMA = pa.schema(
    [
        ("doi", pa.string()),
        ("prefix", pa.string()),
        ("state", pa.string()),
        ("source", pa.string()),
        ("is_active", pa.bool_()),
        ("client_id", pa.string()),
        ("created", pa.string()),
        ("registered", pa.string()),
        ("updated", pa.string()),
        ("published", pa.string()),
        ("publication_year", pa.int32()),
        ("language", pa.string()),
        ("version", pa.string()),
        ("metadata_version", pa.int32()),
        ("schema_version", pa.string()),
        ("url", pa.string()),
        ("publisher", pa.string()),
        ("resource_type_general", pa.string()),
        ("resource_type", pa.string()),
        ("reason", pa.string()),
        ("title", pa.string()),
    ]
    + [(name, pa.int32()) for name, _ in METRIC_KEYS]
    + [
        ("types_json", pa.string()),
    ]
    + [(name, pa.string()) for name in JSON_KEYS]
    + [("extra_json", pa.string())]
)
COLUMNS = [f.name for f in SCHEMA]


def _dumps(v):
    return orjson.dumps(v).decode()


def _str(v):
    if v is None or isinstance(v, str):
        return v
    if isinstance(v, (dict, list)):
        return _dumps(v)
    return str(v)


def _coerce_column(values, typ):
    if pa.types.is_string(typ):
        return [_str(v) for v in values]
    if pa.types.is_integer(typ):
        out = []
        for v in values:
            try:
                out.append(int(v) if v is not None else None)
            except (TypeError, ValueError):
                out.append(None)
        return out
    if pa.types.is_boolean(typ):
        return [
            v if v is None or isinstance(v, bool) else str(v).lower() in ("true", "1")
            for v in values
        ]
    return values


def build_batch(cols):
    """Fast path straight to Arrow; on a type surprise (e.g. version: 1 as
    int), coerce only the offending columns instead of losing the member."""
    try:
        return pa.RecordBatch.from_pydict(cols, schema=SCHEMA)
    except (pa.ArrowInvalid, pa.ArrowTypeError):
        arrays = []
        for field in SCHEMA:
            vals = cols[field.name]
            try:
                arrays.append(pa.array(vals, type=field.type))
            except (pa.ArrowInvalid, pa.ArrowTypeError):
                arrays.append(pa.array(_coerce_column(vals, field.type), type=field.type))
        return pa.RecordBatch.from_arrays(arrays, schema=SCHEMA)


def parse_member(name, data):
    """Worker: raw member bytes -> (RecordBatch, n_bad, first_error)."""
    if name.endswith(".gz"):
        data = gzip.decompress(data)
    cols = {c: [] for c in COLUMNS}
    n_bad = 0
    first_error = None
    for line in data.splitlines():
        if not line.strip():
            continue
        try:
            rec = orjson.loads(line)
            client_id = None
            if "attributes" in rec and "doi" not in rec:  # 2024+ envelope
                client_id = (
                    ((rec.get("relationships") or {}).get("client") or {}).get("data")
                    or {}
                ).get("id")
                rec = rec["attributes"]

            extra = {}
            publisher = rec.get("publisher")
            if isinstance(publisher, dict):  # 2024+ publisher object
                extra["publisher_obj"] = publisher
                publisher = publisher.get("name")

            types = rec.get("types")
            if not isinstance(types, dict):
                if types:
                    extra["types_raw"] = types
                types = {}
            titles = rec.get("titles")
            if not isinstance(titles, list):
                titles = [titles] if titles else []
            t0 = titles[0] if titles else None
            title = t0.get("title") if isinstance(t0, dict) else _str(t0)

            year = rec.get("publicationYear")
            if not isinstance(year, int):
                try:
                    year = int(year) if year else None
                except (TypeError, ValueError):
                    extra["publicationYear_raw"] = year
                    year = None

            for k, v in rec.items():
                if k not in KNOWN_KEYS and k != "relationships":
                    extra[k] = v

            cols["doi"].append(rec.get("doi"))
            cols["prefix"].append(rec.get("prefix"))
            cols["state"].append(rec.get("state"))
            cols["source"].append(rec.get("source"))
            cols["is_active"].append(rec.get("isActive"))
            cols["client_id"].append(client_id)
            cols["created"].append(rec.get("created"))
            cols["registered"].append(rec.get("registered"))
            cols["updated"].append(rec.get("updated"))
            cols["published"].append(rec.get("published"))
            cols["publication_year"].append(year)
            cols["language"].append(rec.get("language"))
            cols["version"].append(rec.get("version"))
            cols["metadata_version"].append(rec.get("metadataVersion"))
            cols["schema_version"].append(rec.get("schemaVersion"))
            cols["url"].append(rec.get("url"))
            cols["publisher"].append(publisher)
            cols["resource_type_general"].append(types.get("resourceTypeGeneral"))
            cols["resource_type"].append(types.get("resourceType"))
            cols["reason"].append(rec.get("reason"))
            cols["title"].append(title)
            for col, key in METRIC_KEYS:
                cols[col].append(rec.get(key))
            cols["types_json"].append(_dumps(types) if types else None)
            for col, key in JSON_KEYS.items():
                v = rec.get(key)
                cols[col].append(_dumps(v) if v else None)
            cols["extra_json"].append(_dumps(extra) if extra else None)
        except Exception as e:  # noqa: BLE001 - count and move on
            n_bad += 1
            if first_error is None:
                first_error = f"{name}: {e!r} :: {line[:200]!r}"
    batch = build_batch(cols)
    return batch, n_bad, first_error


def is_data_member(m):
    return m.isfile() and (m.name.endswith(".jsonl") or m.name.endswith(".jsonl.gz"))


class RollingWriter:
    """Writes Arrow batches to sequential part-NNNNN.parquet files and
    checkpoints member progress each time a file closes."""

    def __init__(self, out_dir, rows_per_file, chunk_rows, checkpoint):
        self.out_dir = out_dir
        self.rows_per_file = rows_per_file
        self.chunk_rows = chunk_rows
        self.checkpoint = checkpoint
        self.writer = None
        self.file_index = checkpoint["files"]
        self.pending = []
        self.pending_rows = 0
        self.file_rows = 0
        self.members_in_file = 0

    def _flush_chunk(self):
        if not self.pending:
            return
        if self.writer is None:
            path = os.path.join(self.out_dir, f"part-{self.file_index:05d}.parquet")
            self.writer = pq.ParquetWriter(
                path, SCHEMA, compression="zstd", compression_level=3
            )
        table = pa.Table.from_batches(self.pending, schema=SCHEMA)
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
            self.checkpoint["members_done"] += self.members_in_file
            self.checkpoint["rows"] += self.file_rows
            save_checkpoint(self.out_dir, self.checkpoint)
        self.file_rows = 0
        self.members_in_file = 0

    def add_member_batch(self, batch):
        self.pending.append(batch)
        self.pending_rows += batch.num_rows
        self.members_in_file += 1
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


def main():
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("--tar", default=DEFAULT_TAR)
    ap.add_argument("--out", default=DEFAULT_OUT)
    ap.add_argument("--workers", type=int, default=min(20, max(4, os.cpu_count() - 4)))
    ap.add_argument("--rows-per-file", type=int, default=500_000)
    ap.add_argument("--chunk-rows", type=int, default=131_072)
    ap.add_argument("--max-members", type=int, default=None, help="test mode: stop after N data members")
    ap.add_argument("--fresh", action="store_true", help="ignore existing checkpoint")
    args = ap.parse_args()

    os.makedirs(args.out, exist_ok=True)
    cp = load_checkpoint(args.out, args.fresh)
    skip = cp["members_done"]
    if skip:
        print(f"resuming: skipping {skip} already-converted members "
              f"({cp['rows']} rows in {cp['files']} closed files)", flush=True)

    tar_size = os.path.getsize(args.tar)
    raw = open(args.tar, "rb")
    tf = tarfile.open(fileobj=raw, mode="r|*")  # 2023 is .tar.gz, 2024+ plain .tar

    writer = RollingWriter(args.out, args.rows_per_file, args.chunk_rows, cp)
    total_bad = 0
    first_error = None
    n_seen = 0       # data members encountered in stream order
    n_submitted = 0
    n_written = skip
    t0 = time.time()
    inflight = deque()
    max_inflight = args.workers * 3

    def drain_one():
        nonlocal total_bad, first_error, n_written
        batch, n_bad, err = inflight.popleft().result()
        total_bad += n_bad
        if err and first_error is None:
            first_error = err
        writer.add_member_batch(batch)
        n_written += 1
        if n_written % 500 == 0:
            frac = raw.tell() / tar_size
            elapsed = time.time() - t0
            eta = elapsed / frac * (1 - frac) if frac > 0 else 0
            print(
                f"[{elapsed/60:6.1f} min] members {n_written:>6}  "
                f"rows {cp['rows'] + writer.file_rows + writer.pending_rows:>11,}  "
                f"tar {frac*100:5.1f}%  eta {eta/60:5.1f} min  bad {total_bad}",
                flush=True,
            )

    with ProcessPoolExecutor(max_workers=args.workers) as pool:
        for member in tf:
            if not is_data_member(member):
                continue
            n_seen += 1
            if n_seen <= skip:
                continue
            if args.max_members is not None and n_submitted >= args.max_members:
                break
            data = tf.extractfile(member).read()
            inflight.append(pool.submit(parse_member, member.name, data))
            n_submitted += 1
            if len(inflight) >= max_inflight:
                drain_one()
        while inflight:
            drain_one()

    writer.finalize()
    tf.close()
    raw.close()

    elapsed = time.time() - t0
    print(
        f"DONE in {elapsed/60:.1f} min: {cp['rows']:,} rows total, "
        f"{cp['files']} parquet files, {total_bad} bad lines",
        flush=True,
    )
    if first_error:
        print(f"first bad line: {first_error}", flush=True)


if __name__ == "__main__":
    main()
