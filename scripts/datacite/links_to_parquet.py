"""Stream a DataCite PID Links Data File tar.gz into a Parquet edge table —
without extracting the archive.

Each input line is one PID Graph relationship event:
  subj --relation_type_id--> obj, with provenance (source_id), schema.org
  types, publication dates on both endpoints, and event timestamps.

Output: one row per event. Endpoint ids are stored as bare DOIs when they
are doi.org URLs (full URL otherwise). The derivable `prefix` and `doi`
pair-arrays are dropped; any other unknown key lands in extra_json, and
endpoint fields beyond id/@type/date_published land in subj/obj_extra_json.

Same streaming/resume machinery as tar_to_parquet.py.

Usage:
  python links_to_parquet.py --tar data/datacite/PID_Links_Data_File_2023.tar.gz \
      --out data/datacite/parquet-links-2023
"""

import argparse
import gzip
import json
import os
import time
import tarfile
from collections import deque
from concurrent.futures import ProcessPoolExecutor

import orjson
import pyarrow as pa
import pyarrow.parquet as pq

KNOWN_KEYS = {
    "subj", "obj", "relation_type_id", "source_id", "citation_type",
    "occurred_at", "created_at", "updated_at", "uuid", "prefix", "doi",
}
ENDPOINT_KEYS = {"id", "@type", "date_published"}

SCHEMA = pa.schema(
    [
        ("subj_id", pa.string()),
        ("obj_id", pa.string()),
        ("relation_type", pa.string()),
        ("source_id", pa.string()),
        ("citation_type", pa.string()),
        ("subj_type", pa.string()),
        ("obj_type", pa.string()),
        ("subj_published", pa.string()),
        ("obj_published", pa.string()),
        ("subj_year", pa.int32()),
        ("obj_year", pa.int32()),
        ("occurred_at", pa.string()),
        ("created_at", pa.string()),
        ("updated_at", pa.string()),
        ("uuid", pa.string()),
        ("subj_extra_json", pa.string()),
        ("obj_extra_json", pa.string()),
        ("extra_json", pa.string()),
    ]
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
    return values


def build_batch(cols):
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


def _pid(s):
    """doi.org URL -> bare DOI; anything else unchanged."""
    if isinstance(s, str):
        if s.startswith("https://doi.org/"):
            return s[16:]
        if s.startswith("http://doi.org/"):
            return s[15:]
    return s


def _year(published):
    if isinstance(published, str) and len(published) >= 4 and published[:4].isdigit():
        y = int(published[:4])
        return y if y > 0 else None
    return None


def _endpoint(node):
    """-> (id, type, published, year, extra_json)"""
    if not isinstance(node, dict):
        return _str(node), None, None, None, None
    rest = {k: v for k, v in node.items() if k not in ENDPOINT_KEYS and v not in (None, [], {})}
    published = node.get("date_published")
    return (
        _pid(node.get("id")),
        node.get("@type"),
        published,
        _year(published),
        _dumps(rest) if rest else None,
    )


def parse_member(name, data):
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
            s_id, s_type, s_pub, s_year, s_extra = _endpoint(rec.get("subj"))
            o_id, o_type, o_pub, o_year, o_extra = _endpoint(rec.get("obj"))
            extra = {k: v for k, v in rec.items() if k not in KNOWN_KEYS}
            cols["subj_id"].append(s_id)
            cols["obj_id"].append(o_id)
            cols["relation_type"].append(rec.get("relation_type_id"))
            cols["source_id"].append(rec.get("source_id"))
            cols["citation_type"].append(rec.get("citation_type"))
            cols["subj_type"].append(s_type)
            cols["obj_type"].append(o_type)
            cols["subj_published"].append(s_pub)
            cols["obj_published"].append(o_pub)
            cols["subj_year"].append(s_year)
            cols["obj_year"].append(o_year)
            cols["occurred_at"].append(rec.get("occurred_at"))
            cols["created_at"].append(rec.get("created_at"))
            cols["updated_at"].append(rec.get("updated_at"))
            cols["uuid"].append(rec.get("uuid"))
            cols["subj_extra_json"].append(s_extra)
            cols["obj_extra_json"].append(o_extra)
            cols["extra_json"].append(_dumps(extra) if extra else None)
        except Exception as e:  # noqa: BLE001
            n_bad += 1
            if first_error is None:
                first_error = f"{name}: {e!r} :: {line[:200]!r}"
    return build_batch(cols), n_bad, first_error


def is_data_member(m):
    return m.isfile() and (m.name.endswith(".jsonl") or m.name.endswith(".jsonl.gz"))


class RollingWriter:
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
    ap.add_argument("--tar", required=True)
    ap.add_argument("--out", required=True)
    ap.add_argument("--workers", type=int, default=min(20, max(4, os.cpu_count() - 4)))
    ap.add_argument("--rows-per-file", type=int, default=5_000_000)
    ap.add_argument("--chunk-rows", type=int, default=500_000)
    ap.add_argument("--max-members", type=int, default=None)
    ap.add_argument("--fresh", action="store_true")
    args = ap.parse_args()

    os.makedirs(args.out, exist_ok=True)
    cp = load_checkpoint(args.out, args.fresh)
    skip = cp["members_done"]
    if skip:
        print(f"resuming: skipping {skip} already-converted members "
              f"({cp['rows']} rows in {cp['files']} closed files)", flush=True)

    tar_size = os.path.getsize(args.tar)
    raw = open(args.tar, "rb")
    tf = tarfile.open(fileobj=raw, mode="r|*")

    writer = RollingWriter(args.out, args.rows_per_file, args.chunk_rows, cp)
    total_bad = 0
    first_error = None
    n_seen = 0
    n_submitted = 0
    n_written = skip
    t0 = time.time()
    inflight = deque()
    max_inflight = args.workers * 2  # members are ~50 MB, keep buffer modest

    def drain_one():
        nonlocal total_bad, first_error, n_written
        batch, n_bad, err = inflight.popleft().result()
        total_bad += n_bad
        if err and first_error is None:
            first_error = err
        writer.add_member_batch(batch)
        n_written += 1
        if n_written % 100 == 0:
            frac = raw.tell() / tar_size
            elapsed = time.time() - t0
            eta = elapsed / frac * (1 - frac) if frac > 0 else 0
            print(
                f"[{elapsed/60:6.1f} min] members {n_written:>6}  "
                f"rows {cp['rows'] + writer.file_rows + writer.pending_rows:>12,}  "
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
