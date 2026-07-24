"""Convert the EPFL GraphOntology MySQL dump (graph_ontology_2025-06-26.tar.gz,
Zenodo 10.5281/zenodo.20306788, Apache-2.0) to Parquet — streaming, no extraction.

Archive layout: one folder per table, each holding CREATE_TABLE_NO_KEYS.sql
plus thousands of chunked mysqldump files (extended INSERTs, ~10k rows each):

  graph_ontology/<Table>/CREATE_TABLE_NO_KEYS.sql
  graph_ontology/<Table>/<Table>_0000010000.sql
  ...

Two passes over the tar stream:
  1. schema pass  — collect every CREATE_TABLE_NO_KEYS.sql, map MySQL types
                    to Arrow (ints -> int64, float/double/decimal -> float64,
                    rest -> string); cached in <out>/_schemas.json
  2. convert pass — workers parse INSERT tuples (regex tokenizer + MySQL
                    unescape), main routes batches to per-table rolling
                    zstd Parquet writers: <out>/<Table>/part-*.parquet

No checkpoint/resume: a full run is ~15-30 min; rerun on failure.

Usage:
  python scripts/epfl-graph/sql_to_parquet.py
  python scripts/epfl-graph/sql_to_parquet.py --max-units 200   # test slice
"""

import argparse
import json
import os
import re
import tarfile
import time
from collections import deque
from concurrent.futures import ProcessPoolExecutor

import pyarrow as pa
import pyarrow.parquet as pq

TAR = r"D:\pro\rete\data\epfl-graph\graph_ontology_2025-06-26.tar.gz"
OUT = r"D:\pro\rete\data\epfl-graph\parquet"

INT_TYPES = {"tinyint", "smallint", "mediumint", "int", "integer", "bigint"}
FLOAT_TYPES = {"float", "double", "decimal", "numeric", "real"}

COL_RE = re.compile(r"^\s*`([^`]+)`\s+([a-zA-Z]+)", re.M)
UNESCAPE_MAP = {"0": "\0", "'": "'", '"': '"', "b": "\b", "n": "\n",
                "r": "\r", "t": "\t", "Z": "\x1a", "\\": "\\", "%": "\\%",
                "_": "\\_"}


def parse_values_tuples(s, start):
    """Deterministic MySQL VALUES parser. Walks char-by-char from `start`,
    yielding one list of Python values per (...) tuple. Correct for arbitrarily
    long fields, backslash escapes, '' doubling, and parens/commas inside
    strings — none of which a regex tokenizer handles reliably at scale.
    Values are str, None (NULL), or numeric literals kept as str (cast later)."""
    i, n = start, len(s)
    tuples = []
    while i < n:
        while i < n and s[i] != "(":
            i += 1
        if i >= n:
            break
        i += 1
        row = []
        while i < n:
            c = s[i]
            if c in " \t\r\n":
                i += 1
            elif c == ")":
                i += 1
                break
            elif c == ",":
                i += 1
            elif c == "'":
                buf = []
                i += 1
                while i < n:
                    ch = s[i]
                    if ch == "\\" and i + 1 < n:
                        buf.append(UNESCAPE_MAP.get(s[i + 1], s[i + 1]))
                        i += 2
                    elif ch == "'":
                        if i + 1 < n and s[i + 1] == "'":  # '' -> literal '
                            buf.append("'")
                            i += 2
                        else:
                            i += 1
                            break
                    else:
                        buf.append(ch)
                        i += 1
                row.append("".join(buf))
            else:  # bare literal: number, NULL, etc. up to , or )
                j = i
                while j < n and s[j] not in ",)":
                    j += 1
                lit = s[i:j].strip()
                row.append(None if lit == "NULL" else lit)
                i = j
        tuples.append(row)
    return tuples


def parse_create_table(sql_text):
    """-> list of (column_name, arrow_type_name)"""
    cols = []
    body = sql_text[sql_text.find("("):]
    for name, typ in COL_RE.findall(body):
        t = typ.lower()
        if t in INT_TYPES:
            at = "int64"
        elif t in FLOAT_TYPES:
            at = "float64"
        else:
            at = "string"
        cols.append((name, at))
    return cols


def arrow_schema(cols):
    m = {"int64": pa.int64(), "float64": pa.float64(), "string": pa.string()}
    return pa.schema([(n, m[t]) for n, t in cols])


def parse_insert_chunk(table, cols, data):
    """Worker: one mysqldump chunk -> (RecordBatch-ready dict, n_rows, n_bad, err)."""
    names = [n for n, _ in cols]
    types = [t for _, t in cols]
    ncols = len(names)
    out = {n: [] for n in names}
    n_bad = 0
    first_error = None
    text = data.decode("utf-8", "replace")
    # split ONLY on the mysqldump line terminator \n — NOT str.splitlines(),
    # which also breaks on NEL/U+2028/vertical-tab etc. that occur INSIDE the
    # multilingual Wikipedia text fields and would fragment INSERT statements.
    for line in text.split("\n"):
        if not line.startswith("INSERT INTO"):
            continue
        vi = line.find(" VALUES ")
        if vi < 0:
            continue
        for row in parse_values_tuples(line, vi + 8):
            if len(row) == ncols:
                for i, v in enumerate(row):
                    out[names[i]].append(v)
            else:
                n_bad += 1
                if first_error is None:
                    first_error = (f"{table}: row width {len(row)} != {ncols}"
                                   f" :: {line[vi:vi+160]!r}")
    # cast to arrow arrays
    arrays = []
    schema = arrow_schema(cols)
    for i, n in enumerate(names):
        vals = out[n]
        t = types[i]
        try:
            if t == "int64":
                vals = [int(v) if v is not None else None for v in vals]
            elif t == "float64":
                vals = [float(v) if v is not None else None for v in vals]
            arrays.append(pa.array(vals, type=schema.field(i).type))
        except (ValueError, TypeError, pa.ArrowInvalid):
            fixed = []
            for v in vals:
                try:
                    fixed.append((int(v) if t == "int64" else float(v))
                                 if v is not None else None)
                except (ValueError, TypeError):
                    fixed.append(None)
            arrays.append(pa.array(fixed, type=schema.field(i).type))
    batch = pa.RecordBatch.from_arrays(arrays, schema=schema)
    return batch, batch.num_rows, n_bad, first_error


def iter_sql_members(tar_path):
    with tarfile.open(tar_path, mode="r|*") as tf:
        for m in tf:
            bn = os.path.basename(m.name)
            if not m.isfile() or bn.startswith("._") or not bn.endswith(".sql"):
                continue
            table = os.path.basename(os.path.dirname(m.name))
            yield table, bn, m, tf


class TableWriter:
    def __init__(self, out_dir, schema, rows_per_file=2_000_000, chunk_rows=200_000):
        self.out_dir = out_dir
        self.schema = schema
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
            path = os.path.join(self.out_dir, f"part-{self.file_index:05d}.parquet")
            self.writer = pq.ParquetWriter(path, self.schema, compression="zstd",
                                           compression_level=3)
        table = pa.Table.from_batches(self.pending, schema=self.schema)
        self.writer.write_table(table, row_group_size=self.chunk_rows)
        self.file_rows += table.num_rows
        self.pending = []
        self.pending_rows = 0
        if self.file_rows >= self.rows_per_file:
            self.writer.close()
            self.writer = None
            self.file_index += 1
            self.file_rows = 0

    def add(self, batch):
        self.pending.append(batch)
        self.pending_rows += batch.num_rows
        self.total_rows += batch.num_rows
        if self.pending_rows >= self.chunk_rows:
            self._flush()

    def close(self):
        self._flush()
        if self.writer is not None:
            self.writer.close()
            self.writer = None


def collect_schemas(tar_path, out_base):
    cache = os.path.join(out_base, "_schemas.json")
    if os.path.exists(cache):
        with open(cache, encoding="utf-8") as f:
            return json.load(f)
    print("schema pass: scanning for CREATE_TABLE_NO_KEYS.sql ...", flush=True)
    schemas = {}
    t0 = time.time()
    for table, bn, m, tf in iter_sql_members(tar_path):
        if bn == "CREATE_TABLE_NO_KEYS.sql":
            sql = tf.extractfile(m).read().decode("utf-8", "replace")
            schemas[table] = parse_create_table(sql)
            print(f"  {table}: {len(schemas[table])} cols", flush=True)
    os.makedirs(out_base, exist_ok=True)
    with open(cache, "w", encoding="utf-8") as f:
        json.dump(schemas, f, indent=1)
    print(f"schema pass done in {(time.time()-t0)/60:.1f} min: "
          f"{len(schemas)} tables", flush=True)
    return schemas


def main():
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("--tar", default=TAR)
    ap.add_argument("--out", default=OUT)
    ap.add_argument("--workers", type=int, default=min(14, max(4, os.cpu_count() - 4)))
    ap.add_argument("--max-units", type=int, default=None)
    args = ap.parse_args()

    schemas = collect_schemas(args.tar, args.out)
    writers = {}
    totals_bad = 0
    first_error = None
    n_units = 0
    t0 = time.time()
    inflight = deque()

    def drain_one():
        nonlocal totals_bad, first_error
        table, fut = inflight.popleft()
        batch, n_rows, n_bad, err = fut.result()
        totals_bad += n_bad
        if err and first_error is None:
            first_error = err
        writers[table].add(batch)

    with ProcessPoolExecutor(max_workers=args.workers) as pool:
        for table, bn, m, tf in iter_sql_members(args.tar):
            if bn == "CREATE_TABLE_NO_KEYS.sql" or table not in schemas:
                continue
            if args.max_units is not None and n_units >= args.max_units:
                break
            if table not in writers:
                writers[table] = TableWriter(os.path.join(args.out, table),
                                             arrow_schema(schemas[table]))
            data = tf.extractfile(m).read()
            inflight.append((table, pool.submit(
                parse_insert_chunk, table, schemas[table], data)))
            n_units += 1
            if len(inflight) >= args.workers * 2:
                drain_one()
            if n_units % 2000 == 0:
                rows = sum(w.total_rows for w in writers.values())
                print(f"[{(time.time()-t0)/60:6.1f} min] units {n_units:>6}  "
                      f"rows {rows:>12,}  bad {totals_bad}", flush=True)
        while inflight:
            drain_one()

    summary = {}
    for table, w in sorted(writers.items()):
        w.close()
        summary[table] = w.total_rows
        print(f"  {table}: {w.total_rows:,} rows", flush=True)
    with open(os.path.join(args.out, "_tables.json"), "w", encoding="utf-8") as f:
        json.dump(summary, f, indent=1)
    print(f"DONE in {(time.time()-t0)/60:.1f} min: "
          f"{sum(summary.values()):,} rows across {len(summary)} tables, "
          f"{totals_bad} bad rows", flush=True)
    if first_error:
        print("first bad:", first_error, flush=True)


if __name__ == "__main__":
    main()
