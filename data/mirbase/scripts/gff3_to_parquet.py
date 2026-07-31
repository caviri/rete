#!/usr/bin/env python3
"""miRBase GFF3 genome coordinates -> Parquet (all 31 species in one table).

Each shipped file carries a header block naming the genome build the
coordinates are against — that provenance is the whole point of these files, so
it is preserved in a second table rather than thrown away::

    ##gff-version 3
    ##date 2018-3-5
    # Chromosomal coordinates of Homo sapiens microRNAs
    # microRNAs:               miRBase v22
    # genome-build-id:         GRCh38
    # genome-build-accession:  NCBI_Assembly:GCA_000001405.15

Feature rows are the standard 9 GFF3 columns; the attribute column is split
into its four miRBase keys (ID/Alias/Name/Derives_from) AND kept verbatim so
the reverse conversion cannot drift.

    bash data/mirbase/scripts/py.sh gff3_to_parquet.py
"""
from __future__ import annotations

from pathlib import Path

import pyarrow as pa
import pyarrow.parquet as pq

BASE = Path(__file__).resolve().parent.parent
GENOMES = BASE / "raw" / "genomes"
OUT = BASE / "parquet"

FEATURES = pa.schema([
    ("organism", pa.string()),      # file stem: hsa, mmu, ...
    ("ordinal", pa.int32()),        # row order within the file
    ("seqid", pa.string()),         # chr1 / supercont1.1 / ...
    ("source", pa.string()),
    ("type", pa.string()),          # miRNA_primary_transcript | miRNA
    ("start", pa.int64()),
    ("end", pa.int64()),
    ("score", pa.string()),
    ("strand", pa.string()),
    ("phase", pa.string()),
    ("attributes", pa.string()),    # verbatim column 9
    ("attr_id", pa.string()),       # MI0022705 / MIMAT0027618
    ("attr_alias", pa.string()),
    ("attr_name", pa.string()),     # hsa-mir-6859-1
    ("derives_from", pa.string()),  # mature -> its hairpin
])

HEADERS = pa.schema([
    ("organism", pa.string()),
    ("header", pa.string()),        # the full leading comment block, verbatim
    ("genome_build_id", pa.string()),
    ("genome_build_accession", pa.string()),
    ("gff_date", pa.string()),
    ("feature_count", pa.int32()),
])


def main() -> None:
    OUT.mkdir(parents=True, exist_ok=True)
    feats: list[dict] = []
    heads: list[dict] = []

    for path in sorted(GENOMES.glob("*.gff3")):
        org = path.stem
        header_lines: list[str] = []
        build = accession = date = ""
        n = 0
        in_header = True

        for line in path.read_text(encoding="utf-8", errors="replace").splitlines():
            if line.startswith("#"):
                # only the LEADING block is the header; trailing comments (rare)
                # would otherwise be re-emitted in the wrong place
                if in_header:
                    header_lines.append(line)
                    if "genome-build-id:" in line:
                        build = line.split(":", 1)[1].strip()
                    elif "genome-build-accession:" in line:
                        accession = line.split(":", 1)[1].strip()
                    elif line.startswith("##date"):
                        date = line.split(" ", 1)[1].strip()
                continue
            if not line.strip():
                if in_header:
                    header_lines.append(line)
                continue
            in_header = False
            c = line.split("\t")
            if len(c) < 9:
                continue
            attrs = {}
            for kv in c[8].rstrip(";").split(";"):
                if "=" in kv:
                    k, v = kv.split("=", 1)
                    attrs[k] = v
            feats.append({
                "organism": org, "ordinal": n,
                "seqid": c[0], "source": c[1], "type": c[2],
                "start": int(c[3]), "end": int(c[4]),
                "score": c[5], "strand": c[6], "phase": c[7],
                "attributes": c[8],
                "attr_id": attrs.get("ID", ""),
                "attr_alias": attrs.get("Alias", ""),
                "attr_name": attrs.get("Name", ""),
                "derives_from": attrs.get("Derives_from", ""),
            })
            n += 1

        heads.append({
            "organism": org,
            "header": "\n".join(header_lines),
            "genome_build_id": build,
            "genome_build_accession": accession,
            "gff_date": date,
            "feature_count": n,
        })

    ft = pa.Table.from_pylist(feats, schema=FEATURES)
    ht = pa.Table.from_pylist(heads, schema=HEADERS)
    pq.write_table(ft, OUT / "gff3_features.parquet", compression="zstd")
    pq.write_table(ht, OUT / "gff3_headers.parquet", compression="zstd")
    print(f"  gff3_features.parquet  {ft.num_rows:,} rows "
          f"({(OUT / 'gff3_features.parquet').stat().st_size:,} bytes)")
    print(f"  gff3_headers.parquet   {ht.num_rows:,} files")


if __name__ == "__main__":
    main()
