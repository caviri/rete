#!/usr/bin/env python3
"""Denormalize the Wikidata truthy-dump triples into **columnar property
tables** — one Parquet file per entity type — as a companion to the `.rete`
graph, for comparing storage/query technologies.

The classic RDF "property table" / class-partitioned layout: group entities by
their type (`wdt:P31`, instance-of), and for each class emit a wide table whose
rows are the entities and whose columns are that class's most common
properties. Multi-valued properties become `LIST(VARCHAR)` columns; an English
`label` column is added; the raw object strings are kept (IRIs and literals)
so it round-trips the graph. Output: `<class>.parquet` per class plus a
`_manifest.parquet` (class IRI, label, entity/column counts, file, and the
P-id → label map so the columns are interpretable).

Everything runs in DuckDB straight from the source Parquet (httpfs streams it),
matching the `.rete` slice when given the same `--parts`/`--limit`.

Requires:  pip install --break-system-packages duckdb

Usage:
  uv run python scripts/rdf_to_property_tables.py --parts 10 --limit 120000000 -o data/wd-tables
  uv run python scripts/rdf_to_property_tables.py --local-dir /data/triplets --classes 50 --props 25
  # then, to assemble a single DuckDB over the tables:
  #   duckdb wd.duckdb "CREATE VIEW human AS SELECT * FROM 'data/wd-tables/Q5.parquet'; ..."
"""

from __future__ import annotations

import argparse
import os
import re
import sys

HF_BASE = "https://huggingface.co/datasets/piebro/wikidata-extraction/resolve/main/triplets"
P31 = "http://www.wikidata.org/prop/direct/P31"  # instance of -> the entity's class
RDFS_LABEL = "http://www.w3.org/2000/01/rdf-schema#label"

# Multilingual labelling/description predicates: captured by the dedicated
# `label` column, and otherwise they dominate every class's top-N properties
# and crowd out the structured ones (occupation, citizenship, …). Excluded
# from the column selection so the tables are about the entities' real
# properties.
LABEL_PREDS = (
    "http://www.w3.org/2000/01/rdf-schema#label",
    "http://www.w3.org/2004/02/skos/core#prefLabel",
    "http://www.w3.org/2004/02/skos/core#altLabel",
    "http://schema.org/name",
    "http://schema.org/description",
)


def localname(iri: str) -> str:
    """Last path/fragment segment of an IRI, for column/file naming (P569, Q5)."""
    m = re.search(r"[/#]([^/#]+)/?$", iri.rstrip("/"))
    return m.group(1) if m else iri


def col_ident(pid: str) -> str:
    """A safe SQL/Parquet column identifier from a predicate id."""
    return re.sub(r"[^A-Za-z0-9_]", "_", localname(pid))


def main() -> None:
    ap = argparse.ArgumentParser(
        description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter
    )
    ap.add_argument("--parts", type=int, default=2, help="number of ~900 MB source partitions")
    ap.add_argument("--limit", type=int, default=None, help="cap on source triples (match the .rete slice)")
    ap.add_argument("--local-dir", default=None, help="read part_*.parquet locally instead of from HF")
    ap.add_argument("--classes", type=int, default=40, help="emit tables for the top-N classes by entity count")
    ap.add_argument("--props", type=int, default=24, help="columns per table = top-N properties of the class")
    ap.add_argument("-o", "--output", default="data/wd-tables", help="output directory for the per-class Parquet")
    args = ap.parse_args()

    try:
        import duckdb
    except ModuleNotFoundError:
        sys.exit("duckdb not installed — run: pip install --break-system-packages duckdb")

    os.makedirs(args.output, exist_ok=True)
    con = duckdb.connect()
    con.execute("PRAGMA threads=8")

    if args.local_dir:
        srcs = [os.path.join(args.local_dir, f"part_{i:04d}.parquet") for i in range(args.parts)]
        srcs = [s for s in srcs if os.path.exists(s)]
    else:
        con.execute("INSTALL httpfs; LOAD httpfs;")
        srcs = [f"{HF_BASE}/part_{i:04d}.parquet" for i in range(args.parts)]
    src_list = ", ".join(f"'{s}'" for s in srcs)
    limit_sql = f" LIMIT {args.limit}" if args.limit else ""

    # 1. Materialize the triple slice once (DuckDB spills to disk as needed).
    print(f"materializing triple slice from {len(srcs)} partition(s)…", file=sys.stderr)
    con.execute(
        f"CREATE TABLE tr AS SELECT subject AS s, predicate AS p, object AS o, language AS lang "
        f"FROM read_parquet([{src_list}]){limit_sql}"
    )
    n = con.execute("SELECT COUNT(*) FROM tr").fetchone()[0]
    print(f"  {n:,} triples", file=sys.stderr)

    # 2. Class membership (an entity sits in every class it is an instance of)
    #    and an English-label lookup.
    con.execute(f"CREATE TABLE ec AS SELECT s, o AS class FROM tr WHERE p = '{P31}'")
    con.execute(
        f"CREATE TABLE lbl AS SELECT s, any_value(o) AS label FROM tr "
        f"WHERE p = '{RDFS_LABEL}' AND lang = 'en' GROUP BY s"
    )
    con.execute("CREATE INDEX ec_s ON ec(s)")

    # 3. Top classes by distinct entity count.
    classes = con.execute(
        f"SELECT class, COUNT(DISTINCT s) AS n FROM ec GROUP BY class ORDER BY n DESC LIMIT {args.classes}"
    ).fetchall()
    print(f"  {len(classes)} classes selected", file=sys.stderr)

    manifest = []
    for cls, ecount in classes:
        # The class's most common properties (excluding P31 and the label/
        # description predicates) become columns.
        excluded = "', '".join((P31, *LABEL_PREDS))
        props = con.execute(
            f"SELECT tr.p, COUNT(*) c FROM tr JOIN ec ON tr.s = ec.s "
            f"WHERE ec.class = ? AND tr.p NOT IN ('{excluded}') "
            f"GROUP BY tr.p ORDER BY c DESC LIMIT ?",
            [cls, args.props],
        ).fetchall()
        # Dynamic wide projection: one LIST column per property.
        cols, used = [], set()
        for pid, _ in props:
            ident = col_ident(pid)
            while ident in used:
                ident += "_"
            used.add(ident)
            cols.append(
                f"list(tr.o) FILTER (WHERE tr.p = '{pid}') AS \"{ident}\""
            )
        proj = ",\n  ".join(cols) if cols else "NULL AS _empty"
        fname = col_ident(cls) + ".parquet"
        con.execute(
            f"COPY (SELECT tr.s AS entity, any_value(lbl.label) AS label,\n  {proj}\n"
            f"FROM tr JOIN ec ON tr.s = ec.s LEFT JOIN lbl ON lbl.s = tr.s\n"
            f"WHERE ec.class = '{cls}' GROUP BY tr.s) "
            f"TO '{os.path.join(args.output, fname)}' (FORMAT parquet)"
        )
        clabel = con.execute("SELECT label FROM lbl WHERE s = ? LIMIT 1", [cls]).fetchone()
        manifest.append((cls, clabel[0] if clabel else None, ecount, len(props), fname,
                         {col_ident(p): p for p, _ in props}))
        print(f"  {localname(cls):>10} {clabel[0] if clabel else '':<28} {ecount:>9,} entities, {len(props)} cols -> {fname}",
              file=sys.stderr)

    # 4. Manifest table (class IRI, label, counts, file, column->predicate map).
    con.execute(
        "CREATE TABLE manifest (class VARCHAR, label VARCHAR, entities BIGINT, "
        "columns INTEGER, file VARCHAR, column_map JSON)"
    )
    import json
    for cls, lab, ecount, ncols, fname, cmap in manifest:
        con.execute("INSERT INTO manifest VALUES (?, ?, ?, ?, ?, ?)",
                    [cls, lab, ecount, ncols, fname, json.dumps(cmap)])
    con.execute(f"COPY manifest TO '{os.path.join(args.output, '_manifest.parquet')}' (FORMAT parquet)")

    total = sum(os.path.getsize(os.path.join(args.output, f)) for f in os.listdir(args.output))
    print(f"\nwrote {len(manifest)} class tables + _manifest.parquet to {args.output} "
          f"({total / (1024 * 1024):.0f} MB)", file=sys.stderr)


if __name__ == "__main__":
    main()
