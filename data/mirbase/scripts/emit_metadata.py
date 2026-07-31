#!/usr/bin/env python3
"""Emit data/mirbase/schema.json + croissant.jsonld for the flat Parquet layout.

The shared skill emitter discovers one table per `parquet-<name>/` DIRECTORY.
miRBase keeps its 32 tables as one file each in a single `parquet/` directory
(the round-trip converters address them by filename), so this wrapper reuses the
skill's context, type mapping and validation and only swaps the discovery step —
rather than duplicating the logic or reshuffling a verified layout.

    bash data/mirbase/scripts/py.sh emit_metadata.py
"""
from __future__ import annotations

import hashlib
import json
import sys
from pathlib import Path

import pyarrow as pa
import pyarrow.parquet as pq

REPO = Path(__file__).resolve().parents[3]
sys.path.insert(0, str(REPO / ".claude" / "skills" / "data-ontology" / "scripts"))

from emit_schema_croissant import CTX, cr_type, js_spec  # noqa: E402

ROOT = REPO / "data" / "mirbase"
PARQ = ROOT / "parquet"

NAME = "mirbase"
VERSION = "22.1"
LICENSE = "https://creativecommons.org/publicdomain/zero/1.0/"
URL = "https://www.mirbase.org/"
DATE = "2026-07-27"
DESCRIPTION = (
    "miRBase release 22.1 — the reference catalogue of published microRNA "
    "sequences and annotation — as Parquet. 38,589 hairpin stem-loops and "
    "48,885 mature miRNAs across 271 species: the 17-table relational dump, "
    "the FASTA sequence sets, the EMBL flat files split into records/features/"
    "references/xrefs, and the 31 assembly-stamped GFF3 genome-coordinate "
    "files. Public domain."
)

# what each table is, so the Croissant/JSON Schema are readable rather than bare
TABLE_DOC = {
    "db_confidence": "Per-stem-loop confidence evidence counts (read support, overhangs, folding energy).",
    "db_confidence_score": "The miRBase high-confidence score (0, 1 or 2) per stem-loop.",
    "db_dead_mirna": "Withdrawn accessions, with the entry each forwards to.",
    "db_literature_references": "Cited publications (MEDLINE/PubMed id, title, authors, journal).",
    "db_mature_database_links": "Cross-references from mature miRNAs to external databases.",
    "db_mature_database_url": "URL templates for the mature cross-reference targets.",
    "db_mirna": "The stem-loop (hairpin precursor) table — accession, name, sequence, species.",
    "db_mirna_2_prefam": "Stem-loop to miRNA-family membership.",
    "db_mirna_chromosome_build": "Genome coordinates per stem-loop for 153 species (no assembly stamp).",
    "db_mirna_context": "Overlap of a stem-loop with a host transcript (exon/intron/3UTR/5UTR).",
    "db_mirna_database_links": "Cross-references from stem-loops to external databases.",
    "db_mirna_database_url": "URL templates for the stem-loop cross-reference targets.",
    "db_mirna_literature_references": "Stem-loop to publication links.",
    "db_mirna_mature": "The mature miRNA table — accession, name, evidence, experiment.",
    "db_mirna_pre_mature": "Which matures are excised from which stem-loop, with offsets.",
    "db_mirna_prefam": "miRNA precursor families (MIPF accessions).",
    "db_mirna_species": "Organisms: miRBase code, NCBI taxon id, lineage, genome assembly.",
    "embl_records": "One row per stem-loop in miRNA.dat (EMBL): ids, description, sequence.",
    "embl_features": "The EMBL FT block — mature products located on their stem-loop.",
    "embl_references": "The EMBL RN/RX/RA/RT/RL citation blocks.",
    "embl_xrefs": "The EMBL DR cross-reference lines.",
    "embl_high_conf_records": "As embl_records, for the high-confidence subset.",
    "embl_high_conf_features": "As embl_features, for the high-confidence subset.",
    "embl_high_conf_references": "As embl_references, for the high-confidence subset.",
    "embl_high_conf_xrefs": "As embl_xrefs, for the high-confidence subset.",
    "fasta_hairpin": "hairpin.fa — 38,589 stem-loop precursor sequences.",
    "fasta_hairpin_high_conf": "hairpin_high_conf.fa — 3,320 high-confidence stem-loops.",
    "fasta_mature": "mature.fa — 48,885 mature miRNA sequences.",
    "fasta_mature_high_conf": "mature_high_conf.fa — 5,563 high-confidence matures.",
    "gff3_features": "The 31 GFF3 files as rows: assembly-stamped genome coordinates.",
    "gff3_headers": "Per-GFF3-file header block, genome build id and assembly accession.",
}


def main() -> None:
    defs: dict[str, dict] = {}
    dist: list[dict] = []
    rsets: list[dict] = []
    total = 0

    for f in sorted(PARQ.glob("*.parquet")):
        tname = f.stem
        pf = pq.ParquetFile(f)
        schema = pf.schema_arrow
        rows = pf.metadata.num_rows
        total += rows
        doc = TABLE_DOC.get(tname, f"{tname} table.")

        props = {}
        for fld in schema:
            spec = js_spec(fld.type)
            if fld.nullable:
                spec["type"] = [spec["type"], "null"]
            if fld.name.endswith("_json"):
                spec["contentMediaType"] = "application/json"
            props[fld.name] = spec
        defs[tname] = {
            "type": "object", "title": tname,
            "description": f"{doc} {rows:,} rows.",
            "properties": props, "additionalProperties": False,
        }

        fs = f"parquet/{tname}.parquet"
        # a cr:FileObject must carry a checksum, and it makes the Croissant
        # actually verifiable rather than merely descriptive
        dist.append({
            "@type": "cr:FileObject", "@id": fs, "name": fs,
            "description": f"{doc} {rows:,} rows.",
            "encodingFormat": "application/x-parquet",
            "contentUrl": fs,
            "contentSize": f"{f.stat().st_size} B",
            "sha256": hashlib.sha256(f.read_bytes()).hexdigest(),
        })
        rsets.append({
            "@type": "cr:RecordSet", "@id": tname, "name": tname,
            "description": f"{doc} {rows:,} rows.",
            "field": [
                dict(
                    {"@type": "cr:Field", "@id": f"{tname}/{fld.name}",
                     "name": fld.name, "dataType": cr_type(fld.type),
                     "source": {"fileObject": {"@id": fs},
                                "extract": {"column": fld.name}}},
                    **({"repeated": True}
                       if pa.types.is_list(fld.type) or pa.types.is_large_list(fld.type)
                       else {}),
                )
                for fld in schema
            ],
        })

    schema_doc = {
        "$schema": "https://json-schema.org/draft/2020-12/schema",
        "$id": f"https://w3id.org/rete/{NAME}/schema.json",
        "title": f"{NAME} {VERSION} — Parquet table schemas",
        "description": DESCRIPTION,
        "$defs": defs,
    }
    croissant = {
        "@context": CTX, "@type": "Dataset",
        "conformsTo": "http://mlcommons.org/croissant/1.0",
        "name": NAME, "description": DESCRIPTION,
        "url": URL, "license": LICENSE, "version": VERSION,
        "citeAs": ("Kozomara A, Birgaoanu M, Griffiths-Jones S. miRBase: from "
                   "microRNA sequences to function. Nucleic Acids Res. "
                   "2019;47(D1):D155-D162."),
        "datePublished": DATE,
        "distribution": dist,
        "recordSet": rsets,
    }

    sp, cp = ROOT / "schema.json", ROOT / "croissant.jsonld"
    sp.write_text(json.dumps(schema_doc, indent=1, ensure_ascii=False), encoding="utf-8")
    cp.write_text(json.dumps(croissant, indent=1, ensure_ascii=False), encoding="utf-8")
    print(f"wrote {sp} ({len(defs)} tables, {total:,} rows)")
    print(f"wrote {cp} ({len(dist)} fileObjects / {len(rsets)} recordSets)")

    try:
        from jsonschema import Draft202012Validator
        Draft202012Validator.check_schema(schema_doc)
        print("  schema.json: VALID (draft 2020-12)")
    except ImportError:
        print("  (pip install jsonschema to validate)")
    try:
        import mlcroissant as mlc
        ds = mlc.Dataset(jsonld=str(cp))
        print(f"  croissant.jsonld: VALID ({len(ds.metadata.record_sets)} recordSets)")
    except ImportError:
        print("  (pip install mlcroissant to validate)")


if __name__ == "__main__":
    main()
