#!/usr/bin/env python3
"""Lossless entity tables: one table per type, readable columns AND nothing lost.

The best of both worlds between the graph and a columnar layout. Entities are
grouped by their type (`wdt:P31`) into one Parquet table per class, where:

  * the class's most common properties become named LIST columns (occupation,
    citizenship, date of birth, …) — the readable, queryable projection;
  * a `types` LIST column keeps *all* of the entity's P31 values, so a
    multi-typed entity lives in exactly one table (its largest type) and is
    never duplicated;
  * an `extra` MAP(predicate -> LIST(object)) column catches *every other*
    property (rare ones, all the multilingual labels/descriptions, …) — so the
    row is a complete record, not a top-N projection;
  * a `label` column (English) is added for readability.

Entities with no `P31` go to an `_untyped` table (entity, label, extra). Object
values are stored as their **N-Triples term tokens** (`<iri>`, `"lit"`,
`"lit"@en`) — the canonical form — so IRIs, literals and language tags all
round-trip. That makes the whole set **lossless**: explode `types` + every
named column + `extra` across all tables and you get back exactly the triples.
`--verify` checks that (reconstructed distinct triples == input distinct
triples). A `_manifest.parquet` records, per class, the column -> predicate map
so the reconstruction is mechanical.

Runs in DuckDB from the source Parquet. Requires: pip install --break-system-packages duckdb

Usage:
  uv run python scripts/rdf_to_entity_tables.py --parts 1 --limit 12000000 --props 24 -o data/ent --verify
"""

from __future__ import annotations

import argparse
import json
import os
import re
import sys

HF_BASE = "https://huggingface.co/datasets/piebro/wikidata-extraction/resolve/main/triplets"
P31 = "http://www.wikidata.org/prop/direct/P31"
LABEL_PREDS = (
    "http://www.w3.org/2000/01/rdf-schema#label",
    "http://www.w3.org/2004/02/skos/core#prefLabel",
    "http://www.w3.org/2004/02/skos/core#altLabel",
    "http://schema.org/name",
    "http://schema.org/description",
)


def localname(iri: str) -> str:
    m = re.search(r"[/#]([^/#]+)/?$", iri.rstrip("/"))
    return m.group(1) if m else iri


def col_ident(pid: str) -> str:
    return re.sub(r"[^A-Za-z0-9_]", "_", localname(pid))


def main() -> None:
    # The type predicate is configurable (Wikidata uses P31; OBO/RDF use rdf:type).
    # We rebind the module-level P31 after parsing so every '{P31}' query below
    # picks up the chosen predicate — `global` keeps the argparse default (which
    # reads P31) valid instead of turning P31 into an unbound local.
    global P31
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--parts", type=int, default=1)
    ap.add_argument("--limit", type=int, default=None)
    ap.add_argument("--local-dir", default=None)
    ap.add_argument("--nt", default=None, help="read N-Triples directly (any RDF graph), instead of the Wikidata Parquet source")
    ap.add_argument("--type-predicate", default=P31, help="predicate that types entities (default Wikidata P31; use rdf:type for OBO/most RDF)")
    ap.add_argument("--props", type=int, default=24, help="named columns per class (the rest go to `extra`)")
    ap.add_argument("--min-entities", type=int, default=1, help="give a class its own table only above this size")
    ap.add_argument("-o", "--output", default="data/entity-tables")
    ap.add_argument("--duckdb", default=None, help="also assemble a DuckDB file with one table per class")
    ap.add_argument("--sqlite", default=None, help="also assemble a SQLite file (list/map columns as JSON text)")
    ap.add_argument("--verify", action="store_true", help="reconstruct triples and check the count is lossless")
    args = ap.parse_args()
    P31 = args.type_predicate

    try:
        import duckdb
    except ModuleNotFoundError:
        sys.exit("duckdb not installed — run: pip install --break-system-packages duckdb")

    os.makedirs(args.output, exist_ok=True)
    con = duckdb.connect()
    con.execute("PRAGMA threads=8")
    src_list, limit_sql = "", ""
    if args.nt:
        # N-Triples mode (any RDF graph): read one line per row with the CSV
        # reader (a rare delimiter so literals never split), then pull
        # subject/predicate/object with RE2. The object token is kept verbatim —
        # IRIs, language tags and datatypes all round-trip losslessly. Subjects
        # may be IRIs or blank nodes; predicates are always IRIs.
        print(f"parsing N-Triples from {args.nt}…", file=sys.stderr)
        con.execute(f"""
            CREATE TABLE base AS
            SELECT DISTINCT
              coalesce(nullif(regexp_extract(line, '^<([^>]*)>', 1), ''),
                       regexp_extract(line, '^(_:[^ ]+)', 1)) AS s,
              regexp_extract(line, '^\\S+\\s+<([^>]*)>', 1) AS p,
              regexp_extract(line, '^\\S+\\s+\\S+\\s+(.*?)\\s*\\.\\s*$', 1) AS tok
            FROM read_csv('{args.nt}', delim='\\x01', header=false,
                          columns={{'line': 'VARCHAR'}}, quote='', escape='', ignore_errors=true)
            WHERE line LIKE '<%' OR line LIKE '_:%'
        """)
    else:
        if args.local_dir:
            srcs = [os.path.join(args.local_dir, f"part_{i:04d}.parquet") for i in range(args.parts)]
            srcs = [s for s in srcs if os.path.exists(s)]
        else:
            con.execute("INSTALL httpfs; LOAD httpfs;")
            srcs = [f"{HF_BASE}/part_{i:04d}.parquet" for i in range(args.parts)]
        src_list = ", ".join(f"'{s}'" for s in srcs)
        limit_sql = f" LIMIT {args.limit}" if args.limit else ""

        # 1. Canonical, deduplicated triples with each object as an N-Triples token.
        print("materializing triples (objects -> N-Triples tokens)…", file=sys.stderr)
        con.execute(f"""
            CREATE TABLE base AS
            SELECT DISTINCT subject AS s, predicate AS p,
              CASE
                WHEN language IS NOT NULL
                  THEN '"' || replace(replace(object, '\\', '\\\\'), '"', '\\"') || '"@' || language
                WHEN starts_with(object, 'http://') OR starts_with(object, 'https://')
                  THEN '<' || object || '>'
                WHEN starts_with(object, '_:') THEN object
                ELSE '"' || replace(replace(object, '\\', '\\\\'), '"', '\\"') || '"'
              END AS tok
            FROM (SELECT * FROM read_parquet([{src_list}]){limit_sql})
        """)
    ntriples = con.execute("SELECT COUNT(*) FROM base").fetchone()[0]
    print(f"  {ntriples:,} distinct triples", file=sys.stderr)

    # 2. Per (entity, predicate) token lists; class membership; primary class
    #    (the entity's largest type, so it lands in one readable table); labels.
    con.execute("CREATE TABLE pe AS SELECT s, p, list(tok) AS toks FROM base GROUP BY s, p")
    # s -> each of its type tokens (one row per (entity, type)).
    con.execute(f"CREATE TABLE ec AS SELECT s, tok AS class FROM base WHERE p = '{P31}'")
    con.execute("CREATE TABLE csize AS SELECT class, COUNT(*) n FROM ec GROUP BY class")
    # Primary class = the entity's largest type, so it lands in one readable table.
    con.execute("""
        CREATE TABLE prim AS
        SELECT s, arg_max(ec.class, csize.n) AS class
        FROM ec JOIN csize ON ec.class = csize.class GROUP BY s
    """)
    if args.nt:
        # Labels come from the token table: English (…"@en) or untagged literals.
        con.execute(f"""
            CREATE TABLE lbl AS
            SELECT s, any_value(regexp_extract(tok, '^"(.*)"(?:@en)?$', 1)) AS label
            FROM base
            WHERE p = '{LABEL_PREDS[0]}' AND (tok LIKE '%"@en' OR tok NOT LIKE '%"@%')
            GROUP BY s
        """)
    else:
        con.execute(f"""
            CREATE TABLE lbl AS SELECT s, any_value(object) AS label FROM
            (SELECT subject AS s, object, language, predicate FROM read_parquet([{src_list}]){limit_sql})
            WHERE predicate = '{LABEL_PREDS[0]}' AND language = 'en' GROUP BY s
        """)

    classes = con.execute(
        f"SELECT class, COUNT(*) n FROM prim GROUP BY class HAVING n >= {args.min_entities} ORDER BY n DESC"
    ).fetchall()
    # Entities actually written to a class table (their primary class cleared
    # the threshold). Everyone else — untyped, or in a sub-threshold class —
    # falls into the residual table with ALL their triples (P31 included), so
    # nothing is dropped at any threshold.
    con.execute("CREATE TABLE kept (class VARCHAR)")
    for cls, _ in classes:
        con.execute("INSERT INTO kept VALUES (?)", [cls])
    con.execute("CREATE TABLE tabled AS SELECT s FROM prim JOIN kept USING(class)")
    print(f"  {len(classes)} typed classes (>= {args.min_entities} entities)", file=sys.stderr)

    manifest = []
    excl = "', '".join([P31, *LABEL_PREDS])
    for cls, ecount in classes:
        props = con.execute(
            f"SELECT p, COUNT(*) c FROM pe JOIN prim USING(s) WHERE prim.class = ? "
            f"AND p NOT IN ('{excl}') GROUP BY p ORDER BY c DESC LIMIT ?",
            [cls, args.props],
        ).fetchall()
        colmap, cols, used = {}, [], set()
        for pid, _ in props:
            ident = col_ident(pid)
            while ident in used or ident in ("entity", "label", "types", "extra"):
                ident += "_"
            used.add(ident)
            colmap[ident] = pid
            cols.append(f"any_value(toks) FILTER (WHERE p = '{pid}') AS \"{ident}\"")
        named = list(colmap.values())
        not_extra = "', '".join([P31, *named])
        proj = (",\n  ".join(cols) + ",") if cols else ""
        con.execute(f"""
            COPY (
              SELECT pe.s AS entity,
                any_value(lbl.label) AS label,
                any_value(toks) FILTER (WHERE p = '{P31}') AS types,
                {proj}
                map_from_entries(list(struct_pack(k := p, v := toks))
                  FILTER (WHERE p NOT IN ('{not_extra}'))) AS extra
              FROM pe JOIN prim ON prim.s = pe.s AND prim.class = '{cls}'
                LEFT JOIN lbl ON lbl.s = pe.s
              GROUP BY pe.s
            ) TO '{os.path.join(args.output, col_ident(cls) + ".parquet")}' (FORMAT parquet)
        """)
        clabel = con.execute("SELECT label FROM lbl WHERE s = ? LIMIT 1", [cls]).fetchone()
        manifest.append((cls, clabel[0] if clabel else None, ecount, len(named),
                         col_ident(cls) + ".parquet", colmap))

    # 3. Untyped residual: every subject with no P31, all its triples in `extra`.
    con.execute(f"""
        COPY (
          SELECT pe.s AS entity, any_value(lbl.label) AS label,
            map_from_entries(list(struct_pack(k := p, v := toks))) AS extra
          FROM pe LEFT JOIN lbl ON lbl.s = pe.s
          WHERE pe.s NOT IN (SELECT s FROM tabled)
          GROUP BY pe.s
        ) TO '{os.path.join(args.output, "_untyped.parquet")}' (FORMAT parquet)
    """)
    untyped = con.execute("SELECT COUNT(DISTINCT s) FROM pe WHERE s NOT IN (SELECT s FROM tabled)").fetchone()[0]
    print(f"  {len(classes)} class tables + _untyped ({untyped:,} subjects)", file=sys.stderr)

    # 4. Manifest.
    con.execute("CREATE TABLE manifest (class VARCHAR, label VARCHAR, entities BIGINT, named_columns INTEGER, file VARCHAR, column_map JSON)")
    for cls, lab, ecount, ncols, fname, cmap in manifest:
        con.execute("INSERT INTO manifest VALUES (?, ?, ?, ?, ?, ?)", [cls, lab, ecount, ncols, fname, json.dumps(cmap)])
    con.execute(f"COPY manifest TO '{os.path.join(args.output, '_manifest.parquet')}' (FORMAT parquet)")

    # 5. Losslessness check: reconstruct triples from every table and count.
    if args.verify:
        files = [f for f in os.listdir(args.output) if f.endswith(".parquet") and f != "_manifest.parquet"]
        recon = 0
        for f in files:
            path = os.path.join(args.output, f)
            cols = [r[0] for r in con.execute(f"DESCRIBE SELECT * FROM '{path}'").fetchall()]
            parts = []
            if "types" in cols:
                parts.append(f"SELECT entity, len(types) AS k FROM '{path}'")
            for c in cols:
                if c in ("entity", "label", "types", "extra"):
                    continue
                parts.append(f"SELECT entity, coalesce(len(\"{c}\"),0) AS k FROM '{path}'")
            # extra: sum of list lengths over the map values
            parts.append(f"SELECT entity, coalesce(list_sum(list_transform(map_values(extra), x -> len(x))),0) AS k FROM '{path}'")
            recon += con.execute(f"SELECT coalesce(sum(k),0) FROM ({' UNION ALL '.join(parts)})").fetchone()[0]
        print(f"  lossless check: reconstructed {recon:,} vs {ntriples:,} input triples — "
              f"{'OK' if recon == ntriples else 'MISMATCH'}", file=sys.stderr)
        if recon != ntriples:
            sys.exit("losslessness check FAILED")

    total = sum(os.path.getsize(os.path.join(args.output, f)) for f in os.listdir(args.output))
    print(f"wrote {len(manifest)} class tables + _untyped + manifest to {args.output} "
          f"({total / (1024 * 1024):.0f} MB)", file=sys.stderr)

    # Human-readable, unique table name per class (English label, else Q-id).
    used: set[str] = set()

    def tname(label: str | None, qid: str) -> str:
        base = re.sub(r"[^A-Za-z0-9_]", "_", (label or qid).strip()).strip("_") or qid
        if base[0].isdigit():
            base = "t_" + base
        name, i = base, 2
        while name in used:
            name, i = f"{base}_{i}", i + 1
        used.add(name)
        return name

    files = [(tname(lab, localname(cls)), fname) for cls, lab, _, _, fname, _ in manifest]
    files.append(("untyped", "_untyped.parquet"))

    # DuckDB: one native table per class (LIST/MAP columns preserved as-is).
    if args.duckdb:
        if os.path.exists(args.duckdb):
            os.remove(args.duckdb)
        db = duckdb.connect(args.duckdb)
        for nm, fname in files:
            db.execute(f'CREATE TABLE "{nm}" AS SELECT * FROM \'{os.path.join(args.output, fname)}\'')
        db.execute(f"CREATE TABLE _manifest AS SELECT * FROM '{os.path.join(args.output, '_manifest.parquet')}'")
        db.close()
        print(f"  duckdb: {len(files)} tables -> {args.duckdb}", file=sys.stderr)

    # SQLite: same tables; LIST/MAP columns serialized to JSON text (sqlite has
    # no nested types) — still lossless, recoverable by parsing the JSON.
    if args.sqlite:
        import sqlite3
        if os.path.exists(args.sqlite):
            os.remove(args.sqlite)
        sq = sqlite3.connect(args.sqlite)
        for nm, fname in files:
            path = os.path.join(args.output, fname)
            cols = [r[0] for r in con.execute(f"DESCRIBE SELECT * FROM '{path}'").fetchall()]
            sel = ", ".join(
                f'"{c}"' if c in ("entity", "label") else f'to_json("{c}") AS "{c}"' for c in cols
            )
            coldefs = ", ".join('"' + c + '" TEXT' for c in cols)
            sq.execute(f'CREATE TABLE "{nm}" ({coldefs})')
            cur = con.execute(f"SELECT {sel} FROM '{path}'")
            ph = ", ".join("?" * len(cols))
            while True:
                rows = cur.fetchmany(50_000)
                if not rows:
                    break
                sq.executemany(f'INSERT INTO "{nm}" VALUES ({ph})', rows)
        sq.commit()
        sq.close()
        print(f"  sqlite: {len(files)} tables -> {args.sqlite}", file=sys.stderr)


if __name__ == "__main__":
    main()
