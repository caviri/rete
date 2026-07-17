#!/usr/bin/env python3
"""Extract the FULL Wikidata ontology — classes, no instances — from the
piebro/wikidata-extraction truthy Parquet into N-Triples for `rete build`.

Wikidata publishes no class-ontology file (wikiba.se/ontology is only the data
model); its ontology lives in the data as `wdt:P31` (instance of) and
`wdt:P279` (subclass of). This derives it in two passes over the local
partitions (download them first — see scripts/wikidata_parquet_to_nt.py or
`huggingface_hub.snapshot_download`):

* Pass A: the class set = subjects∪objects of P279 ∪ objects of P31, plus a
  per-class direct-instance count. The set + counts + the raw P279 edge list
  are also saved as small Parquet side-tables (`classes.parquet`,
  `class_counts.parquet`, `p279.parquet`) — reusable for planning the
  class-based instance shards later.
* Pass B: for every class (and every property entity `wd:P…`), emit its full
  truthy star — labels/aliases/descriptions in ALL languages, P279/P31 and any
  other wdt: statements. `wdt:P279` is dual-emitted as `rdfs:subClassOf` and
  `wdt:P31` as `rdf:type`, so standard tooling (schema pyramid, OWL-style
  reasoning, generic SPARQL) works alongside Wikidata-native queries.
* Finally each class gets `<https://w3id.org/rete/instanceCount> "n"^^xsd:integer`
  — its direct P31 instance count in the full dump.

Usage:
  python scripts/wikidata_ontology_to_nt.py            # all defaults
  python scripts/wikidata_ontology_to_nt.py --local-dir data/wikidata-parquet/triplets \
      -o data/wikidata-ontology/ontology.nt
Then:
  rete build data/wikidata-ontology/ontology.nt -o web/wikidata-ontology.rete \
      --pyramid-algo types --card
"""

from __future__ import annotations

import argparse
import glob
import os
import sys
import time

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from wikidata_parquet_to_nt import fetch_property_datatypes, nt_object  # noqa: E402

P31 = "http://www.wikidata.org/prop/direct/P31"
P279 = "http://www.wikidata.org/prop/direct/P279"
RDF_TYPE = "http://www.w3.org/1999/02/22-rdf-syntax-ns#type"
RDFS_SUBCLASS = "http://www.w3.org/2000/01/rdf-schema#subClassOf"
INSTANCE_COUNT = "https://w3id.org/rete/instanceCount"
XSD_INTEGER = "http://www.w3.org/2001/XMLSchema#integer"
ENT_Q = "http://www.wikidata.org/entity/Q"


def main() -> None:
    ap = argparse.ArgumentParser(
        description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter
    )
    ap.add_argument("--local-dir", default="data/wikidata-parquet/triplets",
                    help="dir with the downloaded part_*.parquet partitions")
    ap.add_argument("-o", "--output", default="data/wikidata-ontology/ontology.nt")
    ap.add_argument("--datatypes", choices=["auto", "none"], default="auto",
                    help="auto: recover typed literals from the property map cache")
    ap.add_argument("--datatype-cache", default="data/wd_property_types.csv")
    ap.add_argument("--memory-limit", default="16GB", help="DuckDB memory budget")
    args = ap.parse_args()

    import duckdb

    parts = sorted(glob.glob(os.path.join(args.local_dir, "part_*.parquet")))
    if not parts:
        sys.exit(f"no part_*.parquet in {args.local_dir} — download the dump first")
    out_dir = os.path.dirname(args.output) or "."
    os.makedirs(out_dir, exist_ok=True)

    dtypes: dict[str, str] = {}
    if args.datatypes == "auto":
        dtypes = fetch_property_datatypes(args.datatype_cache)
        print(f"{len(dtypes)} properties carry a typed literal", file=sys.stderr)

    con = duckdb.connect()
    con.execute("SET enable_progress_bar=false")
    con.execute(f"SET memory_limit='{args.memory_limit}'")
    con.execute(f"SET temp_directory='{os.path.join(out_dir, 'duckdb_tmp')}'")
    src = "[" + ", ".join(f"'{p}'" for p in parts) + "]"

    # ---- Pass A: class set + instance counts + P279 edges (one filtered scan)
    t0 = time.time()
    print(f"pass A: scanning {len(parts)} partitions for P31/P279…", file=sys.stderr)
    con.execute(
        f"CREATE TEMP TABLE pt AS SELECT subject, predicate, object "
        f"FROM read_parquet({src}) WHERE predicate IN ('{P31}', '{P279}')"
    )
    con.execute("CREATE TEMP TABLE p279 AS SELECT subject, object FROM pt "
                f"WHERE predicate = '{P279}'")
    con.execute("CREATE TEMP TABLE counts AS SELECT object AS cls, count(*) AS n "
                f"FROM pt WHERE predicate = '{P31}' GROUP BY object")
    con.execute(
        "CREATE TEMP TABLE classes AS SELECT DISTINCT cls FROM ("
        "  SELECT subject AS cls FROM p279"
        "  UNION ALL SELECT object FROM p279"
        "  UNION ALL SELECT cls FROM counts"
        f") WHERE cls LIKE '{ENT_Q}%'"
    )
    con.execute("DROP TABLE pt")
    n_classes = con.execute("SELECT count(*) FROM classes").fetchone()[0]
    n_p279 = con.execute("SELECT count(*) FROM p279").fetchone()[0]
    n_p31 = con.execute("SELECT sum(n) FROM counts").fetchone()[0]
    print(f"  {n_classes:,} classes, {n_p279:,} P279 edges, "
          f"{n_p31:,} P31 rows counted ({time.time() - t0:.0f}s)", file=sys.stderr)
    for tbl, fname in (("classes", "classes.parquet"),
                       ("counts", "class_counts.parquet"),
                       ("p279", "p279.parquet")):
        con.execute(f"COPY {tbl} TO '{os.path.join(out_dir, fname)}' (FORMAT parquet)")

    # ---- Pass B: emit the full star of every class + every property entity
    written = 0
    with open(args.output, "w", encoding="utf-8", newline="\n") as f:
        for pi, part in enumerate(parts):
            t1 = time.time()
            cur = con.execute(
                "SELECT subject, predicate, object, language "
                f"FROM read_parquet('{part}') "
                "WHERE subject IN (SELECT cls FROM classes) "
                "   OR regexp_matches(subject, "
                "'^http://www\\.wikidata\\.org/entity/P[0-9]+$')"
            )
            part_rows = 0
            while True:
                batch = cur.fetchmany(200_000)
                if not batch:
                    break
                lines = []
                for subject, predicate, obj, lang in batch:
                    if subject is None or predicate is None or obj is None:
                        continue
                    dtype = dtypes.get(predicate.rsplit("/", 1)[-1]) if dtypes else None
                    o = nt_object(obj, lang, dtype)
                    lines.append(f"<{subject}> <{predicate}> {o} .\n")
                    # dual-emit the standards vocabulary alongside wdt:
                    if predicate == P279:
                        lines.append(f"<{subject}> <{RDFS_SUBCLASS}> {o} .\n")
                    elif predicate == P31:
                        lines.append(f"<{subject}> <{RDF_TYPE}> {o} .\n")
                f.write("".join(lines))
                part_rows += len(lines)
            written += part_rows
            print(f"  part {pi:>3}/{len(parts)}: {part_rows:>9,} lines "
                  f"({time.time() - t1:.0f}s, total {written:,})", file=sys.stderr)

        # ---- instance-count annotations
        cur = con.execute("SELECT cls, n FROM counts WHERE cls LIKE ? ORDER BY cls",
                          [ENT_Q + "%"])
        n_counts = 0
        while True:
            batch = cur.fetchmany(200_000)
            if not batch:
                break
            f.write("".join(
                f'<{cls}> <{INSTANCE_COUNT}> "{n}"^^<{XSD_INTEGER}> .\n'
                for cls, n in batch
            ))
            n_counts += len(batch)
        written += n_counts
        print(f"  +{n_counts:,} instanceCount triples", file=sys.stderr)

    gb = os.path.getsize(args.output) / 1e9
    print(f"wrote {written:,} triples ({gb:.2f} GB) to {args.output} "
          f"in {time.time() - t0:.0f}s", file=sys.stderr)


if __name__ == "__main__":
    main()
