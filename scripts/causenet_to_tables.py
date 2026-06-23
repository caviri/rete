#!/usr/bin/env python3
"""CauseNet JSONL -> clean relational companions (Parquet + DuckDB + SQLite).

The lossless `.rete` graph is the primary artifact; these are the columnar
"CauseNet in a database" companions a data scientist actually wants:

  relations(cause, effect, support)                         -- 11.6M causal edges
  sources(cause, effect, source_type, sentence, pattern,    -- ~55M provenance rows
          + every Wikipedia/ClueWeb12 payload field)
  concepts(concept, out_degree, in_degree, degree)          -- ~12M concepts

DuckDB streams the (decompressed) JSONL with read_json, unnesting the per-source
provenance array; the payload struct is the union of all source-type fields
(missing ones NULL), so the flat `sources` table keeps every field losslessly.

Usage (Docker):
  python scripts/causenet_to_tables.py /scratch/causenet-full.jsonl /scratch/companions
"""

from __future__ import annotations

import os
import sys

import duckdb

# payload key -> output column (only these are projected; all are nullable).
SRC_FIELDS = [
    "sentence",
    "path_pattern",
    "wikipedia_page_id",
    "wikipedia_page_title",
    "wikipedia_revision_id",
    "wikipedia_revision_timestamp",
    "sentence_section_heading",
    "sentence_section_level",
    "clueweb12_page_id",
    "clueweb12_page_reference",
    "clueweb12_page_timestamp",
    "infobox_template",
    "infobox_title",
    "infobox_argument",
    "list_toc_parent_title",
    "list_toc_section_heading",
    "list_toc_section_level",
]


def main() -> None:
    inp, outdir = sys.argv[1], sys.argv[2]
    os.makedirs(outdir, exist_ok=True)
    con = duckdb.connect()
    con.execute("PRAGMA threads=8")
    con.execute(f"SET temp_directory='{outdir}/duck_tmp'")
    con.execute("SET preserve_insertion_order=false")

    read = (
        f"read_json('{inp}', format='newline_delimited', records=true, "
        f"maximum_object_size=33554432)"
    )

    print("relations.parquet …", file=sys.stderr)
    con.execute(
        f"""
        COPY (
          SELECT causal_relation.cause.concept  AS cause,
                 causal_relation.effect.concept AS effect,
                 length(sources)                AS support
          FROM {read}
        ) TO '{outdir}/relations.parquet' (FORMAT parquet, COMPRESSION zstd)
        """
    )

    print("concepts.parquet …", file=sys.stderr)
    con.execute(
        f"""
        COPY (
          WITH e AS (
            SELECT causal_relation.cause.concept  AS cause,
                   causal_relation.effect.concept AS effect
            FROM {read}
          ),
          o AS (SELECT cause AS c, COUNT(*) n FROM e GROUP BY cause),
          i AS (SELECT effect AS c, COUNT(*) n FROM e GROUP BY effect)
          SELECT coalesce(o.c, i.c)            AS concept,
                 coalesce(o.n, 0)              AS out_degree,
                 coalesce(i.n, 0)              AS in_degree,
                 coalesce(o.n, 0)+coalesce(i.n, 0) AS degree
          FROM o FULL OUTER JOIN i ON o.c = i.c
        ) TO '{outdir}/concepts.parquet' (FORMAT parquet, COMPRESSION zstd)
        """
    )

    sel = ",\n                 ".join(
        f"s.payload.{k} AS {('pattern' if k == 'path_pattern' else k)}"
        for k in SRC_FIELDS
    )
    print("sources.parquet …", file=sys.stderr)
    con.execute(
        f"""
        COPY (
          SELECT causal_relation.cause.concept  AS cause,
                 causal_relation.effect.concept AS effect,
                 s.type                         AS source_type,
                 {sel}
          FROM {read}, unnest(sources) AS t(s)
        ) TO '{outdir}/sources.parquet' (FORMAT parquet, COMPRESSION zstd)
        """
    )

    # Assemble a DuckDB file (native tables) and a SQLite file (same tables).
    for name in ("relations", "concepts", "sources"):
        n = con.execute(
            f"SELECT COUNT(*) FROM '{outdir}/{name}.parquet'"
        ).fetchone()[0]
        print(f"  {name}: {n:,} rows", file=sys.stderr)

    ddb = f"{outdir}/causenet-full.duckdb"
    if os.path.exists(ddb):
        os.remove(ddb)
    db = duckdb.connect(ddb)
    for name in ("relations", "concepts", "sources"):
        db.execute(
            f"CREATE TABLE {name} AS SELECT * FROM '{outdir}/{name}.parquet'"
        )
    db.execute("CREATE INDEX rel_cause ON relations(cause)")
    db.execute("CREATE INDEX rel_effect ON relations(effect)")
    db.execute("CREATE INDEX src_ce ON sources(cause, effect)")
    db.close()
    print(f"  duckdb -> {ddb}", file=sys.stderr)

    sq = f"{outdir}/causenet-full.sqlite"
    if os.path.exists(sq):
        os.remove(sq)
    con.execute("INSTALL sqlite; LOAD sqlite;")
    con.execute(f"ATTACH '{sq}' AS sq (TYPE sqlite)")
    for name in ("relations", "concepts", "sources"):
        con.execute(
            f"CREATE TABLE sq.{name} AS SELECT * FROM '{outdir}/{name}.parquet'"
        )
    con.execute("DETACH sq")
    # Indexes via sqlite3 (DuckDB's sqlite ATTACH can't name schema-qualified
    # indexes). A separate connection on the finished file.
    import sqlite3

    s = sqlite3.connect(sq)
    s.execute("CREATE INDEX rel_cause ON relations(cause)")
    s.execute("CREATE INDEX rel_effect ON relations(effect)")
    s.execute("CREATE INDEX src_ce ON sources(cause, effect)")
    s.commit()
    s.close()
    print(f"  sqlite -> {sq}", file=sys.stderr)

    total = sum(
        os.path.getsize(os.path.join(outdir, f))
        for f in os.listdir(outdir)
        if os.path.isfile(os.path.join(outdir, f))
    )
    print(f"DONE companions ({total/1e9:.2f} GB) -> {outdir}", file=sys.stderr)


if __name__ == "__main__":
    main()
