#!/usr/bin/env python3
"""miRBase EMBL (miRNA.dat) -> Parquet, as four related tables.

The EMBL flat file is the richest of the miRBase distributions: it carries the
hairpin sequence AND the mature products AND the literature AND the free-text
comments in one place. It is split here into::

    embl_records     one row per stem-loop (ID/AC/DE/CC/SQ + sequence)
    embl_features    the FT block: mature miRNAs with their location on the hairpin
    embl_references  the RN/RX/RA/RT/RL citation blocks
    embl_xrefs       the DR cross-references (present in the high-confidence file)

Line-wrapped fields (RA, RT, CC, /experiment, ...) keep their original line
breaks as embedded newlines so `parquet_to_embl.py` can re-emit the file
byte-for-byte instead of guessing a re-wrap. Every record is round-tripped
in-process at write time; the handful that do not reproduce exactly (if any)
carry their original text in `raw_block`, so the inverse is exact by
construction rather than by hope.

    bash data/mirbase/scripts/py.sh embl_to_parquet.py
"""
from __future__ import annotations

from pathlib import Path

import pyarrow as pa
import pyarrow.parquet as pq

BASE = Path(__file__).resolve().parent.parent
RAW = BASE / "raw"
OUT = BASE / "parquet"

SOURCES = {"embl": RAW / "miRNA.dat", "embl_high_conf": RAW / "miRNA_high_conf.dat"}

RECORDS = pa.schema([
    ("ordinal", pa.int32()),
    ("name", pa.string()),           # cel-let-7
    ("accession", pa.string()),      # MI0000001
    ("description", pa.string()),    # DE line(s)
    ("division", pa.string()),       # CEL
    ("mol_type", pa.string()),       # RNA
    ("id_rest", pa.string()),        # verbatim tail of the ID line
    ("seq_length", pa.int32()),
    ("sequence", pa.string()),       # lowercase, unwrapped
    ("sq_header", pa.string()),      # verbatim SQ line
    ("comment", pa.string()),        # CC block, newline-joined
    ("raw_block", pa.string()),      # non-empty ONLY if re-serialization differed
])

FEATURES = pa.schema([
    ("record_accession", pa.string()),
    ("ordinal", pa.int32()),
    ("key", pa.string()),            # miRNA | modified_base
    ("location", pa.string()),       # 17..38
    ("start", pa.int32()),           # parsed from location when simple
    ("end", pa.int32()),
    ("accession", pa.string()),      # MIMAT0000001
    ("product", pa.string()),        # cel-let-7-5p
    ("evidence", pa.string()),       # experimental | not_experimental
    ("experiment", pa.string()),     # newline-preserving
    ("similarity", pa.string()),
    ("qualifiers_raw", pa.string()), # every qualifier line verbatim
])

REFERENCES = pa.schema([
    ("record_accession", pa.string()),
    ("number", pa.int32()),          # RN [1]
    ("pubmed", pa.string()),         # RX PUBMED; 11679671.
    ("medline", pa.string()),        # some refs carry BOTH a MEDLINE and a PUBMED RX
    ("rx_raw", pa.string()),         # every RX line, newline-joined, in order
    ("comment", pa.string()),        # RC — retraction / erratum notices
    ("authors", pa.string()),        # RA, newline-preserving
    ("title", pa.string()),          # RT
    ("journal", pa.string()),        # RL
])

XREFS = pa.schema([
    ("record_accession", pa.string()),
    ("ordinal", pa.int32()),
    ("line", pa.string()),           # verbatim DR line body
])


def split_records(text: str) -> list[str]:
    """Split the flat file on the `//` terminator, keeping each block whole."""
    blocks, cur = [], []
    for line in text.split("\n"):
        if line == "//":
            blocks.append("\n".join(cur))
            cur = []
        else:
            cur.append(line)
    return blocks


def parse_record(block: str, ordinal: int) -> tuple[dict, list[dict], list[dict], list[dict]]:
    rec = {
        "ordinal": ordinal, "name": "", "accession": "", "description": "",
        "division": "", "mol_type": "", "id_rest": "", "seq_length": 0,
        "sequence": "", "sq_header": "", "comment": "", "raw_block": "",
    }
    feats: list[dict] = []
    refs: list[dict] = []
    xrefs: list[dict] = []

    cc: list[str] = []
    de: list[str] = []
    seq_parts: list[str] = []
    cur_ref: dict | None = None
    cur_feat: dict | None = None
    ra: list[str] = []
    rt: list[str] = []
    in_seq = False

    def close_ref() -> None:
        nonlocal cur_ref, ra, rt
        if cur_ref is not None:
            cur_ref["authors"] = "\n".join(ra)
            cur_ref["title"] = "\n".join(rt)
            refs.append(cur_ref)
        cur_ref, ra, rt = None, [], []

    def close_feat() -> None:
        nonlocal cur_feat
        if cur_feat is not None:
            feats.append(cur_feat)
        cur_feat = None

    for line in block.split("\n"):
        code = line[:2]
        body = line[5:] if len(line) > 5 else ""

        if in_seq:
            # sequence lines have no code; strip the trailing position number
            seq_parts.append("".join(line[:70].split()))
            continue

        if code == "ID":
            # `ID   cel-let-7         standard; RNA; CEL; 99 BP.`
            rest = line[5:]
            name = rest.split(" ", 1)[0]
            tail = rest[len(name):].lstrip()
            rec["name"] = name
            rec["id_rest"] = tail
            parts = [p.strip() for p in tail.rstrip(".").split(";")]
            if len(parts) >= 4:
                rec["mol_type"] = parts[1]
                rec["division"] = parts[2]
                rec["seq_length"] = int(parts[3].split()[0]) if parts[3].split() else 0
        elif code == "AC":
            rec["accession"] = body.strip().rstrip(";")
        elif code == "DE":
            de.append(body)
        elif code == "CC":
            cc.append(body)
        elif code == "DR":
            xrefs.append({"record_accession": "", "ordinal": len(xrefs),
                          "line": body})
        elif code == "RN":
            close_ref()
            num = body.strip().strip("[]")
            cur_ref = {"record_accession": "", "number": int(num) if num.isdigit() else 0,
                       "pubmed": "", "medline": "", "rx_raw": "", "comment": "",
                       "authors": "", "title": "", "journal": ""}
        elif code == "RC" and cur_ref is not None:
            cur_ref["comment"] = (cur_ref["comment"] + "\n" + body
                                  if cur_ref["comment"] else body)
        elif code == "RX" and cur_ref is not None:
            # a reference may have several RX lines (MEDLINE and PUBMED)
            cur_ref["rx_raw"] = (cur_ref["rx_raw"] + "\n" + body
                                 if cur_ref["rx_raw"] else body)
            if "PUBMED;" in body:
                cur_ref["pubmed"] = body.split("PUBMED;", 1)[1].strip().rstrip(".")
            elif "MEDLINE;" in body:
                cur_ref["medline"] = body.split("MEDLINE;", 1)[1].strip().rstrip(".")
        elif code == "RA" and cur_ref is not None:
            ra.append(body)
        elif code == "RT" and cur_ref is not None:
            rt.append(body)
        elif code == "RL" and cur_ref is not None:
            cur_ref["journal"] = body
        elif code == "FT":
            if body[:16].strip():          # a feature key starts in col 5
                close_feat()
                key = body.split()[0]
                loc = body[16:].strip()
                start = end = 0
                if ".." in loc:
                    a, b = loc.split("..", 1)
                    if a.strip().isdigit() and b.strip().isdigit():
                        start, end = int(a), int(b)
                elif loc.isdigit():
                    start = end = int(loc)
                cur_feat = {
                    "record_accession": "", "ordinal": len(feats), "key": key,
                    "location": loc, "start": start, "end": end,
                    "accession": "", "product": "", "evidence": "",
                    "experiment": "", "similarity": "", "qualifiers_raw": "",
                }
            elif cur_feat is not None:
                q = body.strip()
                cur_feat["qualifiers_raw"] += (body + "\n")
                if q.startswith("/"):
                    k, _, v = q[1:].partition("=")
                    v = v.strip()
                    if v.startswith('"'):
                        v = v[1:]
                    if v.endswith('"'):
                        v = v[:-1]
                    if k in ("accession", "product", "evidence", "experiment",
                             "similarity"):
                        cur_feat[k] = v
                else:
                    # continuation of the previous quoted qualifier
                    cont = q[:-1] if q.endswith('"') else q
                    if cur_feat["experiment"]:
                        cur_feat["experiment"] += "\n" + cont
                    elif cur_feat["similarity"]:
                        cur_feat["similarity"] += "\n" + cont
        elif code == "SQ":
            close_ref()
            close_feat()
            rec["sq_header"] = line
            in_seq = True

    close_ref()
    close_feat()

    rec["description"] = "\n".join(de)
    rec["comment"] = "\n".join(cc)
    rec["sequence"] = "".join(seq_parts)
    if not rec["seq_length"]:
        rec["seq_length"] = len(rec["sequence"])

    acc = rec["accession"]
    for lst in (feats, refs, xrefs):
        for r in lst:
            r["record_accession"] = acc
    return rec, feats, refs, xrefs


def main() -> None:
    OUT.mkdir(parents=True, exist_ok=True)
    # imported lazily so the two scripts stay independently runnable
    from parquet_to_embl import serialize_record

    for key, path in SOURCES.items():
        if not path.exists():
            print(f"!! missing {path}")
            continue
        text = path.read_text(encoding="utf-8", errors="replace")
        blocks = split_records(text)
        # a trailing empty block after the final `//\n`
        blocks = [b for b in blocks if b.strip()]

        recs, feats, refs, xrefs = [], [], [], []
        mismatches = 0
        for i, block in enumerate(blocks):
            r, f, rf, x = parse_record(block, i)
            # verify the inverse NOW; keep the original only where it differs
            if serialize_record(r, f, rf, x) != block:
                r["raw_block"] = block
                mismatches += 1
            recs.append(r)
            feats.extend(f)
            refs.extend(rf)
            xrefs.extend(x)

        for name, rows, schema in (
            (f"{key}_records", recs, RECORDS),
            (f"{key}_features", feats, FEATURES),
            (f"{key}_references", refs, REFERENCES),
            (f"{key}_xrefs", xrefs, XREFS),
        ):
            tbl = pa.Table.from_pylist(rows, schema=schema)
            dst = OUT / f"{name}.parquet"
            pq.write_table(tbl, dst, compression="zstd")
            print(f"  {name:<28} {tbl.num_rows:>8,} rows "
                  f"({dst.stat().st_size:,} bytes)")
        pct = 100.0 * (len(recs) - mismatches) / max(len(recs), 1)
        print(f"  -> {key}: {len(recs) - mismatches:,}/{len(recs):,} records "
              f"({pct:.2f}%) reconstruct exactly from structure alone; "
              f"{mismatches:,} kept verbatim\n")


if __name__ == "__main__":
    main()
