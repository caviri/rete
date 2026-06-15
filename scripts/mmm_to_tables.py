#!/usr/bin/env python3
"""Lossless per-class entity tables for a **generic RDF graph** (here: MMM) —
one Parquet table per `rdf:type`, readable columns AND nothing lost.

This is the MMM-shaped sibling of `scripts/rdf_to_entity_tables.py`. That one
reads the Wikidata truthy Parquet and types on `wdt:P31`; this one reads a plain
**N-Triples** file (the lossless `rete export` of a `.rete`) and types on
`rdf:type`, labels on `skos:prefLabel` — the CIDOC-CRM / FRBRoo shape MMM uses.
The table design is identical, so the two are interchangeable downstream:

  * the class's most common properties become named LIST columns (the readable,
    queryable projection — e.g. `P108i_was_produced_by`, `source`, …);
  * a `types` LIST column keeps *all* of the entity's `rdf:type` values, so a
    multi-typed entity lives in exactly one table (its largest type) and is
    never duplicated;
  * an `extra` MAP(predicate -> LIST(object)) column catches *every other*
    property (rare ones, all the `skos:altLabel` / multilingual labels, …) — so
    the row is a complete record, not a top-N projection;
  * a `label` column (from `skos:prefLabel`, else `rdfs:label`, else
    `skos:altLabel`) is added for readability.

Subjects with no `rdf:type` (or whose primary type is below `--min-entities`)
fall into an `_untyped` table carrying ALL their triples, so nothing is dropped
at any threshold. Object values are stored as their **N-Triples term tokens**
(`<iri>`, `"lit"`, `"lit"@en`, `"lit"^^<dt>`) — the canonical form — so IRIs,
literals and language tags all round-trip. That makes the whole set **lossless**:
explode `types` + every named column + `extra` across all tables and you get
back exactly the input triples. `--verify` checks that (reconstructed distinct
triples == input distinct triples). A `_manifest.parquet` records, per class,
the column -> predicate map so the reconstruction is mechanical.

Everything runs in DuckDB straight from the N-Triples file (DuckDB spills to
disk as needed). Requires: pip install --break-system-packages duckdb

Build the input first:
  rete export data/mmm/mmm-full.rete > data/mmm/mmm-full.nt   # lossless N-Triples

Usage:
  uv run --no-project --with duckdb python scripts/mmm_to_tables.py \\
      --nt data/mmm/mmm-full.nt -o data/mmm/tables \\
      --duckdb data/mmm/mmm-tables.duckdb --verify
"""

from __future__ import annotations

import argparse
import json
import os
import re
import sys

# Term tokens as they appear in the `p` column (bracketed IRIs).
RDF_TYPE = "<http://www.w3.org/1999/02/22-rdf-syntax-ns#type>"
# Labelling predicates: captured by the dedicated `label` column and (for the
# ones not chosen as named columns) preserved in `extra`. Excluded from the
# named-column selection so the tables are about the entities' real properties,
# not the half-dozen ways MMM spells a name.
PREF_LABEL = "<http://www.w3.org/2004/02/skos/core#prefLabel>"
RDFS_LABEL = "<http://www.w3.org/2000/01/rdf-schema#label>"
ALT_LABEL = "<http://www.w3.org/2004/02/skos/core#altLabel>"
LABEL_PREDS = (PREF_LABEL, RDFS_LABEL, ALT_LABEL)


def localname(token: str) -> str:
    """Last path/fragment segment of a `<iri>` token, for column/file naming."""
    iri = token[1:-1] if token.startswith("<") and token.endswith(">") else token
    m = re.search(r"[/#:]([^/#:]+)/?$", iri.rstrip("/"))
    return m.group(1) if m else iri


def col_ident(token: str) -> str:
    """A safe SQL/Parquet column identifier from a predicate/class token."""
    return re.sub(r"[^A-Za-z0-9_]", "_", localname(token))


def sql_in(tokens) -> str:
    """A SQL `IN (...)` body of single-quoted tokens (these are IRIs, so they
    carry no `'`; double any just in case)."""
    return ", ".join("'" + t.replace("'", "''") + "'" for t in tokens)


def main() -> None:
    ap = argparse.ArgumentParser(
        description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter
    )
    ap.add_argument("--nt", required=True, help="N-Triples file (lossless `rete export` of the .rete)")
    ap.add_argument("--props", type=int, default=24, help="named columns per class (the rest go to `extra`)")
    ap.add_argument("--min-entities", type=int, default=1, help="give a class its own table only above this size")
    ap.add_argument("-o", "--output", default="data/mmm/tables")
    ap.add_argument("--duckdb", default=None, help="also assemble a DuckDB file with one table per class")
    ap.add_argument("--sqlite", default=None, help="also assemble a SQLite file (list/map columns as JSON text)")
    ap.add_argument("--verify", action="store_true", help="reconstruct triples and check the count is lossless")
    args = ap.parse_args()

    try:
        import duckdb
    except ModuleNotFoundError:
        sys.exit("duckdb not installed — run: pip install --break-system-packages duckdb")

    if not os.path.exists(args.nt):
        sys.exit(f"input not found: {args.nt} — run `rete export <file> > {args.nt}` first")

    os.makedirs(args.output, exist_ok=True)
    con = duckdb.connect()
    con.execute("PRAGMA threads=8")

    # 1. Parse N-Triples into (s, p, o) with each object kept as its canonical
    #    token. Read each line as ONE VARCHAR field: pick a delimiter / quote /
    #    escape from control bytes that never occur in canonical N-Triples
    #    (0x01–0x03), embedded as real single chars so DuckDB takes them literally
    #    (its SQL parser does not decode `\xNN` escapes). Then drop the trailing
    #    " ." and split off subject + predicate (neither contains a space); the
    #    remainder is the object token verbatim — so literals with spaces,
    #    language tags and datatypes survive intact.
    print(f"parsing N-Triples from {args.nt}…", file=sys.stderr)
    delim, quote, esc = chr(1), chr(2), chr(3)
    nt = args.nt.replace("\\", "/").replace("'", "''")
    con.execute(
        "CREATE TABLE base AS "
        "WITH raw AS ("
        f"  SELECT line FROM read_csv('{nt}', columns={{'line': 'VARCHAR'}}, "
        f"    header=false, auto_detect=false, delim='{delim}', quote='{quote}', escape='{esc}')"
        "), body AS ("
        "  SELECT substr(line, 1, length(line) - 2) AS b "
        "  FROM raw WHERE length(line) > 4 AND right(line, 2) = ' .'"
        "), parsed AS ("
        "  SELECT split_part(b, ' ', 1) AS s, split_part(b, ' ', 2) AS p, "
        "    substr(b, length(split_part(b, ' ', 1)) + length(split_part(b, ' ', 2)) + 3) AS o "
        "  FROM body"
        ") SELECT DISTINCT s, p, o FROM parsed WHERE s <> '' AND p <> '' AND o <> ''"
    )
    ntriples = con.execute("SELECT COUNT(*) FROM base").fetchone()[0]
    print(f"  {ntriples:,} distinct triples", file=sys.stderr)

    # 2. Per (entity, predicate) token lists; class membership; primary class
    #    (the entity's largest type, so it lands in one readable table); labels.
    con.execute("CREATE TABLE pe AS SELECT s, p, list(o) AS toks FROM base GROUP BY s, p")
    con.execute(f"CREATE TABLE ec AS SELECT s, o AS class FROM base WHERE p = '{RDF_TYPE}'")
    con.execute("CREATE TABLE csize AS SELECT class, COUNT(*) n FROM ec GROUP BY class")
    con.execute(
        """
        CREATE TABLE prim AS
        SELECT s, arg_max(ec.class, csize.n) AS class
        FROM ec JOIN csize ON ec.class = csize.class GROUP BY s
        """
    )
    # Label: prefer skos:prefLabel, then rdfs:label, then skos:altLabel; store
    # the lexical form (strip the surrounding quotes + any @lang / ^^<dt>).
    con.execute(
        f"""
        CREATE TABLE lbl AS
        SELECT s, regexp_extract(coalesce(
                 any_value(o) FILTER (WHERE p = '{PREF_LABEL}'),
                 any_value(o) FILTER (WHERE p = '{RDFS_LABEL}'),
                 any_value(o) FILTER (WHERE p = '{ALT_LABEL}')
               ), '^"(.*)"', 1) AS label
        FROM base WHERE p IN ({sql_in(LABEL_PREDS)}) GROUP BY s
        """
    )

    classes = con.execute(
        f"SELECT class, COUNT(*) n FROM prim GROUP BY class HAVING n >= {args.min_entities} ORDER BY n DESC"
    ).fetchall()
    con.execute("CREATE TABLE kept (class VARCHAR)")
    for cls, _ in classes:
        con.execute("INSERT INTO kept VALUES (?)", [cls])
    con.execute("CREATE TABLE tabled AS SELECT s FROM prim JOIN kept USING(class)")
    print(f"  {len(classes)} typed classes (>= {args.min_entities} entities)", file=sys.stderr)

    manifest = []
    excl = sql_in([RDF_TYPE, *LABEL_PREDS])
    # Unique output filename per class: different namespaces share localnames
    # (rdfs:Class vs owl:Class -> both "Class"), so a bare localname would let one
    # COPY clobber another and silently drop entities. Disambiguate with a counter.
    used_files: set[str] = {"_untyped.parquet", "_manifest.parquet"}

    def class_file(cls: str) -> str:
        base, i, name = col_ident(cls), 2, col_ident(cls)
        while name + ".parquet" in used_files:
            name, i = f"{base}_{i}", i + 1
        used_files.add(name + ".parquet")
        return name + ".parquet"

    for cls, ecount in classes:
        fname = class_file(cls)
        props = con.execute(
            f"SELECT p, COUNT(*) c FROM pe JOIN prim USING(s) WHERE prim.class = ? "
            f"AND p NOT IN ({excl}) GROUP BY p ORDER BY c DESC LIMIT ?",
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
        not_extra = sql_in([RDF_TYPE, *named])
        proj = (",\n  ".join(cols) + ",") if cols else ""
        con.execute(
            f"""
            COPY (
              SELECT pe.s AS entity,
                any_value(lbl.label) AS label,
                any_value(toks) FILTER (WHERE p = '{RDF_TYPE}') AS types,
                {proj}
                map_from_entries(list(struct_pack(k := p, v := toks))
                  FILTER (WHERE p NOT IN ({not_extra}))) AS extra
              FROM pe JOIN prim ON prim.s = pe.s AND prim.class = '{cls}'
                LEFT JOIN lbl ON lbl.s = pe.s
              GROUP BY pe.s
            ) TO '{os.path.join(args.output, fname)}' (FORMAT parquet)
            """
        )
        clabel = con.execute("SELECT label FROM lbl WHERE s = ? LIMIT 1", [cls]).fetchone()
        manifest.append((cls, clabel[0] if clabel else None, ecount, len(named), fname, colmap))

    # 3. Residual: every subject not written above, all its triples in `extra`.
    con.execute(
        f"""
        COPY (
          SELECT pe.s AS entity, any_value(lbl.label) AS label,
            map_from_entries(list(struct_pack(k := p, v := toks))) AS extra
          FROM pe LEFT JOIN lbl ON lbl.s = pe.s
          WHERE pe.s NOT IN (SELECT s FROM tabled)
          GROUP BY pe.s
        ) TO '{os.path.join(args.output, "_untyped.parquet")}' (FORMAT parquet)
        """
    )
    untyped = con.execute("SELECT COUNT(DISTINCT s) FROM pe WHERE s NOT IN (SELECT s FROM tabled)").fetchone()[0]
    print(f"  {len(classes)} class tables + _untyped ({untyped:,} subjects)", file=sys.stderr)

    # 4. Manifest.
    con.execute("CREATE TABLE manifest (class VARCHAR, label VARCHAR, entities BIGINT, named_columns INTEGER, file VARCHAR, column_map JSON)")
    for cls, lab, ecount, ncols, fname, cmap in manifest:
        con.execute("INSERT INTO manifest VALUES (?, ?, ?, ?, ?, ?)", [cls, lab, ecount, ncols, fname, json.dumps(cmap)])
    con.execute(f"COPY manifest TO '{os.path.join(args.output, '_manifest.parquet')}' (FORMAT parquet)")

    # 5. Losslessness check: reconstruct triple count from every table.
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
            parts.append(f"SELECT entity, coalesce(list_sum(list_transform(map_values(extra), x -> len(x))),0) AS k FROM '{path}'")
            recon += con.execute(f"SELECT coalesce(sum(k),0) FROM ({' UNION ALL '.join(parts)})").fetchone()[0]
        ok = recon == ntriples
        print(f"  lossless check: reconstructed {recon:,} vs {ntriples:,} input triples — "
              f"{'OK' if ok else 'MISMATCH'}", file=sys.stderr)
        if not ok:
            sys.exit("losslessness check FAILED")

    total = sum(os.path.getsize(os.path.join(args.output, f)) for f in os.listdir(args.output))
    print(f"wrote {len(manifest)} class tables + _untyped + manifest to {args.output} "
          f"({total / (1024 * 1024):.0f} MB)", file=sys.stderr)

    # Human-readable, unique table name per class (English label, else localname).
    used_names: set[str] = set()

    def tname(label: str | None, base_name: str) -> str:
        base = re.sub(r"[^A-Za-z0-9_]", "_", (label or base_name).strip()).strip("_") or base_name
        if base[0].isdigit():
            base = "t_" + base
        name, i = base, 2
        while name in used_names:
            name, i = f"{base}_{i}", i + 1
        used_names.add(name)
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

    # SQLite: same tables; LIST/MAP columns serialized to JSON text.
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
