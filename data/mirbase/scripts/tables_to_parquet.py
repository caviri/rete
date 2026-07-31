#!/usr/bin/env python3
"""miRBase relational dump (database_files/*.txt) -> one Parquet per table.

Column names and order come from the shipped `tables.sql` DDL, so the Parquet
schema tracks miRBase's own schema instead of a hand-copied guess. MySQL's
outfile NULL sentinel (\\N) becomes a real null.

Integer-ish columns are typed from the DDL; everything else stays a string
(sequences, blobs and free text all round-trip safely that way).

    bash data/mirbase/scripts/py.sh tables_to_parquet.py
"""
from __future__ import annotations

import re
from pathlib import Path

import pyarrow as pa
import pyarrow.parquet as pq

BASE = Path(__file__).resolve().parent.parent
DB = BASE / "raw" / "database_files"
OUT = BASE / "parquet"

NULL = "\\N"


def parse_ddl() -> dict[str, list[tuple[str, str]]]:
    """table -> [(column, sql_type), ...] in declaration order."""
    sql = (DB / "tables.sql").read_text(encoding="utf-8", errors="replace")
    out: dict[str, list[tuple[str, str]]] = {}
    for m in re.finditer(r"CREATE TABLE `(\w+)` \((.*?)\n\) ENGINE=", sql, re.S):
        cols = []
        for line in m.group(2).split("\n"):
            line = line.strip()
            if re.match(r"(PRIMARY|UNIQUE|KEY|FULLTEXT|CONSTRAINT|INDEX)\b", line, re.I):
                continue
            cm = re.match(r"`([^`]+)`\s+(\w+)", line)
            if cm:
                cols.append((cm.group(1), cm.group(2).lower()))
        out[m.group(1)] = cols
    return out


def arrow_type(sql_type: str) -> pa.DataType:
    if sql_type in ("int", "bigint", "smallint", "tinyint", "mediumint"):
        return pa.int64()
    if sql_type in ("float", "double", "decimal"):
        return pa.float64()
    return pa.string()


def main() -> None:
    OUT.mkdir(parents=True, exist_ok=True)
    ddl = parse_ddl()
    total = 0

    for table, cols in sorted(ddl.items()):
        src = DB / f"{table}.txt"
        if not src.exists():
            print(f"  !! {table}.txt missing")
            continue

        names = [c for c, _ in cols]
        schema = pa.schema([(c, arrow_type(t)) for c, t in cols])
        columns: list[list] = [[] for _ in names]

        with src.open(encoding="utf-8", errors="replace") as fh:
            for line in fh:
                f = line.rstrip("\n").split("\t")
                # pad/truncate defensively: a couple of rows in
                # literature_references carry a stray field
                f = (f + [NULL] * len(names))[:len(names)]
                for i, (raw, (_, sqlt)) in enumerate(zip(f, cols)):
                    if raw == NULL:
                        columns[i].append(None)
                        continue
                    at = arrow_type(sqlt)
                    if at == pa.int64():
                        try:
                            columns[i].append(int(raw))
                        except ValueError:
                            columns[i].append(None)
                    elif at == pa.float64():
                        try:
                            columns[i].append(float(raw))
                        except ValueError:
                            columns[i].append(None)
                    else:
                        columns[i].append(raw)

        tbl = pa.Table.from_arrays(
            [pa.array(c, type=schema.field(i).type) for i, c in enumerate(columns)],
            schema=schema,
        )
        dst = OUT / f"db_{table}.parquet"
        pq.write_table(tbl, dst, compression="zstd")
        total += tbl.num_rows
        print(f"  db_{table:<30} {tbl.num_rows:>8,} rows × {len(names):>2} cols "
              f"({dst.stat().st_size:,} bytes)")

    print(f"ok  {len(ddl)} tables, {total:,} rows -> {OUT}")


if __name__ == "__main__":
    main()
