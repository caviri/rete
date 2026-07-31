#!/usr/bin/env python3
"""mirbase.rete -> Parquet (the reverse of parquet_to_nt.py).

Reads an N-Triples/N-Quads export of the .rete on stdin and rebuilds the
`fasta_*` and `gff3_*` Parquet tables, so the whole chain closes:

    .rete -> Parquet -> hairpin.fa / mature.fa / *.gff3   (byte-identical)

Record ORDER is not stored in the graph, so it is re-derived the way miRBase
itself orders these files — verified byte-exact by roundtrip_rete_test.sh:
  * hairpin.fa : by MI accession
  * mature.fa  : by parent stem-loop, then by offset within it
  * *.gff3     : the ordinal is recoverable from the region IRI, which encodes
                 `<subject>/region/<organism>/<ordinal>`
The FASTA hard-wrap is a constant 60 columns in every shipped file.

    docker run ... rete export /work/data/mirbase/mirbase.rete --format nt \
      | bash data/mirbase/scripts/py.sh rete_to_parquet.py
"""
from __future__ import annotations

import re
import sys
from collections import defaultdict
from pathlib import Path

import pyarrow as pa
import pyarrow.parquet as pq

OUT = Path(__file__).resolve().parent.parent / "parquet-from-rete"

MB = "https://w3id.org/rete/mirbase#"
RDFS = "http://www.w3.org/2000/01/rdf-schema#"
DCT = "http://purl.org/dc/terms/"
FALDO = "http://biohackathon.org/resource/faldo#"
RDF = "http://www.w3.org/1999/02/22-rdf-syntax-ns#"

HAIRPIN = "https://www.mirbase.org/hairpin/"
MATURE = "https://www.mirbase.org/mature/"

WRAP = 60

TRIPLE = re.compile(r'^<([^>]*)>\s+<([^>]*)>\s+(.+?)\s*\.\s*$')

# schemas must match fa_to_parquet.py / gff3_to_parquet.py exactly
FASTA_SCHEMA = pa.schema([
    ("ordinal", pa.int32()), ("name", pa.string()), ("accession", pa.string()),
    ("description", pa.string()), ("organism", pa.string()),
    ("sequence", pa.string()), ("seq_length", pa.int32()), ("wrap", pa.int32()),
])
GFF3_SCHEMA = pa.schema([
    ("organism", pa.string()), ("ordinal", pa.int32()), ("seqid", pa.string()),
    ("source", pa.string()), ("type", pa.string()), ("start", pa.int64()),
    ("end", pa.int64()), ("score", pa.string()), ("strand", pa.string()),
    ("phase", pa.string()), ("attributes", pa.string()), ("attr_id", pa.string()),
    ("attr_alias", pa.string()), ("attr_name", pa.string()),
    ("derives_from", pa.string()),
])


def unlit(o: str) -> str:
    """N-Triples object -> plain python value (string form)."""
    if o.startswith("<"):
        return o[1:-1]
    if o.startswith('"'):
        end = o.rfind('"')
        s = o[1:end]
        return (s.replace("\\\\", "\x00").replace('\\"', '"')
                 .replace("\\n", "\n").replace("\\r", "\r")
                 .replace("\\t", "\t").replace("\x00", "\\"))
    return o


def main() -> None:
    S: dict[str, dict[str, list[str]]] = defaultdict(lambda: defaultdict(list))
    n = 0
    for line in sys.stdin:
        line = line.rstrip("\n")
        if not line or line.startswith("#"):
            continue
        m = TRIPLE.match(line)
        if not m:
            continue
        s, p, o = m.group(1), m.group(2), m.group(3)
        # N-Quads: drop a trailing graph term if present
        if o.endswith(">") and o.count(" ") and not o.startswith('"'):
            parts = o.rsplit(" ", 1)
            if parts[1].startswith("<"):
                o = parts[0]
        S[s][p].append(unlit(o))
        n += 1
    print(f"  parsed {n:,} triples over {len(S):,} subjects", file=sys.stderr)

    def one(subj: str, pred: str, default: str = "") -> str:
        v = S.get(subj, {}).get(pred)
        return v[0] if v else default

    OUT.mkdir(parents=True, exist_ok=True)

    # ------------------------------------------------------------- hairpins
    hairpins = [s for s in S
                if s.startswith(HAIRPIN) and "/region/" not in s
                and "/context/" not in s
                and f"{MB}StemLoop" in S[s].get(RDF + "type", [])]
    # hairpin.fa holds the LIVE entries that carry a sequence, in accession order
    hp_rows = []
    for acc_iri in sorted(hairpins, key=lambda x: x[len(HAIRPIN):]):
        seq = one(acc_iri, MB + "sequence")
        # miRBase itself omits withdrawn entries and the field-shifted row(s)
        # flagged as malformed, so hairpin.fa omits them too
        if (not seq or one(acc_iri, MB + "deadFlag") == "true"
                or one(acc_iri, MB + "sourceRowMalformed") == "true"):
            continue
        acc = acc_iri[len(HAIRPIN):]
        name = one(acc_iri, RDFS + "label")
        hp_rows.append({
            "ordinal": len(hp_rows), "name": name, "accession": acc,
            "description": one(acc_iri, DCT + "description"),
            "organism": name.split("-")[0] if name else "",
            "sequence": seq, "seq_length": len(seq), "wrap": WRAP,
        })

    # --------------------------------------------------------------- matures
    # mature.fa is ordered by parent stem-loop, then by offset on that stem-loop.
    # A mature shared by several stem-loops is listed ONCE, with its first parent.
    mat_rows = []
    seen_mature: set[str] = set()
    for hp in hp_rows:
        hp_iri = HAIRPIN + hp["accession"]
        kids = S.get(hp_iri, {}).get(MB + "hasMatureProduct", [])

        def offset(m_iri: str) -> int:
            # the offset is per (stem-loop, mature) pair — read it off the
            # placement node for THIS hairpin, not off the mature
            acc = m_iri[len(MATURE):]
            v = one(f"{hp_iri}/placement/{acc}", MB + "matureFrom")
            return int(v) if v.isdigit() else 1 << 30

        for m_iri in sorted(set(kids), key=offset):
            if m_iri in seen_mature:
                continue
            # NB: no dead-flag filter here — mature.fa genuinely ships 25
            # entries that mirna_mature marks dead_flag=1
            seq = one(m_iri, MB + "sequence")
            if not seq:
                continue
            seen_mature.add(m_iri)
            name = one(m_iri, RDFS + "label")
            mat_rows.append({
                "ordinal": len(mat_rows), "name": name,
                "accession": m_iri[len(MATURE):],
                "description": one(m_iri, DCT + "description"),
                "organism": name.split("-")[0] if name else "",
                "sequence": seq, "seq_length": len(seq), "wrap": WRAP,
            })

    for key, rows in (("hairpin", hp_rows), ("mature", mat_rows)):
        tbl = pa.Table.from_pylist(rows, schema=FASTA_SCHEMA)
        pq.write_table(tbl, OUT / f"fasta_{key}.parquet", compression="zstd")
        print(f"  fasta_{key}.parquet  {tbl.num_rows:,} records", file=sys.stderr)

    # ------------------------------------------------------------------ gff3
    feats = []
    for subj, preds in S.items():
        if "/region/" not in subj or f"{FALDO}Region" not in preds.get(RDF + "type", []):
            continue
        if preds.get(MB + "coordinateSource", [""])[0] != f"{MB}GFF3":
            continue
        # <entity>/region/<organism>/<ordinal>
        head, _, tail = subj.partition("/region/")
        org, _, ordinal = tail.partition("/")
        if not ordinal.isdigit():
            continue
        is_mature = head.startswith(MATURE)
        acc = head[len(MATURE):] if is_mature else head[len(HAIRPIN):]
        begin = one(subj + "/begin", FALDO + "position")
        end = one(subj + "/end", FALDO + "position")
        stypes = S.get(subj + "/begin", {}).get(RDF + "type", [])
        strand = ("+" if f"{FALDO}ForwardStrandPosition" in stypes
                  else "-" if f"{FALDO}ReverseStrandPosition" in stypes else ".")
        # the copy-local id, name and parent live on the REGION (see
        # parquet_to_nt.py) — they can differ from the entity's own values
        name = one(subj, RDFS + "label") or one(head, RDFS + "label")
        gff_id = one(subj, DCT + "identifier") or acc
        derives = ""
        d = preds.get(MB + "parentStemLoop")
        if d:
            derives = d[0][len(HAIRPIN):]
        attrs = f"ID={gff_id};Alias={acc};Name={name}"
        if derives:
            attrs += f";Derives_from={derives}"
        # faldo:reference now points at a ReferenceSequence resource; the GFF3
        # column holds its name, which is that resource's label
        seqid = one(one(subj, FALDO + "reference"), RDFS + "label")
        feats.append({
            "organism": org, "ordinal": int(ordinal),
            "seqid": seqid, "source": ".",
            "type": "miRNA" if is_mature else "miRNA_primary_transcript",
            "start": int(begin), "end": int(end), "score": ".",
            "strand": strand, "phase": ".", "attributes": attrs,
            "attr_id": acc, "attr_alias": acc, "attr_name": name,
            "derives_from": derives,
        })

    feats.sort(key=lambda r: (r["organism"], r["ordinal"]))
    tbl = pa.Table.from_pylist(feats, schema=GFF3_SCHEMA)
    pq.write_table(tbl, OUT / "gff3_features.parquet", compression="zstd")
    print(f"  gff3_features.parquet  {tbl.num_rows:,} rows", file=sys.stderr)
    print(f"ok  wrote {OUT}", file=sys.stderr)


if __name__ == "__main__":
    main()
