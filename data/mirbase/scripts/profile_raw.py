#!/usr/bin/env python3
"""Profile the miRBase 22.1 raw drop: relational tables, FASTA, EMBL, GFF3.

Reads column names straight out of raw/database_files/tables.sql so the report
can never drift from the shipped schema. Reports row counts, fill rates, sample
values, join integrity between tables, and the genome-coordinate coverage.

Run (from repo root):
    docker run --rm -v "$PWD:/w" -w //w python:3.12-slim \
        python data/mirbase/scripts/inspect.py
"""
from __future__ import annotations

import re
from collections import Counter
from pathlib import Path

RAW = Path(__file__).resolve().parent.parent / "raw"
DB = RAW / "database_files"

NULL = "\\N"


def parse_tables_sql() -> dict[str, list[str]]:
    """table name -> ordered column names, from the MySQL dump's DDL."""
    sql = (DB / "tables.sql").read_text(encoding="utf-8", errors="replace")
    out: dict[str, list[str]] = {}
    for m in re.finditer(r"CREATE TABLE `(\w+)` \((.*?)\n\) ENGINE=", sql, re.S):
        name, body = m.group(1), m.group(2)
        cols = []
        for line in body.split("\n"):
            line = line.strip()
            cm = re.match(r"`([^`]+)`\s+\S", line)
            # skip index/key definitions, which also start with a backtick word
            if cm and not re.match(
                r"(PRIMARY|UNIQUE|KEY|FULLTEXT|CONSTRAINT|INDEX)\b", line, re.I
            ):
                cols.append(cm.group(1))
        out[name] = cols
    return out


def read_rows(path: Path, ncols: int) -> tuple[list[list[str]], int]:
    """Tab-delimited MySQL outfile -> rows. Returns (rows, ragged_line_count).

    The dump escapes embedded newlines as a literal backslash-n, so one physical
    line == one row; we still count any line whose field count is unexpected.
    """
    rows, ragged = [], 0
    with path.open(encoding="utf-8", errors="replace") as fh:
        for line in fh:
            f = line.rstrip("\n").split("\t")
            if len(f) != ncols:
                ragged += 1
            rows.append(f)
    return rows, ragged


def profile_table(name: str, cols: list[str]) -> list[list[str]] | None:
    path = DB / f"{name}.txt"
    if not path.exists():
        print(f"  !! {name}.txt MISSING")
        return None
    rows, ragged = read_rows(path, len(cols))
    print(f"\n  {name}  —  {len(rows):,} rows × {len(cols)} cols"
          + (f"   [!! {ragged:,} ragged lines]" if ragged else ""))
    for i, c in enumerate(cols):
        vals = [r[i] for r in rows if i < len(r)]
        nonnull = [v for v in vals if v != NULL and v != ""]
        uniq = len(set(nonnull))
        fill = 100.0 * len(nonnull) / len(vals) if vals else 0.0
        sample = next((v for v in nonnull), "")
        if len(sample) > 42:
            sample = sample[:39] + "..."
        print(f"      {c:<24} fill {fill:5.1f}%  uniq {uniq:>7,}  e.g. {sample!r}")
    return rows


def col(rows: list[list[str]], cols: list[str], name: str) -> list[str]:
    i = cols.index(name)
    return [r[i] for r in rows if i < len(r)]


def fasta_stats(path: Path, label: str) -> None:
    if not path.exists():
        print(f"  !! {path.name} MISSING")
        return
    n, species, accs, seqlens, cur = 0, Counter(), set(), [], 0
    with path.open(encoding="utf-8", errors="replace") as fh:
        for line in fh:
            if line.startswith(">"):
                if n:
                    seqlens.append(cur)
                cur = 0
                n += 1
                parts = line[1:].split()
                if parts:
                    species[parts[0].split("-")[0]] += 1
                if len(parts) > 1:
                    accs.add(parts[1])
            else:
                cur += len(line.strip())
    if n:
        seqlens.append(cur)
    print(f"  {label:<22} {n:,} records | {len(species)} species prefixes | "
          f"{len(accs):,} accessions | len min/med/max "
          f"{min(seqlens)}/{sorted(seqlens)[len(seqlens)//2]}/{max(seqlens)}")
    print(f"      top species: "
          + ", ".join(f"{k}={v:,}" for k, v in species.most_common(6)))


def embl_stats(path: Path, label: str) -> None:
    if not path.exists():
        print(f"  !! {path.name} MISSING")
        return
    line_codes, ft_keys, quals = Counter(), Counter(), Counter()
    n_id = n_term = 0
    in_ft = False
    with path.open(encoding="utf-8", errors="replace") as fh:
        for line in fh:
            code = line[:2]
            if line.startswith("//"):
                n_term += 1
                in_ft = False
                continue
            line_codes[code] += 1
            if code == "ID":
                n_id += 1
            if code == "FT":
                body = line[5:].rstrip("\n")
                stripped = body.strip()
                # `FT   miRNA           17..38`  -> feature key in cols 5..21
                # `FT                   /product="..."` -> qualifier (indented)
                if stripped.startswith("/"):
                    quals[stripped.split("=", 1)[0][1:]] += 1
                elif body[:16].strip():
                    ft_keys[body.split()[0]] += 1
                else:
                    quals["<continuation>"] += 1
                in_ft = True
    print(f"  {label:<22} {n_id:,} ID lines | {n_term:,} '//' terminators "
          f"{'(balanced)' if n_id == n_term else '(!! UNBALANCED)'}")
    print(f"      line codes : "
          + ", ".join(f"{k}={v:,}" for k, v in line_codes.most_common(12)))
    print(f"      FT keys    : " + ", ".join(f"{k}={v:,}" for k, v in ft_keys.most_common()))
    print(f"      FT quals   : " + ", ".join(f"{k}={v:,}" for k, v in quals.most_common()))


def gff3_stats() -> None:
    gdir = RAW / "genomes"
    files = sorted(gdir.glob("*.gff3"))
    print(f"  {len(files)} GFF3 files")
    total, types, builds = 0, Counter(), {}
    attr_keys = Counter()
    for f in files:
        n = 0
        build = accession = ""
        for line in f.read_text(encoding="utf-8", errors="replace").splitlines():
            if line.startswith("#"):
                if "genome-build-id:" in line:
                    build = line.split(":", 1)[1].strip()
                elif "genome-build-accession:" in line:
                    accession = line.split(":", 1)[1].strip()
                continue
            if not line.strip():
                continue
            n += 1
            parts = line.split("\t")
            if len(parts) >= 9:
                types[parts[2]] += 1
                for kv in parts[8].rstrip(";").split(";"):
                    if "=" in kv:
                        attr_keys[kv.split("=")[0]] += 1
        builds[f.stem] = (build, accession, n)
        total += n
    print(f"  {total:,} feature rows total")
    print(f"      types      : " + ", ".join(f"{k}={v:,}" for k, v in types.most_common()))
    print(f"      attr keys  : " + ", ".join(f"{k}={v:,}" for k, v in attr_keys.most_common()))
    print("      per-species (organism, assembly, accession, features):")
    for sp, (b, a, n) in sorted(builds.items(), key=lambda kv: -kv[1][2]):
        print(f"        {sp:<6} {b:<16} {a:<34} {n:>6,}")


def main() -> None:
    print("=" * 78)
    print("miRBase 22.1 — raw drop profile")
    print("=" * 78)

    schema = parse_tables_sql()
    print(f"\n### tables.sql declares {len(schema)} tables")

    print("\n### relational tables")
    data: dict[str, list[list[str]]] = {}
    for name in sorted(schema):
        rows = profile_table(name, schema[name])
        if rows is not None:
            data[name] = rows

    # ---- join integrity + the counts that need explaining -------------------
    print("\n### join integrity")
    if "mirna" in data:
        cols = schema["mirna"]
        dead = Counter(col(data["mirna"], cols, "dead_flag"))
        accs = set(col(data["mirna"], cols, "mirna_acc"))
        print(f"  mirna rows {len(data['mirna']):,}; dead_flag {dict(dead)}; "
              f"distinct mirna_acc {len(accs):,}")
        live = sum(v for k, v in dead.items() if k == "0")
        print(f"  -> live hairpins {live:,} (hairpin.fa should match)")
        auto = set(col(data["mirna"], cols, "auto_mirna"))

        for tbl, fk in [("mirna_chromosome_build", "auto_mirna"),
                        ("mirna_pre_mature", "auto_mirna"),
                        ("mirna_context", "auto_mirna"),
                        ("mirna_2_prefam", "auto_mirna"),
                        ("mirna_literature_references", "auto_mirna"),
                        ("mirna_database_links", "auto_mirna"),
                        ("confidence_score", "auto_mirna")]:
            if tbl in data:
                vals = set(col(data[tbl], schema[tbl], fk))
                orphan = vals - auto
                print(f"  {tbl:<30} {len(data[tbl]):>7,} rows | "
                      f"{len(vals):>6,} distinct {fk} | "
                      f"{len(vals & auto):>6,} join | {len(orphan):>5,} orphan")

    if "mirna_mature" in data and "mirna_pre_mature" in data:
        mcols = schema["mirna_mature"]
        mat_auto = set(col(data["mirna_mature"], mcols, "auto_mature"))
        pm = set(col(data["mirna_pre_mature"], schema["mirna_pre_mature"], "auto_mature"))
        mdead = Counter(col(data["mirna_mature"], mcols, "dead_flag"))
        print(f"  mirna_mature dead_flag {dict(mdead)}; "
              f"{len(pm & mat_auto):,} of {len(mat_auto):,} mature linked to a hairpin")

    if "mirna_species" in data:
        scols = schema["mirna_species"]
        sp = data["mirna_species"]
        with_tax = [r for r in sp if r[scols.index("taxon_id")] not in (NULL, "")]
        with_asm = [r for r in sp
                    if r[scols.index("genome_assembly")] not in (NULL, "")]
        print(f"  species {len(sp):,} | with taxon_id {len(with_tax):,} | "
              f"with genome_assembly {len(with_asm):,}")
        div = Counter(r[scols.index("division")] for r in sp)
        print(f"  divisions: " + ", ".join(f"{k}={v}" for k, v in div.most_common()))

    # genome-coordinate coverage — the whole point of including GFF3 + chrom build
    print("\n### genome-coordinate coverage")
    if "mirna_chromosome_build" in data and "mirna" in data:
        ccols = schema["mirna_chromosome_build"]
        mcols = schema["mirna"]
        auto2sp = {r[mcols.index("auto_mirna")]: r[mcols.index("auto_species")]
                   for r in data["mirna"]}
        coord_auto = set(col(data["mirna_chromosome_build"], ccols, "auto_mirna"))
        sp_covered = {auto2sp.get(a) for a in coord_auto} - {None}
        print(f"  mirna_chromosome_build: {len(data['mirna_chromosome_build']):,} rows, "
              f"{len(coord_auto):,} distinct hairpins, "
              f"{len(sp_covered):,} species")
        print(f"  -> vs {len(auto2sp):,} hairpins total "
              f"({100.0*len(coord_auto)/max(len(auto2sp),1):.1f}% have coordinates)")
        strands = Counter(col(data["mirna_chromosome_build"], ccols, "strand"))
        print(f"  strands: {dict(strands)}")

    print("\n### GFF3 (curated per-species genome coordinates)")
    gff3_stats()

    print("\n### FASTA")
    for fn, lbl in [("hairpin.fa", "hairpin.fa"),
                    ("hairpin_high_conf.fa", "hairpin_high_conf.fa"),
                    ("mature.fa", "mature.fa"),
                    ("mature_high_conf.fa", "mature_high_conf.fa")]:
        fasta_stats(RAW / fn, lbl)

    print("\n### EMBL")
    embl_stats(RAW / "miRNA.dat", "miRNA.dat")
    embl_stats(RAW / "miRNA_high_conf.dat", "miRNA_high_conf.dat")

    print("\n### other files")
    for fn in ["miRNA.csv", "miRNA.str", "miRNA.dead", "miRNA.diff",
               "README", "LICENSE"]:
        p = RAW / fn
        if p.exists():
            b = p.read_bytes()
            bom = " [UTF-8 BOM]" if b.startswith(b"\xef\xbb\xbf") else ""
            print(f"  {fn:<16} {len(b):>12,} bytes  "
                  f"{b.count(chr(10).encode()):>9,} lines{bom}")


if __name__ == "__main__":
    main()
