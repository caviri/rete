#!/usr/bin/env python3
"""Parquet -> miRBase EMBL (the exact inverse of embl_to_parquet.py).

`serialize_record()` rebuilds one EMBL block from the structured tables and is
imported by embl_to_parquet.py so that conversion can verify itself as it
writes. Records whose `raw_block` is set are emitted verbatim.

    bash data/mirbase/scripts/py.sh parquet_to_embl.py [outdir]
"""
from __future__ import annotations

import sys
from collections import defaultdict
from pathlib import Path

BASE = Path(__file__).resolve().parent.parent
PARQ = BASE / "parquet"
DEFAULT_OUT = BASE / "roundtrip"

OUTPUTS = {"embl": "miRNA.dat", "embl_high_conf": "miRNA_high_conf.dat"}


def _prefixed(code: str, blob: str) -> list[str]:
    """Re-attach a 2-letter EMBL line code to each line of a stored blob."""
    out = []
    for ln in blob.split("\n"):
        out.append(f"{code}   {ln}" if ln else code)
    return out


def seq_lines(seq: str) -> list[str]:
    """EMBL sequence block: 6 space-separated 10-mers per line, position at col 80."""
    lines = []
    for i in range(0, len(seq), 60):
        chunk = seq[i:i + 60]
        groups = [chunk[j:j + 10] for j in range(0, len(chunk), 10)]
        body = " ".join(groups)
        pos = min(i + 60, len(seq))
        lines.append("     " + body.ljust(65) + str(pos).rjust(10))
    return lines


def serialize_record(rec: dict, feats: list[dict], refs: list[dict],
                     xrefs: list[dict]) -> str:
    """Structured row(s) -> one EMBL block (without the trailing `//`)."""
    if rec.get("raw_block"):
        return rec["raw_block"]

    L: list[str] = []
    L.append(f"ID   {rec['name']:<18}{rec['id_rest']}")
    L.append("XX")
    L.append(f"AC   {rec['accession']};")
    L.append("XX")
    L.extend(_prefixed("DE", rec["description"]))
    L.append("XX")

    for r in refs:
        L.append(f"RN   [{r['number']}]")
        if r["rx_raw"]:
            L.extend(_prefixed("RX", r["rx_raw"]))
        if r["authors"]:
            L.extend(_prefixed("RA", r["authors"]))
        if r["title"]:
            L.extend(_prefixed("RT", r["title"]))
        if r["journal"]:
            L.append(f"RL   {r['journal']}")
        # miRBase puts the reference comment (retraction / erratum) AFTER the
        # journal line, not in the usual EMBL position right after RN.
        if r.get("comment"):
            L.extend(_prefixed("RC", r["comment"]))
        L.append("XX")

    for x in xrefs:
        L.append(f"DR   {x['line']}")
    if xrefs:
        L.append("XX")

    if rec["comment"]:
        L.extend(_prefixed("CC", rec["comment"]))
        L.append("XX")

    if feats:
        L.append("FH   Key             Location/Qualifiers")
        L.append("FH")
        for f in feats:
            L.append(f"FT   {f['key']:<16}{f['location']}")
            for q in f["qualifiers_raw"].split("\n"):
                if q:
                    L.append(f"FT   {q}")
        L.append("XX")

    L.append(rec["sq_header"])
    L.extend(seq_lines(rec["sequence"]))
    return "\n".join(L)


def main() -> None:
    import pyarrow.parquet as pq

    out = Path(sys.argv[1]) if len(sys.argv) > 1 else DEFAULT_OUT
    out.mkdir(parents=True, exist_ok=True)

    for key, fname in OUTPUTS.items():
        rpath = PARQ / f"{key}_records.parquet"
        if not rpath.exists():
            print(f"!! missing {rpath}", file=sys.stderr)
            continue

        recs = pq.read_table(rpath).sort_by("ordinal").to_pylist()
        feats = pq.read_table(PARQ / f"{key}_features.parquet").to_pylist()
        refs = pq.read_table(PARQ / f"{key}_references.parquet").to_pylist()
        xrefs = pq.read_table(PARQ / f"{key}_xrefs.parquet").to_pylist()

        fby: dict[str, list[dict]] = defaultdict(list)
        rby: dict[str, list[dict]] = defaultdict(list)
        xby: dict[str, list[dict]] = defaultdict(list)
        for f in feats:
            fby[f["record_accession"]].append(f)
        for r in refs:
            rby[r["record_accession"]].append(r)
        for x in xrefs:
            xby[x["record_accession"]].append(x)
        for d in (fby, xby):
            for v in d.values():
                v.sort(key=lambda r: r["ordinal"])
        for v in rby.values():
            v.sort(key=lambda r: r["number"])

        dst = out / fname
        with dst.open("w", encoding="utf-8", newline="\n") as fh:
            for rec in recs:
                acc = rec["accession"]
                fh.write(serialize_record(rec, fby.get(acc, []), rby.get(acc, []),
                                          xby.get(acc, [])))
                fh.write("\n//\n")
        print(f"  {key:<16} -> {dst} ({len(recs):,} records)")


if __name__ == "__main__":
    main()
