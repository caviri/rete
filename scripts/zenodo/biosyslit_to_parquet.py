"""Stream the Zenodo `biosyslit/records-json.tar.gz` exporter dump into Parquet
— without ever extracting the archive.

This is the Biodiversity Literature Repository (BLR) community slice, exported
as **Zenodo-native REST record JSON** (one `<recid>.json` per record) — richer
than the site-wide DataCite XML: it carries file listings, IIIF manifest links,
usage stats, and Darwin Core `custom_fields` (dwc:family/genus/kingdom/…).

We flatten the analytically useful scalars to typed columns and keep every
nested block whole as a JSON-string column (NULL when empty). `doi` matches the
site-wide `parquet-metadata` and the DataCite tables, so BLR records join the
rest of the graph; `record_id` is the Zenodo recid.

Members are read sequentially from the compressed tar and parsed in batches by
a process pool, then written as rolling zstd Parquet files.

Usage:
  python biosyslit_to_parquet.py                       # full run
  python biosyslit_to_parquet.py --max-records 3000 --out /tmp/btest --fresh
"""

import argparse
import json
import os
import tarfile
import time
from collections import deque
from concurrent.futures import ProcessPoolExecutor

import orjson
import pyarrow as pa
import pyarrow.parquet as pq

DEFAULT_TAR = r"D:\pro\rete\data\zenodo\biosyslit-records-json-2026-03-27.tar.gz"
DEFAULT_OUT = r"D:\pro\rete\data\zenodo\parquet-biosyslit"

SCHEMA = pa.schema(
    [
        ("doi", pa.string()),
        ("record_id", pa.string()),
        ("parent_id", pa.string()),
        ("parent_doi", pa.string()),
        ("created", pa.string()),
        ("updated", pa.string()),
        ("publication_date", pa.string()),
        ("publisher", pa.string()),
        ("resource_type_id", pa.string()),
        ("resource_type_title", pa.string()),   # English label
        ("title", pa.string()),
        ("is_published", pa.bool_()),
        ("access_status", pa.string()),
        ("communities", pa.string()),            # JSON array of slugs
        ("views", pa.int64()),
        ("unique_views", pa.int64()),
        ("downloads", pa.int64()),
        ("unique_downloads", pa.int64()),
        ("file_count", pa.int32()),
        ("total_bytes", pa.int64()),
        ("description", pa.string()),
        ("creators_json", pa.string()),
        ("subjects_json", pa.string()),
        ("identifiers_json", pa.string()),
        ("related_identifiers_json", pa.string()),
        ("rights_json", pa.string()),
        ("additional_descriptions_json", pa.string()),
        ("references_json", pa.string()),
        ("custom_fields_json", pa.string()),     # dwc:* taxonomy, journal:*
        ("files_json", pa.string()),
        ("pids_json", pa.string()),
        ("iiif_manifest", pa.string()),
        ("extra_json", pa.string()),
    ]
)
COLUMNS = [f.name for f in SCHEMA]

# metadata keys promoted to their own column (so extra_json stays small)
_MD_HANDLED = {
    "resource_type", "creators", "title", "publisher", "publication_date",
    "subjects", "identifiers", "related_identifiers", "rights", "description",
    "additional_descriptions", "references",
}


def _dumps(v):
    return orjson.dumps(v).decode() if v else None


def _bare_doi(url):
    if not url:
        return None
    for p in ("https://doi.org/", "http://doi.org/"):
        if url.startswith(p):
            return url[len(p):]
    return url


def parse_record(recid, rec):
    row = {c: None for c in COLUMNS}
    row["record_id"] = rec.get("id") or recid
    row["created"] = rec.get("created")
    row["updated"] = rec.get("updated")
    row["is_published"] = rec.get("is_published")

    pids = rec.get("pids") or {}
    row["doi"] = (pids.get("doi") or {}).get("identifier")
    if not row["doi"]:
        row["doi"] = _bare_doi((rec.get("links") or {}).get("self_doi"))
    row["pids_json"] = _dumps(pids)

    parent = rec.get("parent") or {}
    row["parent_id"] = parent.get("id")
    row["parent_doi"] = _bare_doi((rec.get("links") or {}).get("parent_doi"))
    # community slugs
    slugs = []
    for e in ((parent.get("communities") or {}).get("entries") or []):
        if e.get("slug"):
            slugs.append(e["slug"])
    row["communities"] = _dumps(slugs)

    row["access_status"] = (rec.get("access") or {}).get("status")
    row["iiif_manifest"] = (rec.get("links") or {}).get("self_iiif_manifest")

    stats = (rec.get("stats") or {}).get("all_versions") or {}
    row["views"] = stats.get("views")
    row["unique_views"] = stats.get("unique_views")
    row["downloads"] = stats.get("downloads")
    row["unique_downloads"] = stats.get("unique_downloads")

    files = rec.get("files") or {}
    row["file_count"] = files.get("count")
    row["total_bytes"] = files.get("total_bytes")
    row["files_json"] = _dumps(files.get("entries"))

    row["custom_fields_json"] = _dumps(rec.get("custom_fields"))

    md = rec.get("metadata") or {}
    rt = md.get("resource_type") or {}
    row["resource_type_id"] = rt.get("id")
    title = rt.get("title")
    if isinstance(title, dict):
        row["resource_type_title"] = title.get("en") or next(iter(title.values()), None)
    elif isinstance(title, str):
        row["resource_type_title"] = title
    row["title"] = md.get("title")
    row["publisher"] = md.get("publisher")
    row["publication_date"] = md.get("publication_date")
    row["description"] = md.get("description")
    row["creators_json"] = _dumps(md.get("creators"))
    row["subjects_json"] = _dumps(md.get("subjects"))
    row["identifiers_json"] = _dumps(md.get("identifiers"))
    row["related_identifiers_json"] = _dumps(md.get("related_identifiers"))
    row["rights_json"] = _dumps(md.get("rights"))
    row["additional_descriptions_json"] = _dumps(md.get("additional_descriptions"))
    row["references_json"] = _dumps(md.get("references"))

    extra = {k: v for k, v in md.items() if k not in _MD_HANDLED}
    row["extra_json"] = _dumps(extra)
    return row


def parse_batch(items):
    cols = {c: [] for c in COLUMNS}
    n_bad = 0
    first_err = None
    for recid, data in items:
        try:
            rec = orjson.loads(data)
            row = parse_record(recid, rec)
            for c in COLUMNS:
                cols[c].append(row[c])
        except Exception as e:  # noqa: BLE001
            n_bad += 1
            if first_err is None:
                first_err = f"{recid}: {e!r}"
    batch = pa.RecordBatch.from_pydict(cols, schema=SCHEMA)
    return batch, len(items), n_bad, first_err


class RollingWriter:
    def __init__(self, out_dir, rows_per_file, chunk_rows, checkpoint):
        self.out_dir = out_dir
        self.rows_per_file = rows_per_file
        self.chunk_rows = chunk_rows
        self.cp = checkpoint
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
            self.writer = pq.ParquetWriter(path, SCHEMA, compression="zstd", compression_level=3)
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
            self.cp["files"] = self.file_index
            self.cp["members_done"] += self.members_in_file
            self.cp["rows"] += self.file_rows
            save_checkpoint(self.out_dir, self.cp)
        self.file_rows = 0
        self.members_in_file = 0

    def add_batch(self, batch, n_members):
        self.pending.append(batch)
        self.pending_rows += batch.num_rows
        self.members_in_file += n_members
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
    ap.add_argument("--batch-size", type=int, default=2000)
    ap.add_argument("--rows-per-file", type=int, default=500_000)
    ap.add_argument("--chunk-rows", type=int, default=131_072)
    ap.add_argument("--max-records", type=int, default=None)
    ap.add_argument("--fresh", action="store_true")
    args = ap.parse_args()

    os.makedirs(args.out, exist_ok=True)
    cp = load_checkpoint(args.out, args.fresh)
    skip = cp["members_done"]
    if skip:
        print(f"resuming: skipping {skip:,} records "
              f"({cp['rows']:,} rows in {cp['files']} closed files)", flush=True)

    tar_size = os.path.getsize(args.tar)
    raw = open(args.tar, "rb")
    tf = tarfile.open(fileobj=raw, mode="r|gz")

    writer = RollingWriter(args.out, args.rows_per_file, args.chunk_rows, cp)
    total_bad = 0
    first_err = None
    n_seen = 0
    n_submitted = 0
    n_written = skip
    t0 = time.time()
    inflight = deque()
    max_inflight = args.workers * 2
    batch = []

    def drain_one():
        nonlocal total_bad, first_err, n_written
        b, n_members, n_bad, err = inflight.popleft().result()
        total_bad += n_bad
        if err and first_err is None:
            first_err = err
        writer.add_batch(b, n_members)
        n_written += n_members
        frac = raw.tell() / tar_size
        elapsed = time.time() - t0
        eta = elapsed / frac * (1 - frac) if frac > 0 else 0
        print(
            f"[{elapsed/60:6.1f} min] records {n_written:>9,}  "
            f"rows {cp['rows'] + writer.file_rows + writer.pending_rows:>10,}  "
            f"tar {frac*100:5.1f}%  eta {eta/60:5.1f} min  bad {total_bad}",
            flush=True,
        )

    with ProcessPoolExecutor(max_workers=args.workers) as pool:
        for member in tf:
            if not (member.isfile() and member.name.endswith(".json")):
                continue
            n_seen += 1
            if n_seen <= skip:
                continue
            if args.max_records is not None and n_submitted >= args.max_records:
                break
            recid = os.path.splitext(os.path.basename(member.name))[0]
            batch.append((recid, tf.extractfile(member).read()))
            n_submitted += 1
            if len(batch) >= args.batch_size:
                inflight.append(pool.submit(parse_batch, batch))
                batch = []
                if len(inflight) >= max_inflight:
                    drain_one()
        if batch:
            inflight.append(pool.submit(parse_batch, batch))
        while inflight:
            drain_one()

    writer.finalize()
    tf.close()
    raw.close()

    elapsed = time.time() - t0
    print(
        f"DONE in {elapsed/60:.1f} min: {cp['rows']:,} rows total, "
        f"{cp['files']} parquet files, {total_bad} bad records",
        flush=True,
    )
    if first_err:
        print(f"first bad record: {first_err}", flush=True)


if __name__ == "__main__":
    main()
